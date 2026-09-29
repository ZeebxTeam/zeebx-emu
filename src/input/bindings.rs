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
/// A zona morta do manche, em fração do curso.
///
/// Pública porque o núcleo Libretro precisa da **mesma** regra no laço que lê o RetroPad: lá o
/// manche parado chega como zero, e escrever esse zero por cima apagaria o que o espelho do
/// direcional acabou de pôr — a opção `zeebx_dpad_to_analog_pN` ficaria sem efeito, e só no núcleo.
pub const DEADZONE: f32 = 0.12;

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
    /// O Dragon, o controle que anuncia `1EAA:0135` ("New Zeebo Game Controller" no
    /// `hid_devices.cfg`). É o que o descritor USB capturado de um console mostra, e o que o
    /// emulador sempre apresentou; o nome serializado continua `Controle` por isso.
    Controle,
    /// O Z-Pad, que anuncia `1A5C:3033` ("Zeebo Game Controller"). Os botões e os eixos são os
    /// mesmos do Dragon — o que muda é a pegada —, então para o jogo a diferença é só o par
    /// VID/PID, que os jogos da Boomerang Sports usam para escolher o tratamento.
    ZPad,
    /// Um teclado USB. O console enumera; jogo que o use, ainda não vimos.
    Teclado,
    /// O Boomerang, o controle de movimento: direcional, botões 1 e 2, HOME e acelerômetro. Os
    /// jogos da Boomerang Sports o reconhecem e passam a jogar pelo movimento; os outros o veem
    /// como um controle com poucos botões. Ver [`crate::machine`], `pacote_do_boomerang`.
    Boomerang,
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
    ///
    /// Vazio deixa os eixos **em repouso**: o direcional digital não os alimenta por conta própria.
    /// Para que ele os alimente há [`Player::direcional_nos_eixos`].
    #[serde(default)]
    pub axes: BTreeMap<String, AxisSource>,
    /// Espelha o direcional nos eixos `X`/`Y`, para jogo que só lê o manche.
    ///
    /// **Desligado por padrão, e é o mesmo ajuste nos dois frontends:** aqui e no
    /// `zeebx_dpad_to_analog` do core Libretro. O motivo de não ser o padrão está em
    /// [`Pad::espelha_o_direcional_nos_eixos`], com as duas tentativas medidas que o desfizeram.
    #[serde(default)]
    pub direcional_nos_eixos: bool,
    /// A calibração do sensor de movimento que alimenta esta porta, para o Boomerang.
    #[serde(default)]
    pub calibracao_movimento: CalibracaoDeMovimento,
}

/// A correção de um acelerômetro do host: o que ele mede parado, de face para cima, vira
/// exatamente `[0, 0, 1]` g. Em milésimos, para o mapeamento continuar comparável por igualdade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalibracaoDeMovimento {
    /// O desvio de cada eixo, em mg.
    pub zero_mg: [i32; 3],
    /// Quanto o sensor mede para 1 g, em mg.
    pub escala_mg: i32,
}

impl Default for CalibracaoDeMovimento {
    fn default() -> Self {
        Self {
            zero_mg: [0; 3],
            escala_mg: 1000,
        }
    }
}

impl CalibracaoDeMovimento {
    /// O maior desvio por eixo que uma calibração pode corrigir, em mg. Um acelerômetro de
    /// controle erra por algumas dezenas de mg; um desvio de 1 g é o controle calibrado de lado.
    const DESVIO_MAXIMO_MG: i32 = 250;

