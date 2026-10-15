//! O turbo: um botão que se aperta e se solta sozinho, no ritmo do jogo.
//!
//! **O ritmo é o do relógio virtual, e não o do host.** O frontend entrega o controle uma vez por
//! volta da janela, e uma volta pode rodar vários quadros do jogo — quatro para alcançar o
//! relógio, dez no fast-forward a 10x. Um turbo que alternasse o botão a cada volta pulsaria dez
//! vezes mais devagar para o jogo no fast-forward, e ficaria parado durante a recuperação. Por isso
//! a decisão de "apertado agora" é da sessão, antes de cada volta do laço de eventos, pela fase do
//! relógio do jogo: o mesmo jogo, com o mesmo turbo, vê os mesmos toques, ande o host como andar.
//! Ver `docs/implementacao/24-velocidade.md`.

use serde::{Deserialize, Serialize};

use super::{Interruptor, ModoDoAtalho};
use crate::input::Pad;

/// Como a tecla de turbo de um jogador funciona. São os modos do RetroArch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModoDoTurbo {
    #[default]
    Desligado,
    /// Segurar a tecla de turbo junto com um botão faz **aquele** botão pulsar.
    Classico,
    /// Um toque na tecla de turbo transforma o botão padrão em turbo, e outro desfaz. O botão
    /// pulsa enquanto está segurado — e não sozinho: um autofire que dispara sem ninguém tocar
    /// atravessaria os menus, onde o botão 1 é o "confirma" da maioria dos jogos.
    UnicoAlternar,
    /// Enquanto a tecla de turbo está segurada, o botão padrão pulsa sozinho.
    UnicoSegurar,
}

impl ModoDoTurbo {
    pub const TODOS: [Self; 4] = [
        Self::Desligado,
        Self::Classico,
        Self::UnicoAlternar,
        Self::UnicoSegurar,
    ];

    pub fn chave(self) -> &'static str {
        match self {
            Self::Desligado => "common.off",
            Self::Classico => "turbo.mode.classic",
            Self::UnicoAlternar => "turbo.mode.toggle",
            Self::UnicoSegurar => "turbo.mode.hold",
        }
    }
}

/// O nome, no mapeamento de cada jogador, da tecla de turbo. Não é botão do Zeebo: o
/// [`Pad::button_by_name`] não o conhece, e por isso ele não aperta nada no jogo.
pub const BOTAO_DO_TURBO: &str = "turbo";

/// O botão padrão de fábrica dos modos de botão único.
pub const BOTAO_PADRAO: &str = "b1";

/// Os botões que podem pulsar, na ordem dos menus: os do mapeamento
/// ([`crate::input::bindings::CONFIGURABLE`]) menos o direcional e o HOME. O RetroArch exclui o
/// direcional por padrão, e o HOME pulsando abriria e fecharia o menu do console.
pub const PULSAVEIS: [&str; 8] = ["b1", "b2", "b3", "b4", "zl", "zr", "lthumb", "rthumb"];

/// Toques por segundo: a faixa e o padrão. Dez é o período de 6 quadros a 60 Hz que o RetroArch
/// usa de fábrica.
pub const TOQUES_MENOS: u8 = 5;
pub const TOQUES_MAIS: u8 = 20;
pub const TOQUES_PADRAO: u8 = 10;

/// A máscara dos botões pulsáveis.
fn mascara_pulsavel() -> u32 {
    PULSAVEIS
        .iter()
        .filter_map(|nome| Pad::button_by_name(nome))
        .fold(0, |mascara, indice| mascara | (1 << indice))
}

/// A máscara de um botão pelo nome, se ele pode pulsar.
fn mascara_do_botao(nome: &str) -> u32 {
    match PULSAVEIS.contains(&nome) {
        true => Pad::button_by_name(nome).map_or(0, |indice| 1 << indice),
        false => 0,
    }
}

/// O turbo de uma porta: o que é preciso lembrar de uma volta para a outra.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurboDaPorta {
    /// O estado do modo de alternar.
    alternar: Interruptor,
}

impl TurboDaPorta {
    /// Os botões que pulsam nesta volta, como máscara do [`Pad`].
    ///
    /// `cru` é o controle como o jogador o segura, e `apertado` diz se a tecla de turbo está
    /// apertada. No modo de segurar, o botão padrão pulsa mesmo que o jogador não o segure: é o
    /// que o modo quer dizer.
    pub fn pulsando(&mut self, modo: ModoDoTurbo, botao: &str, cru: &Pad, apertado: bool) -> u32 {
        match modo {
            ModoDoTurbo::Desligado => {
                self.alternar.desliga();
                0
            }
            ModoDoTurbo::Classico => match apertado {
                true => cru.buttons & mascara_pulsavel(),
                false => 0,
            },
            ModoDoTurbo::UnicoSegurar => match apertado {
                true => mascara_do_botao(botao),
                false => 0,
            },
            ModoDoTurbo::UnicoAlternar => {
                match self.alternar.atualiza(apertado, ModoDoAtalho::Alternar) {
                    true => cru.buttons & mascara_do_botao(botao),
                    false => 0,
                }
            }
        }
    }

