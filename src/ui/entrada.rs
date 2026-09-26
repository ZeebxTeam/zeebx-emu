//! A entrada do desktop: os controles do host, os Wii Remotes e os sensores de movimento, lidos
//! pelo mapeamento de cada porta.
//!
//! Saiu do `App` do egui para que a janela Qt monte o controle e o movimento do Boomerang do mesmo
//! jeito — ver `docs/implementacao/21-migracao-para-qt.md`, fase 1. O teclado é a única parte que
//! cada janela lê por conta própria: ela entrega o conjunto de teclas apertadas, como `egui::Key`,
//! e o mapeamento é conferido contra ele.

use std::collections::HashSet;

use crate::input::bindings::{Aparelho, Controls, Source};
use crate::input::gamepads::Gamepads;
use crate::input::sensores::{EstadoDoSensor, Sensores};
use crate::input::wiimote::{EstadoWiimote, Wiimotes};
use crate::input::{PORTAS, Pad};

/// O VID da Nintendo, cujos controles têm o comprimento no Y. Ver
/// [`EntradaDoDesktop::movimento_da_porta`].
const VENDOR_NINTENDO: u16 = 0x057e;

/// O movimento de uma porta sem sensor: parado, de face para cima.
pub const EM_REPOUSO: [f32; 3] = [0.0, 0.0, 1.0];

/// De onde vem o movimento do Boomerang de uma porta. Ver [`EntradaDoDesktop::sensor_da_porta`].
pub enum SensorDaPorta {
    Wiimote(EstadoWiimote),
    Controle(EstadoDoSensor),
    /// Um controle escolhido que não tem sensor de movimento, pelo nome da lista.
    SemSensor(String),
    Nenhum,
}

impl SensorDaPorta {
    /// O que dizer sobre o sensor, na tela de controles e no aviso de calibração.
    pub fn descreve(&self, catalogo: &crate::ui::i18n::Catalog) -> String {
        let (chave, nome) = match self {
            Self::Wiimote(w) if w.com_acelerometro => ("controls.boomerang.sensor", "Wii Remote"),
            Self::Wiimote(_) => ("controls.boomerang.sensor_waiting", "Wii Remote"),
            Self::Controle(s) if s.sem_permissao => ("controls.boomerang.sensor_denied", s.nome.as_str()),
            Self::Controle(s) if s.com_leitura => ("controls.boomerang.sensor", s.nome.as_str()),
            Self::Controle(s) => ("controls.boomerang.sensor_waiting", s.nome.as_str()),
            Self::SemSensor(nome) => ("controls.boomerang.no_sensor", nome.as_str()),
            Self::Nenhum => ("controls.boomerang.no_device", ""),
        };
        catalogo.get(chave).replace("{nome}", nome)
    }

    /// A aceleração medida, quando o sensor já mandou alguma.
    pub fn aceleracao(&self) -> Option<[f32; 3]> {
        match self {
            Self::Wiimote(wiimote) if wiimote.com_acelerometro => Some(wiimote.aceleracao),
            Self::Controle(sensor) if sensor.com_leitura => Some(sensor.aceleracao),
            _ => None,
        }
    }
}

/// Se uma origem de teclado do mapeamento está apertada.
///
/// **A comparação é pela tecla, nunca pelo texto.** O mesmo botão aparece com duas grafias no
/// `settings.json`: o mapeamento de fábrica grava `ArrowUp` (`input/bindings.rs`), e uma tecla
/// capturada na tela de controles grava o `egui::Key::name()`, que é `Up`. O `from_name` aceita as
/// duas. A primeira versão da janela Qt comparava o texto, e o direcional do teclado não chegava
/// aos jogos — só à Z-Wheel, que navega pelas teclas do BREW e não pelo controle.
pub fn tecla_apertada(fonte: &Source, teclas: &HashSet<egui::Key>) -> bool {
    match fonte {
        Source::Key { name } => egui::Key::from_name(name).is_some_and(|tecla| teclas.contains(&tecla)),
        _ => false,
    }
}

pub struct EntradaDoDesktop {
    /// Os controles de verdade ligados no computador.
    pub gamepads: Gamepads,
    /// Os Wii Remotes: controles mapeáveis como os outros, com o acelerômetro deles.
    pub wiimotes: Wiimotes,
    /// Os sensores de movimento dos outros controles, que alimentam o Boomerang da porta.
    pub sensores: Sensores,
}

