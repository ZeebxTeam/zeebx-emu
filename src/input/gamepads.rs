//! Os controles de verdade ligados no computador.
//!
//! Este módulo é a ponte entre os nomes que o [`crate::input::bindings`] guarda e o que a biblioteca
//! de controles entende. Os nomes são guardados como texto justamente para o arquivo de
//! configuração não depender da numeração interna de biblioteca nenhuma.

use gilrs::{Axis, Button, Gilrs};

use crate::input::bindings::Source;

/// A partir de quanto um eixo analógico conta como acionado. Meio curso é o que separa "o
/// jogador empurrou" de "o manche não voltou exatamente ao centro".
const AXIS_THRESHOLD: f32 = 0.5;

/// Os botões que sabemos nomear, com o nome que vai para o arquivo de configuração.
const BUTTONS: [(&str, Button); 19] = [
    ("South", Button::South),
    ("East", Button::East),
    ("North", Button::North),
    ("West", Button::West),
    ("C", Button::C),
    ("Z", Button::Z),
    ("LeftTrigger", Button::LeftTrigger),
    ("LeftTrigger2", Button::LeftTrigger2),
    ("RightTrigger", Button::RightTrigger),
    ("RightTrigger2", Button::RightTrigger2),
    ("Select", Button::Select),
    ("Start", Button::Start),
    ("Mode", Button::Mode),
    ("LeftThumb", Button::LeftThumb),
    ("RightThumb", Button::RightThumb),
    ("DPadUp", Button::DPadUp),
    ("DPadDown", Button::DPadDown),
    ("DPadLeft", Button::DPadLeft),
    ("DPadRight", Button::DPadRight),
];

const AXES: [(&str, Axis); 4] = [
    ("LeftStickX", Axis::LeftStickX),
    ("LeftStickY", Axis::LeftStickY),
    ("RightStickX", Axis::RightStickX),
    ("RightStickY", Axis::RightStickY),
];

/// Os nomes de eixo que a tela de configuração oferece.
pub fn axis_names() -> impl Iterator<Item = &'static str> {
    AXES.iter().map(|(name, _)| *name)
}

pub fn button_by_name(name: &str) -> Option<Button> {
    BUTTONS
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, button)| *button)
}

pub fn axis_by_name(name: &str) -> Option<Axis> {
    AXES.iter()
        .find(|(known, _)| *known == name)
        .map(|(_, axis)| *axis)
}

/// Os controles ligados, e o estado deles.
///
/// Um computador sem nenhum controle — ou sem permissão para lê-los — não pode impedir o
/// emulador de abrir, então a falha vira "nenhum controle" e o teclado segue funcionando.
pub struct Gamepads {
    gilrs: Option<Gilrs>,
}

impl Default for Gamepads {
    fn default() -> Self {
        Self::new()
    }
}

impl Gamepads {
    pub fn new() -> Self {
        Self {
            gilrs: Gilrs::new().ok(),
        }
    }

    /// Consome os eventos pendentes. Precisa ser chamado a cada quadro: é isso que mantém o
    /// estado dos botões em dia e detecta um controle ligado no meio do caminho.
    pub fn poll(&mut self) {
        let Some(gilrs) = &mut self.gilrs else {
            return;
        };
        while gilrs.next_event().is_some() {}
    }

    /// O nome de cada controle ligado, na ordem em que o sistema os lista.
    ///
    /// **Dois controles do mesmo modelo têm o mesmo nome**, e quem escolhe "o segundo" na lista
    /// precisa de algo que o distinga do primeiro. A partir da segunda aparição o nome ganha um
    /// ` #2`, ` #3` e por aí. Enquanto o nome era só o do modelo, marcar o segundo controle na
    /// porta 2 guardava a mesma string da porta 1 — e a prévia, que procura o controle por
    /// nome, encontrava sempre o primeiro: a porta 2 respondia ao controle 1.
    ///
    /// Continua sendo nome, e não índice: o índice do sistema muda quando alguém desliga um
    /// controle, e a configuração salva ontem tem de valer hoje.
    pub fn names(&self) -> Vec<String> {
        let Some(gilrs) = &self.gilrs else {
            return Vec::new();
        };
        numera(gilrs.gamepads().map(|(_, pad)| pad.name().to_string()))
    }

