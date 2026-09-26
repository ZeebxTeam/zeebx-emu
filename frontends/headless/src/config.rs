//! O `config.ini` virando o [`Settings`] que o núcleo já entende.
//!
//! **Não há um segundo modelo de configuração aqui.** Gráficos, áudio e mapeamento de controle
//! são os mesmos campos que a interface do desktop grava no `settings.json`; o que muda é o
//! formato do arquivo, porque quem usa este frontend edita à mão e INI é o que se edita à mão.
//! O que sobra — como a janela abre, o que fazer com o quadro quando não há janela — é do
//! frontend e mora no [`Headless`].
//!
//! Toda chave é opcional e cai num padrão. Um arquivo ausente vale como um arquivo vazio: o
//! emulador precisa abrir com as opções de fábrica, não recusar.

use std::path::PathBuf;

use zeebx::input::bindings::{Aparelho, AxisSource, Controls, Player, Source};
use zeebx::registro::Ajuste;
use zeebx::ui::settings::{Audio, Graphics, ModoDaJanela, Proporcao, Scaling, Settings};

use crate::ini::{Ini, Valor, sem_aspas};

/// O que só existe neste frontend.
#[derive(Debug, Clone)]
pub struct Headless {
    /// Se abre janela, e como.
    pub video: Video,
    /// O que fazer com o quadro sem janela. Ver [`Despejo`].
    pub despejo: Despejo,
    /// Título da janela. Um frontend que gerencia janelas costuma casar pelo título.
    pub titulo: String,
    /// O tamanho com que a janela abre, quando ela não é cheia.
    pub tamanho: (u32, u32),
    /// Esperar o retraço da tela para trocar o quadro. Desligado, a janela corre solta.
    pub vsync: bool,
    /// Onde estão os jogos. É o que permite a Z-Wheel abrir um título pelo ClassID.
    pub roms: Option<PathBuf>,
    /// O pacote da Z-Wheel, para onde o console volta quando um jogo sai.
    pub z_wheel: Option<PathBuf>,
    /// Segundos de tempo do jogo antes de sair sozinho. `None` roda até fecharem.
    pub segundos: Option<u32>,
    /// Sair quando o jogo sair, em vez de voltar à Z-Wheel.
    pub sair_com_o_jogo: bool,
}

/// Como o quadro chega a quem está olhando.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Video {
    /// Uma janela com o jogo e nada mais.
    Janela,
    /// A mesma janela, ocupando a tela.
    TelaCheia,
    /// Nenhuma janela. O quadro sai pelo [`Despejo`], e quem pinta é o frontend de fora.
    Nenhum,
}

/// Para onde os quadros vão quando não há janela.
#[derive(Debug, Clone)]
pub struct Despejo {
    pub formato: Formato,
    /// `None` é a saída padrão; com caminho, o arquivo — que pode ser um FIFO, e é assim que um
    /// frontend de fora lê sem passar por disco.
    pub destino: Option<PathBuf>,
    /// Parar depois de tantos quadros. `0` é sem limite.
    pub limite: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Formato {
    /// Dois bytes por pixel, do jeito que o console guarda. É o mais barato: nada é convertido.
    Rgb565,
    /// Quatro bytes por pixel, na ordem R, G, B, A. Custa uma conversão por quadro e é o que a
    /// maioria das bibliotecas de imagem aceita sem cerimônia.
    Rgba,
    /// Um `.png` por quadro, numerado, numa pasta. Para conferir, não para tocar.
    Png,
}

impl Default for Headless {
    fn default() -> Self {
        Self {
            video: Video::Janela,
            despejo: Despejo {
                formato: Formato::Rgb565,
                destino: None,
                limite: 0,
            },
            titulo: "Zeebx".to_string(),
            tamanho: (1280, 960),
            vsync: true,
            roms: None,
            z_wheel: None,
            segundos: None,
            sair_com_o_jogo: false,
        }
    }
}

/// O que saiu da leitura: a configuração e o que se tem a dizer sobre o arquivo.
pub struct Lido {
    pub settings: Settings,
    pub headless: Headless,
    /// De onde a configuração veio, ou onde ela deveria estar.
    pub origem: Origem,
    /// Linha errada, chave desconhecida, valor que não é do tipo esperado. Nada aqui impede o
    /// emulador de rodar; tudo aqui é coisa que quem editou o arquivo quer saber.
    pub avisos: Vec<String>,
}

/// Se havia arquivo, e qual.
pub enum Origem {
    /// Havia um arquivo, e ele foi lido. Qual é já foi dito no primeiro aviso.
    Lida,
    /// Não havia nenhum. O caminho é onde um deve ser criado.
    Faltando(PathBuf),
}

/// Onde o arquivo é procurado quando ninguém aponta um: ao lado do executável primeiro, e
/// depois na pasta de configuração do sistema.
///
/// A ordem é essa de propósito. Uma cópia portátil — a pasta que o frontend de fora distribui,
/// com o binário e o `config.ini` dentro — precisa ganhar do que está instalado na máquina,
/// senão dois frontends no mesmo computador brigam pelo mesmo arquivo.
pub fn caminhos_padrao() -> Vec<PathBuf> {
    let mut caminhos = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            caminhos.push(dir.join("config.ini"));
        }
    }
    caminhos.push(zeebx::ui::settings::config_dir().join("config.ini"));
    caminhos
}

