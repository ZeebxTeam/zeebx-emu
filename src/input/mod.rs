//! O controle do Zeebo, independente de onde a entrada vem.
//!
//! Os identificadores saem do `hid_devices.original.cfg` do console — a entrada com o VID
//! `0x1EAA` e o PID `0x0135`, do "New Zeebo Game Controller" — e os nomes dos UIDs, de
//! `AEEHIDDevice_Joystick.h` do SDK do Zeebo. O jogo identifica cada botão pelo UID, não pelo
//! índice, então o que precisa estar certo é a tabela.

pub mod bindings;
pub mod gamepads;
pub mod padview;

/// Quantas portas de entrada o console tem.
///
/// São as duas USB da frente do aparelho. Não é número escolhido: o `GetConnectedDevices` da
/// Z-Wheel passa **dois** como tamanho do vetor de saída, nas duas vezes em que o chama.
pub const PORTAS: usize = 2;

/// O evento de tecla do BREW, que é como o console entrega teclado ao aplicativo.
///
/// Teclado no BREW **não passa pelo `IHID`**: o `IHID` diz que existe um, e as teclas chegam
/// como evento ao tratador do aplicativo, com o código virtual no `wParam`. A Z-Wheel confirma:
/// o tratador do formulário de abertura, em `0x11828`, testa `r1 == 0x100` e compara o `r2` com
/// `0xe030`, `0xe04a` e outros — que são `AVK_`.
pub const EVT_KEY: u32 = 0x100;

/// Os códigos virtuais do BREW que o console usa, do `AEEVCodes.h`.
///
/// A lista não é chute: são os que aparecem como literal no módulo da Z-Wheel — `0xe015`,
/// `0xe030` a `0xe035`, `0xe046`, `0xe04a`, `0xe063` — mais os quatro sentidos, que ficam logo
/// antes do `AVK_SELECT` na numeração do header.
pub mod avk {
    /// **Os quatro sentidos aqui são inferência, e a Z-Wheel não usa nenhum deles.**
    ///
    /// Vieram de supor que ficassem logo antes do `AVK_SELECT` na numeração do header. O módulo
    /// da Z-Wheel desmente em parte: `0xe011` e `0xe013` não aparecem nele de forma alguma, e
    /// quem gira a roda de jogos são `0xe033` e `0xe034` — medido, comparando o quadro com e
    /// sem cada tecla. Ficam porque outro jogo pode usá-las e porque tirar sem medir seria
    /// trocar uma suposição por outra.
    pub const UP: u32 = 0xe011;
    pub const DOWN: u32 = 0xe012;
    pub const LEFT: u32 = 0xe013;
    pub const RIGHT: u32 = 0xe014;
    /// O "OK". É o código que a Z-Wheel guarda em `0xe015`.
    pub const SELECT: u32 = 0xe015;
    /// `AVK_0` a `AVK_9` são contíguos.
    pub const ZERO: u32 = 0xe030;
    pub const STAR: u32 = 0xe03a;
    pub const POUND: u32 = 0xe03b;
    pub const CLR: u32 = 0xe04a;

    /// O código de um dígito, ou `None` se não for dígito.
    pub fn digito(n: u32) -> Option<u32> {
        (n <= 9).then_some(ZERO + n)
    }

    /// As teclas do console que a Z-Wheel escuta, **medidas** e não deduzidas.
    ///
    /// A medida foi comparar o quadro com e sem cada candidata, com o desenho já
    /// determinístico: `0xe033` e `0xe034` giram a roda de jogos para um lado e para o outro, e
    /// `0xe064` avança a tela de instruções do z-pad — o mesmo que a tecla `0` faz ali.
    ///
    /// Pela numeração dos dígitos, `0xe033` e `0xe034` são o `3` e o `4`. Não sabemos que nome
    /// o header do BREW lhes dá nem por que a roda usa justamente esses dois; sabemos o que
    /// eles fazem. Ficam com nome do que fazem, e não do que se imagina que sejam.
    pub const RODA_ANTERIOR: u32 = 0xe033;
    pub const RODA_SEGUINTE: u32 = 0xe034;
    pub const CONFIRMA: u32 = 0xe064;

