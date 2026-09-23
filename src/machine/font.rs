//! `IFont`: as fontes de bitmap do sistema.
//!
//! O BREW não desenha texto só com a fonte do aparelho: ele tem **classes de fonte** prontas, e o
//! aplicativo pede uma delas ao `ISHELL_CreateInstance`. São 29 classes em quatro famílias:
//!
//! | Família | Classes |
//! |---|---|
//! | `AEECLSID_FONTSYS*` | `NORMAL`, `LARGE`, `BOLD`, `ITALIC`, `BOLDITALIC`, `LARGEITALIC` |
//! | `AEECLSID_FONT_STANDARD*` | `11`, `11B`, `15`, `15B`, `18`, `18B`, `23`, `23B`, `26`, `26B`, `36` |
//! | `AEECLSID_FONT_BASIC*` | `6`, `9`, `10`, `11`, `11B`, `12`, `12B`, `14`, `15` |
//! | avulsas | `AEECLSID_FONT`, `FONT_FIXED4X6`, `BITFONTFOUNDRY` |
//!
//! **Não é o `ITypeface`.** O `ITypeface` cria fontes a partir de um TTF; o `IFont` já **é** a
//! fonte desenhável. As duas interfaces têm métodos diferentes, e tratá-las como a mesma coisa é o
//! que fazia o Double Dragon, o Resident Evil 4 e os ports da Data East caírem na tela
//! "Memory is insufficient. Please delete some files." — a mensagem genérica de falha deles quando
//! o `CreateInstance` da fonte volta nulo.
//!
//! A ordem dos seis métodos e a tabela de métricas vêm do código de referência que já implementa
//! esta interface (`zeebo-emulator/research/sources/zeemu/brew/BrewFont.cpp`), e a assinatura dos
//! métodos vem dos próprios exemplos do SDK:
//!
//! ```c
//! IFont_GetInfo(pFont, &info, sizeof(info))
//! IFont_MeasureText(pFont, pszBuf, WSTRLEN(pszBuf), IFONT_MAXWIDTH, &nChars, &extent.width)
//! ```

use super::*;

/// O que uma fonte do sistema diz de si mesma, em pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Metricas {
    pub ascent: u16,
    pub descent: u16,
    pub leading: u16,
    pub max_char_width: u16,
    pub height: u16,
    pub bold: bool,
    pub italic: bool,
}

/// As classes que respondem por esta interface.
///
/// Sem esta lista, o `CreateInstance` dessas classes devolve "não suportado" — e o jogo, que não
/// distingue "não existe" de "acabou a memória", mostra a tela de falta de memória.
pub const CLASSES_DE_FONTE: [u32; 29] = [
    // FONTSYS
    0x0101_2786,
    0x0101_2787,
    0x0101_2788,
    0x0101_402c,
    0x0101_402d,
    0x0101_402e,
    // FONT
    0x0100_1022,
    // FONT_STANDARD
    0x0102_f679,
    0x0102_f67a,
    0x0103_0852,
    0x0103_0853,
    0x0102_f67b,
    0x0102_f67c,
    0x0102_f67d,
    0x0102_f67e,
    0x0102_f67f,
    0x0102_f680,
    0x0102_f681,
    // FIXED4X6
    0x0100_a001,
    // BASIC
    0x0100_a002,
    0x0100_a003,
    0x0100_a004,
    0x0100_a005,
    0x0100_a006,
    0x0100_a007,
    0x0100_a008,
    0x0100_a009,
    0x0100_a00a,
    // BITFONTFOUNDRY
    0x0100_a100,
];

