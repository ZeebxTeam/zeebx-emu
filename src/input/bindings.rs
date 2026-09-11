//! De onde vem cada botão do controle do Zeebo.
//!
//! O mapeamento é guardado por **nome** — o nome da tecla, o do botão do controle do host, o do
//! botão do Zeebo — e não por índice. Índices mudam quando a tabela muda; nomes sobrevivem, e é
//! o que permite um arquivo de configuração escrito hoje continuar valendo depois.
//!
//! Este módulo não conhece nem o teclado nem o controle: ele só diz *o que* aciona *o quê*.
//! Quem sabe se a tecla `Z` está apertada é a interface, e é ela que traduz.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::input::{self, Pad};

/// Uma origem de entrada no host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", rename_all = "snake_case")]
pub enum Source {
    /// Uma tecla, pelo nome que a interface usa (`Z`, `Space`, `ArrowUp`).
    Key { name: String },
    /// Um botão de um controle de verdade (`South`, `Start`, `LeftTrigger`).
    Button { name: String },
    /// Um eixo de um controle, com o sentido que conta. É assim que um direcional analógico
    /// aciona um direcional digital.
    Axis { name: String, positive: bool },
}

/// De onde vem um eixo analógico do console.
///
/// É diferente de uma [`Source::Axis`]: aquela pergunta "o jogador empurrou para este lado?" e
/// serve para acionar um botão; esta traz o valor inteiro do eixo, que é o que um analógico é.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisSource {
    /// O eixo do controle do host, pelo nome (`LeftStickX`).
    pub name: String,
    /// Se o sentido vai ao contrário do que o console espera.
    #[serde(default)]
    pub invert: bool,
}

impl AxisSource {
    fn new(name: &str, invert: bool) -> Self {
        Self {
            name: name.to_string(),
            invert,
        }
    }
}

/// Abaixo de quanto curso o analógico conta como parado.
///
/// Sem isso um manche que não volta exatamente ao centro deixaria o eixo tremendo perto do
/// zero, e o jogo veria o controle oscilando sozinho.
const DEADZONE: f32 = 0.12;

impl Source {
    pub fn key(name: &str) -> Self {
        Self::Key {
            name: name.to_string(),
        }
    }

    pub fn button(name: &str) -> Self {
        Self::Button {
            name: name.to_string(),
        }
    }

    /// Como a origem aparece na tela.
    pub fn label(&self) -> String {
        match self {
            Self::Key { name } => name.clone(),
            Self::Button { name } => name.clone(),
            Self::Axis { name, positive } => {
                let sign = match positive {
                    true => '+',
                    false => '-',
                };
                format!("{name}{sign}")
            }
        }
    }

    /// Se a origem é do teclado — o que decide de onde a interface vai lê-la.
    pub fn is_key(&self) -> bool {
        matches!(self, Self::Key { .. })
    }
}

/// O que o console enxerga ligado numa porta.
///
/// O Zeebo tem duas USB e aceita as duas coisas: o `IHID::GetConnectedDevices` da Z-Wheel é
/// chamado duas vezes, uma pedindo `0x0106c3fd` — o joystick — e outra pedindo `0x0106c3fc`.
/// Quando a segunda volta vazia, ela imprime `No keyboard reported`. Ou seja, **teclado é um
/// aparelho de verdade** para o console, não um jeito de falar.
///
/// Isto é separado de qual controle do host alimenta a porta: teclado do computador movendo um
/// joystick do console é a combinação que todo jogo entende, e continua sendo o padrão.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Aparelho {
    /// O controle do Zeebo. É o que todos os jogos usam.
    Controle,
    /// Um teclado USB. O console enumera; jogo que o use, ainda não vimos.
    Teclado,
}

impl Default for Aparelho {
    fn default() -> Self {
        Self::Controle
    }
}