    /// O código de uma tecla pelo nome que a configuração usa.
    pub fn por_nome(nome: &str) -> Option<u32> {
        Some(match nome {
            "up" => UP,
            "down" => DOWN,
            "left" => LEFT,
            "right" => RIGHT,
            "select" | "ok" => SELECT,
            // Os dois que giram a roda da Z-Wheel. Terem nome é o que deixa um roteiro de
            // teste legível: `--teclas=31000:roda-seguinte` diz o que `0xe034` não diz.
            "roda-anterior" => RODA_ANTERIOR,
            "roda-seguinte" => RODA_SEGUINTE,
            "star" => STAR,
            "pound" => POUND,
            "clr" => CLR,
            // Um código cru, em hexadecimal, para quando se está descobrindo qual é a tecla:
            // `0xe063` diz o que `right` ainda não sabe dizer.
            outro if outro.starts_with("0x") => {
                return u32::from_str_radix(&outro[2..], 16).ok();
            }
            outro => return outro.parse::<u32>().ok().and_then(digito),
        })
    }
}

/// Quantos botões o controle tem.
pub const BUTTONS: usize = 18;

/// UID de cada botão, na ordem em que o console os enumera.
///
/// A lista tem esquisitices — o índice 3 traz o UID de um eixo, e o `Right_Shoulder_Upper`
/// aparece duas vezes — mas é o que o arquivo do console diz, e é o que os jogos viram quando
/// foram feitos. Corrigir aqui seria inventar um controle que não existiu.
pub const BUTTON_UIDS: [u32; BUTTONS] = [
    0x0106_c40b, // Button_2
    0x0106_c408, // Right_Shoulder_Upper
    0x0106_c40d, // Button_4
    0x0106_c4d0, // LeftThumb_X
    0x0106_c408, // Right_Shoulder_Upper
    0x0106_c407, // Left_Shoulder_Lower
    0x0106_c406, // Left_Shoulder_Upper
    0x0106_c409, // Right_Shoulder_Lower
    0x0106_c405, // Right_Thumbstick
    0x0106_c403, // Back
    0x0106_c404, // Left_Thumbstick
    0x0106_c402, // Start
    // O direcional não está na lista do arquivo do console, que o descreve como os eixos `X` e
    // `Y`. Mas é como botão que os jogos o leem: com estes quatro UIDs presentes, o menu do
    // Quake anda; sem eles, o cursor não sai do lugar por mais que o eixo mude. Um direcional
    // digital em USB HID costuma ser reportado das duas formas, e é o que fazemos.
    0x0106_c3fe, // DPad_Up
    0x0106_c400, // DPad_Down
    0x0106_c3ff, // DPad_Left
    0x0106_c401, // DPad_Right
    // O `Button_1` e o `Button_3` também faltam na lista do console, que traz o `Button_2` e o
    // `Button_4` mas põe um UID de eixo no lugar de um deles. A tela de ajuda do próprio Quake
    // nomeia os quatro — "aperte 1 para pular", "aperte 3 para ativar mira" —, então eles
    // existem no controle.
    0x0106_c40a, // Button_1
    0x0106_c40c, // Button_3
];

/// Índices dos quatro sentidos do direcional em [`BUTTON_UIDS`], na ordem cima, baixo,
/// esquerda, direita.
pub const DPAD: [usize; 4] = [12, 13, 14, 15];

