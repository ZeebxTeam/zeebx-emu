//! Formatação estilo `printf` para o `DBGPRINTF` do BREW.
//!
//! O guest passa a string de formato e os argumentos pela AAPCS; aqui só traduzimos. É um
//! subconjunto deliberado — o suficiente para o que os jogos usam em log — e qualquer
//! especificador desconhecido é copiado como veio, para nunca perdermos informação.

/// De onde vêm os argumentos variádicos e como ler strings do guest.
pub trait ArgSource {
    /// Próximo argumento de 32 bits.
    fn next_word(&mut self) -> u32;
    /// String terminada em zero no endereço dado.
    fn read_cstring(&mut self, addr: u32) -> String;
}

/// Flags, largura e precisão de um especificador.
#[derive(Default)]
struct Spec {
    /// `-`: alinha à esquerda.
    left: bool,
    /// `0`: completa com zeros em vez de espaços.
    zero: bool,
    /// `+`: sinal explícito nos positivos.
    plus: bool,
    /// ` `: espaço no lugar do sinal nos positivos.
    space: bool,
    /// `#`: prefixo `0x` no hexadecimal.
    alt: bool,
    width: usize,
    precision: Option<usize>,
}

impl Spec {
    /// Aplica largura e alinhamento a um campo já pronto.
    fn pad(&self, body: String) -> String {
        if body.chars().count() >= self.width {
            return body;
        }
        let fill = self.width - body.chars().count();
        match self.left {
            true => body + &" ".repeat(fill),
            false => " ".repeat(fill) + &body,
        }
    }

    /// Monta um número: precisão é o mínimo de dígitos, e o zero à esquerda só vale quando não
    /// há precisão nem alinhamento à esquerda — é o que o C faz.
    fn number(&self, sign: &str, digits: String) -> String {
        let least = self.precision.unwrap_or(0);
        let mut digits = match digits.len() < least {
            true => "0".repeat(least - digits.len()) + &digits,
            false => digits,
        };
        if self.zero && !self.left && self.precision.is_none() {
            let used = digits.len() + sign.len();
            if used < self.width {
                digits = "0".repeat(self.width - used) + &digits;
            }
        }
        self.pad(format!("{sign}{digits}"))
    }
}