    /// A calibração que leva a média `parado` — lida com o controle imóvel de face para cima —
    /// a `[0, 0, 1]`. `None` quando a leitura não é a de um controle de face para cima: calibrar
    /// assim grava um desvio de 1 g, e todo jogo passa a ver a gravidade dobrada.
    pub fn de_repouso(parado: [f32; 3]) -> Option<Self> {
        let escala = parado.iter().map(|v| v * v).sum::<f32>().sqrt().max(0.1);
        let zero = [parado[0], parado[1], parado[2] - escala];
        let calibracao = Self {
            zero_mg: zero.map(|v| (v * 1000.0).round() as i32),
            escala_mg: (escala * 1000.0).round() as i32,
        };
        (parado[2] > 0.0 && calibracao.plausivel()).then_some(calibracao)
    }

    /// Se a calibração corrige só o que um sensor erra. Uma gravada de lado, de antes desta
    /// verificação existir, não vale.
    pub fn plausivel(&self) -> bool {
        self.zero_mg.iter().all(|d| d.abs() <= Self::DESVIO_MAXIMO_MG)
            && (500..=2000).contains(&self.escala_mg)
    }

    pub fn aplica(&self, bruto: [f32; 3]) -> [f32; 3] {
        if !self.plausivel() {
            return bruto;
        }
        let escala = self.escala_mg.max(100) as f32 / 1000.0;
        std::array::from_fn(|i| (bruto[i] - self.zero_mg[i] as f32 / 1000.0) / escala)
    }
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
            // O teclado não tem analógico: os eixos ficam em repouso, e quem os alimenta é o
            // manche de um controle ou o direcional com `direcional_nos_eixos` ligado.
            axes: BTreeMap::new(),
            direcional_nos_eixos: false,
            calibracao_movimento: CalibracaoDeMovimento::default(),
        }
    }
}

impl Player {
    /// O mapeamento típico do controle do host `device` — ou o teclado puro, sem controle. É o
    /// que o "restaurar" da tela de controles põe na porta.
    pub fn padrao_do_controle(device: Option<String>) -> Self {
        match device {
            // O Wii Remote não passa pelo gilrs, mas é um controle como os outros: o mapeamento
            // típico dele vem junto, e muda-se na tela como qualquer outro.
            Some(nome) if crate::input::wiimote::Wiimotes::indice_do_nome(&nome).is_some() => {
                Self::with_wiimote(nome)
            }
            Some(nome) => Self::with_gamepad(nome),
            None => Self::default(),
        }
    }

    /// Troca o controle do host que alimenta esta porta.
    ///
    /// Escolher um controle traz o mapeamento típico dele junto; ficar sem controle volta para o
    /// teclado puro. Nos dois casos o que estava configurado à mão se perde, e é por isso que a
    /// troca é um clique deliberado numa lista. Trocar o controle troca **o mapeamento**, não a
    /// porta: se ela está ligada e o que o console vê nela foram decididos antes, e perder isso
    /// aqui seria a configuração se desfazer sozinha ao escolher um aparelho na lista.
    pub fn troca_controle(&mut self, device: Option<String>) {
        let (ligada, aparelho) = (self.ligada, self.aparelho);
        *self = Self::padrao_do_controle(device);
        self.ligada = ligada;
        // **Um controle do host numa porta de teclado vira um controle para o console.** O
        // `aparelho` é o que o console enumera, e uma porta marcada como teclado não entra na
        // lista de joysticks que os jogos pedem: quem escolhia o segundo controle para a porta
        // dois continuava sem ser visto como segundo jogador. As outras escolhas (Z-Pad,
        // Boomerang) já são controle e ficam onde estão.
        self.aparelho = match (aparelho, &self.device) {
            (Aparelho::Teclado, Some(_)) => Aparelho::Controle,
            (outro, _) => outro,
        };
    }