/// Onde criar um arquivo que não existe: a pasta de configuração do sistema.
///
/// **Não é ao lado do executável**, que é o primeiro lugar da procura. Ali a cópia instalada
/// costuma não ter permissão de escrita, e uma que tenha seria um arquivo criado dentro da
/// instalação — que some na próxima atualização e não é de ninguém. A pasta de configuração é
/// de quem rodou, e é onde ele vai procurar depois.
pub fn onde_criar() -> PathBuf {
    zeebx::ui::settings::config_dir().join("config.ini")
}

/// Lê o arquivo. Um caminho que não existe devolve o padrão, dizendo onde um deveria estar.
pub fn carrega(caminho: Option<&std::path::Path>) -> Lido {
    // Um `--config=` apontando para o que não existe **não** cai no padrão: a pessoa nomeou o
    // arquivo que quer, e é nele que um novo será criado.
    let escolhido = match caminho {
        Some(caminho) => Some(caminho.to_path_buf()),
        None => caminhos_padrao().into_iter().find(|c| c.is_file()),
    };
    let faltando = |onde: PathBuf, avisos: Vec<String>| Lido {
        settings: Settings::default(),
        headless: Headless::default(),
        origem: Origem::Faltando(onde),
        avisos,
    };
    let Some(escolhido) = escolhido else {
        return faltando(onde_criar(), Vec::new());
    };
    match std::fs::read_to_string(&escolhido) {
        Ok(texto) => {
            let mut lido = de_texto(&texto);
            lido.avisos.insert(0, format!("reading {}", escolhido.display()));
            lido.origem = Origem::Lida;
            lido
        }
        // Existe e não abre: permissão, um diretório com esse nome. Criar por cima seria pior.
        Err(erro) if escolhido.exists() => {
            let recado = format!("could not read {}: {erro}", escolhido.display());
            faltando(escolhido, vec![recado])
        }
        Err(_) => faltando(escolhido, Vec::new()),
    }
}

/// Escreve um `config.ini` de fábrica, comentado, em `caminho`.
///
/// **Nunca por cima de um que exista**, nem quando o arquivo está lá e só não abriu: um
/// `config.ini` cheio de ajustes é trabalho de quem o escreveu, e trocá-lo pelo exemplo por
/// causa de um erro de permissão seria apagar o que não é nosso.
pub fn cria(caminho: &std::path::Path) -> Result<(), String> {
    if caminho.exists() {
        return Err(format!("{} already exists", caminho.display()));
    }
    let texto = modelo();
    if let Some(pasta) = caminho.parent() {
        std::fs::create_dir_all(pasta)
            .map_err(|erro| format!("could not create {}: {erro}", pasta.display()))?;
    }
    std::fs::write(caminho, texto)
        .map_err(|erro| format!("could not write {}: {erro}", caminho.display()))
}

/// O exemplo distribuído, **antes** de as portas serem preenchidas. Ver [`modelo`].
const EXEMPLO: &str = include_str!("../config.ini");

/// A linha do exemplo que dá lugar às seções de porta.
///
/// É um comentário de propósito: se a substituição algum dia não acontecer, o que sobra ainda é
/// um `config.ini` válido, com uma linha estranha, em vez de um arquivo quebrado.
const MARCA: &str = "#<<portas>>";

/// O `config.ini` completo: o que `--exemplo` imprime e o que a primeira execução escreve.
///
/// **As portas são geradas, não escritas à mão.** Elas são a parte que mais se quer editar — os
/// botões — e também a que mais tem linha: treze por porta, e duas portas. Escrevê-las no
/// exemplo significaria mantê-las de acordo com [`Controls::default`] para sempre, e um exemplo
/// que diz `botao:East` onde o padrão é `tecla:X` é pior que um exemplo sem a linha: quem o lê
/// passa a acreditar no que está ali.
pub fn modelo() -> String {
    EXEMPLO.replace(MARCA, &secoes_das_portas())
}