    /// Se o modo de alternar está ligado: a janela diz "Turbo" na tela, porque um turbo esquecido
    /// ligado parece defeito do jogo.
    pub fn ligado(&self) -> bool {
        self.alternar.ligado()
    }

    pub fn desliga(&mut self) {
        self.alternar.desliga();
    }
}

/// Se os botões que pulsam estão apertados no instante virtual `relogio_ms`.
///
/// Metade do período apertado, metade solto: o ciclo de trabalho é fixo em 50%.
pub fn fase_apertada(relogio_ms: u32, toques_por_segundo: u8) -> bool {
    let toques = u32::from(toques_por_segundo.clamp(TOQUES_MENOS, TOQUES_MAIS));
    let meio_periodo = (500 / toques).max(1);
    (relogio_ms / meio_periodo).is_multiple_of(2)
}

/// O controle que o jogo vê: o cru, com os botões que pulsam na fase de agora.
pub fn modula(cru: Pad, pulsando: u32, apertada: bool) -> Pad {
    let mut efetivo = cru;
    efetivo.buttons &= !pulsando;
    if apertada {
        efetivo.buttons |= pulsando;
    }
    efetivo
}

#[cfg(test)]
mod tests {
    use super::*;

    fn com(botoes: &[&str]) -> Pad {
        let mut pad = Pad::default();
        for nome in botoes {
            pad.press(Pad::button_by_name(nome).unwrap(), true);
        }
        pad
    }

    fn bit(nome: &str) -> u32 {
        1 << Pad::button_by_name(nome).unwrap()
    }

    /// Dez toques por segundo são 50 ms apertado e 50 ms solto, pelo relógio do jogo.
    #[test]
    fn a_fase_segue_o_relogio_virtual() {
        let fases: Vec<bool> = [0, 49, 50, 99, 100].iter().map(|&t| fase_apertada(t, 10)).collect();
        assert_eq!(fases, [true, true, false, false, true]);
        // Vinte por segundo: 25 ms cada metade.
        assert!(!fase_apertada(25, 20));
        // Fora da faixa, vale o limite.
        assert_eq!(fase_apertada(110, 200), fase_apertada(110, TOQUES_MAIS));
    }

    /// O clássico pulsa o que está segurado junto com o turbo — menos o direcional e o HOME.
    #[test]
    fn o_classico_pulsa_o_que_se_segura_menos_direcional_e_home() {
        let mut turbo = TurboDaPorta::default();
        let cru = com(&["b1", "b3", "up", "back"]);
        let pulsando = turbo.pulsando(ModoDoTurbo::Classico, BOTAO_PADRAO, &cru, true);
        assert_eq!(pulsando, bit("b1") | bit("b3"));
        assert_eq!(turbo.pulsando(ModoDoTurbo::Classico, BOTAO_PADRAO, &cru, false), 0);
    }

    /// Segurar pulsa o botão padrão sem ele estar segurado; alternar só enquanto ele está.
    #[test]
    fn os_modos_de_botao_unico() {
        let mut turbo = TurboDaPorta::default();
        let nada = Pad::default();
        assert_eq!(turbo.pulsando(ModoDoTurbo::UnicoSegurar, "b2", &nada, true), bit("b2"));
        assert_eq!(turbo.pulsando(ModoDoTurbo::UnicoSegurar, "b2", &nada, false), 0);

        let mut turbo = TurboDaPorta::default();
        let b1 = com(&["b1"]);
        // Um toque liga; o botão pulsa só segurado.
        assert_eq!(turbo.pulsando(ModoDoTurbo::UnicoAlternar, "b1", &nada, true), 0);
        assert!(turbo.ligado());
        assert_eq!(turbo.pulsando(ModoDoTurbo::UnicoAlternar, "b1", &b1, false), bit("b1"));
        // Outro toque desliga.
        turbo.pulsando(ModoDoTurbo::UnicoAlternar, "b1", &b1, true);
        assert!(!turbo.ligado());
        assert_eq!(turbo.pulsando(ModoDoTurbo::UnicoAlternar, "b1", &b1, false), 0);
    }

    /// Um botão padrão que não pode pulsar não pulsa, e desligar o modo zera o alternar.
    #[test]
    fn botao_que_nao_pulsa_e_desligar() {
        let mut turbo = TurboDaPorta::default();
        assert_eq!(turbo.pulsando(ModoDoTurbo::UnicoSegurar, "back", &Pad::default(), true), 0);
        turbo.pulsando(ModoDoTurbo::UnicoAlternar, "b1", &Pad::default(), true);
        assert!(turbo.ligado());
        turbo.pulsando(ModoDoTurbo::Desligado, "b1", &Pad::default(), false);
        assert!(!turbo.ligado());
    }

    #[test]
    fn modular_so_mexe_no_que_pulsa() {
        let cru = com(&["b1", "up"]);
        let pulsando = bit("b1") | bit("b2");
        assert_eq!(modula(cru, pulsando, true).buttons, cru.buttons | bit("b2"));
        assert_eq!(modula(cru, pulsando, false).buttons, bit("up"));
    }
}
