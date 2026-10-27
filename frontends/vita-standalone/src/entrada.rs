//! O controle da Vita virando o controle do Zeebo.
//!
//! Os números são os `SceCtrlButtons` do vitasdk, no firmware 3.60. A posição é a do aparelho,
//! a mesma tabela do RetroPad no core: o botão de baixo é o 1, o da esquerda é o 2, o de cima é
//! o 3 e o da direita é o 4.

use zeebx::input::Pad;

pub const SELECT: u32 = 0x0000_0001;
pub const CLIQUE_ESQUERDO: u32 = 0x0000_0002;
pub const CLIQUE_DIREITO: u32 = 0x0000_0004;
pub const START: u32 = 0x0000_0008;
pub const CIMA: u32 = 0x0000_0010;
pub const DIREITA: u32 = 0x0000_0020;
pub const BAIXO: u32 = 0x0000_0040;
pub const ESQUERDA: u32 = 0x0000_0080;
pub const GATILHO_ESQUERDO: u32 = 0x0000_0100;
pub const GATILHO_DIREITO: u32 = 0x0000_0200;
pub const TRIANGULO: u32 = 0x0000_1000;
pub const CIRCULO: u32 = 0x0000_2000;
pub const CRUZ: u32 = 0x0000_4000;
pub const QUADRADO: u32 = 0x0000_8000;

/// Quanto o manche pode tremer em volta do centro sem o jogo andar.
///
/// O repouso do analógico da Vita é 128, e parado ele oscila alguns pontos. Sem este corte o
/// boneco caminha sozinho, que é o defeito que o centro em 128 do console existe para evitar
/// quando o valor chega errado.
pub const ZONA_MORTA: i32 = 16;

/// Select e Start juntos, segurados, devolvem à lista. Meio segundo: um toque dos dois no meio
/// de um jogo não pode sair.
pub const SAIDA: std::time::Duration = std::time::Duration::from_millis(500);

const MAPA: &[(u32, &str)] = &[
    (CIMA, "up"),
    (BAIXO, "down"),
    (ESQUERDA, "left"),
    (DIREITA, "right"),
    (CRUZ, "b1"),
    (QUADRADO, "b2"),
    (TRIANGULO, "b3"),
    (CIRCULO, "b4"),
    (GATILHO_ESQUERDO, "zl"),
    (GATILHO_DIREITO, "zr"),
    (START, "start"),
    (SELECT, "back"),
    (CLIQUE_ESQUERDO, "lthumb"),
    (CLIQUE_DIREITO, "rthumb"),
];

/// Uma leitura do `sceCtrl`, já separada do layout da struct do firmware.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Leitura {
    pub botoes: u32,
    pub lx: u8,
    pub ly: u8,
    pub rx: u8,
    pub ry: u8,
}

/// O manche da Vita cresce para a direita e para baixo, com o centro em 128. O eixo interno do
/// console cresce igual e guarda o valor já centrado em zero: cima é negativo. Por isso o `ly`
/// cru entra direto, sem inverter.
fn eixo(cru: u8) -> i32 {
    let valor = i32::from(cru) - 128;
    if valor.abs() <= ZONA_MORTA { 0 } else { valor }
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
    pad.set_axis(0, eixo(leitura.lx));
    pad.set_axis(1, eixo(leitura.ly));
    pad.set_axis(2, eixo(leitura.rx));
    pad.set_axis(3, eixo(leitura.ry));
    pad
}

/// Select e Start ao mesmo tempo são o pedido de sair, e não entram no jogo. Soltar os dois no
/// guest enquanto a corda está presa evita abrir o menu do jogo no meio do gesto de saída.
pub fn filtra_saida(botoes: u32) -> u32 {
    if sai_do_jogo(botoes) {
        botoes & !(SELECT | START)
    } else {
        botoes
    }
}

pub fn sai_do_jogo(botoes: u32) -> bool {
    botoes & SELECT != 0 && botoes & START != 0
}

pub fn tempo_de_saida(decorrido: std::time::Duration) -> bool {
    decorrido >= SAIDA
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cruz_e_o_botao_de_baixo_e_o_direcional_e_botao() {
        let leitura = Leitura {
            botoes: CRUZ | CIMA,
            lx: 0,
            ly: 128,
            rx: 255,
            ry: 128,
        };
        let pad = para_pad(&leitura);
        assert!(pad.is_down(Pad::button_by_name("b1").unwrap()));
        assert!(pad.is_down(Pad::button_by_name("up").unwrap()));
        assert!(!pad.is_down(Pad::button_by_name("b4").unwrap()));
        assert_eq!(pad.axes[0], -128, "manche todo à esquerda");
        assert_eq!(pad.axes[1], 0, "o centro não anda");
        assert_eq!(pad.axes[2], 127, "manche direito todo à direita");
    }

    #[test]
    fn a_zona_morta_segura_o_repouso_e_deixa_passar_o_resto() {
        let parado = Leitura {
            lx: 140,
            ly: 128,
            rx: 128,
            ry: 128,
            ..Leitura::default()
        };
        assert_eq!(para_pad(&parado).axes[0], 0);
        let andando = Leitura { lx: 145, ..parado };
        assert_eq!(para_pad(&andando).axes[0], 17);
    }

    #[test]
    fn select_e_start_juntos_saem_e_somem_do_jogo() {
        assert!(!sai_do_jogo(SELECT | CRUZ));
        assert!(sai_do_jogo(SELECT | START));
        assert_eq!(filtra_saida(SELECT | START | CRUZ), CRUZ);
        assert_eq!(filtra_saida(SELECT | CRUZ), SELECT | CRUZ);
        assert!(!tempo_de_saida(std::time::Duration::from_millis(499)));
        assert!(tempo_de_saida(SAIDA));
    }
}