/// O mapeamento de um jogador.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Player {
    /// Se a porta tem alguma coisa ligada. Uma porta desligada não é enumerada, e é assim que
    /// se testa um jogo que se comporta diferente com dois controles.
    pub ligada: bool,
    /// Que aparelho o console vê nesta porta.
    pub aparelho: Aparelho,
    /// Qual controle do host alimenta este jogador, pelo nome que o sistema dá a ele. `None`
    /// deixa o jogador só no teclado.
    pub device: Option<String>,
    /// As origens de cada botão do Zeebo, pelo nome do botão. Um botão pode ter mais de uma —
    /// é o que permite `Espaço` e `X` fazerem a mesma coisa.
    pub buttons: BTreeMap<String, Vec<Source>>,
    /// De onde vem cada eixo analógico do console, pelo nome do eixo (`x`, `y`, `z`, `rz`).
    /// Vazio deixa os eixos por conta do direcional digital, que é o que o teclado permite.
    #[serde(default)]
    pub axes: BTreeMap<String, AxisSource>,
}

impl Default for Player {
    /// O padrão é o de qualquer emulador: a mão esquerda nas setas e a direita nas teclas de
    /// ação. O `H` no HOME está aí porque o botão se chama `Back` por dentro, e ninguém
    /// adivinharia isso lendo "APERTE O BOTÃO HOME" na tela do Double Dragon.
    fn default() -> Self {
        let keyboard: [(&str, &[&str]); 12] = [
            ("up", &["ArrowUp"]),
            ("down", &["ArrowDown"]),
            ("left", &["ArrowLeft"]),
            ("right", &["ArrowRight"]),
            ("b1", &["Z"]),
            ("b2", &["Space", "X"]),
            ("b3", &["C"]),
            ("b4", &["V"]),
            ("zl", &["Q"]),
            ("zr", &["W"]),
            ("lthumb", &["F"]),
            ("rthumb", &["G"]),
        ];
        let mut buttons: BTreeMap<String, Vec<Source>> = keyboard
            .iter()
            .map(|(button, keys)| {
                (
                    button.to_string(),
                    keys.iter().map(|key| Source::key(key)).collect(),
                )
            })
            .collect();
        // O HOME do console é o `Back` do BREW, e responde pelos dois nomes. O controle do
        // Zeebo não tem Start: o HOME faz esse papel, então o Enter cai nele.
        buttons.insert(
            "back".to_string(),
            vec![
                Source::key("H"),
                Source::key("Backspace"),
                Source::key("Enter"),
            ],
        );
        Self {
            // A porta um nasce ligada; as outras, não. Ver [`Controls::default`].
            ligada: true,
            aparelho: Aparelho::Controle,
            device: None,
            buttons,
            // O teclado não tem analógico: os eixos ficam com o direcional digital.
            axes: BTreeMap::new(),
        }
    }
}

impl Player {
    /// O mapeamento típico de um controle moderno, para quem liga um e quer jogar.
    pub fn with_gamepad(device: String) -> Self {
        let pad: [(&str, &str); 9] = [
            ("b1", "South"),
            ("b2", "East"),
            ("b3", "West"),
            ("b4", "North"),
            ("zl", "LeftTrigger"),
            ("zr", "RightTrigger"),
            // O controle do Zeebo não tem Start; o HOME ocupa o lugar dele.
            ("back", "Start"),
            ("lthumb", "LeftThumb"),
            ("rthumb", "RightThumb"),
        ];
        let dpad: [(&str, &str); 4] = [
            ("up", "DPadUp"),
            ("down", "DPadDown"),
            ("left", "DPadLeft"),
            ("right", "DPadRight"),
        ];
        let mut player = Self::default();
        for (button, source) in pad.iter().chain(dpad.iter()) {
            player
                .buttons
                .entry(button.to_string())
                .or_default()
                .push(Source::button(source));
        }
        // O manche **não** aperta o direcional. Ele é um eixo, e é como eixo que entra logo
        // abaixo; ligá-lo também aos botões faria os dois agirem juntos e nenhum dos dois
        // sozinho, que é justamente o que se quer evitar num controle com direcional de cruz.
        player.buttons.entry("back".to_string()).or_default().push(
            // O HOME do controle do host cai no HOME do Zeebo, que é o `Back`.
            Source::button("Select"),
        );
        player.axes = Self::default_axes();
        player.device = Some(device);
        player
    }