/// As seções `[porta1]` e `[porta2]` como o arquivo as escreve, a partir dos padrões.
fn secoes_das_portas() -> String {
    let controls = Controls::default();
    (0..zeebx::input::PORTAS)
        .map(|indice| {
            let padrao = Player::default();
            let player = controls.player(indice).unwrap_or(&padrao);
            secao_da_porta(indice, player)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn secao_da_porta(indice: usize, player: &Player) -> String {
    let mut linhas = vec![
        format!("[port{}]", indice + 1),
        format!("enabled = {}", sim_ou_nao(player.ligada)),
        "# O que o console enxerga ligado: gamepad (o Dragon), zpad, keyboard ou boomerang.".to_string(),
        format!("device = {}", nome_do_aparelho(player.aparelho)),
        "# Qual controle do host alimenta esta porta. `--controllers` lista os nomes que o".to_string(),
        "# sistema dá a eles. Sem esta linha, a porta fica só no teclado.".to_string(),
    ];
    match &player.device {
        Some(nome) => linhas.push(format!("controller = \"{nome}\"")),
        None => linhas.push("# controller = \"Xbox Wireless Controller\"".to_string()),
    }
    linhas.push(
        "# O direcional também empurra o manche esquerdo, para jogo que só escuta o eixo.".to_string(),
    );
    linhas.push(
        "# **Desligado por padrão**: quem lê os dois canais anda duas casas por toque.".to_string(),
    );
    linhas.push(format!(
        "dpad_to_analog = {}",
        sim_ou_nao(player.direcional_nos_eixos)
    ));
    linhas.push(String::new());

    // Na ordem em que a interface os mostra, e não na alfabética do mapa: `up, down, left,
    // right` lidos em sequência são um direcional, e `b1, b2, b3, b4` são os quatro botões.
    for botao in zeebx::input::bindings::CONFIGURABLE {
        let origens: Vec<String> = player.sources(botao).iter().map(escreve_origem).collect();
        linhas.push(format!("{botao} = {}", origens.join(", ")));
    }
    linhas.push(String::new());

    // Os eixos ficam comentados porque **é isso que o padrão é**: sem um controle escolhido, o
    // console não tem eixo analógico nenhum, e escrevê-los aqui mudaria o comportamento em vez
    // de descrevê-lo. Assim que a linha `controle` existir, eles entram sozinhos.
    linhas.push(
        "# Os quatro eixos analógicos do console, cada um vindo de um eixo do controle do host."
            .to_string(),
    );
    linhas.push(
        "# Eles só existem com um `controle` escolhido, e aí entram sozinhos nestes valores —".to_string(),
    );
    linhas.push("# as linhas abaixo servem para mudá-los. `:invertido` vira o sentido, e uma".to_string());
    linhas.push("# linha vazia desliga o eixo.".to_string());
    // Na ordem do console — x, y, z, rz —, e não na alfabética do mapa: `x` e `y` são um
    // manche, `z` e `rz` são o outro, e lê-los fora de par não quer dizer nada.
    let padroes = Player::default_axes();
    for eixo in zeebx::input::AXIS_NAMES {
        let Some(origem) = padroes.get(eixo) else {
            continue;
        };
        let sufixo = match origem.invert {
            true => ":inverted",
            false => "",
        };
        linhas.push(format!("# axis_{eixo} = {}{sufixo}", origem.name));
    }
    linhas.push(String::new());
    linhas.join("\n")
}

fn escreve_origem(source: &Source) -> String {
    match source {
        Source::Key { name } => format!("key:{name}"),
        Source::Button { name } => format!("button:{name}"),
        Source::Axis { name, positive } => {
            let sinal = match positive {
                true => '+',
                false => '-',
            };
            format!("axis:{name}{sinal}")
        }
    }
}

fn nome_do_aparelho(aparelho: Aparelho) -> &'static str {
    match aparelho {
        Aparelho::Controle => "gamepad",
        Aparelho::ZPad => "zpad",
        Aparelho::Teclado => "keyboard",
        Aparelho::Boomerang => "boomerang",
    }
}

fn sim_ou_nao(valor: bool) -> &'static str {
    match valor {
        true => "true",
        false => "false",
    }
}

/// O miolo, separado do disco para poder ser testado.
pub fn de_texto(texto: &str) -> Lido {
    let (mut ini, mut avisos) = Ini::ler(texto);
    let mut settings = Settings::default();
    let mut headless = Headless::default();

    // ---- [video] -----------------------------------------------------------------------
    if let Some(v) = ini.pega("video", "mode") {
        match v.texto.to_lowercase().as_str() {
            "window" => headless.video = Video::Janela,
            "fullscreen" => headless.video = Video::TelaCheia,
            "none" => headless.video = Video::Nenhum,
            outro => avisa(&mut avisos, &v, &format!(
                "`{outro}` is not a video mode; use window, fullscreen or none"
            )),
        }
    }
    // O `Settings` tem o seu próprio jeito de dizer isso, e a sessão o consulta: mantê-los de
    // acordo evita que a janela abra cheia e o núcleo pense que não.
    settings.graphics.janela_do_jogo = match headless.video {
        Video::TelaCheia => ModoDaJanela::TelaCheia,
        _ => ModoDaJanela::Janela,
    };
    settings.graphics.janela = settings.graphics.janela_do_jogo;

    if let Some(v) = ini.pega("video", "scaling") {
        match v.texto.to_lowercase().as_str() {
            "integer" => settings.graphics.scaling = Scaling::Integer,
            "fit" => settings.graphics.scaling = Scaling::Fit,
            "stretch" => settings.graphics.scaling = Scaling::Stretch,
            outro => avisa(&mut avisos, &v, &format!(
                "`{outro}` is not a scaling mode; use integer, fit or stretch"
            )),
        }
    }
    booleano(&mut ini, "video", "smooth", &mut settings.graphics.smooth, &mut avisos);
    booleano(&mut ini, "video", "keep_aspect", &mut settings.graphics.keep_aspect, &mut avisos);
    booleano(&mut ini, "video", "speed_limit", &mut settings.graphics.speed_limit, &mut avisos);
    booleano(&mut ini, "video", "vsync", &mut headless.vsync, &mut avisos);
    if let Some(v) = ini.pega("video", "title") {
        headless.titulo = sem_aspas(&v.texto).to_string();
    }
    let (mut largura, mut altura) = headless.tamanho;
    inteiro(&mut ini, "video", "width", &mut largura, &mut avisos);
    inteiro(&mut ini, "video", "height", &mut altura, &mut avisos);
    headless.tamanho = (largura.max(160), altura.max(120));

    // ---- [graphics] ---------------------------------------------------------------------
    // O `gpu_present` do núcleo não aparece aqui: ele escolhe entre pôr o quadro pela placa ou
    // mandá-lo como textura do egui, e aqui não há egui. A janela crua sempre apresenta pela
    // placa, que é o caminho barato — o quadro já está em RGB565 e quem amplia é ela.
    let g: &mut Graphics = &mut settings.graphics;
    booleano(&mut ini, "graphics", "gpu_rasterizer", &mut g.gpu_rasterizer, &mut avisos);
    booleano(&mut ini, "graphics", "fog", &mut g.neblina, &mut avisos);
    oito(&mut ini, "graphics", "internal_resolution", &mut g.resolucao_interna, 1, 8, &mut avisos);
    oito(&mut ini, "graphics", "antialias", &mut g.antialias, 1, 16, &mut avisos);
    oito(&mut ini, "graphics", "anisotropic", &mut g.anisotropico, 1, 16, &mut avisos);
    if let Some(v) = ini.pega("graphics", "aspect") {
        match v.texto.to_lowercase().as_str() {
            "native" => g.proporcao = Proporcao::Nativa,
            "16x9" => g.proporcao = Proporcao::Larga16x9,
            "16x10" => g.proporcao = Proporcao::Larga16x10,
            "window" => g.proporcao = Proporcao::Janela,
            outro => avisa(&mut avisos, &v, &format!(
                "`{outro}` is not an aspect; use native, 16x9, 16x10 or window"
            )),
        }
    }

    // ---- [audio] -----------------------------------------------------------------------
    let a: &mut Audio = &mut settings.audio;
    booleano(&mut ini, "audio", "enabled", &mut a.enabled, &mut avisos);
    oito(&mut ini, "audio", "volume", &mut a.volume, 0, 100, &mut avisos);

    // ---- [dump] ---------------------------------------------------------------------
    if let Some(v) = ini.pega("dump", "format") {
        match v.texto.to_lowercase().as_str() {
            "rgb565" => headless.despejo.formato = Formato::Rgb565,
            "rgba" => headless.despejo.formato = Formato::Rgba,
            "png" => headless.despejo.formato = Formato::Png,
            outro => avisa(&mut avisos, &v, &format!(
                "`{outro}` is not a dump format; use rgb565, rgba or png"
            )),
        }
    }
    if let Some(v) = ini.pega("dump", "destination") {
        let texto = sem_aspas(&v.texto);
        headless.despejo.destino = match texto.is_empty() || texto == "-" {
            true => None,
            false => Some(PathBuf::from(texto)),
        };
    }
    if let Some(v) = ini.pega("dump", "frame_limit") {
        match v.texto.parse::<u64>() {
            Ok(n) => headless.despejo.limite = n,
            Err(_) => avisa(&mut avisos, &v, "frame_limit expects a number"),
        }
    }

    // ---- [system] ---------------------------------------------------------------------
    if let Some(v) = ini.pega("system", "roms") {
        let caminho = PathBuf::from(sem_aspas(&v.texto));
        settings.roms_dir = Some(caminho.clone());
        headless.roms = Some(caminho);
    }
    if let Some(v) = ini.pega("system", "z_wheel") {
        let caminho = PathBuf::from(sem_aspas(&v.texto));
        settings.z_wheel_path = Some(caminho.clone());
        headless.z_wheel = Some(caminho);
    }
    if let Some(v) = ini.pega("system", "language") {
        settings.language = Some(sem_aspas(&v.texto).to_string());
    }
    if let Some(v) = ini.pega("system", "seconds") {
        match v.texto.parse::<u32>() {
            Ok(0) => headless.segundos = None,
            Ok(n) => headless.segundos = Some(n),
            Err(_) => avisa(&mut avisos, &v, "seconds expects a number"),
        }
    }
    if let Some(v) = ini.pega("system", "log") {
        let texto = sem_aspas(&v.texto);
        match Ajuste::de_texto(texto) {
            Some(ajuste) => {
                ajuste.aplica();
                // **O nome canônico, e não o texto que veio.** O `config.ini` fala inglês
                // (`log = warn`) e o ajuste guardado é em português (`aviso`): guardar o texto cru
                // deixava o mesmo nível com duas formas, e o `config.ini` de fábrica passava a
                // descrever um padrão que não era o do emulador. Ver [`Nivel::nome`].
                settings.debug.nivel_de_log = match ajuste {
                    Ajuste::Desligado => "desligado".to_string(),
                    Ajuste::Ate(nivel) => nivel.nome().to_string(),
                };
            }
            // Um nível escrito errado não pode ser aceito em silêncio: quem depura precisa saber
            // que a linha não fez nada, em vez de concluir que o log é que está quebrado.
            None => avisa(
                &mut avisos,
                &v,
                "`log` expects off, fatal, error, warn, info or debug",
            ),
        }
    }
    booleano(&mut ini, "system", "exit_with_game", &mut headless.sair_com_o_jogo, &mut avisos);
    booleano(&mut ini, "system", "z_wheel_end_of_life", &mut settings.z_wheel.fim_de_vida, &mut avisos);

    // ---- [port1], [port2] ------------------------------------------------------------
    //
    // Uma porta que o arquivo não cita fica como está no padrão: a primeira com um controle e
    // as outras livres. Citar a seção, mesmo vazia, já liga a porta — é o que alguém quer dizer
    // ao escrever `[porta2]`.
    let mut controls = Controls::default();
    for (indice, nome) in nomes_de_porta().into_iter().enumerate() {
        if !ini.tem_secao(&nome) {
            continue;
        }
        let player = controls.player_mut(indice);
        player.ligada = true;
        le_porta(&mut ini, &nome, player, &mut avisos);
    }
    for sobrando in ini.secoes_com("port") {
        if !nomes_de_porta().contains(&sobrando) {
            avisos.push(format!(
                "[{sobrando}]: the console has {} ports, from port1 to port{}",
                zeebx::input::PORTAS,
                zeebx::input::PORTAS
            ));
        }
    }
    settings.controls = controls;

    avisos.extend(ini.sobras());
    Lido {
        settings,
        headless,
        // Quem chamou é que sabe de onde o texto veio; o `carrega` corrige isto em seguida.
        origem: Origem::Faltando(onde_criar()),
        avisos,
    }
}

fn nomes_de_porta() -> Vec<String> {
    (1..=zeebx::input::PORTAS).map(|n| format!("port{n}")).collect()
}

/// Uma porta: que aparelho o console vê, qual controle do host a alimenta, e o mapeamento.
fn le_porta(ini: &mut Ini, secao: &str, player: &mut Player, avisos: &mut Vec<String>) {
    let mut ligada = player.ligada;
    booleano(ini, secao, "enabled", &mut ligada, avisos);
    player.ligada = ligada;

    // O direcional espelhado nos eixos: o mesmo ajuste da caixa na tela de controles do desktop
    // e do `zeebx_dpad_to_analog_pN` do núcleo. Sem esta linha o frontend sem janela não teria
    // como ligá-lo — o `Controls` chegaria com o padrão e ninguém saberia por quê.
    let mut espelha = player.direcional_nos_eixos;
    booleano(ini, secao, "dpad_to_analog", &mut espelha, avisos);
    player.direcional_nos_eixos = espelha;

    if let Some(v) = ini.pega(secao, "device") {
        match v.texto.to_lowercase().as_str() {
            "gamepad" | "dragon" => player.aparelho = Aparelho::Controle,
            "zpad" | "z-pad" => player.aparelho = Aparelho::ZPad,
            "keyboard" => player.aparelho = Aparelho::Teclado,
            "boomerang" => player.aparelho = Aparelho::Boomerang,
            outro => avisa(avisos, &v, &format!(
                "`{outro}` is not a device; use gamepad, zpad, keyboard or boomerang"
            )),
        }
    }
    if let Some(v) = ini.pega(secao, "controller") {
        let nome = sem_aspas(&v.texto);
        // Um controle escolhido pelo nome ganha os eixos padrão junto: sem eles o manche fica
        // mudo, e quem escreveu só o nome não teria como adivinhar que faltava mais.
        player.device = match nome.is_empty() {
            true => None,
            false => Some(nome.to_string()),
        };
        if player.device.is_some() && player.axes.is_empty() {
            player.axes = Player::default_axes();
        }
    }

    // Os botões. Uma chave por botão do console, com as origens separadas por vírgula. Escrever
    // a chave **substitui** o padrão daquele botão: quem redefine `b1` quer o que escreveu, não
    // o que escreveu mais o `Z` de fábrica.
    for botao in zeebx::input::bindings::CONFIGURABLE {
        let Some(v) = ini.pega(secao, botao) else {
            continue;
        };
        player.clear(botao);
        if sem_aspas(&v.texto).trim().is_empty() {
            continue;
        }
        for pedaco in v.texto.split(',') {
            match origem(pedaco.trim()) {
                Some(source) => player.bind(botao, source),
                None => avisa(avisos, &v, &format!(
                    "`{}` is not a source; use key:NAME, button:NAME or axis:NAME+",
                    pedaco.trim()
                )),
            }
        }
    }

    // Os eixos analógicos do console. `eixo_x = LeftStickX` ou `eixo_y = LeftStickY:invertido`.
    for eixo in zeebx::input::AXIS_NAMES {
        let chave = format!("axis_{eixo}");
        let Some(v) = ini.pega(secao, &chave) else {
            continue;
        };
        let texto = sem_aspas(&v.texto).trim().to_string();
        if texto.is_empty() {
            player.axes.remove(eixo);
            continue;
        }
        let (nome, invert) = match texto.split_once(':') {
            Some((nome, marca)) => {
                let invert = matches!(marca.trim().to_lowercase().as_str(), "inverted");
                if !invert {
                    avisa(avisos, &v, &format!("`{marca}` is not `inverted`"));
                }
                (nome.trim(), invert)
            }
            None => (texto.as_str(), false),
        };
        player.axes.insert(
            eixo.to_string(),
            AxisSource {
                name: nome.to_string(),
                invert,
            },
        );
    }
}

/// `tecla:Z`, `botao:South`, `eixo:LeftStickY-`.
///
/// Os nomes são **os mesmos do `settings.json`** de propósito: quem copiou um mapeamento da
/// interface para cá não precisa traduzir nada, e a tela de controles do desktop continua
/// sendo a maneira mais fácil de descobrir como um botão se chama.
fn origem(texto: &str) -> Option<Source> {
    let (tipo, nome) = texto.split_once(':')?;
    let nome = nome.trim();
    if nome.is_empty() {
        return None;
    }
    match tipo.trim().to_lowercase().as_str() {
        "key" => Some(Source::key(nome)),
        "button" => Some(Source::button(nome)),
        "axis" => {
            // O sinal vem colado no fim, que é como a interface mostra: `LeftStickY-`.
            let (nome, positive) = match nome.strip_suffix('+') {
                Some(nome) => (nome, true),
                None => (nome.strip_suffix('-')?, false),
            };
            match nome.trim().is_empty() {
                true => None,
                false => Some(Source::Axis {
                    name: nome.trim().to_string(),
                    positive,
                }),
            }
        }
        _ => None,
    }
}

fn avisa(avisos: &mut Vec<String>, valor: &Valor, texto: &str) {
    avisos.push(format!("line {}: {texto}", valor.linha));
}

/// `true`, `sim`, `1`, `ligado` — e os contrários.
fn booleano(ini: &mut Ini, secao: &str, chave: &str, destino: &mut bool, avisos: &mut Vec<String>) {
    let Some(v) = ini.pega(secao, chave) else {
        return;
    };
    match v.texto.to_lowercase().as_str() {
        "true" | "yes" | "1" | "on" => *destino = true,
        "false" | "no" | "0" | "off" => *destino = false,
        outro => avisa(avisos, &v, &format!("`{chave}` expects true or false, not `{outro}`")),
    }
}

fn inteiro(ini: &mut Ini, secao: &str, chave: &str, destino: &mut u32, avisos: &mut Vec<String>) {
    let Some(v) = ini.pega(secao, chave) else {
        return;
    };
    match v.texto.parse::<u32>() {
        Ok(n) => *destino = n,
        Err(_) => avisa(avisos, &v, &format!("`{chave}` expects a number")),
    }
}

/// Um número pequeno, preso a uma faixa. Fora dela é erro de quem escreveu, e dizer o limite
/// vale mais que aceitar em silêncio um `resolucao_interna = 40` que não caberia na placa.
fn oito(
    ini: &mut Ini,
    secao: &str,
    chave: &str,
    destino: &mut u8,
    minimo: u8,
    maximo: u8,
    avisos: &mut Vec<String>,
) {
    let Some(v) = ini.pega(secao, chave) else {
        return;
    };
    match v.texto.parse::<u8>() {
        Ok(n) if (minimo..=maximo).contains(&n) => *destino = n,
        Ok(n) => avisa(avisos, &v, &format!("`{chave}` = {n} is outside {minimo}..{maximo}")),
        Err(_) => avisa(avisos, &v, &format!("`{chave}` expects a number")),
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use zeebx::ui::settings::ModoDaJanela;

    /// Um arquivo vazio precisa dar exatamente as opções de fábrica: é o que faz um
    /// `config.ini` ausente ser a mesma coisa que um `config.ini` sem nada dentro.
    #[test]
    fn arquivo_vazio_e_o_padrao() {
        let lido = de_texto("");
        // O padrão da interface é abrir maximizado, e o daqui é abrir no tamanho que o arquivo
        // pede — uma janela maximizada faria `largura` e `altura` não quererem dizer nada.
        let esperado = Settings {
            graphics: Graphics {
                janela: ModoDaJanela::Janela,
                janela_do_jogo: ModoDaJanela::Janela,
                ..Graphics::default()
            },
            ..Settings::default()
        };
        assert_eq!(lido.settings, esperado);
        assert_eq!(lido.headless.video, Video::Janela);
        assert!(lido.avisos.is_empty(), "{:?}", lido.avisos);
    }

    #[test]
    fn as_chaves_chegam_no_settings_do_nucleo() {
        let lido = de_texto(
            "[video]\n\
             mode = fullscreen\n\
             scaling = integer\n\
             smooth = true\n\
             speed_limit = false\n\
             [graphics]\n\
             gpu_rasterizer = true\n\
             internal_resolution = 2\n\
             aspect = 16x9\n\
             [audio]\n\
             volume = 42\n",
        );
        assert!(lido.avisos.is_empty(), "{:?}", lido.avisos);
        let g = &lido.settings.graphics;
        assert_eq!(lido.headless.video, Video::TelaCheia);
        assert_eq!(g.janela_do_jogo, ModoDaJanela::TelaCheia);
        assert_eq!(g.scaling, Scaling::Integer);
        assert!(g.smooth);
        assert!(!g.speed_limit);
        assert!(g.gpu_rasterizer);
        assert_eq!(g.resolucao_interna, 2);
        assert_eq!(g.proporcao, Proporcao::Larga16x9);
        assert_eq!(lido.settings.audio.volume, 42);
    }

    /// Citar a seção já liga a porta, e uma porta não citada fica como vem de fábrica.
    #[test]
    fn a_secao_da_porta_liga_a_porta() {
        let lido = de_texto("[port2]\ndevice = keyboard\n");
        let portas = &lido.settings.controls;
        assert!(portas.player(0).unwrap().ligada, "a primeira porta é de fábrica");
        let segunda = portas.player(1).unwrap();
        assert!(segunda.ligada);
        assert_eq!(segunda.aparelho, Aparelho::Teclado);
    }

    /// O direcional nos eixos sai do `config.ini` como qualquer outra chave da porta — e a
    /// porta que não escreve fica com o padrão, que é desligado.
    #[test]
    fn o_direcional_nos_eixos_vem_do_ini_e_o_padrao_e_desligado() {
        let lido = de_texto("[port1]\ndpad_to_analog = yes\n");
        assert!(lido.avisos.is_empty(), "{:?}", lido.avisos);
        assert!(lido.settings.controls.player(0).unwrap().direcional_nos_eixos);
        assert!(
            !lido.settings.controls.player(1).unwrap().direcional_nos_eixos,
            "a segunda porta não escreveu, e o padrão é desligado"
        );

        // E o valor de fábrica que o gerador escreve volta como o padrão do emulador: é o que
        // cobra o teste que compara o arquivo completo com `de_texto("")`.
        let padrao = de_texto("[port1]\ndpad_to_analog = false\n");
        assert!(!padrao.settings.controls.player(0).unwrap().direcional_nos_eixos);
    }

    /// Escrever um botão substitui o padrão dele, e só dele.
    #[test]
    fn o_botao_escrito_substitui_o_padrao() {
        let lido = de_texto("[port1]\nb1 = key:Q, button:North\n");
        assert!(lido.avisos.is_empty(), "{:?}", lido.avisos);
        let porta = lido.settings.controls.player(0).unwrap();
        assert_eq!(
            porta.sources("b1"),
            [Source::key("Q"), Source::button("North")]
        );
        // O `b2` não foi citado: continua com o que vem de fábrica.
        assert!(!porta.sources("b2").is_empty());
    }

    /// Um botão com a linha vazia fica sem origem nenhuma — é assim que se desliga um botão.
    #[test]
    fn o_botao_vazio_fica_sem_origem() {
        let lido = de_texto("[port1]\nb2 =\n");
        assert!(lido.settings.controls.player(0).unwrap().sources("b2").is_empty());
    }

    #[test]
    fn os_eixos_saem_com_o_sentido_pedido() {
        let lido = de_texto("[port1]\naxis_y = LeftStickY:inverted\naxis_x = RightStickX\n");
        assert!(lido.avisos.is_empty(), "{:?}", lido.avisos);
        let porta = lido.settings.controls.player(0).unwrap();
        assert_eq!(porta.axes["y"], AxisSource { name: "LeftStickY".into(), invert: true });
        assert_eq!(porta.axes["x"], AxisSource { name: "RightStickX".into(), invert: false });
    }

    /// O sinal do eixo como origem de botão: é o que faz um manche acionar o direcional.
    #[test]
    fn o_eixo_como_origem_traz_o_sentido() {
        let lido = de_texto("[port1]\nup = axis:LeftStickY-\n");
        assert_eq!(
            lido.settings.controls.player(0).unwrap().sources("up"),
            [Source::Axis { name: "LeftStickY".into(), positive: false }]
        );
    }

    /// Valor fora da faixa, origem sem sentido e chave desconhecida precisam **aparecer**: no
    /// silêncio, quem editou o arquivo acha que configurou e não configurou.
    #[test]
    fn o_que_esta_errado_e_dito() {
        let lido = de_texto(
            "[graphics]\n\
             internal_resolution = 40\n\
             [video]\n\
             mode = square\n\
             [port1]\n\
             b1 = Z\n\
             [audio]\n\
             volumee = 10\n",
        );
        let tudo = lido.avisos.join("\n");
        assert!(tudo.contains("internal_resolution"), "{tudo}");
        assert!(tudo.contains("square"), "{tudo}");
        assert!(tudo.contains("is not a source"), "{tudo}");
        assert!(tudo.contains("volumee"), "{tudo}");
        // E nada disso impede o resto de valer.
        assert_eq!(lido.settings.graphics.resolucao_interna, 1);
    }

    /// Um controle escolhido pelo nome ganha os eixos padrão junto, senão o manche fica mudo e
    /// quem escreveu só o nome não teria como adivinhar que faltava mais.
    #[test]
    fn o_controle_escolhido_ganha_os_eixos() {
        let lido = de_texto("[port1]\ncontroller = \"Pad #2\"\n");
        let porta = lido.settings.controls.player(0).unwrap();
        assert_eq!(porta.device.as_deref(), Some("Pad #2"));
        assert_eq!(porta.axes, Player::default_axes());
    }

    /// O exemplo que o `--exemplo` imprime tem de ser um arquivo que o leitor aceita sem
    /// reclamar. Um exemplo que dá aviso ensina errado.
    #[test]
    fn o_exemplo_distribuido_e_valido() {
        let lido = de_texto(&modelo());
        assert!(lido.avisos.is_empty(), "{:?}", lido.avisos);
    }

    /// **Cada linha do arquivo gerado diz o que já valeria sem ela.** É o que permite entregá-lo
    /// com tudo escrito: quem edita vê o padrão e o troca, em vez de adivinhar o que escrever.
    /// Um padrão que mude no código sem mudar aqui cai neste teste.
    #[test]
    fn o_arquivo_completo_descreve_os_proprios_padroes() {
        assert_eq!(de_texto(&modelo()).settings, de_texto("").settings);
    }

    /// E as portas saem completas: as duas seções, com os treze botões cada uma.
    #[test]
    fn as_portas_saem_com_todos_os_botoes() {
        let texto = modelo();
        for porta in 1..=zeebx::input::PORTAS {
            assert!(texto.contains(&format!("[port{porta}]")), "falta a porta {porta}");
        }
        let lido = de_texto(&texto);
        for indice in 0..zeebx::input::PORTAS {
            let player = lido.settings.controls.player(indice).unwrap();
            for botao in zeebx::input::bindings::CONFIGURABLE {
                assert!(
                    !player.sources(botao).is_empty(),
                    "porta {indice}: o botão {botao} saiu sem origem"
                );
            }
        }
        // A segunda continua desligada: ela aparece para ser editada, não para ser ligada.
        assert!(!lido.settings.controls.player(1).unwrap().ligada);
    }
}

#[cfg(test)]
mod testes_de_criacao {
    use super::*;

    fn pasta(nome: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zeebx-headless-{nome}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// O que a primeira execução escreve tem de voltar a ser lido sem nenhum aviso, e dar
    /// exatamente as opções de fábrica. Um arquivo criado que já nasce reclamando seria pior
    /// que nenhum.
    #[test]
    fn o_arquivo_criado_e_lido_de_volta_sem_avisos() {
        let caminho = pasta("cria").join("config.ini");
        cria(&caminho).unwrap();

        let lido = carrega(Some(&caminho));
        assert!(matches!(lido.origem, Origem::Lida));
        // O primeiro aviso é o "lendo ..."; nenhum outro.
        assert_eq!(lido.avisos.len(), 1, "{:?}", lido.avisos);
        assert_eq!(lido.settings, de_texto("").settings);
        // E sai reto: um arquivo que nasce torto ensina torto a quem for editá-lo.
        let texto = std::fs::read_to_string(&caminho).unwrap();
        for linha in texto.lines() {
            assert!(
                !linha.starts_with(' ') && !linha.starts_with('\t'),
                "linha indentada no arquivo gerado: {linha:?}"
            );
        }
        let _ = std::fs::remove_dir_all(caminho.parent().unwrap());
    }

    /// **Nunca por cima.** Um `config.ini` cheio de ajustes é trabalho de quem o escreveu.
    #[test]
    fn nao_escreve_por_cima_do_que_existe() {
        let caminho = pasta("existente").join("config.ini");
        std::fs::create_dir_all(caminho.parent().unwrap()).unwrap();
        std::fs::write(&caminho, "[audio]\nvolume = 7\n").unwrap();

        assert!(cria(&caminho).is_err());
        assert_eq!(carrega(Some(&caminho)).settings.audio.volume, 7);
        let _ = std::fs::remove_dir_all(caminho.parent().unwrap());
    }

    /// Um `--config=` apontando para o que não existe diz que falta **naquele** caminho: é ali
    /// que o arquivo deve nascer, e não na pasta de configuração do sistema.
    #[test]
    fn o_config_pedido_e_onde_o_arquivo_vai_nascer() {
        let caminho = pasta("pedido").join("meu.ini");
        match carrega(Some(&caminho)).origem {
            Origem::Faltando(onde) => assert_eq!(onde, caminho),
            Origem::Lida => panic!("não deveria haver arquivo"),
        }
    }
}