/// Nome de cada botão, para o mapeamento de teclas e para a linha de comando.
/// O controle tem **um** gatilho de cada lado, o ZL e o ZR, e é o "superior" de cada par que
/// eles reportam: no Zeeboids, que desenha `ZL` e `ZR` na tela e gira o personagem com eles, o
/// `Left_Shoulder_Upper` e o `Right_Shoulder_Upper` giram e os "inferiores" não fazem nada. Os
/// dois inferiores continuam aqui porque estão no arquivo do console, mas não têm botão.
/// **`b1` fica no UID que o arquivo do console rotula `Button_2`, e isto é deliberado.**
///
/// Os rótulos `Button_N` da entrada do controle no `hid_devices.original.cfg` estão deslocados —
/// o mesmo arquivo põe um UID de eixo no `BUTTON:3`, então a inconsistência dele já era
/// conhecida. Quem resolve o deslocamento é o comportamento dos jogos, medido no aparelho: com
/// `b1` no `0x0106c40a`, o botão sul chegava aos jogos como **2** e o leste como **1**. Como o
/// sul é o 1 impresso no Z-Pad, o UID do botão 1 é o `0x0106c40b`.
///
/// Os nomes `b3` e `b4` seguem onde estavam: não há medida deles ainda.
pub const BUTTON_NAMES: [&str; BUTTONS] = [
    "b1", "zr", "b4", "lx", "zrb", "l2", "zl", "r2", "rthumb", "back", "lthumb", "start", "up",
    "down", "left", "right", "b2", "b3",
];

/// UID de cada eixo: `X`, `Y`, `Z` e `RZ`.
///
/// Três deles são **literalmente** os do `hid_devices.original.cfg` do console, na entrada do
/// controle do Zeebo (`VID:0x1EAA:PID:0x0135`):
///
/// ```text
/// AXIS:X:0x0106C40C      <- este não
/// AXIS:Y:0x0106C4D1
/// AXIS:Z:0x0106C4CE
/// AXIS:RZ:0x0106C4CF
/// ```
///
/// O arquivo tem uma troca, e ela é visível dos dois lados: o `AXIS:X` recebeu `0x0106C40C`,
/// que é UID de botão (o `Button_3`), e o `BUTTON:3` da mesma entrada recebeu `0x0106C4D0`,
/// que é UID de eixo — o `LeftThumb_X` das outras entradas do próprio arquivo.
///
/// **Aqui desfazemos a troca, e o que decidiu foi medida, não dedução.** Com `0x0106C40C` no
/// `X`, o manche não move esquerda e direita em jogo nenhum: o jogo varre a tabela do
/// `GetAxesInfo` procurando UID de eixo, não acha nenhum para o `X` e nunca guarda o campo
/// dele — o `Y`, o `Z` e o `RZ` andam, e só o horizontal fica morto. Com `0x0106C4D0` o manche
/// anda inteiro.
///
/// Isto **não** contradiz a lição de que fonte primária ganha de inferência: a pergunta aqui
/// não é "o que o arquivo diz", é "o que o jogo procura", e quem responde essa é o jogo. O
/// arquivo continua sendo a fonte de todo o resto da tabela.
pub const AXIS_UIDS: [u32; 4] = [0x0106_c4d0, 0x0106_c4d1, 0x0106_c4ce, 0x0106_c4cf];

/// Nome de cada eixo, na ordem de [`Pad::axes`], para o mapeamento e a tela de configuração.
pub const AXIS_NAMES: [&str; 4] = ["x", "y", "z", "rz"];

/// Em qual palavra do `AEEHIDPositionInfo` cada eixo cai.
///
/// A struct começa com `boolean bRelativeAxes` e segue com `nX`, `nY`, `nZ`, `nRx`, `nRy`,
/// `nRz` e mais dezoito campos (`inc/AEEIHIDDevice.h` do SDK do Zeebo). O console usa `X`,
/// `Y`, `Z` e `RZ`, que são as palavras 1, 2, 3 e 6.
pub const AXIS_SLOTS: [usize; 4] = [1, 2, 3, 6];