    /// Os eixos de um controle moderno nos eixos do console.
    ///
    /// O direcional do Zeebo é reportado como `X` e `Y`, então o manche esquerdo cai neles e o
    /// direito nos que sobram, `Z` e `RZ`.
    ///
    /// `Y` vai invertido porque no console cima é o valor negativo, e na biblioteca de controles
    /// cima é positivo. O par da direita segue a mesma convenção por falta de outra referência:
    /// o arquivo do console nomeia `Z` e `RZ` sem dizer o sentido de nenhum dos dois.
    pub fn default_axes() -> BTreeMap<String, AxisSource> {
        [
            ("x", "LeftStickX", false),
            ("y", "LeftStickY", true),
            ("z", "RightStickX", false),
            ("rz", "RightStickY", true),
        ]
        .into_iter()
        .map(|(axis, source, invert)| (axis.to_string(), AxisSource::new(source, invert)))
        .collect()
    }

    /// Dá eixos a um mapeamento salvo antes de eles existirem.
    ///
    /// Sem isto, quem já tinha um controle configurado ficava com os manches mudos até refazer
    /// o mapeamento à mão — e não teria como adivinhar que era preciso.
    fn adopt_axes(&mut self) {
        if self.device.is_none() || !self.axes.is_empty() {
            return;
        }
        self.axes = Self::default_axes();
        // O mapeamento antigo ligava o manche aos botões do direcional, que era o jeito de ele
        // servir para alguma coisa quando não havia eixo. Agora há, e deixar os dois faria o
        // manche apertar o direcional além de mover o eixo.
        for button in ["up", "down", "left", "right"] {
            if let Some(sources) = self.buttons.get_mut(button) {
                sources.retain(|source| !matches!(source, Source::Axis { .. }));
            }
        }
    }

    /// As origens de um botão, ou nada se ele não tem nenhuma.
    pub fn sources(&self, button: &str) -> &[Source] {
        self.buttons.get(button).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Liga `source` a `button`, sem repetir o que já está lá.
    pub fn bind(&mut self, button: &str, source: Source) {
        let sources = self.buttons.entry(button.to_string()).or_default();
        if !sources.contains(&source) {
            sources.push(source);
        }
    }

    /// Desliga tudo de `button`.
    pub fn clear(&mut self, button: &str) {
        self.buttons.remove(button);
    }

    /// Monta o estado do controle: `active` diz se uma origem está acionada, e `analog` dá o
    /// curso de um eixo do controle do host, de -1 a 1.
    pub fn pad(
        &self,
        active: impl Fn(&Source) -> bool,
        analog: impl Fn(&str) -> Option<f32>,
    ) -> Pad {
        let mut pad = Pad::default();
        for (button, sources) in &self.buttons {
            if !sources.iter().any(&active) {
                continue;
            }
            if let Some(index) = Pad::button_by_name(button) {
                pad.press(index, true);
            }
        }
        // O direcional **não** escreve nos eixos. Isto já foi feito aqui, e era uma segunda
        // cópia do mesmo espelhamento que o `Pad::press` fazia: tirar de lá consertou o caminho
        // sem janela e deixou a interface intacta, porque é por aqui que ela monta o controle.
        //
        // O motivo de não fazer está medido no Zeeboids. O menu dele anda uma casa por toque
        // quando o direcional é só botão, e volta para a opção anterior quando também é eixo —
        // soltar a direção manda o eixo de volta ao centro, e essa volta é uma segunda mudança,
        // que o jogo lê como um passo no sentido contrário.
        //
        // O analógico continua entrando, e só quando está fora do centro.
        for (index, name) in input::AXIS_NAMES.iter().enumerate() {
            let Some(source) = self.axes.get(*name) else {
                continue;
            };
            let Some(value) = analog(&source.name) else {
                continue;
            };
            let value = match source.invert {
                true => -value,
                false => value,
            };
            if value.abs() < DEADZONE {
                continue;
            }
            pad.set_axis(index, (value * input::AXIS_MAX as f32) as i32);
        }
        pad
    }
}

/// Os mapeamentos de todos os jogadores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Controls {
    pub players: Vec<Player>,
}

impl Default for Controls {
    /// Uma porta ligada e as outras livres.
    ///
    /// Ligar as duas por omissão seria pior do que parece: quem joga com um controle só veria o
    /// teclado e o controle disputando a **mesma** porta, ou um jogo de dois enxergando um
    /// segundo jogador parado.
    fn default() -> Self {
        let mut players = vec![Player::default()];
        while players.len() < crate::input::PORTAS {
            players.push(Player {
                ligada: false,
                ..Player::default()
            });
        }
        Self { players }
    }
}

