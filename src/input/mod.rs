//! O controle do Zeebo, independente de onde a entrada vem.
//!
//! Os identificadores saem do `hid_devices.original.cfg` do console — a entrada com o VID
//! `0x1EAA` e o PID `0x0135`, do "New Zeebo Game Controller" — e os nomes dos UIDs, de
//! `AEEHIDDevice_Joystick.h` do SDK do Zeebo. O jogo identifica cada botão pelo UID, não pelo
//! índice, então o que precisa estar certo é a tabela.

pub mod bindings;
#[cfg(feature = "desktop")]
pub mod gamepads;
#[cfg(feature = "desktop")]
pub mod padview;
pub mod sensores;
pub mod wiimote;

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

/// Os códigos virtuais do BREW que o console usa, na numeração do `AEEVCodes.h`.
///
/// A numeração foi conferida no módulo da Z-Wheel, que é quem mais depende dela:
///
/// - a função em `0x44914` é a tradução que a própria Z-Wheel faz do analógico em teclas. Ela leva
///   os quatro sentidos, na ordem cima, baixo, esquerda, direita, em `0xe031` a `0xe034`;
/// - os tratadores de `0x11888` e `0x158c0` tratam `0xe030` igual a `0xe04a` (voltar) e `0xe035`
///   igual a `0xe064` (confirmar) — `AVK_CLR` e `AVK_SELECT` ao lado dos botões do controle.
pub mod avk {
    pub const ZERO: u32 = 0xe021;
    pub const STAR: u32 = 0xe02b;
    pub const POUND: u32 = 0xe02c;
    pub const CLR: u32 = 0xe030;
    pub const UP: u32 = 0xe031;
    pub const DOWN: u32 = 0xe032;
    pub const LEFT: u32 = 0xe033;
    pub const RIGHT: u32 = 0xe034;
    pub const SELECT: u32 = 0xe035;
    /// O botão de confirmar do controle. A Z-Wheel o aceita onde aceita o `SELECT`; a tela de
    /// instruções do z-pad só avança com ele.
    pub const CONFIRMA: u32 = 0xe064;

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
            "confirma" => CONFIRMA,
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
    // **Os quatro botões de face vêm primeiro, na ordem 1, 2, 3, 4.** Quem decidiu isto foi o
    // jogo: os onze ports de arcade da Data East não esperam evento de botão, eles varrem
    // `GetButtonInfo` de 0 a 15 a cada quadro e guardam o estado **por índice** (o laço do Bad
    // Dudes está em `0x1f2a8` no módulo dele). Para quem lê assim, o UID não importa — importa
    // a posição. Enquanto o `Button_2` abria a lista e o `Right_Shoulder_Upper` vinha logo
    // atrás, o botão 2 do controle chegava ao arcade como o gatilho direito e o 4 como o 3,
    // enquanto o nosso `b2` e o nosso `b3`, nas posições 16 e 17, não chegavam a ser lidos.
    0x0106_c40b, // Button_1 do Z-Pad (o arquivo do console o rotula `Button_2` — ver abaixo)
    0x0106_c40a, // Button_2
    0x0106_c40c, // Button_3
    0x0106_c40d, // Button_4
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
    //
    // **A ordem é a dos UIDs: cima, esquerda, baixo, direita.** Há quem leia o botão pela
    // posição na lista, e não pelo UID: o `GamepadMgr` das amostras do SDK guarda o estado num
    // vetor indexado pelo `id` do `GetNextButtonEvent` (`0x16b68` no Dragon Vs Chicken). Na ordem
    // cima, baixo, esquerda, direita, a demo andava para baixo quando se apertava esquerda.
    0x0106_c3fe, // DPad_Up
    0x0106_c3ff, // DPad_Left
    0x0106_c400, // DPad_Down
    0x0106_c401, // DPad_Right
    // O que sobra fica **depois** da faixa que os ports varrem. O `lx` guarda o UID de eixo que
    // o arquivo do console deixou no meio da lista de botões, e o `zrb` é a segunda aparição do
    // `Right_Shoulder_Upper` no mesmo arquivo: nenhum dos dois é botão do controle, e deixá-los
    // na frente custava duas posições das dezesseis que o arcade lê.
    0x0106_c4d0, // LeftThumb_X
    0x0106_c408, // Right_Shoulder_Upper, de novo
];