/// Quantas palavras tem o `AEEHIDPositionInfo`: `bRelativeAxes` e vinte e quatro eixos.
pub const POSITION_INFO_WORDS: usize = 25;

/// Faixa de um eixo **como o console a reporta**: um byte sem sinal, com o repouso no meio.
///
/// Isto não é escolha nossa, é medida. O Zeebo F.C. Super League converte cada eixo assim
/// (`0xfd4c0` no módulo dele, e o mesmo trecho está no Tênis, no Zeeboids e em todo jogo que
/// usa essa camada do SDK):
///
/// ```text
/// mvn   r0, #0x7f        ; r0 = -128
/// sxtah r4, r0, r4       ; valor = (int16)eixo - 128
/// ```
///
/// O jogo subtrai **128** do valor cru para achar o centro, então o centro do aparelho é 128 e
/// a faixa é `0..=255` — o que também é o que reportam os manches USB que o
/// `hid_devices.original.cfg` lista (Logitech Dual Action, RumblePad2). Enquanto mandávamos
/// zero, todo jogo dessa camada lia o extremo e andava sozinho para um canto: era o "boneco
/// andando para cima" com o manche parado.
///
/// O Zeeboids escapava por acidente: ele trata "os quatro eixos exatamente no mínimo" como
/// "não há manche aqui" e zera tudo — uma defesa contra exatamente o que a gente fazia.
pub const AXIS_MIN: i32 = 0;
pub const AXIS_MAX: i32 = 255;

/// O repouso, na faixa do console.
pub const AXIS_CENTRO: i32 = 128;

/// Quanto [`Pad::axes`] anda para cada lado.
///
/// Guardamos o eixo **centrado no zero**, que é a forma com que o próprio jogo trabalha depois
/// da subtração acima; quem traduz para a faixa do aparelho é o `IHIDDevice`, no único lugar em
/// que o console é quem lê.
pub const AXIS_CURSO: i32 = 128;

/// O estado do controle num instante.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Pad {
    /// Um bit por botão, no índice em que o console o enumera.
    pub buttons: u32,
    /// Os eixos `X`, `Y`, `Z` e `RZ`, nessa ordem.
    pub axes: [i32; 4],
}

impl Pad {
    pub fn is_down(&self, index: usize) -> bool {
        self.buttons & (1 << index) != 0
    }

    /// Aperta ou solta um botão. **O direcional não toca nos eixos**, e a razão é o aparelho.
    ///
    /// O `hid_devices.original.cfg` lista **quatro** eixos — `X`/`Y` e `Z`/`RZ` —, que são dois
    /// manches. O Z-Pad tem manche analógico e direcional digital: o direcional é botão, e quem
    /// alimenta `X`/`Y` é o manche esquerdo. A Z-Wheel registra `RegisterForPositionChange`
    /// porque ela é navegada **pelo manche**.
    ///
    /// Jogar um direcional digital nos eixos quebra todo jogo que lê variação em vez de estado:
    /// soltar a direção manda o eixo de volta ao centro, e essa volta é uma segunda mudança, de
    /// sinal oposto. No Zeeboids o menu desce uma opção e volta na hora. Isto já foi tentado
    /// três vezes — espelhado nos dois canais, só nos eixos, e de novo aqui — e falhou nas três.
    pub fn press(&mut self, index: usize, down: bool) {
        let bit = 1 << index;
        if down {
            self.buttons |= bit;
        } else {
            self.buttons &= !bit;
        }
    }

    /// Põe um eixo no valor dado, preso ao curso do manche. O zero é o repouso.
    pub fn set_axis(&mut self, index: usize, value: i32) {
        if let Some(axis) = self.axes.get_mut(index) {
            *axis = value.clamp(-AXIS_CURSO, AXIS_CURSO);
        }
    }

    /// O eixo na faixa do console: o repouso vira 128, e o curso, `0..=255`.
    pub fn eixo_do_console(&self, index: usize) -> i32 {
        let valor = self.axes.get(index).copied().unwrap_or(0);
        (valor + AXIS_CENTRO).clamp(AXIS_MIN, AXIS_MAX)
    }