impl EntradaDoDesktop {
    pub fn inicia() -> Self {
        Self {
            gamepads: Gamepads::default(),
            wiimotes: Wiimotes::inicia(),
            sensores: Sensores::inicia(),
        }
    }

    /// O estado de cada porta ligada agora, montado a partir do mapeamento dela.
    ///
    /// Uma porta desligada não entra: o que sai daqui são os pares `(porta, controle)` das que
    /// estão ligadas, e é o que a sessão entrega ao jogo.
    pub fn pads(&mut self, controles: &Controls, teclas: &HashSet<egui::Key>) -> Vec<(usize, Pad)> {
        self.gamepads.poll();
        controles
            .ligadas()
            .map(|(porta, _)| (porta, self.pad(controles, porta, teclas)))
            .collect()
    }

    /// O estado do controle de uma porta, montado a partir do mapeamento dela.
    ///
    /// O teclado e o controle são consultados juntos: quem tem os dois pode usar os dois, e é
    /// isso que ter mais de uma origem por botão significa.
    pub fn pad(&self, controles: &Controls, porta: usize, teclas: &HashSet<egui::Key>) -> Pad {
        let Some(player) = controles.player(porta) else {
            return Pad::default();
        };
        let device = player.device.as_deref();
        let gamepads = &self.gamepads;
        // O Wii Remote não passa pelo gilrs: os botões dele são origens próprias (`WiiA`,
        // `Wii1`…), lidas do estado que a thread dele mantém.
        let wiimote = self.wiimote_da_porta(controles, porta);
        let mut pad = player.pad(
            |source| {
                tecla_apertada(source, teclas)
                    || gamepads.is_active(device, porta, source)
                    || wiimote.is_some_and(|w| w.fonte_acionada(source))
            },
            |axis| gamepads.value(device, porta, axis),
        );
        // O Boomerang sem controle escolhido pega o Wii Remote da vez, e aí o mapeamento da
        // porta é o de teclado, sem nenhum botão dele: os botões se somam por esta tabela, que é
        // o mapeamento padrão do Wii Remote.
        if let (None, Some(wiimote)) = (device, wiimote) {
            const DO_WIIMOTE: [(&str, &str); 9] = [
                ("up", "up"),
                ("down", "down"),
                ("left", "left"),
                ("right", "right"),
                ("1", "b1"),
                ("a", "b1"),
                ("2", "b2"),
                ("b", "b2"),
                ("home", "back"),
            ];
            for (dele, nosso) in DO_WIIMOTE {
                if wiimote.apertado(dele)
                    && let Some(indice) = Pad::button_by_name(nosso)
                {
                    pad.press(indice, true);
                }
            }
        }
        pad
    }

    /// O Wii Remote de uma porta: o escolhido na lista de controles, ou, numa porta de
    /// Boomerang sem controle escolhido, o primeiro Wii Remote para o primeiro Boomerang e o
    /// segundo para o segundo. Com outro controle escolhido, nenhum.
    pub fn wiimote_da_porta(&self, controles: &Controls, porta: usize) -> Option<EstadoWiimote> {
        let jogador = controles.player(porta)?;
        if let Some(escolhido) = jogador.device.as_deref() {
            return self.wiimotes.estado(Wiimotes::indice_do_nome(escolhido)?);
        }
        if jogador.aparelho != Aparelho::Boomerang {
            return None;
        }
        let boomerangs: Vec<usize> = controles
            .ligadas()
            .filter(|(_, jogador)| jogador.aparelho == Aparelho::Boomerang)
            .map(|(indice, _)| indice)
            .collect();
        let ordem = boomerangs.iter().position(|&p| p == porta)?;
        self.wiimotes.estado(ordem)
    }