/// As métricas de cada classe, como a referência as traz.
///
/// Os números são declarados no `.bid` para algumas (`FONT_STANDARD11` tem ascent 13 e descent 3) e
/// medidos na referência para o resto. As duas fontes concordam onde se sobrepõem.
pub fn metricas_da_classe(classe: u32) -> Metricas {
    let (ascent, descent, leading, max_char_width, height, bold) = match classe {
        0x0100_a001 => (4, 2, 0, 4, 6, false),
        0x0100_a002 => (6, 2, 1, 5, 9, false),
        0x0100_a003 => (9, 3, 1, 6, 13, false),
        0x0100_a004 => (10, 3, 1, 7, 14, false),
        0x0100_a005 | 0x0101_2786 | 0x0101_402c | 0x0100_1022 => (13, 3, 2, 8, 18, false),
        0x0100_a006 | 0x0101_2788 | 0x0101_402d => (13, 3, 2, 8, 18, true),
        0x0100_a007 => (14, 4, 2, 9, 20, false),
        0x0100_a008 => (12, 4, 2, 8, 18, false),
        0x0100_a009 => (12, 4, 2, 8, 18, true),
        0x0100_a00a => (15, 4, 2, 10, 21, false),
        0x0101_2787 | 0x0101_402e => (21, 5, 2, 10, 28, false),
        // A família `FONT_STANDARD`, uma linha por classe, com `ascent` e `descent` **copiados do
        // `AEEFontsStandard.BID`** e o negrito que o próprio texto do `.bid` declara ("Bolded").
        // A tabela errava três: a `STANDARD15B` respondia 17/4 — os números da 18B —, e o negrito
        // da 23B e da 26B vivia num remendo fora da tabela, onde a próxima edição não o veria.
        // Um jogo que reserva altura pela resposta do `GetInfo` desenha linha em cima de linha
        // quando ela vem errada, e a diferença de 2 px entre 15/3 e 17/4 é a altura de uma linha
        // inteira numa tela de 320 pixels.
        0x0102_f679 => (13, 3, 2, 8, 18, false), // STANDARD11
        0x0102_f67a => (13, 3, 2, 8, 18, true),  // STANDARD11B
        0x0103_0852 => (15, 3, 2, 8, 20, false), // STANDARD15
        0x0103_0853 => (15, 3, 2, 8, 20, true),  // STANDARD15B
        0x0102_f67b => (17, 4, 2, 9, 23, false), // STANDARD18
        0x0102_f67c => (17, 4, 2, 9, 23, true),  // STANDARD18B
        0x0102_f67d => (21, 5, 2, 10, 28, false), // STANDARD23
        0x0102_f67e => (21, 5, 2, 10, 28, true), // STANDARD23B
        0x0102_f67f => (23, 6, 2, 10, 31, false), // STANDARD26
        0x0102_f680 => (23, 6, 2, 10, 31, true), // STANDARD26B
        // A maior do sistema, usada em títulos grandes.
        0x0102_f681 => (38, 10, 2, 12, 50, false), // STANDARD36
        _ => (12, 4, 2, 8, 16, false),
    };
    let negrito = bold;
    Metricas {
        ascent,
        descent,
        leading,
        max_char_width,
        height,
        bold: negrito,
        italic: matches!(classe, 0x0101_402c | 0x0101_402d | 0x0101_402e),
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    /// **As métricas do `FONT_STANDARD` são transcritas do `.bid`, não inventadas.**
    ///
    /// O `AEEFontsStandard.BID` declara ascent e descent das onze classes, e diz quais são negrito.
    /// Este teste é a cópia literal daquele arquivo: se alguém "arrumar" a tabela de memória, ele
    /// discorda — que é o ponto, porque a memória já errou três linhas uma vez.
    #[test]
    fn as_metricas_do_standard_batem_com_o_bid() {
        let bid = [
            (0x0102_f679u32, 13u16, 3u16, false), // STANDARD11
            (0x0102_f67a, 13, 3, true),           // STANDARD11B
            (0x0103_0852, 15, 3, false),          // STANDARD15
            (0x0103_0853, 15, 3, true),           // STANDARD15B
            (0x0102_f67b, 17, 4, false),          // STANDARD18
            (0x0102_f67c, 17, 4, true),           // STANDARD18B
            (0x0102_f67d, 21, 5, false),          // STANDARD23
            (0x0102_f67e, 21, 5, true),           // STANDARD23B
            (0x0102_f67f, 23, 6, false),          // STANDARD26
            (0x0102_f680, 23, 6, true),           // STANDARD26B
            (0x0102_f681, 38, 10, false),         // STANDARD36
        ];
        for (classe, ascent, descent, negrito) in bid {
            let medida = metricas_da_classe(classe);
            assert_eq!(
                (medida.ascent, medida.descent),
                (ascent, descent),
                "ascent/descent da classe {classe:#010x}"
            );
            assert_eq!(medida.bold, negrito, "negrito da classe {classe:#010x}");
            assert!(
                medida.height >= medida.ascent + medida.descent,
                "altura menor que a soma de ascent e descent na {classe:#010x}"
            );
        }
    }

    /// A `STANDARD15B` responde como a 15, e não como a 18: era o defeito que esteve aqui.
    #[test]
    fn a_standard15b_nao_herda_os_numeros_da_18b() {
        let quinze_b = metricas_da_classe(0x0103_0853);
        let quinze = metricas_da_classe(0x0103_0852);
        assert_eq!((quinze_b.ascent, quinze_b.descent), (quinze.ascent, quinze.descent));
        assert_ne!((quinze_b.ascent, quinze_b.descent), (17, 4));
    }
}

impl<C: CpuBackend> Machine<C> {
    /// Cria o objeto de uma fonte do sistema, guardando de qual classe ele é.
    pub(super) fn cria_fonte(&mut self, classe: u32, out: u32) -> Result<u32, CpuError> {
        let objeto = self.new_object(Interface::Font)?;
        if objeto == 0 {
            return Ok(ENOMEMORY);
        }
        self.fontes.insert(objeto, metricas_da_classe(classe));
        if out != 0 {
            self.cpu.write_u32(out, objeto)?;
        }
        Ok(SUCCESS)
    }

    /// Atende o `IFont`.
    pub(super) fn font_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Font.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let metricas = self.fontes.get(&this).copied().unwrap_or_default();
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let restantes = self.objects.release(this);
                if restantes == 0 {
                    self.fontes.remove(&this);
                }
                restantes
            }
            // int QueryInterface(IFont *, AEECLSID, void **ppv)
            "QueryInterface" => {
                let out = self.cpu.read_reg(Reg::R2);
                if out != 0 {
                    self.cpu.write_u32(out, this)?;
                }
                SUCCESS
            }
            // int GetInfo(IFont *, AEEFontInfo *pInfo, int nSize)
            //
            // A struct começa em quatro `int16` — ascent, descent, leading e a largura máxima de
            // caractere. Só se escreve o que couber no tamanho que o chamador declarou.
            "GetInfo" => {
                let info = self.cpu.read_reg(Reg::R1);
                let tamanho = self.cpu.read_reg(Reg::R2);
                if info != 0 {
                    let campos: [u16; 5] = [
                        metricas.ascent,
                        metricas.descent,
                        metricas.leading,
                        metricas.max_char_width,
                        metricas.height,
                    ];
                    for (i, valor) in campos.iter().enumerate() {
                        let deslocamento = 4 + i as u32 * 2;
                        if tamanho != 0 && deslocamento + 2 > tamanho {
                            break;
                        }
                        self.cpu.write_mem(info + deslocamento, &valor.to_le_bytes())?;
                    }
                }
                SUCCESS
            }
            // int MeasureText(IFont *, const AECHAR *pchText, int nChars, int nMaxWidth,
            //                 int *pnChars, int *pnWidth)
            //
            // A assinatura vem do exemplo do próprio SDK. `nChars` pode ser `-1`, que quer dizer
            // "até o terminador".
            "MeasureText" => {
                let texto = self.cpu.read_reg(Reg::R1);
                let pedidos = self.cpu.read_reg(Reg::R2) as i32;
                let out_chars = self.stack_arg(0)?;
                let out_largura = self.stack_arg(1)?;
                let mut quantos = 0u32;
                if texto != 0 {
                    let mut cursor = texto;
                    let limite = match pedidos {
                        n if n < 0 => MAX_STRING as u32,
                        n => n as u32,
                    };
                    while quantos < limite && quantos < MAX_STRING as u32 {
                        let mut par = [0u8; 2];
                        self.cpu.read_mem(cursor, &mut par)?;
                        if u16::from_le_bytes(par) == 0 {
                            break;
                        }
                        quantos += 1;
                        cursor += 2;
                    }
                }
                if out_chars != 0 {
                    self.cpu.write_u32(out_chars, quantos)?;
                }
                if out_largura != 0 {
                    self.cpu
                        .write_u32(out_largura, quantos * u32::from(metricas.max_char_width))?;
                }
                SUCCESS
            }
            // int DrawText(IFont *, const AECHAR *pchText, int nChars, int x, int y, ...)
            //
            // **O layout dos argumentos ainda não foi medido**, ao contrário dos outros três: a
            // referência só imprime o texto e não desenha. Aqui se desenha no destino corrente,
            // que é o que o `IDISPLAY` faz, e a suposição entra no relatório para quem for medir.
            "DrawText" => {
                let texto = self.cpu.read_reg(Reg::R1);
                let pedidos = self.cpu.read_reg(Reg::R2) as i32;
                let x = self.cpu.read_reg(Reg::R3) as i32;
                let y = self.stack_arg(0)? as i32;
                self.assumptions
                    .insert("IFont::DrawText desenha com layout deduzido, ainda sem medida");
                if texto != 0 {
                    let mut units = self.read_aechar_units(texto)?;
                    if pedidos >= 0 {
                        units.truncate(pedidos as usize);
                    }
                    let texto = String::from_utf16_lossy(&units);
                    self.draw_text(&texto, x, y)?;
                }
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }
}