    /// Os botões que mudaram entre `self` e `next`, com o novo estado de cada um.
    pub fn changes(&self, next: &Pad) -> Vec<(usize, bool)> {
        (0..BUTTONS)
            .filter(|&i| self.is_down(i) != next.is_down(i))
            .map(|i| (i, next.is_down(i)))
            .collect()
    }

    /// O índice do botão de nome `name`.
    ///
    /// Aceita `home` como outro nome do `back`. O controle do Zeebo tem um botão **HOME**
    /// impresso na carcaça, e é ele que o `hid_devices.cfg` do console mapeia no
    /// `AEEUID_HIDJoystick_Back` — não existe UID de "Home" no `AEEHIDDevice_Joystick.h`. O
    /// Double Dragon pede "APERTE O BOTÃO HOME" na tela de título, e quem lê isso não tem como
    /// adivinhar que o botão se chama `back` aqui dentro.
    pub fn button_by_name(name: &str) -> Option<usize> {
        let name = match name {
            "home" => "back",
            other => other,
        };
        BUTTON_NAMES.iter().position(|&n| n == name)
    }
}

/// Um roteiro de teclas, para exercitar a entrada sem janela.
///
/// Existe para poder testar: sem ele, a única forma de saber se a entrada funciona é apertar a
/// tecla e olhar, e isso não cabe num teste nem numa execução automática.
#[derive(Debug, Default, Clone)]
pub struct Script {
    /// Cada entrada é `(início, fim, índice do botão)`, em milissegundos de tempo virtual.
    steps: Vec<(u32, u32, usize)>,
    /// O mesmo para os eixos: `(início, fim, índice do eixo, valor)`.
    ///
    /// Existe porque nem toda entrada do console é botão. A Z-Wheel registra
    /// `RegisterForPositionChange` e **só** isso: a roda de jogos anda pelo eixo, e um roteiro
    /// que só sabe apertar botão não tem como medi-la.
    eixos: Vec<(u32, u32, usize, i32)>,
}

impl Script {
    /// Quanto tempo cada tecla fica apertada, quando o roteiro não diz. Um toque humano.
    pub const HOLD_MS: u32 = 120;

    /// Lê um roteiro no formato `ms:tecla[:duração_ms][,...]`.
    ///
    /// O tempo é o do relógio virtual do jogo, não o número de voltas do laço: uma volta não
    /// dura sempre a mesma coisa, e o mesmo roteiro precisa valer entre execuções.
    ///
    /// As teclas são os nomes dos botões em [`BUTTON_NAMES`], incluindo `up`, `down`, `left` e
    /// `right` para o direcional.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut steps = Vec::new();
        let mut eixos = Vec::new();
        for item in text.split(',').filter(|s| !s.is_empty()) {
            let mut parts = item.split(':');
            let at = parts.next().unwrap_or_default();
            let key = parts
                .next()
                .ok_or_else(|| format!("esperava ms:tecla em {item:?}"))?;
            let at: u32 = at
                .parse()
                .map_err(|_| format!("instante inválido em {item:?}"))?;
            let hold: u32 = match parts.next() {
                Some(text) => text
                    .parse()
                    .map_err(|_| format!("duração inválida em {item:?}"))?,
                None => Self::HOLD_MS,
            };
            let fim = at.saturating_add(hold.max(1));
            if let Some((eixo, valor)) = eixo_por_nome(key) {
                eixos.push((at, fim, eixo, valor));
                continue;
            }
            let index =
                Pad::button_by_name(key).ok_or_else(|| format!("tecla desconhecida: {key:?}"))?;
            steps.push((at, fim, index));
        }
        Ok(Self { steps, eixos })
    }

    /// Põe no controle o que o roteiro manda neste instante.
    /// Um botão fica apertado se **algum** passo o quer apertado agora.
    ///
    /// Aplicar passo a passo parece igual e não é: um roteiro que usa o mesmo botão duas vezes —
    /// e todo roteiro que navega um menu usa, porque confirmar é sempre a mesma tecla — teria o
    /// primeiro aperto desfeito pelo segundo passo, que ainda não chegou e por isso manda
    /// soltar. O aperto sumia sem deixar rastro, e o roteiro parecia não ter efeito.
    pub fn apply(&self, now_ms: u32, pad: &mut Pad) {
        let mut apertados = 0u32;
        for &(start, end, index) in &self.steps {
            if (start..end).contains(&now_ms) {
                apertados |= 1 << index;
            }
        }
        for &(_, _, index) in &self.steps {
            pad.press(index, apertados & (1 << index) != 0);
        }
        // O eixo volta ao centro quando nenhum passo o quer deslocado — soltar o analógico é
        // parte do gesto, e um eixo preso num extremo gira a roda para sempre.
        for &(_, _, eixo, _) in &self.eixos {
            let valor = self
                .eixos
                .iter()
                .find(|&&(inicio, fim, qual, _)| qual == eixo && (inicio..fim).contains(&now_ms))
                .map_or(0, |&(_, _, _, valor)| valor);
            pad.set_axis(eixo, valor);
        }
    }
}