impl Controls {
    /// O mapeamento do jogador `index`, criando os que faltarem.
    pub fn player_mut(&mut self, index: usize) -> &mut Player {
        while self.players.len() <= index {
            self.players.push(Player::default());
        }
        &mut self.players[index]
    }

    pub fn player(&self, index: usize) -> Option<&Player> {
        self.players.get(index)
    }

    /// As portas ligadas, em ordem, como `(índice, jogador)`.
    pub fn ligadas(&self) -> impl Iterator<Item = (usize, &Player)> {
        self.players
            .iter()
            .enumerate()
            .take(crate::input::PORTAS)
            .filter(|(_, jogador)| jogador.ligada)
    }

    /// Ajusta um mapeamento vindo de uma versão anterior do emulador.
    ///
    /// Um arquivo salvo antes das portas tem **um** jogador e nenhum campo `ligada`. Quem
    /// resolve isso é o `serde(default)` da struct, que preenche campo faltante a partir do
    /// `Player::default()` — e lá `ligada` é verdadeiro. Assim o controle de quem já usava o
    /// emulador continua ligado, sem precisar de remendo aqui.
    ///
    /// O que falta fazer é só completar as portas que o arquivo antigo não tinha, e elas entram
    /// **desligadas**: ninguém pediu um segundo jogador.
    pub fn adopt(&mut self) {
        for player in &mut self.players {
            player.adopt_axes();
        }
        while self.players.len() < crate::input::PORTAS {
            self.players.push(Player {
                ligada: false,
                ..Player::default()
            });
        }
    }
}

