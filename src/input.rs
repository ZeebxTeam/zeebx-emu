//! O controle do Zeebo, independente de onde a entrada vem.
//!
//! Os identificadores saem do `hid_devices.original.cfg` do console — a entrada com o VID
//! `0x1EAA` e o PID `0x0135`, do "New Zeebo Game Controller" — e os nomes dos UIDs, de
//! `AEEHIDDevice_Joystick.h` do SDK do Zeebo. O jogo identifica cada botão pelo UID, não pelo
//! índice, então o que precisa estar certo é a tabela.

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

    /// O código de uma tecla pelo nome que a configuração usa.
    pub fn por_nome(nome: &str) -> Option<u32> {
        Some(match nome {
            "up" => UP,
            "down" => DOWN,
            "left" => LEFT,
            "right" => RIGHT,
            "select" | "ok" => SELECT,
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
pub const BUTTON_NAMES: [&str; BUTTONS] = [
    "b2", "zr", "b4", "lx", "zrb", "l2", "zl", "r2", "rthumb", "back", "lthumb", "start", "up",
    "down", "left", "right", "b1", "b3",
];

/// UID de cada eixo: `X`, `Y`, `Z` e `RZ`.
///
/// Estes são **literalmente** os do `hid_devices.original.cfg` do console, na entrada do
/// controle do Zeebo (`VID:0x1EAA:PID:0x0135`):
///
/// ```text
/// AXIS:X:0x0106C40C
/// AXIS:Y:0x0106C4D1
/// AXIS:Z:0x0106C4CE
/// AXIS:RZ:0x0106C4CF
/// ```
///
/// O `X` valendo o UID do `Button_3` é esquisito, e a mesma entrada tem a esquisitice espelhada
/// — o `BUTTON:3` vale `0x0106C4D0`, que é UID de eixo. Parece uma troca no arquivo da TecToy.
/// Mas é o arquivo do console, e é o que os jogos viram quando foram feitos: corrigir aqui é
/// inventar um aparelho que não existiu.
///
/// Já tentei "consertar" isto uma vez, deduzindo dos binários dos jogos que `c4ce`/`c4cf` e
/// `c4d0`/`c4d1` são pares de manche — o que é verdade nas outras entradas do arquivo, as dos
/// controles de PC. Para o controle do Zeebo, não é. A dedução era plausível, coerente e
/// errada, e só caiu quando o arquivo apareceu. **Fonte primária ganha de inferência**, e
/// quando as duas discordam é a inferência que está errada.
pub const AXIS_UIDS: [u32; 4] = [0x0106_c40c, 0x0106_c4d1, 0x0106_c4ce, 0x0106_c4cf];

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

/// Faixa de um eixo. O descritor USB do controle está no dump, mas a parte do report que traria
/// os limites veio como "** UNAVAILABLE **", então adotamos a faixa de 16 bits com sinal, que é
/// o padrão de HID analógico.
pub const AXIS_MIN: i32 = i16::MIN as i32;
pub const AXIS_MAX: i32 = i16::MAX as i32;

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

    /// Aperta ou solta um botão.
    ///
    /// O direcional **não** mexe nos eixos, e isto já foi diferente. Espelhá-lo nos dois canais
    /// quebrava todo jogo que lê os dois: o Zeeboids consulta `GetNextButtonEvent` e
    /// `GetPositionState` toda volta, e andava duas casas por toque.
    ///
    /// Tirar o direcional dos botões e deixá-lo só nos eixos, que foi a primeira tentativa,
    /// piorou — e o motivo é o que fecha a questão. Um eixo tem de voltar ao centro quando se
    /// solta a direção, e essa volta é uma segunda mudança de eixo: o jogo lê o `MAX -> 0` como
    /// um passo no sentido contrário e desfaz o que acabou de fazer. Um direcional digital não
    /// tem como ser um eixo sem esse efeito.
    ///
    /// Os eixos continuam existindo e são de quem tem manche de verdade — o `bindings.rs` liga
    /// o analógico do controle de PC neles, sem passar por aqui.
    pub fn press(&mut self, index: usize, down: bool) {
        let bit = 1 << index;
        if down {
            self.buttons |= bit;
        } else {
            self.buttons &= !bit;
        }
    }

    /// Põe um eixo no valor dado, preso à faixa que o console reporta.
    pub fn set_axis(&mut self, index: usize, value: i32) {
        if let Some(axis) = self.axes.get_mut(index) {
            *axis = value.clamp(AXIS_MIN, AXIS_MAX);
        }
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
            let index =
                Pad::button_by_name(key).ok_or_else(|| format!("tecla desconhecida: {key:?}"))?;
            steps.push((at, at.saturating_add(hold.max(1)), index));
        }
        Ok(Self { steps })
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
    }
}

#[cfg(test)]
mod tests {

    /// O código cru serve para descobrir tecla; os nomes continuam valendo.
    #[test]
    fn codigo_cru_em_hexadecimal() {
        assert_eq!(avk::por_nome("0xe034"), Some(0xe034));
        assert_eq!(avk::por_nome("0xE033"), Some(0xe033));
        assert_eq!(avk::por_nome("3"), Some(avk::ZERO + 3));
        assert_eq!(avk::por_nome("clr"), Some(avk::CLR));
        assert_eq!(avk::por_nome("0xzz"), None);
    }
    use super::*;

    #[test]
    fn o_botao_home_e_o_back_por_outro_nome() {
        // O controle do Zeebo tem HOME impresso na carcaça, e o `hid_devices.cfg` do console o
        // mapeia no `Back` — não há UID de "Home" no header do joystick. O Double Dragon pede
        // "APERTE O BOTÃO HOME", e ninguém adivinharia que ele se chama `back` aqui dentro.
        assert_eq!(Pad::button_by_name("home"), Pad::button_by_name("back"));
        assert!(Pad::button_by_name("home").is_some());
    }

    #[test]
    fn o_direcional_mexe_no_botao_e_no_eixo() {
        let [up, down, left, right] = DPAD;
        let mut pad = Pad::default();

        // O direcional é botão, e só. Se ele também escrevesse nos eixos, soltar a direção
        // mandaria o eixo de volta ao centro e um jogo que lê os dois canais desfaria o passo
        // que acabou de dar — era o menu do Zeeboids voltando para a opção anterior.
        pad.press(left, true);
        assert!(pad.is_down(left));
        assert_eq!(pad.axes, [0; 4]);
        pad.press(up, true);
        pad.press(down, true);
        pad.press(left, false);
        assert_eq!(pad.axes, [0; 4]);

        // E o manche de verdade continua chegando aos eixos, sem passar pelos botões.
        pad.set_axis(0, AXIS_MAX);
        assert_eq!(pad.axes[0], AXIS_MAX);
        assert!(!pad.is_down(right));
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
    fn os_eixos_sao_os_do_arquivo_do_console() {
        // Transcrição literal da entrada `VID:0x1EAA:PID:0x0135` do
        // `hid_devices.original.cfg`. O teste existe para que ninguém "conserte" a esquisitice
        // do `X` de novo — eu já fiz isso, deduzindo dos binários dos jogos, e estava errado.
        assert_eq!(
            AXIS_UIDS,
            [0x0106_c40c, 0x0106_c4d1, 0x0106_c4ce, 0x0106_c4cf]
        );
        assert_eq!(AXIS_NAMES, ["x", "y", "z", "rz"]);
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