/// Aplica `fmt` consumindo argumentos de `args`.
pub fn format(fmt: &str, args: &mut impl ArgSource) -> String {
    let mut out = String::new();
    let mut chars = fmt.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }

        // O texto original do especificador é guardado para poder ser devolvido inteiro se a
        // conversão no fim dele for uma que não conhecemos.
        let mut spec = String::from('%');
        let mut f = Spec::default();
        while let Some(&next) = chars.peek() {
            match next {
                '-' => f.left = true,
                '0' => f.zero = true,
                '+' => f.plus = true,
                ' ' => f.space = true,
                '#' => f.alt = true,
                _ => break,
            }
            spec.push(next);
            chars.next();
        }
        // Largura. `*` a toma do próprio argumento, e uma largura negativa por essa via é o
        // mesmo que o flag `-`.
        if chars.peek() == Some(&'*') {
            spec.push('*');
            chars.next();
            let value = args.next_word() as i32;
            f.left |= value < 0;
            f.width = value.unsigned_abs() as usize;
        } else {
            while let Some(&next) = chars.peek() {
                let Some(digit) = next.to_digit(10) else {
                    break;
                };
                f.width = f.width * 10 + digit as usize;
                spec.push(next);
                chars.next();
            }
        }
        if chars.peek() == Some(&'.') {
            spec.push('.');
            chars.next();
            if chars.peek() == Some(&'*') {
                spec.push('*');
                chars.next();
                f.precision = Some((args.next_word() as i32).max(0) as usize);
            } else {
                let mut value = 0usize;
                while let Some(&next) = chars.peek() {
                    let Some(digit) = next.to_digit(10) else {
                        break;
                    };
                    value = value * 10 + digit as usize;
                    spec.push(next);
                    chars.next();
                }
                f.precision = Some(value);
            }
        }
        // Modificadores de tamanho. Todos os inteiros do ARM chegam em palavras de 32 bits, e
        // `%lld` não aparece nos jogos, então eles só precisam ser consumidos.
        while let Some(&next) = chars.peek() {
            if !"lhzjt".contains(next) {
                break;
            }
            spec.push(next);
            chars.next();
        }

        /// O sinal a imprimir num número positivo, conforme os flags.
        fn positive(f: &Spec) -> &'static str {
            match (f.plus, f.space) {
                (true, _) => "+",
                (_, true) => " ",
                _ => "",
            }
        }

        match chars.next() {
            Some('%') => out.push('%'),
            Some('d') | Some('i') => {
                let value = args.next_word() as i32;
                let sign = match value < 0 {
                    true => "-",
                    false => positive(&f),
                };
                out.push_str(&f.number(sign, value.unsigned_abs().to_string()));
            }
            Some('u') => {
                let value = args.next_word();
                out.push_str(&f.number(positive(&f), value.to_string()));
            }
            Some('x') => {
                let value = args.next_word();
                let prefix = match f.alt && value != 0 {
                    true => "0x",
                    false => "",
                };
                out.push_str(&f.number(prefix, format!("{value:x}")));
            }
            Some('X') => {
                let value = args.next_word();
                let prefix = match f.alt && value != 0 {
                    true => "0X",
                    false => "",
                };
                out.push_str(&f.number(prefix, format!("{value:X}")));
            }
            Some('o') => {
                let value = args.next_word();
                out.push_str(&f.number("", format!("{value:o}")));
            }
            Some('p') => out.push_str(&f.pad(format!("{:#010x}", args.next_word()))),
            Some('c') => {
                let value = args.next_word();
                out.push_str(&f.pad(char::from_u32(value).unwrap_or('?').to_string()));
            }
            Some('s') => {
                let addr = args.next_word();
                let mut text = args.read_cstring(addr);
                // Na string a precisão é teto, não piso: ela corta.
                if let Some(limit) = f.precision {
                    text = text.chars().take(limit).collect();
                }
                out.push_str(&f.pad(text));
            }
            // Especificador que não conhecemos: devolve o texto original em vez de sumir com
            // ele, e não consome argumento — consumir errado desalinharia todo o resto.
            Some(other) => {
                out.push_str(&spec);
                out.push(other);
            }
            None => out.push_str(&spec),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        words: Vec<u32>,
        strings: Vec<(u32, &'static str)>,
        index: usize,
    }

    impl ArgSource for Fake {
        fn next_word(&mut self) -> u32 {
            let value = self.words.get(self.index).copied().unwrap_or(0);
            self.index += 1;
            value
        }

        fn read_cstring(&mut self, addr: u32) -> String {
            self.strings
                .iter()
                .find(|&&(a, _)| a == addr)
                .map(|&(_, s)| s.to_string())
                .unwrap_or_default()
        }
    }

    fn fake(words: &[u32], strings: &[(u32, &'static str)]) -> Fake {
        Fake {
            words: words.to_vec(),
            strings: strings.to_vec(),
            index: 0,
        }
    }

    #[test]
    fn formata_a_mensagem_real_do_z_wheel() {
        // A primeira chamada de DBGPRINTF do Z-Wheel, com os argumentos que ele passa.
        let mut args = fake(
            &[4, 0x532c4, 0x71],
            &[(0x532c4, "..\\..\\common\\tectoy_prefsDB.c")],
        );
        assert_eq!(
            format("*dbgprintf-%d* %s:%d", &mut args),
            "*dbgprintf-4* ..\\..\\common\\tectoy_prefsDB.c:113"
        );
    }

    #[test]
    fn cobre_os_especificadores_basicos() {
        let mut args = fake(&[0xffff_ffff, 42, 0xbeef, 65], &[]);
        assert_eq!(format("%d %u %x %c", &mut args), "-1 42 beef A");
    }

    #[test]
    fn porcento_literal_nao_consome_argumento() {
        let mut args = fake(&[7], &[]);
        assert_eq!(format("100%% de %d", &mut args), "100% de 7");
    }

    #[test]
    fn largura_precisao_e_alinhamento() {
        // Precisão é o mínimo de dígitos, largura é o do campo, e `-` alinha à esquerda. O
        // zero à esquerda perde para a precisão, como no C.
        let mut args = fake(&[5], &[]);
        assert_eq!(format("[%-08.3ld]", &mut args), "[005     ]");
        let mut args = fake(&[5, 5, 5, -3i32 as u32], &[]);
        assert_eq!(format("%04d|%4d|%-4d|%+d", &mut args), "0005|   5|5   |-3");
    }

    /// O Resident Evil 4 monta o nome dos arquivos de estágio com `%s_%02d.h2z`. Descartar a
    /// largura dava `3d_stg02_0.h2z`, e o arquivo em disco é `3d_stg02_00.h2z`: o jogo não
    /// achava nenhum dos doze estágios.
    #[test]
    fn zero_a_esquerda_no_nome_de_arquivo() {
        let mut args = fake(&[0x100, 0], &[(0x100, "3d_stg02")]);
        assert_eq!(format("%s_%02d.h2z", &mut args), "3d_stg02_00.h2z");
    }

    #[test]
    fn largura_por_argumento_e_precisao_corta_string() {
        let mut args = fake(&[6, 42], &[]);
        assert_eq!(format("[%*d]", &mut args), "[    42]");
        let mut args = fake(&[0x200], &[(0x200, "abcdef")]);
        assert_eq!(format("[%.3s]", &mut args), "[abc]");
    }

    #[test]
    fn especificador_desconhecido_sobrevive_sem_consumir_argumento() {
        let mut args = fake(&[9], &[]);
        assert_eq!(format("%q %d", &mut args), "%q 9");
    }
}
