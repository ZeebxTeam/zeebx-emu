//! O controle do Switch virando o controle do Zeebo.
//!
//! Os bits são os `HidNpadButton` do libnx. A posição é a do aparelho, a mesma tabela do
//! RetroPad no core e da Vita: o botão de baixo é o 1, o da esquerda é o 2, o de cima é o 3
//! e o da direita é o 4. No Switch essa ordem é B, Y, X, A — o A fica à direita, não embaixo.

use std::time::Duration;

use zeebx::input::{Pad, AXIS_CURSO};

pub const A: u64 = 1 << 0;
pub const B: u64 = 1 << 1;
pub const X: u64 = 1 << 2;
pub const Y: u64 = 1 << 3;
pub const STICK_L: u64 = 1 << 4;
pub const STICK_R: u64 = 1 << 5;
pub const L: u64 = 1 << 6;
pub const R: u64 = 1 << 7;
pub const ZL: u64 = 1 << 8;
pub const ZR: u64 = 1 << 9;
pub const PLUS: u64 = 1 << 10;
pub const MINUS: u64 = 1 << 11;
pub const ESQUERDA: u64 = 1 << 12;
pub const CIMA: u64 = 1 << 13;
pub const DIREITA: u64 = 1 << 14;
pub const BAIXO: u64 = 1 << 15;

/// O analógico do Switch vai de −32768 a 32767. Parado, ele oscila. O corte é a mesma fração
/// da zona morta da Vita (16 em 128): abaixo disso o boneco caminha sozinho.
pub const ZONA_MORTA: i32 = 4096;

/// Curso cheio do analógico, do lado positivo. O negativo chega a −32768 e é preso no curso.
pub const CURSO_DO_JOY: i32 = 32767;

/// Minus e Plus juntos, segurados, devolvem ao tico. Meio segundo: um toque dos dois no meio
/// de um jogo não pode sair.
pub const SAIDA: Duration = Duration::from_millis(500);

const MAPA: &[(u64, &str)] = &[
    (CIMA, "up"),
    (BAIXO, "down"),
    (ESQUERDA, "left"),
    (DIREITA, "right"),
    (B, "b1"),
    (Y, "b2"),
    (X, "b3"),
    (A, "b4"),
    (ZL, "zl"),
    (ZR, "zr"),
    (L, "l2"),
    (R, "r2"),
    (PLUS, "start"),
    (MINUS, "back"),
    (STICK_L, "lthumb"),
    (STICK_R, "rthumb"),
];

/// Uma leitura do `PadState`, já separada da struct do libnx.
///
/// O analógico vem com o sinal do aparelho: a direita e **cima** são positivos. O eixo interno
/// do console cresce para baixo, então quem converte inverte o Y.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Leitura {
    pub botoes: u64,
    pub lx: i32,
    pub ly: i32,
    pub rx: i32,
    pub ry: i32,
}

fn eixo(cru: i32) -> i32 {
    let valor = cru.clamp(-32768, 32767);
    if valor.abs() <= ZONA_MORTA {
        return 0;
    }
    let escalado = i64::from(valor) * i64::from(AXIS_CURSO) / i64::from(CURSO_DO_JOY);
    escalado.clamp(-i64::from(AXIS_CURSO), i64::from(AXIS_CURSO)) as i32
}

pub fn para_pad(leitura: &Leitura) -> Pad {
    let mut pad = Pad::default();
    for (bit, nome) in MAPA {
        if leitura.botoes & bit != 0
            && let Some(indice) = Pad::button_by_name(nome)
        {
            pad.press(indice, true);
        }
    }
    // Cima no Switch é Y positivo. No console, cima é o valor negativo.
    pad.set_axis(0, eixo(leitura.lx));
    pad.set_axis(1, eixo(-leitura.ly));
    pad.set_axis(2, eixo(leitura.rx));
    pad.set_axis(3, eixo(-leitura.ry));
    pad
}

/// Minus e Plus ao mesmo tempo são o pedido de sair, e não entram no jogo. Soltar os dois no
/// guest enquanto a corda está presa evita abrir o menu do jogo no meio do gesto de saída.
pub fn filtra_saida(botoes: u64) -> u64 {
    if sai_do_jogo(botoes) {
        botoes & !(MINUS | PLUS)
    } else {
        botoes
    }
}

pub fn sai_do_jogo(botoes: u64) -> bool {
    botoes & MINUS != 0 && botoes & PLUS != 0
}

pub fn tempo_de_saida(decorrido: Duration) -> bool {
    decorrido >= SAIDA
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_b_e_o_botao_de_baixo_e_cima_no_analogico_e_negativo() {
        let leitura = Leitura {
            botoes: B | CIMA,
            lx: -32767,
            ly: 32767,
            rx: 32767,
            ry: 0,
            ..Leitura::default()
        };
        let pad = para_pad(&leitura);
        assert!(pad.is_down(Pad::button_by_name("b1").unwrap()));
        assert!(pad.is_down(Pad::button_by_name("up").unwrap()));
        assert!(!pad.is_down(Pad::button_by_name("b4").unwrap()));
        assert_eq!(pad.axes[0], -128, "manche todo à esquerda");
        assert_eq!(pad.axes[1], -128, "cima no Switch é o valor baixo no console");
        assert_eq!(pad.axes[2], 128, "o curso cheio do aparelho é o curso cheio do console");
        assert_eq!(pad.axes[3], 0);
    }

    #[test]
    fn a_zona_morta_segura_o_repouso_e_deixa_passar_o_resto() {
        let parado = Leitura {
            lx: 4000,
            ..Leitura::default()
        };
        assert_eq!(para_pad(&parado).axes[0], 0);
        let andando = Leitura {
            lx: 4097,
            ..parado
        };
        assert!(para_pad(&andando).axes[0] > 0);
    }

    #[test]
    fn minus_e_plus_juntos_saem_e_somem_do_jogo() {
        assert!(!sai_do_jogo(MINUS | B));
        assert!(sai_do_jogo(MINUS | PLUS));
        assert_eq!(filtra_saida(MINUS | PLUS | B), B);
        assert_eq!(filtra_saida(MINUS | B), MINUS | B);
        assert!(!tempo_de_saida(Duration::from_millis(499)));
        assert!(tempo_de_saida(SAIDA));
    }
}