    /// O controle de nome `device`; sem nome, o que está na vez da porta.
    ///
    /// O nome procurado é o da lista do [`Self::names`], com a numeração das repetições.
    ///
    /// **Sem nome escolhido, a porta pega o controle da posição dela**: o primeiro para a porta
    /// um, o segundo para a porta dois. Enquanto toda porta sem escolha pegava o primeiro
    /// controle, ligar a porta dois só para jogar com dois duplicava o controle um nas duas — e
    /// o jogo que pede "jogador 2, aperte um botão" nunca via um segundo jogador de verdade.
    fn find(&self, device: Option<&str>, porta: usize) -> Option<gilrs::Gamepad<'_>> {
        let gilrs = self.gilrs.as_ref()?;
        match device {
            Some(name) => {
                let qual = self.names().iter().position(|n| n == name)?;
                gilrs.gamepads().nth(qual)
            }
            None => gilrs.gamepads().nth(porta),
        }
        .map(|(_, pad)| pad)
    }

    /// Quem é o controle da porta, para achar o sensor de movimento dele: o nome que o sistema
    /// dá, o par VID/PID e a posição entre os controles iguais ligados antes dele.
    ///
    /// É o nome **do sistema**, e não o do mapeamento do gilrs: o sensor ganha o nome do
    /// dispositivo com um sufixo ("Pro Controller" e "Pro Controller (IMU)"), e o gilrs chama o
    /// mesmo controle de "Nintendo Switch Pro Controller".
    pub fn identidade(&self, device: Option<&str>, porta: usize) -> Option<(String, u16, u16, usize)> {
        let gilrs = self.gilrs.as_ref()?;
        let pad = self.find(device, porta)?;
        let chave = |p: &gilrs::Gamepad<'_>| (p.os_name().to_string(), p.vendor_id(), p.product_id());
        let (nome, vendor, product) = chave(&pad);
        let ordem = gilrs
            .gamepads()
            .take_while(|(id, _)| *id != pad.id())
            .filter(|(_, outro)| chave(outro) == (nome.clone(), vendor, product))
            .count();
        Some((nome, vendor?, product?, ordem))
    }

    /// Se a origem está acionada no controle do jogador.
    ///
    /// Origens de teclado não pertencem aqui: quem sabe do teclado é a janela.
    pub fn is_active(&self, device: Option<&str>, porta: usize, source: &Source) -> bool {
        let Some(pad) = self.find(device, porta) else {
            return false;
        };
        acionada(&pad, source)
    }

    /// Se `source` está acionada em **algum** controle ligado.
    ///
    /// É a pergunta dos atalhos da janela, como o fast-forward: eles são da sessão, e não de uma
    /// porta, e quem joga no controle dois avança com o botão do controle dois.
    pub fn algum_ativo(&self, source: &Source) -> bool {
        let Some(gilrs) = &self.gilrs else {
            return false;
        };
        gilrs.gamepads().any(|(_, pad)| acionada(&pad, source))
    }

    /// O primeiro botão apertado em qualquer controle ligado, para capturar um atalho. Só
    /// botões: um atalho num eixo dispararia com o manche fora do centro.
    pub fn primeiro_botao_de_algum(&self) -> Option<Source> {
        let gilrs = self.gilrs.as_ref()?;
        gilrs.gamepads().find_map(|(_, pad)| {
            BUTTONS
                .iter()
                .find(|(_, button)| pad.is_pressed(*button))
                .map(|(name, _)| Source::button(name))
        })
    }

    /// O curso de um eixo do controle, de -1 a 1. `None` se não há controle ou o eixo é
    /// desconhecido — e aí o mapeamento daquele eixo simplesmente não vale.
    pub fn value(&self, device: Option<&str>, porta: usize, axis: &str) -> Option<f32> {
        let pad = self.find(device, porta)?;
        Some(pad.value(axis_by_name(axis)?))
    }

    /// A primeira origem acionada agora, para a tela de configuração capturar.
    ///
    /// Os botões vêm antes dos eixos: quem aperta o direcional de cruz de um controle que
    /// também o reporta como eixo quer o botão, que é o mais específico.
    pub fn first_active(&self, device: Option<&str>, porta: usize) -> Option<Source> {
        let pad = self.find(device, porta)?;
        for (name, button) in BUTTONS {
            if pad.is_pressed(button) {
                return Some(Source::button(name));
            }
        }
        for (name, axis) in AXES {
            let value = pad.value(axis);
            if value.abs() >= AXIS_THRESHOLD {
                return Some(Source::Axis {
                    name: name.to_string(),
                    positive: value > 0.0,
                });
            }
        }
        None
    }
}