/// Índices dos quatro sentidos do direcional em [`BUTTON_UIDS`], na ordem cima, baixo,
/// esquerda, direita. Na lista eles ficam na ordem dos UIDs — ver [`BUTTON_UIDS`].
pub const DPAD: [usize; 4] = [12, 14, 13, 15];

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
/// Os UIDs de `b2`, `b3` e `b4` seguem onde estavam: não há medida deles ainda. O que foi
/// medido é a **ordem**, e ela vale para quem varre a lista por índice — ver [`BUTTON_UIDS`].
pub const BUTTON_NAMES: [&str; BUTTONS] = [
    "b1", "b2", "b3", "b4", "zr", "l2", "zl", "r2", "rthumb", "back", "lthumb", "start", "up",
    "left", "down", "right", "lx", "zrb",
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

    /// Espelha o direcional nos eixos `X` e `Y`, para jogo que só lê o manche.
    ///
    /// **Não é o padrão, e a razão é medida.** No console o direcional é botão: quem consulta
    /// estado o vê em `GetButtonInfo`, e quem só escuta o eixo não vê nada. Pôr o direcional nos
    /// eixos foi o que `e705840` fez, e `4418fe9` desfez — o Zeeboids consulta **os dois canais**
    /// toda volta (613 `GetNextButtonEvent` e 612 `GetPositionState` em vinte segundos) e passou a
    /// andar duas casas por toque. Deixá-lo **só** nos eixos foi pior: soltar a direção manda o
    /// eixo de volta ao centro, e quem lê variação lê essa volta como um passo no sentido contrário.
    ///
    /// Fica aqui, atrás de uma opção desligada por padrão, porque o defeito é **por jogo**: quem só
    /// lê o manche passa a enxergar o direcional, e quem lê os dois canais segue como estava.
    ///
    /// Os botões **continuam apertados**: a opção acrescenta o eixo, não troca o canal. Quem lê os
    /// dois é que anda duas casas, e é o caso que a opção existe para deixar desligada.
    ///
    /// Cima é valor **negativo** no `Y` interno: no console o eixo cresce para baixo, e é o valor
    /// baixo que o jogo lê como cima — ver `cima_no_analogico_e_o_valor_baixo_no_console`.
    pub fn espelha_o_direcional_nos_eixos(&mut self) {
        // Os quatro sentidos, na ordem de [`DPAD`]: cima, baixo, esquerda, direita.
        //
        // **Soltar devolve o eixo ao centro**, e não o deixa onde estava: sem isso o manche
        // ficaria empurrado para sempre depois do primeiro toque. É a mesma volta ao centro que o
        // [`Pad::press`] anota como o risco da ideia — quem lê variação lê essa volta como um
        // passo no sentido contrário.
        //
        // Os dois sentidos opostos apertados juntos dão centro, e não o último que chegou.
        let vertical = match (self.is_down(DPAD[0]), self.is_down(DPAD[1])) {
            (true, false) => -AXIS_CURSO,
            (false, true) => AXIS_CURSO,
            _ => 0,
        };
        let horizontal = match (self.is_down(DPAD[2]), self.is_down(DPAD[3])) {
            (true, false) => -AXIS_CURSO,
            (false, true) => AXIS_CURSO,
            _ => 0,
        };
        self.set_axis(1, vertical);
        self.set_axis(0, horizontal);
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

/// Converte uma tecla do frontend para o código virtual BREW correspondente.
///
/// Fica no motor porque desktop, headless e Android precisam da mesma convenção.
///
/// `Esc` e `P` ficam de fora de propósito: na janela do desktop são encerrar e pausar.
pub fn avk_de(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    Some(match key {
        ArrowUp => avk::UP,
        ArrowDown => avk::DOWN,
        ArrowLeft => avk::LEFT,
        ArrowRight => avk::RIGHT,
        Enter | Space => avk::CONFIRMA,
        Backspace | Delete => avk::CLR,
        Num0 | Num1 | Num2 | Num3 | Num4 | Num5 | Num6 | Num7 | Num8 | Num9 =>
            avk::ZERO + (key as u32 - Num0 as u32),
        _ => return None,
    })
}

/// As teclas BREW que valem agora: as do teclado, já em AVK, somadas às que os controles apertam.
///
/// Uma seta física pode estar mapeada também no controle, e continua sendo um aperto só: por isso
/// o resultado é um conjunto, e só a diferença entre dois deles vira evento ([`transicoes`]).
pub fn avks_ativos(
    teclado: impl IntoIterator<Item = u32>,
    pads: &[Pad],
) -> std::collections::HashSet<u32> {
    let mut ativos: std::collections::HashSet<u32> = teclado.into_iter().collect();
    for pad in pads {
        ativos.extend(
            teclas_do_controle(&Pad::default(), pad)
                .into_iter()
                .filter_map(|(key, down)| down.then_some(key)),
        );
    }
    ativos
}

/// Os eventos de tecla que levam de `anteriores` a `atuais`, soltas antes de apertadas, e
/// `anteriores` passa a ser `atuais`.
///
/// Guardar o conjunto entregue, e não cada fonte, é o que faz soltar o `4` enquanto a seta
/// continua apertada não soltar o AVK que as duas representam.
pub fn transicoes(
    anteriores: &mut std::collections::HashSet<u32>,
    atuais: std::collections::HashSet<u32>,
) -> Vec<(u32, bool)> {
    let mut eventos: Vec<_> = anteriores
        .difference(&atuais)
        .map(|&key| (key, false))
        .collect();
    eventos.extend(atuais.difference(anteriores).map(|&key| (key, true)));
    eventos.sort_unstable();
    *anteriores = atuais;
    eventos
}

/// As teclas que o controle manda, comparando com o quadro anterior.
///
/// No console o direcional chega aos aplicativos como as quatro setas do BREW, e é com elas que a
/// Z-Wheel navega: esquerda e direita giram a roda e trocam a aba da lista, cima e baixo passam as
/// páginas. O analógico não entra aqui: a Z-Wheel lê a posição e faz a tradução dela sozinha
/// (`0x44914` no módulo).
///
/// Fica fora da UI porque **todo frontend** precisa desta tradução: a janela do desktop e o core
/// Libretro entregam o mesmo par de quadros e esperam as mesmas teclas.
pub fn teclas_do_controle(antes: &Pad, agora: &Pad) -> Vec<(u32, bool)> {
    // Os dois botões de face seguem a ajuda da própria Z-Wheel (`assets/zeebo/pt/controls.html`):
    // "Sim (Botão 1)" escolhe e "Voltar (Botão 2)" cancela. Voltar é o `AVK_CLR`, medido: na tela
    // de ajuda ele volta ao menu, e o `0xe065` não faz nada.
    const DE_BOTAO: [(&str, u32); 6] = [
        ("up", avk::UP),
        ("down", avk::DOWN),
        ("left", avk::LEFT),
        ("right", avk::RIGHT),
        ("b1", avk::CONFIRMA),
        ("b2", avk::CLR),
    ];
    let mut teclas = Vec::new();
    for (nome, codigo) in DE_BOTAO {
        let Some(indice) = Pad::button_by_name(nome) else {
            continue;
        };
        if agora.is_down(indice) != antes.is_down(indice) {
            teclas.push((codigo, agora.is_down(indice)));
        }
    }
    teclas
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

    #[test]
    fn teclado_e_controle_compartilham_um_aperto() {
        use std::collections::HashSet;
        // O `b1` do controle e o `Enter` do teclado mandam o mesmo `CONFIRMA`: é o par que
        // compartilha um comando depois de o direcional ter saído da tradução.
        let mut pad = Pad::default();
        pad.press(Pad::button_by_name("b1").unwrap(), true);
        let teclado = [avk_de(egui::Key::Enter).unwrap()];
        let mut entregues = HashSet::new();
        let ativos = avks_ativos(teclado, &[pad]);
        assert_eq!(transicoes(&mut entregues, ativos.clone()), vec![(avk::CONFIRMA, true)]);
        assert!(transicoes(&mut entregues, ativos).is_empty());
        // Soltar o teclado não solta um comando ainda mantido pelo controle.
        let ativos = avks_ativos([], &[pad]);
        assert!(transicoes(&mut entregues, ativos).is_empty());
        assert_eq!(transicoes(&mut entregues, HashSet::new()), vec![(avk::CONFIRMA, false)]);
    }

    /// Os dígitos saem da ordem do `egui::Key`, e do `AVK_0` em diante. As duas listas são
    /// contíguas hoje; se uma deixar de ser, é aqui que se descobre.
    #[test]
    fn digitos_viram_avk() {
        use egui::Key;
        assert_eq!(avk_de(Key::Num0), Some(avk::ZERO));
        assert_eq!(avk_de(Key::Num7), Some(avk::ZERO + 7));
        assert_eq!(avk_de(Key::Num9), Some(avk::ZERO + 9));
        assert_eq!(avk_de(Key::Backspace), Some(avk::CLR));
        assert_eq!(avk_de(Key::Escape), None);
        assert_eq!(avk_de(Key::P), None);
    }


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

    /// As setas na numeração do `AEEVCodes.h`, que é a que a Z-Wheel usa em `0x44914`.
    #[test]
    fn as_setas_seguem_o_header() {
        assert_eq!(
            [
                avk::CLR,
                avk::UP,
                avk::DOWN,
                avk::LEFT,
                avk::RIGHT,
                avk::SELECT
            ],
            [0xe030, 0xe031, 0xe032, 0xe033, 0xe034, 0xe035]
        );
        assert_eq!(avk::por_nome("confirma"), Some(0xe064));
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
            assert_eq!(
                pad.eixo_do_console(eixo) - AXIS_CENTRO,
                0,
                "o jogo vê parado"
            );
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
        let lx = BUTTON_NAMES
            .iter()
            .position(|&n| n == "lx")
            .expect("o lx está na lista");
        assert_eq!(
            BUTTON_UIDS[lx], AXIS_UIDS[0],
            "a troca do arquivo é espelhada"
        );
        // E ele fica **fora** das dezesseis primeiras posições, que são as que os ports de
        // arcade varrem: um botão que não existe no aparelho não pode comer a vaga de um que
        // existe.
        assert!(lx >= 16, "o lx não ocupa vaga na faixa que o arcade lê");
    }

    /// A opção do issue #39: o direcional também escreve nos eixos, quando ligada.
    #[test]
    fn o_direcional_espelhado_poe_os_eixos_no_curso_e_nao_solta_o_botao() {
        let mut pad = Pad::default();
        assert_eq!(pad.axes, [0, 0, 0, 0], "o repouso é o zero interno");

        // Cima: negativo no `Y`, porque no console o eixo cresce para baixo.
        pad.press(DPAD[0], true);
        pad.espelha_o_direcional_nos_eixos();
        assert_eq!(pad.axes[1], -AXIS_CURSO);
        assert_eq!(pad.eixo_do_console(1), AXIS_MIN, "cima é o valor baixo");
        assert!(pad.is_down(DPAD[0]), "o botão continua apertado");

        // Baixo, no mesmo `Pad`, para provar que um sentido desfaz o outro.
        pad.press(DPAD[0], false);
        pad.press(DPAD[1], true);
        pad.espelha_o_direcional_nos_eixos();
        assert_eq!(pad.axes[1], AXIS_CURSO);
        assert_eq!(pad.eixo_do_console(1), AXIS_MAX);

        // Direita e esquerda no `X`.
        pad.press(DPAD[1], false);
        pad.press(DPAD[3], true);
        pad.espelha_o_direcional_nos_eixos();
        assert_eq!(pad.axes[0], AXIS_CURSO);
        pad.press(DPAD[3], false);
        pad.press(DPAD[2], true);
        pad.espelha_o_direcional_nos_eixos();
        assert_eq!(pad.axes[0], -AXIS_CURSO);

        // Soltar a direção **não** zera o eixo: quem zera é o quadro seguinte, quando o
        // direcional já não está apertado — e é essa volta ao centro que o `Pad::press` anota.
        pad.press(DPAD[2], false);
        pad.espelha_o_direcional_nos_eixos();
        pad.espelha_o_direcional_nos_eixos();
        assert_eq!(pad.axes[0], 0);

        // E os eixos que não são do manche esquerdo ficam intocados.
        assert_eq!([pad.axes[2], pad.axes[3]], [0, 0]);
    }

    /// O direcional em repouso **centra** os eixos do manche esquerdo, e não toca nos do direito.
    ///
    /// A ordem importa em quem chama: o espelho escreve zero, então ele tem de rodar **antes** do
    /// laço do analógico — senão o manche de verdade, parado no centro, seria apagado sem que
    /// ninguém tivesse apertado nada.
    #[test]
    fn o_direcional_solto_centra_o_manche_esquerdo_e_nao_toca_no_direito() {
        let mut pad = Pad::default();
        pad.set_axis(2, AXIS_CURSO);
        pad.espelha_o_direcional_nos_eixos();
        assert_eq!([pad.axes[0], pad.axes[1]], [0, 0]);
        assert_eq!(pad.axes[2], AXIS_CURSO, "o `Z` não é do direcional");
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