    /// O sensor de movimento que alimenta o Boomerang de uma porta.
    ///
    /// **É o do controle escolhido na porta**, seja ele qual for: um Wii Remote, um Pro
    /// Controller, um DualShock. Antes, uma porta de Boomerang procurava sempre um Wii Remote, e
    /// quem escolhia outro controle com sensor via o Boomerang parado. Sem controle escolhido,
    /// vale o Wii Remote da vez — o primeiro para o primeiro Boomerang.
    pub fn sensor_da_porta(&self, controles: &Controls, porta: usize) -> SensorDaPorta {
        let Some(jogador) = controles.player(porta) else {
            return SensorDaPorta::Nenhum;
        };
        let escolhido = jogador.device.as_deref();
        if escolhido.is_none_or(|nome| Wiimotes::indice_do_nome(nome).is_some()) {
            return match self.wiimote_da_porta(controles, porta) {
                Some(wiimote) => SensorDaPorta::Wiimote(wiimote),
                None => SensorDaPorta::Nenhum,
            };
        }
        let Some((nome, vendor, product, ordem)) = self.gamepads.identidade(escolhido, porta) else {
            return SensorDaPorta::Nenhum;
        };
        match self.sensores.do_controle(&nome, vendor, product, ordem) {
            Some(sensor) => SensorDaPorta::Controle(sensor),
            None => SensorDaPorta::SemSensor(escolhido.unwrap_or(&nome).to_string()),
        }
    }

    /// A aceleração que o Boomerang de uma porta sente. Sem sensor, parado de face para cima.
    ///
    /// **O comprimento do Boomerang é o X dele; o do Wii Remote é o Y.** O Crash Nitro Kart manda
    /// segurar o Boomerang deitado, com as duas mãos e a face para o jogador, e virar como um
    /// volante: a direção é o ângulo da gravidade entre X e Y. Com os eixos passando direto, o Wii
    /// Remote seguro do mesmo jeito punha a gravidade no eixo errado, e o kart virava a esmo. A
    /// face é a mesma nos dois, então o Z fica, e o X e o Y trocam de lugar.
    ///
    /// O sentido do X foi acertado na mão, no Crash Nitro Kart: com o X do Boomerang oposto ao Y
    /// do Wii Remote, virar o volante para a direita levava o kart para a esquerda.
    ///
    /// Os controles da Nintendo também trocam X e Y. O `hid-nintendo` reporta o comprimento do
    /// Pro Controller no Y: em pé, de frente para o jogador, a gravidade caía no X do Boomerang, e
    /// a prévia girava noventa graus para a direita. E o sentido é o oposto do Wii Remote: com o
    /// Y passando como veio, virar para a esquerda levava o kart para a direita. Os outros
    /// controles passam direto até alguém medir.
    pub fn movimento_da_porta(&self, controles: &Controls, porta: usize) -> [f32; 3] {
        let sensor = self.sensor_da_porta(controles, porta);
        let Some(bruto) = sensor.aceleracao() else {
            return EM_REPOUSO;
        };
        let [x, y, z] = controles
            .player(porta)
            .map_or(bruto, |jogador| jogador.calibracao_movimento.aplica(bruto));
        match sensor {
            SensorDaPorta::Wiimote(_) => [y, x, z],
            SensorDaPorta::Controle(s) if s.vendor == VENDOR_NINTENDO => [-y, x, z],
            _ => [x, y, z],
        }
    }

    /// O movimento de todas as portas, na ordem delas.
    pub fn movimentos(&self, controles: &Controls) -> [[f32; 3]; PORTAS] {
        std::array::from_fn(|porta| self.movimento_da_porta(controles, porta))
    }

    /// A aceleração que o sensor da porta mede, sem calibração.
    pub fn movimento_bruto_da_porta(&self, controles: &Controls, porta: usize) -> Option<[f32; 3]> {
        self.sensor_da_porta(controles, porta).aceleracao()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A seta aciona o direcional com as duas grafias que o `settings.json` pode ter.
    #[test]
    fn a_seta_aciona_o_mapeamento_nas_duas_grafias() {
        let apertadas = HashSet::from([egui::Key::ArrowUp]);
        assert!(tecla_apertada(&Source::key("ArrowUp"), &apertadas), "o de fábrica");
        assert!(tecla_apertada(&Source::key("Up"), &apertadas), "o capturado");
        assert!(!tecla_apertada(&Source::key("ArrowDown"), &apertadas));
        assert!(!tecla_apertada(&Source::button("DPadUp"), &apertadas), "botão não é tecla");
    }
}