/// Os botões do Zeebo na ordem em que fazem sentido numa tela de configuração — o direcional
/// primeiro, depois as ações, depois os ombros e o resto.
///
/// Não há Start nem par de ombros: o controle do console tem quatro botões de ação, um gatilho
/// de cada lado (o ZL e o ZR) e o HOME, que faz o papel do Start e aparece aqui como `back`
/// porque é no `AEEUID_HIDJoystick_Back` que o `hid_devices.cfg` do console o mapeia.
///
/// Os UIDs que sobram — o Start e os dois ombros "inferiores" — continuam na tabela de
/// [`crate::input::BUTTON_UIDS`], que descreve o descritor HID do aparelho e não a carcaça
/// dele, mas não há como ligá-los a tecla nenhuma.
pub const CONFIGURABLE: [&str; 13] = [
    "up", "down", "left", "right", "b1", "b2", "b3", "b4", "zl", "zr", "lthumb", "rthumb", "back",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::BUTTON_NAMES;

    /// O índice do `up` no controle do console, para os testes não repetirem a busca.
    const DPAD_UP: usize = input::DPAD[0];

    #[test]
    fn os_botoes_configuraveis_existem_no_controle() {
        // A lista da tela e a tabela do console precisam concordar: um nome que não existe
        // vira uma linha que não faz nada, e ninguém descobre até tentar usar.
        for name in CONFIGURABLE {
            assert!(BUTTON_NAMES.contains(&name), "o botão {name} não existe");
        }
    }

    #[test]
    fn o_padrao_cobre_todo_botao_configuravel() {
        let player = Player::default();
        for button in CONFIGURABLE {
            assert!(
                !player.sources(button).is_empty(),
                "o botão {button} ficou sem tecla"
            );
        }
    }

    #[test]
    fn uma_origem_acionada_aperta_o_botao() {
        let player = Player::default();
        let pad = player.pad(|source| *source == Source::key("Z"), |_| None);
        assert!(pad.is_down(Pad::button_by_name("b1").unwrap()));
        assert!(!pad.is_down(Pad::button_by_name("b2").unwrap()));
    }

    #[test]
    fn duas_teclas_no_mesmo_botao_valem_as_duas() {
        // `Espaço` e `X` fazem a mesma coisa, e é isso que ter mais de uma origem significa.
        let player = Player::default();
        let b2 = Pad::button_by_name("b2").unwrap();
        assert!(
            player
                .pad(|s| *s == Source::key("Space"), |_| None)
                .is_down(b2)
        );
        assert!(player.pad(|s| *s == Source::key("X"), |_| None).is_down(b2));
    }

    #[test]
    fn o_direcional_nao_sai_pelos_eixos() {
        // Ele é botão, e só. Enquanto era os dois, um jogo que lê os dois canais andava duas
        // casas por toque: soltar a direção devolve o eixo ao centro, e essa volta é uma
        // segunda mudança, que o jogo lê como um passo no sentido contrário.
        //
        // Este teste é a segunda metade de um conserto. A primeira tirou o espelhamento do
        // `Pad::press`, e a interface continuou errada porque a cópia daqui ficou — é por aqui
        // que ela monta o controle, e o caminho sem janela não passa por aqui.
        let player = Player::default();
        let direita = Pad::button_by_name("right").unwrap();
        let pad = player.pad(|s| *s == Source::key("ArrowRight"), |_| None);
        assert!(pad.is_down(direita));
        assert_eq!(pad.axes, [0; 4]);
        let pad = player.pad(|s| *s == Source::key("ArrowUp"), |_| None);
        assert_eq!(pad.axes, [0; 4]);
        assert_eq!(player.pad(|_| false, |_| None).axes, [0; 4]);
    }

    #[test]
    fn ligar_e_desligar_uma_origem() {
        let mut player = Player::default();
        player.bind("b1", Source::button("South"));
        assert_eq!(player.sources("b1").len(), 2);
        // Ligar de novo não duplica.
        player.bind("b1", Source::button("South"));
        assert_eq!(player.sources("b1").len(), 2);
        player.clear("b1");
        assert!(player.sources("b1").is_empty());
    }

    #[test]
    fn o_mapeamento_de_controle_mantem_o_teclado() {
        // Ligar um controle não pode tirar o teclado de quem estava usando os dois.
        let player = Player::with_gamepad("Meu Controle".into());
        let b1 = Pad::button_by_name("b1").unwrap();
        assert!(player.pad(|s| *s == Source::key("Z"), |_| None).is_down(b1));
        assert!(
            player
                .pad(|s| *s == Source::button("South"), |_| None)
                .is_down(b1)
        );
        assert_eq!(player.device.as_deref(), Some("Meu Controle"));
    }

    #[test]
    fn o_analogico_escreve_o_eixo_com_o_curso_inteiro() {
        // O manche não é um direcional com outro nome: o jogo precisa ver meio curso como meio
        // curso, e não como "empurrado até o fim".
        let player = Player::with_gamepad("Meu Controle".into());
        let pad = player.pad(
            |_| false,
            |axis| match axis {
                "LeftStickX" => Some(0.5),
                _ => None,
            },
        );
        assert_eq!(pad.axes[0], input::AXIS_MAX / 2);
        assert_eq!(pad.axes[1], 0);
    }

    #[test]
    fn cima_no_analogico_e_negativo_no_console() {
        // A biblioteca de controles diz que cima é positivo; o console, que é negativo. Errar
        // este sinal inverte o eixo vertical de todo jogo que o lê.
        let player = Player::with_gamepad("Meu Controle".into());
        let pad = player.pad(
            |_| false,
            |axis| match axis {
                "LeftStickY" => Some(1.0),
                _ => None,
            },
        );
        assert!(pad.axes[1] < 0, "cima deu {}", pad.axes[1]);
    }

    #[test]
    fn o_manche_direito_alimenta_os_eixos_que_o_direcional_nao_usa() {
        // `Z` e `RZ` existem no controle do console e não tinham nada os alimentando: o manche
        // direito ficava sem função nenhuma.
        let player = Player::with_gamepad("Meu Controle".into());
        let pad = player.pad(
            |_| false,
            |axis| match axis {
                "RightStickX" => Some(-1.0),
                _ => None,
            },
        );
        assert_eq!(pad.axes[2], -input::AXIS_MAX);
        assert_eq!(pad.axes[3], 0);
    }

    #[test]
    fn o_analogico_em_repouso_deixa_o_eixo_no_centro() {
        // A zona morta existe para que um manche que não volta exatamente ao centro não deixe
        // o eixo tremendo, e o jogo não veja o controle andando sozinho.
        let player = Player::with_gamepad("Meu Controle".into());
        let pad = player.pad(
            |source| *source == Source::key("ArrowRight"),
            |_| Some(0.05),
        );
        assert_eq!(pad.axes[0], 0);
    }

    #[test]
    fn sem_controle_os_eixos_ficam_no_centro() {
        // O teclado não tem analógico, e o mapeamento de teclado não mapeia eixo nenhum: nem o
        // valor devolvido pelo analógico chega aos eixos, porque não há origem ligada a eles.
        let player = Player::default();
        assert!(player.axes.is_empty());
        let pad = player.pad(|s| *s == Source::key("ArrowLeft"), |_| Some(1.0));
        assert_eq!(pad.axes, [0; 4]);
    }

    #[test]
    fn o_manche_esquerdo_nao_aperta_o_direcional() {
        // O manche e o direcional de cruz são controles diferentes: ligar o manche também aos
        // botões faria os dois agirem juntos e nenhum deles sozinho.
        let player = Player::with_gamepad("Meu Controle".into());
        for button in ["up", "down", "left", "right"] {
            for source in player.sources(button) {
                assert!(
                    !matches!(source, Source::Axis { .. }),
                    "{button} ainda sai de um eixo"
                );
            }
        }
        let stick = Source::Axis {
            name: "LeftStickY".into(),
            positive: true,
        };
        assert!(!player.pad(|s| *s == stick, |_| None).is_down(DPAD_UP));
    }

    #[test]
    fn adotar_os_eixos_tira_o_manche_de_cima_do_direcional() {
        // O mapeamento antigo ligava o manche aos botões do direcional por falta de eixo.
        // Manter os dois faria o manche apertar o direcional além de mover o eixo.
        let mut player = Player {
            device: Some("Meu Controle".into()),
            ..Player::default()
        };
        player.bind(
            "up",
            Source::Axis {
                name: "LeftStickY".into(),
                positive: true,
            },
        );
        let mut controls = Controls {
            players: vec![player],
        };
        controls.adopt();
        let up = controls.players[0].sources("up");
        assert!(up.contains(&Source::key("ArrowUp")), "a tecla continua");
        assert!(!up.iter().any(|s| matches!(s, Source::Axis { .. })));
    }

    #[test]
    fn um_mapeamento_sem_eixos_ganha_os_do_controle() {
        // Um arquivo escrito antes de os eixos existirem deixaria os manches mudos, e ninguém
        // adivinharia que era preciso refazer o mapeamento à mão.
        let mut controls = Controls {
            players: vec![Player {
                device: Some("Meu Controle".into()),
                ..Player::default()
            }],
        };
        controls.adopt();
        assert_eq!(controls.players[0].axes, Player::default_axes());

        // Quem está só no teclado continua sem eixo nenhum: não há manche para mapear.
        let mut keyboard = Controls::default();
        keyboard.adopt();
        assert!(keyboard.players[0].axes.is_empty());
    }

    #[test]
    fn o_jogador_que_falta_e_criado_com_o_padrao() {
        let mut controls = Controls::default();
        assert_eq!(controls.players.len(), crate::input::PORTAS);
        controls.player_mut(3);
        assert_eq!(controls.players.len(), 4);
        assert_eq!(controls.player(3), Some(&Player::default()));
    }

    /// As duas portas nascem com o mesmo mapeamento, e só a primeira ligada. Ligar as duas por
    /// omissão faria um jogo de dois enxergar um segundo jogador que ninguém pediu.
    #[test]
    fn so_a_primeira_porta_nasce_ligada() {
        let controls = Controls::default();
        let ligadas: Vec<usize> = controls.ligadas().map(|(i, _)| i).collect();
        assert_eq!(ligadas, vec![0]);
        assert_eq!(controls.players[1].aparelho, Aparelho::Controle);
    }

    /// Um arquivo salvo antes das portas não tem o campo `ligada`, e o jogador dele **não pode**
    /// aparecer desligado: quem já usava o emulador ficaria sem controle nenhum.
    #[test]
    fn mapeamento_antigo_continua_ligado() {
        let antigo = r#"{"players":[{"device":null}]}"#;
        let mut controls: Controls = serde_json::from_str(antigo).expect("devia ler");
        controls.adopt();
        assert!(controls.players[0].ligada);
        assert_eq!(controls.players.len(), crate::input::PORTAS);
        assert!(!controls.players[1].ligada);
    }
}