/// Se a origem está acionada num controle. O teclado não é daqui: responde que não.
fn acionada(pad: &gilrs::Gamepad<'_>, source: &Source) -> bool {
    match source {
        Source::Key { .. } => false,
        Source::Button { name } => {
            button_by_name(name).is_some_and(|button| pad.is_pressed(button))
        }
        Source::Axis { name, positive } => axis_by_name(name).is_some_and(|axis| {
            let value = pad.value(axis);
            match positive {
                true => value >= AXIS_THRESHOLD,
                false => value <= -AXIS_THRESHOLD,
            }
        }),
    }
}

/// Distingue nomes repetidos acrescentando ` #2`, ` #3` e por aí, na ordem de chegada.
fn numera(nomes: impl Iterator<Item = String>) -> Vec<String> {
    let mut vistos: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    nomes
        .map(|nome| {
            let quantos = vistos.entry(nome.clone()).or_insert(0);
            *quantos += 1;
            match *quantos {
                1 => nome,
                n => format!("{nome} #{n}"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dois_controles_do_mesmo_modelo_ganham_nomes_diferentes() {
        // Sem isto, escolher "o segundo" na lista guarda a mesma string do primeiro e a porta 2
        // acaba respondendo ao controle da porta 1.
        let nomes = ["Z-Pad", "Z-Pad", "Wii Remote", "Z-Pad"];
        assert_eq!(
            numera(nomes.iter().map(|n| n.to_string())),
            ["Z-Pad", "Z-Pad #2", "Wii Remote", "Z-Pad #3"]
        );
    }

    #[test]
    fn os_nomes_de_botao_vao_e_voltam() {
        // O arquivo de configuração guarda o nome, não o número da biblioteca: uma troca de
        // versão não pode remapear o controle de quem já configurou.
        for (name, button) in BUTTONS {
            assert_eq!(button_by_name(name), Some(button), "botão {name}");
        }
        assert_eq!(button_by_name("NaoExiste"), None);
    }

    #[test]
    fn os_nomes_de_eixo_vao_e_voltam() {
        for (name, axis) in AXES {
            assert_eq!(axis_by_name(name), Some(axis), "eixo {name}");
        }
        assert_eq!(axis_by_name("NaoExiste"), None);
    }

    #[test]
    fn o_mapeamento_padrao_de_controle_so_usa_nomes_conhecidos() {
        // Um nome no mapeamento padrão que a biblioteca não conheça vira um botão que nunca
        // funciona, e ninguém descobre até ligar um controle.
        let player = crate::input::bindings::Player::with_gamepad("qualquer".into());
        for sources in player.buttons.values() {
            for source in sources {
                match source {
                    Source::Key { .. } => {}
                    Source::Button { name } => {
                        assert!(button_by_name(name).is_some(), "botão {name}")
                    }
                    Source::Axis { name, .. } => {
                        assert!(axis_by_name(name).is_some(), "eixo {name}")
                    }
                }
            }
        }
    }

    #[test]
    fn sem_controle_nada_esta_acionado() {
        // Um computador sem controle não pode impedir o emulador de abrir.
        let pads = Gamepads { gilrs: None };
        assert!(pads.names().is_empty());
        assert!(!pads.is_active(None, 0, &Source::button("South")));
        assert_eq!(pads.first_active(None, 0), None);
    }
}