    /// O mapeamento típico de um controle moderno, para quem liga um e quer jogar.
    pub fn with_gamepad(device: String) -> Self {
        // **A posição da mão, e não o rótulo do botão.** No aparelho o 1 fica embaixo, o 2 à
        // esquerda, o 3 no topo e o 4 à direita (imagens oficiais do controle); no controle
        // moderno, `South` é o de baixo, `West` o da esquerda, `North` o de cima e `East` o da
        // direita. Cada botão do Zeebo cai no botão do host que está **no mesmo lugar** — era o
        // que o issue #41 pedia, e o que faz a mão não reaprender nada ao trocar de controle.
        //
        // Os quatro saem de [`Self::botoes_de_acao_por_posicao`], que é **a mesma tabela da
        // migração**: duas listas paralelas foi exatamente o que divergiu no issue #41.
        let pad: [(&str, &str); 5] = [
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
        for (button, source) in Self::botoes_de_acao_por_posicao() {
            player
                .buttons
                .entry(button.to_string())
                .or_default()
                .push(source);
        }
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

    /// O mapeamento típico do Wii Remote, somado ao teclado.
    ///
    /// É o que ele fazia antes de ser mapeável — direcional no direcional, 1 e A no botão 1, 2 e B
    /// no botão 2, HOME no HOME —, com o menos e o mais nos botões 3 e 4 para quem o usa como
    /// Z-Pad. O resto se muda na tela de controles, como em qualquer controle.
    pub fn with_wiimote(device: String) -> Self {
        let mut player = Self::default();
        player.acrescenta_botoes_do_wiimote();
        player.device = Some(device);
        player
    }

    fn acrescenta_botoes_do_wiimote(&mut self) {
        let wiimote: [(&str, &str); 11] = [
            ("up", "WiiUp"),
            ("down", "WiiDown"),
            ("left", "WiiLeft"),
            ("right", "WiiRight"),
            ("b1", "Wii1"),
            ("b1", "WiiA"),
            ("b2", "Wii2"),
            ("b2", "WiiB"),
            ("b3", "WiiMinus"),
            ("b4", "WiiPlus"),
            ("back", "WiiHome"),
        ];
        for (button, source) in wiimote {
            self.bind(button, Source::button(source));
        }
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

    /// Devolve a inversão vertical a quem a perdeu.
    ///
    /// Houve uma versão que salvou os quatro eixos sem inversão nenhuma, e quem a rodou ficou
    /// com o arquivo assim — o padrão mudar de volta não conserta um mapa já gravado. Como o
    /// mapa daquela versão é reconhecível (é o padrão de hoje com `y` e `rz` retos), dá para
    /// desfazê-lo sem tocar em quem mexeu no mapeamento à mão.
    /// Os quatro botões de ação do controle do host, como estavam **antes** do remapeamento por
    /// posição (issue #41): leste no `b2`, oeste no `b3` e norte no `b4`.
    fn botoes_de_acao_antigos() -> [(&'static str, Source); 4] {
        [
            ("b1", Source::button("South")),
            ("b2", Source::button("East")),
            ("b3", Source::button("West")),
            ("b4", Source::button("North")),
        ]
    }

    /// Os mesmos quatro, casando por **posição**: embaixo, esquerda, topo e direita.
    fn botoes_de_acao_por_posicao() -> [(&'static str, Source); 4] {
        [
            ("b1", Source::button("South")),
            ("b2", Source::button("West")),
            ("b3", Source::button("North")),
            ("b4", Source::button("East")),
        ]
    }

    /// As origens de **botão** de um nome — sem as de teclado e sem as de eixo.
    ///
    /// É o que a migração dos botões de ação precisa olhar: quem tem controle carrega junto as
    /// teclas do teclado, porque o [`Self::with_gamepad`] **soma** ao [`Self::default`].
    fn origens_de_botao(&self, nome: &str) -> Vec<Source> {
        self.sources(nome)
            .iter()
            .filter(|origem| matches!(origem, Source::Button { .. }))
            .cloned()
            .collect()
    }

    /// **O remapeamento por posição alcança quem já tinha o mapeamento salvo.**
    ///
    /// Sem isto, só quem apagasse o `settings.json` veria a correção: o que está salvo manda mais
    /// que o padrão novo, e o mapeamento antigo continuaria entregando leste no `b2` — que é
    /// exatamente o defeito do issue #41.
    ///
    /// **A comparação é só entre origens de botão, e não da lista inteira.** O que um jogador com
    /// controle tem salvo é `b2 = [Space, X, East]`: as teclas do teclado mais o botão do
    /// controle. Comparar a lista inteira nunca casaria com o que está salvo, e a migração nunca
    /// aconteceria — que foi o defeito da primeira versão desta função.
    ///
    /// A troca só acontece quando os quatro ainda são **exatamente** os antigos: quem mexeu em
    /// qualquer um deles fica com o que escreveu. É a mesma regra do
    /// [`Self::migrate_axis_convention`], e as teclas do teclado ficam onde estão.
    fn migrate_action_buttons(&mut self) {
        let antigos = Self::botoes_de_acao_antigos();
        let intocado = antigos
            .iter()
            .all(|(nome, fonte)| self.origens_de_botao(nome) == [fonte.clone()]);
        if !intocado {
            return;
        }
        for (nome, fonte) in Self::botoes_de_acao_por_posicao() {
            let origens = self.buttons.entry(nome.to_string()).or_default();
            origens.retain(|origem| !matches!(origem, Source::Button { .. }));
            origens.push(fonte);
        }
    }

    fn migrate_axis_convention(&mut self) {
        let mut reto = Self::default_axes();
        reto.get_mut("y").unwrap().invert = false;
        reto.get_mut("rz").unwrap().invert = false;
        if self.axes == reto {
            self.axes = Self::default_axes();
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
        // O espelho do direcional, quando ligado, roda **antes** do laço do analógico. Ele escreve
        // zero no repouso — é assim que o manche volta ao centro ao soltar a direção —, e quem tem
        // a última palavra precisa ser o manche de verdade, quando ele está fora da zona morta.
        if self.direcional_nos_eixos {
            pad.espelha_o_direcional_nos_eixos();
        }
        // O direcional **não** escreve nos eixos por conta própria — ver a nota do [`Pad::press`].
        // Este laço é do analógico, e ele é quem alimenta `X`/`Y` e `Z`/`RZ`, como no aparelho.
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
            pad.set_axis(index, (value * input::AXIS_CURSO as f32) as i32);
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
            player.migrate_axis_convention();
            player.migrate_action_buttons();
            player.migra_aparelho_do_controle();
            player.migra_botoes_do_wiimote();
        }
        while self.players.len() < crate::input::PORTAS {
            self.players.push(Player {
                ligada: false,
                ..Player::default()
            });
        }
    }
}

impl Player {
    /// Uma porta com controle do host escolhido não é uma porta de teclado.
    ///
    /// Escolher o controle na lista trocava o mapeamento e deixava o `aparelho` como estava, e
    /// `aparelho` é o que o console **enumera**: uma porta marcada como teclado não entra na
    /// lista de joysticks que os jogos pedem. Quem punha o segundo controle na porta dois para
    /// jogar com dois continuava com um joystick só na conta do jogo, e a opção de dois
    /// jogadores ficava apagada.
    ///
    /// Vale também para o Wii Remote, desde que ele é um controle mapeável como os outros.
    fn migra_aparelho_do_controle(&mut self) {
        if self.aparelho == Aparelho::Teclado && self.device.is_some() {
            self.aparelho = Aparelho::Controle;
        }
    }

    /// Quem escolheu um Wii Remote antes de ele ser mapeável continua com os botões dele.
    ///
    /// Os botões do Wii Remote se somavam por conta própria aos do teclado da porta, e o
    /// mapeamento salvo não tinha nenhum. Sem isto, a atualização deixaria o controle mudo.
    fn migra_botoes_do_wiimote(&mut self) {
        let wiimote = self
            .device
            .as_deref()
            .is_some_and(|nome| crate::input::wiimote::Wiimotes::indice_do_nome(nome).is_some());
        let tem_botao_do_wiimote = self.buttons.values().flatten().any(|source| {
            matches!(source, Source::Button { name } if name.starts_with("Wii"))
        });
        if wiimote && !tem_botao_do_wiimote {
            self.acrescenta_botoes_do_wiimote();
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

    /// Trocar o controle troca o mapeamento, não a porta: ela continua ligada, e uma porta de
    /// teclado que ganha um controle passa a ser controle para o console — senão os jogos não a
    /// enumeram como segundo jogador. Um Boomerang continua Boomerang.
    #[test]
    fn trocar_o_controle_preserva_a_porta() {
        let mut teclado = Player {
            ligada: true,
            aparelho: Aparelho::Teclado,
            ..Player::default()
        };
        teclado.troca_controle(Some("Xbox Controller".into()));
        assert!(teclado.ligada);
        assert_eq!(teclado.aparelho, Aparelho::Controle);
        assert_eq!(teclado.device.as_deref(), Some("Xbox Controller"));

        let mut boomerang = Player {
            ligada: true,
            aparelho: Aparelho::Boomerang,
            ..Player::default()
        };
        boomerang.troca_controle(Some("Pro Controller".into()));
        assert_eq!(boomerang.aparelho, Aparelho::Boomerang);

        boomerang.troca_controle(None);
        assert_eq!(boomerang.device, None, "sem controle, volta ao teclado puro");
        assert!(boomerang.ligada);
    }

    #[test]
    fn a_calibracao_leva_o_repouso_a_um_g_para_cima() {
        let calibracao = CalibracaoDeMovimento::de_repouso([0.02, -0.05, 1.14]).unwrap();
        let [x, y, z] = calibracao.aplica([0.02, -0.05, 1.14]);
        assert!(x.abs() < 0.01 && y.abs() < 0.01 && (z - 1.0).abs() < 0.01, "{x} {y} {z}");
    }

    /// Calibrado de lado, o desvio seria de 1 g: recusado, e uma gravada assim é ignorada.
    #[test]
    fn a_calibracao_de_lado_nao_vale() {
        assert!(CalibracaoDeMovimento::de_repouso([-1.0, 0.0, 0.1]).is_none());
        let gravada = CalibracaoDeMovimento {
            zero_mg: [-1002, 5, -906],
            escala_mg: 1007,
        };
        assert_eq!(gravada.aplica([0.0, 0.0, 1.13]), [0.0, 0.0, 1.13]);
    }
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
        // Ele é botão, e só: nos eixos, um direcional digital desfaz o próprio passo ao ser
        // solto. Ver a nota do `Pad::press`. Quem alimenta `X`/`Y` no Z-Pad é o manche.
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
        assert_eq!(pad.axes[0], input::AXIS_CURSO / 2);
        assert_eq!(pad.axes[1], 0);
    }

    /// **O mapeamento salvo de quem já jogava segue a correção do #41** — e só ele: um mapeamento
    /// mexido à mão fica como está.
    #[test]
    fn o_mapeamento_salvo_dos_botoes_de_acao_e_remepeado_para_a_posicao() {
        // **O estado salvo de verdade**, e não um inventado: quem tem controle carrega as teclas do
        // teclado junto (o `with_gamepad` soma ao `default`), então o arquivo diz
        // `b2 = [Space, X, East]`. A primeira versão desta migração comparava a lista inteira e por
        // isso nunca casaria — nunca migraria ninguém.
        let mut antigo = Player::with_gamepad("Controle".into());
        for (nome, fonte) in Player::botoes_de_acao_antigos() {
            let origens = antigo.buttons.entry(nome.to_string()).or_default();
            origens.retain(|origem| !matches!(origem, Source::Button { .. }));
            origens.push(fonte);
        }
        assert_eq!(antigo.origens_de_botao("b2"), [Source::button("East")], "o ponto de partida");

        antigo.migrate_action_buttons();
        assert_eq!(antigo.origens_de_botao("b2"), [Source::button("West")], "leste sai do b2");
        assert_eq!(antigo.origens_de_botao("b3"), [Source::button("North")]);
        assert_eq!(antigo.origens_de_botao("b4"), [Source::button("East")]);
        assert_eq!(antigo.origens_de_botao("b1"), [Source::button("South")]);
        assert!(
            antigo.sources("b2").contains(&Source::key("Space")),
            "as teclas do teclado não podem sumir na migração"
        );

        // Quem mexeu num deles fica com o que escreveu: nada é trocado por baixo.
        let mut mexido = Player::with_gamepad("Controle".into());
        mexido.bind("b2", Source::button("North"));
        let antes = mexido.buttons.clone();
        mexido.migrate_action_buttons();
        assert_eq!(
            mexido.buttons, antes,
            "um mapeamento mexido à mão não pode ser trocado por baixo"
        );

        // E o mapeamento que já nasce certo não é mexido duas vezes. O teclado continua junto das
        // origens do controle, então o que se cobra é a origem do botão, e não a lista inteira.
        let mut novo = Player::with_gamepad("Controle".into());
        novo.migrate_action_buttons();
        assert!(novo.sources("b2").contains(&Source::button("West")));
        assert!(!novo.sources("b2").contains(&Source::button("East")));
        assert!(novo.sources("b4").contains(&Source::button("East")));
    }


    /// O pedido do issue #39, do lado do standalone: é o ajuste que o usuário liga na tela.
    #[test]
    fn o_direcional_nos_eixos_e_opt_in_e_nao_desliga_o_manche() {
        let cima = |source: &Source| *source == Source::key("ArrowUp");
        let mut player = Player::default();

        // Sem a opção, o que já era verdade continua: o direcional só aperta botão.
        let pad = player.pad(cima, |_| None);
        assert_eq!(pad.axes[1], 0, "sem a opção, o eixo não se mexe");
        assert!(pad.is_down(input::DPAD[0]), "e o botão segue apertado");

        player.direcional_nos_eixos = true;
        let pad = player.pad(cima, |_| None);
        assert_eq!(pad.axes[1], -input::AXIS_CURSO);
        assert!(
            pad.eixo_do_console(1) < input::AXIS_CENTRO,
            "cima é o baixo"
        );
        assert!(pad.is_down(input::DPAD[0]));

        // Soltar a direção devolve o eixo ao centro.
        let pad = player.pad(|_| false, |_| None);
        assert_eq!([pad.axes[0], pad.axes[1]], [0, 0]);

        // **O manche tem a última palavra:** com o direcional apertado e o analógico fora da zona
        // morta, quem manda no eixo é o analógico. É para isso que o espelho roda antes do laço —
        // e é só com controle que isto se vê, porque o teclado não tem fonte de eixo nenhuma.
        let mut com_manche = Player::with_gamepad("Controle de teste".into());
        com_manche.direcional_nos_eixos = true;
        let pad = com_manche.pad(
            |source| *source == Source::button("DPadUp"),
            |axis| match axis {
                "LeftStickX" => Some(0.5),
                _ => None,
            },
        );
        assert_eq!(pad.axes[0], input::AXIS_CURSO / 2, "o manche venceu");
        assert_eq!(
            pad.axes[1],
            -input::AXIS_CURSO,
            "e o direcional ficou no outro eixo"
        );
    }

    #[test]
    fn cima_no_analogico_e_o_valor_baixo_no_console() {
        // A biblioteca de controles diz que cima é positivo; o HID, que o eixo `Y` cresce para
        // baixo. Errar este sinal inverte o eixo vertical de todo jogo que o lê.
        let player = Player::with_gamepad("Meu Controle".into());
        let pad = player.pad(
            |_| false,
            |axis| match axis {
                "LeftStickY" => Some(1.0),
                _ => None,
            },
        );
        assert!(pad.axes[1] < 0, "eixo vertical deu {}", pad.axes[1]);
        // E é o valor baixo que chega ao console, que é o que o jogo lê como "cima".
        assert!(pad.eixo_do_console(1) < input::AXIS_CENTRO);
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
        assert_eq!(pad.axes[2], -input::AXIS_CURSO);
        assert_eq!(pad.axes[3], 0);
    }

    #[test]
    fn o_analogico_em_repouso_deixa_o_eixo_no_centro() {
        // A zona morta existe para que um manche que não volta exatamente ao centro não deixe
        // o eixo tremendo, e o jogo não veja o controle andando sozinho.
        let player = Player::with_gamepad("Meu Controle".into());
        let pad = player.pad(|_| false, |_| Some(0.05));
        assert_eq!(pad.axes[0], 0);
    }

    #[test]
    fn sem_controle_os_eixos_ficam_no_centro() {
        // O teclado não tem analógico, e o mapeamento de teclado não mapeia eixo nenhum: nem o
        // valor devolvido pelo analógico chega aos eixos, porque não há origem ligada a eles.
        let player = Player::default();
        assert!(player.axes.is_empty());
        let pad = player.pad(|s| *s == Source::key("Enter"), |_| Some(1.0));
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

    #[test]
    fn porta_com_controle_escolhido_deixa_de_ser_teclado() {
        // O `aparelho` é o que o console enumera: com teclado ali, a porta não conta como
        // joystick e o jogo de dois jogadores não vê o segundo.
        let mut controls = Controls::default();
        controls.players[1].ligada = true;
        controls.players[1].aparelho = Aparelho::Teclado;
        controls.players[1].device = Some("Meu Controle #2".into());
        controls.adopt();
        assert_eq!(controls.players[1].aparelho, Aparelho::Controle);
        // Sem controle escolhido, teclado continua teclado.
        let mut so_teclado = Controls::default();
        so_teclado.players[0].aparelho = Aparelho::Teclado;
        so_teclado.players[0].device = None;
        so_teclado.adopt();
        assert_eq!(so_teclado.players[0].aparelho, Aparelho::Teclado);
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

    #[test]
    fn o_wii_remote_e_um_controle_mapeavel() {
        // Como Z-Pad ele precisa responder pelos botões dele, no mapeamento da porta, e a porta
        // deixa de ser de teclado para o console.
        let mut controls = Controls {
            players: vec![Player {
                aparelho: Aparelho::Teclado,
                ..Player::with_wiimote("Wii Remote 1".into())
            }],
        };
        controls.adopt();
        let jogador = &controls.players[0];
        assert_eq!(jogador.aparelho, Aparelho::Controle);
        assert!(jogador.buttons["b1"].contains(&Source::button("Wii1")));
        assert!(jogador.buttons["back"].contains(&Source::button("WiiHome")));
        // O teclado continua valendo junto.
        assert!(jogador.buttons["b1"].contains(&Source::key("Z")));
    }

    #[test]
    fn quem_ja_tinha_um_wii_remote_continua_com_os_botoes_dele() {
        // Antes os botões se somavam sozinhos e o mapeamento salvo só tinha teclado.
        let mut controls = Controls {
            players: vec![Player {
                device: Some("Wii Remote 1".into()),
                ..Player::default()
            }],
        };
        controls.adopt();
        assert!(controls.players[0].buttons["up"].contains(&Source::button("WiiUp")));
        // E quem já mapeou à mão não ganha nada a mais.
        let mut mapeado = Player {
            device: Some("Wii Remote 1".into()),
            ..Player::default()
        };
        mapeado.bind("b1", Source::button("WiiB"));
        let mut controls = Controls { players: vec![mapeado] };
        controls.adopt();
        assert!(!controls.players[0].buttons["up"].contains(&Source::button("WiiUp")));
    }
}
