//! Os controles de verdade ligados no computador.
//!
//! Este módulo é a ponte entre os nomes que o [`crate::bindings`] guarda e o que a biblioteca
//! de controles entende. Os nomes são guardados como texto justamente para o arquivo de
//! configuração não depender da numeração interna de biblioteca nenhuma.

use gilrs::{Axis, Button, Gilrs};

use crate::bindings::Source;

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
    pub fn names(&self) -> Vec<String> {
        let Some(gilrs) = &self.gilrs else {
            return Vec::new();
        };
        gilrs
            .gamepads()
            .map(|(_, pad)| pad.name().to_string())
            .collect()
    }

    /// O controle de nome `device`, ou o primeiro ligado se `device` for `None`.
    fn find(&self, device: Option<&str>) -> Option<gilrs::Gamepad<'_>> {
        let gilrs = self.gilrs.as_ref()?;
        match device {
            Some(name) => gilrs.gamepads().find(|(_, pad)| pad.name() == name),
            None => gilrs.gamepads().next(),
        }
        .map(|(_, pad)| pad)
    }

    /// Se a origem está acionada no controle do jogador.
    ///
    /// Origens de teclado não pertencem aqui: quem sabe do teclado é a janela.
    pub fn is_active(&self, device: Option<&str>, source: &Source) -> bool {
        let Some(pad) = self.find(device) else {
            return false;
        };
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

    /// O curso de um eixo do controle, de -1 a 1. `None` se não há controle ou o eixo é
    /// desconhecido — e aí o mapeamento daquele eixo simplesmente não vale.
    pub fn value(&self, device: Option<&str>, axis: &str) -> Option<f32> {
        let pad = self.find(device)?;
        Some(pad.value(axis_by_name(axis)?))
    }

    /// A primeira origem acionada agora, para a tela de configuração capturar.
    ///
    /// Os botões vêm antes dos eixos: quem aperta o direcional de cruz de um controle que
    /// também o reporta como eixo quer o botão, que é o mais específico.
    pub fn first_active(&self, device: Option<&str>) -> Option<Source> {
        let pad = self.find(device)?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let player = crate::bindings::Player::with_gamepad("qualquer".into());
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
        assert!(!pads.is_active(None, &Source::button("South")));
        assert_eq!(pads.first_active(None), None);
    }
}