/// Lê `x+`, `x-`, `y+`, `y-`, `z+`, `z-`, `rz+`, `rz-` como `(índice do eixo, valor)`.
///
/// Os quatro eixos são os do `hid_devices.cfg` do console, na ordem em que ele os enumera:
/// `X`, `Y`, `Z` e `RZ`. O sinal diz para que lado, e o valor é o batente — meio caminho não
/// serve para descobrir se um eixo faz alguma coisa.
fn eixo_por_nome(nome: &str) -> Option<(usize, i32)> {
    let (eixo, sinal) = nome.split_at(nome.len().checked_sub(1)?);
    let valor = match sinal {
        "+" => AXIS_CURSO,
        "-" => -AXIS_CURSO,
        _ => return None,
    };
    let eixo = match eixo {
        "x" => 0,
        "y" => 1,
        "z" => 2,
        "rz" => 3,
        _ => return None,
    };
    Some((eixo, valor))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O botão 1 é o sul, e o UID dele é o que o arquivo do console rotula `Button_2`.
    ///
    /// Medido no aparelho: com `b1` no `0x0106c40a`, o sul chegava aos jogos como 2 e o leste
    /// como 1. Ver a nota de [`BUTTON_NAMES`] — este teste existe para que a troca não seja
    /// desfeita por quem confie no rótulo do arquivo.
    #[test]
    fn o_botao_um_usa_o_uid_que_o_arquivo_chama_de_dois() {
        let um = Pad::button_by_name("b1").expect("b1 existe");
        let dois = Pad::button_by_name("b2").expect("b2 existe");
        assert_eq!(BUTTON_UIDS[um], 0x0106_c40b);
        assert_eq!(BUTTON_UIDS[dois], 0x0106_c40a);
    }

    /// Os índices de [`DPAD`] são escritos à mão; este teste impede que eles e [`BUTTON_NAMES`]
    /// se separem sem ninguém notar.
    #[test]
    fn os_indices_do_direcional_batem_com_os_nomes() {
        let [cima, baixo, esquerda, direita] = DPAD;
        for (indice, nome) in [
            (cima, "up"),
            (baixo, "down"),
            (esquerda, "left"),
            (direita, "right"),
        ] {
            assert_eq!(BUTTON_NAMES[indice], nome, "índice {indice}");
        }
    }

    /// O código cru serve para descobrir tecla; os nomes continuam valendo.
    #[test]
    fn codigo_cru_em_hexadecimal() {
        assert_eq!(avk::por_nome("0xe034"), Some(0xe034));
        assert_eq!(avk::por_nome("0xE033"), Some(0xe033));
        assert_eq!(avk::por_nome("3"), Some(avk::ZERO + 3));
        assert_eq!(avk::por_nome("clr"), Some(avk::CLR));
        assert_eq!(avk::por_nome("0xzz"), None);
    }

    /// Os dígitos `3` e `4` são o que gira a roda da Z-Wheel.
    ///
    /// Fica pinado num teste porque é a única coisa que liga [`avk::RODA_ANTERIOR`] e
    /// [`avk::RODA_SEGUINTE`] ao caminho que os produz: o direcional saiu da tradução de teclas
    /// — ele move os eixos, como o console reporta —, e quem manda esses dois códigos agora é o
    /// teclado, pelos dígitos.
    #[test]
    fn os_digitos_tres_e_quatro_sao_os_da_roda() {
        assert_eq!(avk::por_nome("3"), Some(avk::RODA_ANTERIOR));
        assert_eq!(avk::por_nome("4"), Some(avk::RODA_SEGUINTE));
        assert_eq!(avk::por_nome("roda-anterior"), Some(avk::RODA_ANTERIOR));
        assert_eq!(avk::por_nome("roda-seguinte"), Some(avk::RODA_SEGUINTE));
    }

    #[test]
    fn o_botao_home_e_o_back_por_outro_nome() {
        // O controle do Zeebo tem HOME impresso na carcaça, e o `hid_devices.cfg` do console o
        // mapeia no `Back` — não há UID de "Home" no header do joystick. O Double Dragon pede
        // "APERTE O BOTÃO HOME", e ninguém adivinharia que ele se chama `back` aqui dentro.
        assert_eq!(Pad::button_by_name("home"), Pad::button_by_name("back"));
        assert!(Pad::button_by_name("home").is_some());
    }

    #[test]
    fn o_direcional_mexe_no_botao_e_nao_no_eixo() {
        let [up, down, left, right] = DPAD;
        let mut pad = Pad::default();

        // O direcional é botão. Nos eixos, um direcional digital desfaz o próprio passo quando
        // é solto — ver a nota do `press`, e o menu do Zeeboids.
        pad.press(left, true);
        assert!(pad.is_down(left));
        assert_eq!(pad.axes, [0; 4]);
        pad.press(up, true);
        pad.press(down, true);
        pad.press(left, false);
        assert_eq!(pad.axes, [0; 4]);

        // Quem chega aos eixos é o manche, sem passar pelos botões.
        pad.set_axis(0, AXIS_CURSO);
        assert_eq!(pad.axes[0], AXIS_CURSO);
        assert!(!pad.is_down(right));
    }

    #[test]
    fn o_eixo_parado_chega_ao_console_como_o_centro_dele() {
        // O jogo faz `valor = (int16)eixo - 128` (o `0xfd4c0` do Super League). Mandar zero é
        // mandar o batente: era o boneco andando sozinho para um canto com o manche parado.
        let pad = Pad::default();
        for eixo in 0..AXIS_NAMES.len() {
            assert_eq!(pad.eixo_do_console(eixo), AXIS_CENTRO, "eixo {eixo}");
            assert_eq!(pad.eixo_do_console(eixo) - AXIS_CENTRO, 0, "o jogo vê parado");
        }
    }

    #[test]
    fn o_curso_do_manche_cobre_a_faixa_do_console_sem_passar_dela() {
        let mut pad = Pad::default();
        pad.set_axis(0, AXIS_CURSO);
        pad.set_axis(1, -AXIS_CURSO);
        assert_eq!(pad.eixo_do_console(0), AXIS_MAX);
        assert_eq!(pad.eixo_do_console(1), AXIS_MIN);
        // E um valor além do curso não escapa da faixa que dissemos ao jogo no `GetMax`.
        pad.set_axis(2, AXIS_CURSO * 4);
        assert_eq!(pad.eixo_do_console(2), AXIS_MAX);
    }

    #[test]
    fn as_mudancas_saem_na_ordem_dos_indices() {
        let mut before = Pad::default();
        before.press(0, true);
        before.press(11, true);

        let mut after = before;
        after.press(0, false);
        after.press(5, true);

        assert_eq!(before.changes(&after), vec![(0, false), (5, true)]);
        // O botão 11 continua pressionado, então não é mudança.
        assert!(after.is_down(11));
        assert_eq!(after.changes(&after), vec![]);
    }

    #[test]
    fn o_roteiro_aperta_e_solta_botoes_e_direcoes() {
        let start = Pad::button_by_name("start").unwrap();
        let script = Script::parse("1000:start,2000:right:50").unwrap();
        let mut pad = Pad::default();

        script.apply(1000, &mut pad);
        assert!(pad.is_down(start));
        script.apply(1000 + Script::HOLD_MS, &mut pad);
        assert!(!pad.is_down(start));

        // O direcional é botão, e a duração dada no roteiro vale.
        let right = Pad::button_by_name("right").unwrap();
        script.apply(2000, &mut pad);
        assert!(pad.is_down(right));
        script.apply(2050, &mut pad);
        assert!(!pad.is_down(right));

        // O mesmo botão em dois momentos: o primeiro aperto não pode ser desfeito pelo passo
        // seguinte, que ainda não chegou.
        let script = Script::parse("1000:b2:200,3000:b2:200").unwrap();
        let b2 = Pad::button_by_name("b2").unwrap();
        let mut pad = Pad::default();
        script.apply(1100, &mut pad);
        assert!(pad.is_down(b2), "o segundo passo desfez o primeiro aperto");
        script.apply(2000, &mut pad);
        assert!(!pad.is_down(b2));
        script.apply(3100, &mut pad);
        assert!(pad.is_down(b2));

        assert!(Script::parse("10:nao-existe").is_err());
        assert!(Script::parse("start").is_err());
        assert!(Script::parse("10:start:xis").is_err());

        // Um roteiro vazio é válido e não mexe em nada.
        let before = pad;
        Script::parse("").unwrap().apply(20, &mut pad);
        assert_eq!(pad, before);
    }

    #[test]
    fn os_eixos_sao_os_do_arquivo_do_console_menos_a_troca_do_x() {
        // `Y`, `Z` e `RZ` são transcrição literal da entrada `VID:0x1EAA:PID:0x0135` do
        // `hid_devices.original.cfg`. O `X` é o `LeftThumb_X`, e não o UID de botão que o
        // arquivo pôs ali: com o do arquivo, o manche não anda para os lados em jogo nenhum,
        // porque nenhum jogo procura um UID de botão na tabela de eixos. Medido no aparelho.
        assert_eq!(
            AXIS_UIDS,
            [0x0106_c4d0, 0x0106_c4d1, 0x0106_c4ce, 0x0106_c4cf]
        );
        assert_eq!(AXIS_NAMES, ["x", "y", "z", "rz"]);
        // E o UID de eixo não pode ficar também num botão que dispara: seria o mesmo número
        // chegando ao jogo como duas coisas. O `lx` do arquivo é um botão que não existe no
        // aparelho e nunca é apertado — mas o teste registra a sobreposição de propósito.
        assert_eq!(BUTTON_UIDS[3], AXIS_UIDS[0], "a troca do arquivo é espelhada");
        assert_eq!(BUTTON_NAMES[3], "lx");
    }

    #[test]
    fn cada_botao_tem_nome_e_os_nomes_nao_se_repetem() {
        assert_eq!(BUTTON_NAMES.len(), BUTTON_UIDS.len());
        for (i, name) in BUTTON_NAMES.iter().enumerate() {
            assert_eq!(Pad::button_by_name(name), Some(i));
        }
        assert_eq!(Pad::button_by_name("nao-existe"), None);
    }
}
