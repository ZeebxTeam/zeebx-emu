//! As preferências do usuário, guardadas em disco.
//!
//! Tudo aqui é opcional e tem um padrão razoável: um arquivo faltando, truncado ou de uma
//! versão mais nova do emulador precisa deixar o programa abrir, não impedi-lo. Por isso cada
//! campo é `#[serde(default)]` e a leitura nunca falha — no pior caso, volta o padrão.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::input::bindings::Source;
use crate::velocidade::rewind::AjustesDoRewind;
use crate::velocidade::{Avanco, Frameskip, LimiteFps, Ritmo};

const FILE_NAME: &str = "settings.json";

/// Reexportado para preservar a API desktop durante a migração para configuração neutra.
pub use crate::config::config_dir;

/// Como a imagem do console preenche a janela.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scaling {
    /// Só múltiplos inteiros do tamanho original. Nunca borra, mas sobra borda.
    Integer,
    /// O maior tamanho que cabe na janela, mantendo a proporção. É o padrão: numa janela
    /// maximizada o pixel inteiro deixa uma moldura larga à toa.
    #[default]
    Fit,
    /// Preenche a janela inteira, custe o que custar à proporção.
    Stretch,
}

impl Scaling {
    pub const ALL: [Self; 3] = [Self::Integer, Self::Fit, Self::Stretch];

    /// A chave do texto que descreve esta opção.
    pub fn key(self) -> &'static str {
        match self {
            Self::Integer => "graphics.scaling.integer",
            Self::Fit => "graphics.scaling.fit",
            Self::Stretch => "graphics.scaling.stretch",
        }
    }
}

/// Como uma janela abre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModoDaJanela {
    /// Do tamanho que o emulador escolhe.
    Janela,
    #[default]
    Maximizada,
    TelaCheia,
}

impl ModoDaJanela {
    pub const TODOS: [Self; 3] = [Self::Janela, Self::Maximizada, Self::TelaCheia];

    pub fn chave(self) -> &'static str {
        match self {
            Self::Janela => "graphics.window.windowed",
            Self::Maximizada => "graphics.window.maximized",
            Self::TelaCheia => "graphics.window.fullscreen",
        }
    }
}

/// O que o painel de depuração mostra durante a execução.
///
/// Tudo desligado por padrão: é ferramenta de quem está caçando um problema, e informação
/// sobre o quadro atrapalha quem só quer jogar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugView {
    /// Liga o painel. Sem ele, nada do que está abaixo aparece.
    pub overlay: bool,
    /// Velocidade em relação ao console e quadros por segundo.
    pub speed: bool,
    /// Instruções do guest por segundo, e o relógio virtual.
    pub clock: bool,
    /// Heap do jogo e objetos vivos.
    pub memory: bool,
    /// O gráfico com a história recente.
    pub timeline: bool,
    /// A janela separada com o log da execução.
    pub log: bool,
    /// Grava cada execução num arquivo próprio, com carimbo de hora em cada linha. Ver
    /// [`crate::ui::partida::caminho_da_sessao`].
    pub gravar_sessao: bool,
    /// Até que nível o **núcleo** registra: `aviso` (padrão), `informacao`, `depuracao`,
    /// `erro`, `fatal` ou `desligado`. Ver [`crate::registro`].
    ///
    /// É texto e não um `enum` de propósito: o mesmo vocabulário vale no `config.ini` do
    /// headless, na opção `zeebx_log` do core Libretro e na variável `ZEEBX_LOG`, e converter
    /// num lugar só ([`crate::registro::Ajuste::de_texto`]) evita quatro tabelas que divergem.
    pub nivel_de_log: String,
}

impl Default for DebugView {
    fn default() -> Self {
        Self {
            overlay: false,
            speed: true,
            clock: true,
            memory: true,
            timeline: true,
            log: false,
            gravar_sessao: false,
            nivel_de_log: "aviso".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Graphics {
    /// Como a janela principal abre.
    pub janela: ModoDaJanela,
    /// Como a janela do jogo abre.
    pub janela_do_jogo: ModoDaJanela,
    /// O tamanho em que a janela principal estava quando foi fechada, em pontos.
    ///
    /// `None` é o padrão de fábrica (960×720). Só vale em modo janela: maximizada ou em tela
    /// cheia, o tamanho da tela não é escolha do usuário e não é guardado.
    pub tamanho_da_janela: Option<[f32; 2]>,
    /// O mesmo, para a janela do jogo.
    pub tamanho_da_janela_do_jogo: Option<[f32; 2]>,
    pub scaling: Scaling,
    /// Interpolar ao ampliar. Desligado, o pixel do console aparece como bloco.
    pub smooth: bool,
    /// Manter os 4:3 da tela do Zeebo.
    pub keep_aspect: bool,
    /// O `speed_limit` de antes do limite de quadros, **só para ler** um arquivo antigo.
    ///
    /// Desligado virava [`crate::velocidade::LimiteFps::Desligado`]; ver
    /// [`Settings::load_from`]. Nunca é gravado: o arquivo novo diz o mesmo em
    /// [`Velocidade::limite_fps`].
    #[serde(rename = "speed_limit", skip_serializing)]
    pub limite_antigo: Option<bool>,
    /// Pôr o quadro na tela pelo GL da janela, em vez de mandá-lo como textura do egui.
    ///
    /// Pela placa o quadro sobe em RGB565, que é o formato em que ele já está, e quem amplia é
    /// ela. O caminho antigo converte para ARGB e depois para RGBA a cada repaint. **Não muda o
    /// desenho do jogo** — o 3D continua sendo rasterizado em software —, e por isso a diferença
    /// de desempenho é pequena; existe para ser o lugar em que um backend de GPU entraria.
    ///
    /// Um driver que recuse os shaders faz o emulador voltar sozinho ao caminho antigo.
    pub gpu_present: bool,
    /// Preencher o 3D do console na placa, em vez de na CPU.
    ///
    /// Isto **muda quem rasteriza**, e não só como o quadro pronto chega à tela. A etapa de
    /// vértice continua na CPU e é a mesma nos dois — matrizes, matriz de textura e iluminação
    /// —; o que vai para a placa é o preenchimento, que é onde a medição mostrou o tempo.
    ///
    /// Medido, tempo de API: Z-Wheel de 4340 para 2805 ms (-35%) em treze segundos virtuais, e o
    /// Crash na pista de 3438 para 2793 ms (-19%) em trinta.
    ///
    /// O desenho **não** é idêntico ao de software: regra de borda e arredondamento divergem por
    /// construção. Medido na Z-Wheel, 1% dos pixels difere além de oito níveis e 0,06% além de
    /// trinta e dois; sete das nove superfícies saem byte a byte iguais, porque não passam pelo
    /// rasterizador. Sem placa alcançável o emulador segue em software e diz o motivo.
    pub gpu_rasterizer: bool,
    /// A resolução interna do 3D preenchido na placa, em múltiplos de 640×480 por lado.
    ///
    /// Só tem efeito com o [`Graphics::gpu_rasterizer`]: o jogo continua vendo 640×480, e o
    /// quadro grande só chega à janela quando nada 2D foi desenhado sobre ele — ver
    /// [`crate::machine::Machine::quadro_na_placa`]. Nos outros casos a imagem é a de sempre.
    pub resolucao_interna: u8,
    /// Antialias do 3D na placa, em amostras por pixel (1 é desligado).
    pub antialias: u8,
    /// Filtro anisotrópico das texturas do 3D na placa (1 é desligado).
    pub anisotropico: u8,
    /// **Experimental.** A proporção em que o 3D na placa é renderizado.
    pub proporcao: Proporcao,
    /// Deixar a névoa do jogo valer.
    ///
    /// Vale nos dois rasterizadores. No console a névoa costuma esconder o que a distância de
    /// desenho não alcançava, e aqui a cena chega inteira — quem prefere ver longe desliga. Fica
    /// **ligada** por omissão: o jogo pediu a névoa, e em muitos ela é o efeito, não o remendo.
    pub neblina: bool,
}

/// A proporção do 3D renderizado na placa. Fora do nativo, a cena em perspectiva ganha lados em
/// vez de ser esticada; HUD e 2D ficam em 4:3 no centro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Proporcao {
    #[default]
    Nativa,
    Larga16x9,
    Larga16x10,
    /// A da janela do jogo.
    Janela,
}

impl Proporcao {
    pub const TODAS: [Self; 4] = [Self::Nativa, Self::Larga16x9, Self::Larga16x10, Self::Janela];

    pub fn chave(self) -> &'static str {
        match self {
            Self::Nativa => "graphics.aspect.native",
            Self::Larga16x9 => "graphics.aspect.16x9",
            Self::Larga16x10 => "graphics.aspect.16x10",
            Self::Janela => "graphics.aspect.window",
        }
    }

    /// Largura sobre altura, com a da janela quando é o caso. `None` é o nativo.
    pub fn aspecto(self, janela: f32) -> Option<f32> {
        match self {
            Self::Nativa => None,
            Self::Larga16x9 => Some(16.0 / 9.0),
            Self::Larga16x10 => Some(16.0 / 10.0),
            Self::Janela => Some(janela),
        }
    }
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            janela: ModoDaJanela::Maximizada,
            janela_do_jogo: ModoDaJanela::Maximizada,
            tamanho_da_janela: None,
            tamanho_da_janela_do_jogo: None,
            scaling: Scaling::default(),
            smooth: false,
            keep_aspect: true,
            limite_antigo: None,
            gpu_present: true,
            // Desligado por padrão: é um rasterizador novo, e a revisão jogo a jogo é de quem
            // usa. Nos dois títulos medidos ele ganha, mas isso não é licença para trocar o
            // desenho de todos os outros sem que alguém os tenha olhado.
            gpu_rasterizer: false,
            resolucao_interna: 1,
            antialias: 1,
            anisotropico: 1,
            proporcao: Proporcao::Nativa,
            neblina: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    pub enabled: bool,
    /// De 0 a 100.
    pub volume: u8,
    /// O banco `.sf2` com que o MIDI toca. `None` é a busca automática: o primeiro `.sf2` da pasta
    /// `soundfonts` do aparelho. Ver [`crate::audio::soundfont::primeiro_banco`].
    ///
    /// Existe para quem tem o banco do firmware do console, ou outro de gosto, e não quer mexer
    /// na pasta do aparelho. Uma configuração salva antes dele abre com `None`.
    pub soundfont: Option<PathBuf>,
    /// O reverb e o chorus que a partitura pede, quando ela toca pelo banco `.sf2`. Ver
    /// [`crate::audio::soundfont::define_efeitos`].
    pub midi_effects: bool,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            enabled: true,
            volume: 80,
            soundfont: None,
            midi_effects: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Idioma da interface. `None` significa "o do sistema".
    pub language: Option<String>,
    /// Onde procurar os jogos.
    pub roms_dir: Option<PathBuf>,
    /// O pacote da Z-Wheel: abre pela barra de cima e empresta as capas à biblioteca.
    pub z_wheel_path: Option<PathBuf>,
    /// Onde os screenshots são gravados. `None` é a pasta padrão, ver
    /// [`crate::ui::screenshot::pasta_padrao`].
    pub screenshots_dir: Option<PathBuf>,
    pub atalhos: Atalhos,
    /// Como a biblioteca mostra os jogos.
    pub biblioteca: ModoDaBiblioteca,
    pub movimento: Movimento,
    pub controles_na_tela: ControlesNaTela,
    pub graphics: Graphics,
    pub velocidade: Velocidade,
    pub debug: DebugView,
    pub audio: Audio,
    pub controls: crate::input::bindings::Controls,
    pub z_wheel: ZWheel,
    pub discord: Discord,
    pub atualizacoes: Atualizacoes,
    /// A versão em que o aviso de abertura foi dispensado de vez. Outra versão mostra de novo.
    pub aviso_dispensado_na_versao: Option<String>,
}

/// As teclas da janela do jogo que se trocam, pelo nome de tecla do mapeamento (`F9`).
///
/// Esc, P e F11 continuam fixos na janela; é aqui que eles entram se um dia forem trocáveis. Ver
/// `docs/implementacao/22-screenshots.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Atalhos {
    pub screenshot: String,
    /// O fast-forward: tecla ou botão de controle, de qualquer controle ligado. Ver
    /// [`crate::velocidade::Avanco`].
    ///
    /// **Uma lista, e de `Source`**, ao contrário do screenshot: quem joga com controle precisa
    /// de um botão, e o mapeamento dos botões do jogo já sabe guardar um. Um botão que está aqui
    /// não chega ao jogo — ver [`Atalhos::reservadas`].
    pub avancar: Vec<Source>,
    /// O rewind, que se segura: tecla ou botão, como o [`Atalhos::avancar`]. Ver
    /// [`crate::velocidade::rewind`].
    pub voltar: Vec<Source>,
    /// Reiniciar o jogo aberto, na hora. Só de teclado, como o screenshot; vazio é sem tecla.
    pub reiniciar: String,
    /// Os save states da janela do jogo: gravar no slot escolhido, carregar dele, trocar de slot
    /// e desfazer o último carregar. Ver [`crate::ui::estados`].
    ///
    /// **Uma [`Combinacao`], e não uma tecla só**, ao contrário dos de cima: no Mac as teclas F
    /// são de brilho, de volume e do Mission Control, e o padrão de lá é com o ⌘. Só de teclado,
    /// como o screenshot: um botão de controle que grava ou carrega estado é fácil de apertar sem
    /// querer. Vazio é sem tecla.
    pub salvar_estado: String,
    pub carregar_estado: String,
    pub proximo_slot: String,
    pub slot_anterior: String,
    pub desfazer_carregar: String,
}

impl Atalhos {
    pub const SCREENSHOT_PADRAO: &str = "F9";
    /// F10 segurado é o fast-forward desde o primeiro desenho dele, no PR #83; quem já se
    /// acostumou não perde o hábito.
    pub const AVANCAR_PADRAO: &str = "F10";
    /// O Backspace, que seria a escolha óbvia, já é o `AVK_CLR` do jogo.
    pub const VOLTAR_PADRAO: &str = "F8";
    /// O R de "reset", que nenhum botão do teclado de fábrica usa.
    pub const REINICIAR_PADRAO: &str = "R";

    pub fn avancar_padrao() -> Vec<Source> {
        vec![Source::key(Self::AVANCAR_PADRAO)]
    }

    /// Os do save state seguem o PCSX2 no Windows e no Linux — F1 grava, F3 carrega, F2 troca de
    /// slot — e o costume do Mac fora dele, onde ⌘S e ⌘L são o gravar e o abrir de todo programa
    /// e o OpenEmu faz o mesmo. Nenhum jogo usa o ⌘, então nada colide com os botões.
    pub const SALVAR_ESTADO_PADRAO: &str = match cfg!(target_os = "macos") {
        true => "Cmd+S",
        false => "F1",
    };
    pub const CARREGAR_ESTADO_PADRAO: &str = match cfg!(target_os = "macos") {
        true => "Cmd+L",
        false => "F3",
    };
    pub const PROXIMO_SLOT_PADRAO: &str = match cfg!(target_os = "macos") {
        true => "Cmd+CloseBracket",
        false => "F2",
    };
    pub const SLOT_ANTERIOR_PADRAO: &str = match cfg!(target_os = "macos") {
        true => "Cmd+OpenBracket",
        false => "Shift+F2",
    };
    /// O F4 fica ao lado dos três; o ⌘Z é o desfazer de todo programa do Mac.
    pub const DESFAZER_CARREGAR_PADRAO: &str = match cfg!(target_os = "macos") {
        true => "Cmd+Z",
        false => "F4",
    };

    /// As origens que são de atalho e por isso não chegam ao jogo.
    ///
    /// Um botão de controle ligado a um atalho e também a um botão do Zeebo faria as duas coisas
    /// ao mesmo tempo: avançar e, digamos, pular. O atalho ganha, e o editor avisa do conflito.
    pub fn reservadas(&self) -> impl Iterator<Item = &Source> {
        self.avancar.iter().chain(&self.voltar)
    }
}

impl Default for Atalhos {
    fn default() -> Self {
        Self {
            screenshot: Self::SCREENSHOT_PADRAO.to_string(),
            avancar: Self::avancar_padrao(),
            voltar: vec![Source::key(Self::VOLTAR_PADRAO)],
            reiniciar: Self::REINICIAR_PADRAO.to_string(),
            salvar_estado: Self::SALVAR_ESTADO_PADRAO.to_string(),
            carregar_estado: Self::CARREGAR_ESTADO_PADRAO.to_string(),
            proximo_slot: Self::PROXIMO_SLOT_PADRAO.to_string(),
            slot_anterior: Self::SLOT_ANTERIOR_PADRAO.to_string(),
            desfazer_carregar: Self::DESFAZER_CARREGAR_PADRAO.to_string(),
        }
    }
}

/// Os modificadores de uma [`Combinacao`].
///
/// **O `cmd` é o ⌘ no Mac.** Fora dele é a tecla do sistema (a do Windows, o Super), que o Qt
/// chama de Meta nos dois casos — mas no Mac o Qt troca os nomes, e o que ele chama de Control é
/// o ⌘. Quem lê o teclado de um toolkit é quem desfaz a troca.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modificadores {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

impl Modificadores {
    pub fn nenhum(&self) -> bool {
        *self == Self::default()
    }
}

/// Uma tecla e os modificadores que vão com ela: `F1`, `Shift+F2`, `Cmd+S`.
///
/// Gravada como texto, com o nome de tecla do `egui::Key` no fim e os modificadores na frente,
/// separados por `+`. Uma tecla sozinha é escrita como sempre foi (`F9`), e por isso o mesmo texto
/// serve aos atalhos de uma tecla só. Os nomes do egui não têm `+`: o da tecla de mais é `Plus`.
///
/// **Bate só com os mesmos modificadores.** O F2 troca para o próximo slot e o Shift+F2 para o
/// anterior: aceitar modificadores a mais faria o Shift+F2 também andar para a frente.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Combinacao {
    pub tecla: egui::Key,
    pub modificadores: Modificadores,
}

impl Combinacao {
    /// Lê o texto gravado. `None` para o vazio, para um modificador desconhecido ou para uma
    /// tecla que o egui não conhece.
    pub fn le(texto: &str) -> Option<Self> {
        let mut partes: Vec<&str> = texto.split('+').map(str::trim).collect();
        let tecla = egui::Key::from_name(partes.pop()?)?;
        let mut modificadores = Modificadores::default();
        for parte in partes {
            let campo = match parte {
                "Ctrl" => &mut modificadores.ctrl,
                "Alt" => &mut modificadores.alt,
                "Shift" => &mut modificadores.shift,
                "Cmd" => &mut modificadores.cmd,
                _ => return None,
            };
            *campo = true;
        }
        Some(Self {
            tecla,
            modificadores,
        })
    }

    /// O texto que se grava: os modificadores na ordem do Ctrl+Alt+Shift+Cmd, e a tecla.
    pub fn nome(&self) -> String {
        let m = &self.modificadores;
        let mut nome = String::new();
        for (ligado, texto) in [
            (m.ctrl, "Ctrl+"),
            (m.alt, "Alt+"),
            (m.shift, "Shift+"),
            (m.cmd, "Cmd+"),
        ] {
            if ligado {
                nome.push_str(texto);
            }
        }
        nome.push_str(self.tecla.name());
        nome
    }

    /// O que se mostra a quem lê: no Mac, os símbolos de lá (`⇧⌘S`); fora dele, `Shift+F2`. A
    /// tecla vai pelo símbolo quando tem um: `]`, e não `CloseBracket`.
    pub fn rotulo(&self) -> String {
        let m = &self.modificadores;
        let tecla = self.tecla.symbol_or_name();
        if cfg!(target_os = "macos") {
            let mut rotulo = String::new();
            for (ligado, simbolo) in [(m.ctrl, "⌃"), (m.alt, "⌥"), (m.shift, "⇧"), (m.cmd, "⌘")]
            {
                if ligado {
                    rotulo.push_str(simbolo);
                }
            }
            rotulo.push_str(tecla);
            return rotulo;
        }
        let sistema = match cfg!(target_os = "windows") {
            true => "Win+",
            false => "Super+",
        };
        let mut rotulo = String::new();
        for (ligado, texto) in [
            (m.ctrl, "Ctrl+"),
            (m.alt, "Alt+"),
            (m.shift, "Shift+"),
            (m.cmd, sistema),
        ] {
            if ligado {
                rotulo.push_str(texto);
            }
        }
        rotulo.push_str(tecla);
        rotulo
    }
}

/// A procura por versões novas. Ver [`crate::ui::atualizacao`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Atualizacoes {
    /// Perguntar ao GitHub na abertura.
    pub ao_abrir: bool,
    /// Se uma release marcada como pré-lançamento conta como versão nova. **Ligado de fábrica**:
    /// até a 0.4.1 todas as releases saíram assim, e desligado ninguém seria avisado de nada.
    pub pre_lancamentos: bool,
}

impl Default for Atualizacoes {
    fn default() -> Self {
        Self { ao_abrir: true, pre_lancamentos: true }
    }
}

/// O Rich Presence do Discord. Ver [`crate::ui::discord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Discord {
    pub ativo: bool,
    /// Onde as capas estão publicadas, com `{clsid}` ou `{chave}` no lugar do jogo. Vazio usa
    /// as imagens cadastradas no próprio aplicativo.
    pub capas_url: String,
}

impl Default for Discord {
    fn default() -> Self {
        Self {
            ativo: true,
            capas_url: String::new(),
        }
    }
}

/// O controle de movimento, fora do mapeamento de cada porta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Movimento {
    /// O aviso no canto do jogo enquanto ele calibra o Boomerang.
    pub aviso_de_calibracao: bool,
}

impl Default for Movimento {
    fn default() -> Self {
        Self {
            aviso_de_calibracao: true,
        }
    }
}

/// Quando os controles desenhados na tela aparecem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModoDosControlesNaTela {
    /// Aparecem até chegar um aperto de controle físico, e voltam no próximo toque na tela. É o
    /// padrão porque serve aos dois jeitos de jogar sem pedir que ninguém escolha.
    #[default]
    Automatico,
    Sempre,
    Nunca,
}

impl ModoDosControlesNaTela {
    pub const TODOS: [Self; 3] = [Self::Automatico, Self::Sempre, Self::Nunca];

    pub fn chave(self) -> &'static str {
        match self {
            Self::Automatico => "touch.mode.auto",
            Self::Sempre => "touch.mode.always",
            Self::Nunca => "touch.mode.never",
        }
    }
}

/// Os controles desenhados na tela de um celular. Só o Android os desenha; os outros frontends
/// carregam e gravam de volta sem mexer. Ver [`crate::input::toque`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlesNaTela {
    pub modo: ModoDosControlesNaTela,
    /// O tamanho das peças, em porcento do de fábrica.
    pub escala: u8,
    /// Em porcento. Opaco de fábrica não: a peça fica por cima do jogo.
    pub opacidade: u8,
    /// Onde o usuário pôs cada peça, pela [`crate::input::toque::Peca::chave`], em **fração** da
    /// tela. Fração, e não ponto, porque o mesmo arquivo pode ir de um celular para um tablet. A
    /// peça que não está aqui fica no lugar de fábrica, e "restaurar" é esvaziar o mapa.
    pub posicoes: std::collections::BTreeMap<String, [f32; 2]>,
    /// O tamanho de cada peça, em porcento, por cima da [`Self::escala`]. Mesma chave das
    /// posições; a peça que não está aqui fica em 100.
    pub tamanhos: std::collections::BTreeMap<String, u8>,
    /// Mostrar as peças de atalho. O turbo e o rewind, além disso, só aparecem com a função em
    /// uso — ver [`ControlesNaTela::mostra`].
    pub mostrar_avancar: bool,
    pub mostrar_voltar: bool,
    pub mostrar_turbo: bool,
}

impl Default for ControlesNaTela {
    fn default() -> Self {
        Self {
            modo: ModoDosControlesNaTela::default(),
            escala: 100,
            opacidade: 55,
            posicoes: Default::default(),
            tamanhos: Default::default(),
            mostrar_avancar: true,
            mostrar_voltar: true,
            mostrar_turbo: true,
        }
    }
}

impl ControlesNaTela {
    /// Se a peça aparece na tela.
    ///
    /// **O turbo e o rewind só aparecem com a função em uso**: o rewind vem desligado, e quem
    /// nunca o ligou não ganha um botão que não faz nada. O fast-forward não tem ajuste de ligar,
    /// só a velocidade, e por isso aparece sempre — no celular não existe F10, e sem a peça ele
    /// ficaria escondido de quem não procura nos ajustes. Cada um se esconde pela chave dele.
    pub fn mostra(&self, peca: crate::input::toque::Peca, settings: &Settings) -> bool {
        use crate::input::toque::Peca;
        match peca {
            Peca::Avancar => self.mostrar_avancar,
            Peca::Voltar => self.mostrar_voltar && settings.velocidade.rewind.ligado,
            Peca::Turbo => {
                let modo = settings.controls.player(0).map(|jogador| jogador.turbo);
                self.mostrar_turbo
                    && modo.is_some_and(|modo| modo != crate::velocidade::turbo::ModoDoTurbo::Desligado)
            }
            _ => true,
        }
    }
}

/// A disposição da biblioteca na tela principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModoDaBiblioteca {
    /// Cartões lado a lado, quantos couberem na largura.
    #[default]
    Grade,
    /// Um jogo por vez, no jeito da Z-Wheel: o rolo de logos em cima e as caixas passando.
    Slider,
}

/// O ritmo do jogo: o limite de quadros e o pulo de desenho. Ver [`crate::velocidade`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Velocidade {
    pub limite_fps: LimiteFps,
    pub frameskip: Frameskip,
    pub avanco: Avanco,
    /// Quantos toques por segundo o turbo dá, no relógio do jogo. Um só para todos os jogadores:
    /// o modo e o botão é que são de cada um. Ver [`crate::velocidade::turbo`].
    pub turbo_por_segundo: u8,
    pub rewind: AjustesDoRewind,
}

impl Default for Velocidade {
    fn default() -> Self {
        Self {
            limite_fps: LimiteFps::default(),
            frameskip: Frameskip::default(),
            avanco: Avanco::default(),
            turbo_por_segundo: crate::velocidade::turbo::TOQUES_PADRAO,
            rewind: AjustesDoRewind::default(),
        }
    }
}

impl Velocidade {
    /// O ritmo destas escolhas, para entregar à sessão.
    pub fn ritmo(&self) -> Ritmo {
        Ritmo::das_escolhas(self.limite_fps, self.frameskip)
    }

    /// O ritmo de agora: o das escolhas, ou o do fast-forward por cima dele.
    pub fn ritmo_com(&self, avancando: bool) -> Ritmo {
        match avancando {
            true => self.ritmo().acelerado(&self.avanco),
            false => self.ritmo(),
        }
    }
}

/// As configurações de um jogo só, quando ele tem as próprias: áudio, gráficos, velocidade e
/// controles.
///
/// Guardadas em `<config>/jogos/<chave>.json`, uma por jogo; **o arquivo existir é o "usar
/// configurações personalizadas"**. Cada seção é uma cópia inteira da global no momento em que a
/// opção foi ligada, e não só a diferença: mudar a global depois não mexe no jogo, que é o que
/// "personalizado" quer dizer. A chave é a de [`crate::library::chave_do_jogo`].
///
/// Dos gráficos, o que é da janela — modo e tamanho das duas, e o caminho de apresentação —
/// continua global, mesmo estando no arquivo: ver [`ConfigDoJogo::sobre`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigDoJogo {
    pub audio: Audio,
    pub graphics: Graphics,
    pub velocidade: Velocidade,
    pub controls: crate::input::bindings::Controls,
}

impl ConfigDoJogo {
    /// As seções do jogo, copiadas de `settings`.
    pub fn das(settings: &Settings) -> Self {
        Self {
            audio: settings.audio.clone(),
            graphics: settings.graphics.clone(),
            velocidade: settings.velocidade.clone(),
            controls: settings.controls.clone(),
        }
    }

    /// A configuração que vale para o jogo: a `global`, com as seções do jogo por cima.
    pub fn sobre(&self, global: &Settings) -> Settings {
        let janela = &global.graphics;
        Settings {
            audio: self.audio.clone(),
            graphics: Graphics {
                janela: janela.janela,
                janela_do_jogo: janela.janela_do_jogo,
                tamanho_da_janela: janela.tamanho_da_janela,
                tamanho_da_janela_do_jogo: janela.tamanho_da_janela_do_jogo,
                gpu_present: janela.gpu_present,
                limite_antigo: None,
                ..self.graphics.clone()
            },
            velocidade: self.velocidade.clone(),
            controls: self.controls.clone(),
            ..global.clone()
        }
    }

    /// As do jogo de `chave`, se ele tem. Ilegível conta como não ter: o jogo abre com a global.
    pub fn carrega(chave: &str) -> Option<Self> {
        Self::carrega_de(&caminho_do_jogo(chave))
    }

    pub fn carrega_de(path: &Path) -> Option<Self> {
        let texto = std::fs::read_to_string(path).ok()?;
        let mut config: Self = serde_json::from_str(&texto).ok()?;
        config.controls.adopt();
        Some(config)
    }

    pub fn grava(&self, chave: &str) -> std::io::Result<()> {
        self.grava_em(&caminho_do_jogo(chave))
    }

    pub fn grava_em(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let texto = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, texto)
    }

    /// Desliga as configurações do jogo: ele volta à global.
    pub fn apaga(chave: &str) -> std::io::Result<()> {
        match std::fs::remove_file(caminho_do_jogo(chave)) {
            Err(erro) if erro.kind() == std::io::ErrorKind::NotFound => Ok(()),
            resultado => resultado,
        }
    }
}

/// Onde moram as configurações do jogo de `chave`. Ver [`ConfigDoJogo`].
pub fn caminho_do_jogo(chave: &str) -> PathBuf {
    config_dir().join("jogos").join(format!("{chave}.json"))
}

/// Reexportado pela UI para não quebrar preferências serializadas e chamadas desktop.
pub use crate::config::ZWheel;

impl Settings {
    /// Lê as preferências de `path`. Arquivo ausente ou ilegível devolve o padrão — abrir com
    /// as opções de fábrica é sempre melhor que não abrir.
    pub fn load_from(path: &Path) -> Self {
        let texto = std::fs::read_to_string(path).ok();
        let mut settings: Self = texto
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default();
        // Um arquivo de uma versão anterior não traz o que ela não conhecia, e o padrão de um
        // campo novo nem sempre é o vazio.
        settings.controls.adopt();
        settings.adota_o_limite_antigo();
        if let Some(texto) = &texto {
            settings.adota_os_atalhos_novos(texto, path);
        }
        settings
    }

    /// Um atalho novo chega sem tecla para quem já usa a tecla de fábrica dele num botão do Zeebo.
    ///
    /// O R de reiniciar e o F1 a F4 do save state não estão em nenhum botão do teclado de fábrica,
    /// mas quem remapeou pode tê-los posto num: o atalho, perguntado antes do jogo, roubaria o
    /// botão, e um toque nele reiniciaria a partida ou carregaria um estado. Só vale para o arquivo
    /// que ainda não conhecia o atalho — quem o escolheu depois escolheu. Uma combinação com
    /// modificador (o Shift+F2, o ⌘S) não colide com botão nenhum, e chega sempre.
    ///
    /// **As configurações de cada jogo contam também.** O atalho é um só, e um jogo com a tecla num
    /// botão perderia o botão do mesmo jeito. Elas moram em `jogos/`, ao lado de `path`.
    fn adota_os_atalhos_novos(&mut self, texto: &str, path: &Path) {
        let valor = serde_json::from_str::<serde_json::Value>(texto).ok();
        let atalhos = &mut self.atalhos;
        let novos: Vec<&mut String> = [
            ("reiniciar", &mut atalhos.reiniciar),
            ("salvar_estado", &mut atalhos.salvar_estado),
            ("carregar_estado", &mut atalhos.carregar_estado),
            ("proximo_slot", &mut atalhos.proximo_slot),
            ("slot_anterior", &mut atalhos.slot_anterior),
            ("desfazer_carregar", &mut atalhos.desfazer_carregar),
        ]
        .into_iter()
        .filter(|(campo, _)| {
            let conhecia = valor
                .as_ref()
                .is_some_and(|valor| valor.pointer(&format!("/atalhos/{campo}")).is_some());
            !conhecia
        })
        .map(|(_, atalho)| atalho)
        .filter(|atalho| !atalho.contains('+'))
        .collect();
        if novos.is_empty() {
            return;
        }
        let usa = |controles: &crate::input::bindings::Controls, tecla: &str| {
            controles.players.iter().any(|jogador| {
                jogador
                    .buttons
                    .values()
                    .flatten()
                    .any(|fonte| matches!(fonte, Source::Key { name } if name == tecla))
            })
        };
        let jogos: Vec<ConfigDoJogo> = path
            .parent()
            .map(|dir| dir.join("jogos"))
            .and_then(|pasta| std::fs::read_dir(pasta).ok())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entrada| ConfigDoJogo::carrega_de(&entrada.path()))
            .collect();
        for atalho in novos {
            let tecla = atalho.clone();
            if usa(&self.controls, &tecla) || jogos.iter().any(|jogo| usa(&jogo.controls, &tecla)) {
                atalho.clear();
            }
        }
    }

    /// O `speed_limit` desligado de um arquivo antigo vira o limite de quadros desligado.
    ///
    /// Ligado não muda nada: era o padrão, e o padrão do limite novo é o mesmo freio. O campo
    /// antigo some na leitura, e a próxima gravação já sai sem ele.
    fn adota_o_limite_antigo(&mut self) {
        if self.graphics.limite_antigo.take() == Some(false) {
            self.velocidade.limite_fps = LimiteFps::Desligado;
        }
    }

    pub fn load() -> Self {
        Self::load_from(&settings_path())
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&settings_path())
    }
}

/// O rótulo de um fator de resolução interna: o fator, o tamanho e a referência de mercado.
pub fn rotulo_da_resolucao(fator: u8) -> String {
    let (largura, altura) = (640 * u32::from(fator), 480 * u32::from(fator));
    let referencia = match fator {
        1 => "nativa",
        2 => "~720p",
        3 => "~1080p",
        4 => "~1440p",
        _ => "~4K",
    };
    format!("{fator}x · {largura}×{altura} · {referencia}")
}

/// O rótulo de uma escolha de pulo de desenho: "Desligado", "Automático", ou quantos quadros são
/// desenhados no fixo — "1 de 3" diz mais a quem joga que o `n` da conta.
pub fn rotulo_do_frameskip(frameskip: Frameskip, catalogo: &crate::ui::i18n::Catalog) -> String {
    match frameskip {
        Frameskip::Desligado => catalogo.get("common.off").to_string(),
        Frameskip::Automatico => catalogo.get("speed.frameskip.auto").to_string(),
        Frameskip::Fixo(n) => format!(
            "{} 1/{}",
            catalogo.get("speed.frameskip.fixed"),
            u32::from(n) + 1
        ),
    }
}

/// O rótulo de um nível de antialias ou de anisotrópico: 1 é desligado.
pub fn rotulo_de_nivel(nivel: u8, sigla: &str, desligado: &str) -> String {
    match nivel {
        0 | 1 => desligado.to_string(),
        n => format!("{n}x {sigla}"),
    }
}

pub fn settings_path() -> PathBuf {
    config_dir().join(FILE_NAME)
}

/// Onde procurar idiomas soltos: ao lado do executável e na pasta de configuração.
///
/// A primeira serve para uma cópia portátil do emulador; a segunda, para quem quer acrescentar
/// um idioma sem mexer na instalação.
pub fn language_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("lang"));
        }
    }
    dirs.push(config_dir().join("lang"));
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_e_volta_pelo_disco_preserva_tudo() {
        let dir = std::env::temp_dir().join("zeebx-testes-settings");
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);

        let settings = Settings {
            language: Some("pt-BR".into()),
            roms_dir: Some(PathBuf::from("/jogos/zeebo")),
            z_wheel_path: Some(PathBuf::from("/jogos/Z-Wheel.zip")),
            screenshots_dir: Some(PathBuf::from("/imagens/zeebo")),
            atalhos: Atalhos {
                screenshot: "F12".into(),
                avancar: vec![Source::key("F7"), Source::button("LeftThumb")],
                voltar: vec![Source::button("RightThumb")],
                reiniciar: "Home".into(),
                salvar_estado: "Ctrl+S".into(),
                carregar_estado: "F6".into(),
                proximo_slot: "Alt+Shift+CloseBracket".into(),
                slot_anterior: String::new(),
                desfazer_carregar: "Cmd+Z".into(),
            },
            biblioteca: ModoDaBiblioteca::Slider,
            movimento: Movimento {
                aviso_de_calibracao: false,
            },
            controles_na_tela: ControlesNaTela {
                modo: ModoDosControlesNaTela::Nunca,
                escala: 130,
                opacidade: 80,
                posicoes: [("botoes".to_string(), [0.75, 0.5])].into(),
                tamanhos: [("dpad".to_string(), 140)].into(),
                mostrar_avancar: false,
                mostrar_voltar: true,
                mostrar_turbo: false,
            },
            graphics: Graphics {
                scaling: Scaling::Fit,
                ..Graphics::default()
            },
            velocidade: Velocidade {
                limite_fps: LimiteFps::Trinta,
                frameskip: Frameskip::Fixo(2),
                avanco: Avanco {
                    proporcao: 0,
                    modo: crate::velocidade::ModoDoAtalho::Alternar,
                    sem_som: true,
                },
                turbo_por_segundo: 15,
                rewind: AjustesDoRewind {
                    ligado: true,
                    intervalo_ms: 2000,
                    memoria_mb: 512,
                },
            },
            debug: DebugView {
                overlay: true,
                ..DebugView::default()
            },
            audio: Audio {
                volume: 42,
                ..Audio::default()
            },
            controls: crate::input::bindings::Controls::default(),
            z_wheel: ZWheel {
                fim_de_vida: false,
                ..ZWheel::default()
            },
            discord: Discord {
                ativo: false,
                capas_url: "https://exemplo/{chave}.png".into(),
            },
            atualizacoes: Atualizacoes { ao_abrir: false, pre_lancamentos: false },
            aviso_dispensado_na_versao: Some("0.1.0".into()),
        };
        settings.save_to(&path).unwrap();

        assert_eq!(Settings::load_from(&path), settings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Quem tem um `settings.json` de antes do tamanho de janela ganha `None` nos dois
    /// campos novos, e o que foi guardado volta igual do disco.
    #[test]
    fn o_tamanho_da_janela_vai_e_volta_e_falta_nas_versoes_antigas() {
        let dir = std::env::temp_dir().join("zeebx-testes-janela");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let antigo: Settings =
            serde_json::from_str(r#"{ "graphics": { "janela": "maximizada" } }"#).unwrap();
        assert_eq!(antigo.graphics.tamanho_da_janela, None);
        assert_eq!(antigo.graphics.tamanho_da_janela_do_jogo, None);

        let mut novo = Settings::default();
        novo.graphics.tamanho_da_janela = Some([1280.0, 800.0]);
        novo.graphics.tamanho_da_janela_do_jogo = Some([640.0, 512.0]);
        novo.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), novo);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Quem tem um `settings.json` de antes do screenshot ganha o F9 e a pasta padrão, sem
    /// perder o resto.
    #[test]
    fn um_arquivo_de_antes_dos_atalhos_ganha_o_f9() {
        let lido: Settings = serde_json::from_str(r#"{ "roms_dir": "/jogos" }"#).unwrap();
        assert_eq!(lido.atalhos.screenshot, "F9");
        assert_eq!(lido.atalhos.avancar, [Source::key("F10")]);
        assert_eq!(lido.screenshots_dir, None);
        assert_eq!(lido.roms_dir, Some(PathBuf::from("/jogos")));
        // Um arquivo que já tinha o F9 trocado, de antes do fast-forward, ganha o F10 também.
        let trocado: Settings =
            serde_json::from_str(r#"{ "atalhos": { "screenshot": "F12" } }"#).unwrap();
        assert_eq!(trocado.atalhos.screenshot, "F12");
        assert_eq!(trocado.atalhos.avancar, [Source::key("F10")]);
    }

    /// O `speed_limit` desligado de antes do limite de quadros continua desligado, e o campo
    /// antigo não volta a ser gravado.
    #[test]
    fn o_speed_limit_antigo_vira_o_limite_de_quadros() {
        let dir = std::env::temp_dir().join("zeebx-testes-speed-limit");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        std::fs::write(&path, r#"{"graphics": {"speed_limit": false, "smooth": true}}"#).unwrap();
        let lido = Settings::load_from(&path);
        assert_eq!(lido.velocidade.limite_fps, LimiteFps::Desligado);
        assert!(lido.graphics.smooth);
        assert_eq!(lido.graphics.limite_antigo, None);
        lido.save_to(&path).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("speed_limit"));

        std::fs::write(&path, r#"{"graphics": {"speed_limit": true}}"#).unwrap();
        assert_eq!(Settings::load_from(&path).velocidade.limite_fps, LimiteFps::Sessenta);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// O R de reiniciar chega para quem não o usa, e não rouba o botão de quem já o mapeou. Quem
    /// já conhecia o atalho fica com o que escolheu.
    #[test]
    fn o_atalho_de_reiniciar_nao_rouba_um_botao() {
        let dir = std::env::temp_dir().join(format!("zeebx-testes-reiniciar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        std::fs::write(&path, r#"{ "roms_dir": "/jogos" }"#).unwrap();
        assert_eq!(Settings::load_from(&path).atalhos.reiniciar, "R");

        let mut com_r = Settings::default();
        com_r.controls.player_mut(0).bind("b4", Source::key("R"));
        let mut valor = serde_json::to_value(&com_r).unwrap();
        valor["atalhos"].as_object_mut().unwrap().remove("reiniciar");
        std::fs::write(&path, valor.to_string()).unwrap();
        assert_eq!(Settings::load_from(&path).atalhos.reiniciar, "", "o R do b4 virou reinício");

        // Gravado com o atalho, ele vale mesmo com o R num botão: foi escolha.
        com_r.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path).atalhos.reiniciar, "R");

        // O R num botão de um jogo só, com as gerais sem ele, também deixa o atalho sem tecla.
        let mut jogo = ConfigDoJogo::das(&Settings::default());
        jogo.controls.player_mut(0).bind("b4", Source::key("R"));
        jogo.grava_em(&dir.join("jogos").join("01234567.json")).unwrap();
        std::fs::write(&path, r#"{ "roms_dir": "/jogos" }"#).unwrap();
        assert_eq!(Settings::load_from(&path).atalhos.reiniciar, "", "o R do jogo virou reinício");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// O F1 do save state segue a regra do R: chega para quem não o usa, e não rouba o botão de
    /// quem o pôs num. A combinação com modificador chega sempre.
    #[test]
    fn os_atalhos_do_save_state_nao_roubam_um_botao() {
        let dir = std::env::temp_dir().join(format!("zeebx-testes-estado-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let mut com_f1 = Settings::default();
        com_f1.controls.player_mut(0).bind("b1", Source::key("F1"));
        com_f1.controls.player_mut(0).bind("b2", Source::key("F2"));
        let mut valor = serde_json::to_value(&com_f1).unwrap();
        for campo in [
            "salvar_estado",
            "carregar_estado",
            "proximo_slot",
            "slot_anterior",
            "desfazer_carregar",
        ] {
            valor["atalhos"].as_object_mut().unwrap().remove(campo);
        }
        std::fs::write(&path, valor.to_string()).unwrap();
        let lido = Settings::load_from(&path).atalhos;
        assert_eq!(lido.carregar_estado, Atalhos::CARREGAR_ESTADO_PADRAO);
        assert_eq!(lido.slot_anterior, Atalhos::SLOT_ANTERIOR_PADRAO);
        if cfg!(target_os = "macos") {
            assert_eq!(lido.salvar_estado, "Cmd+S");
        } else {
            assert_eq!(lido.salvar_estado, "", "o F1 do b1 virou save state");
            assert_eq!(lido.proximo_slot, "", "o F2 do b2 virou troca de slot");
        }

        // Gravado com o atalho, ele vale mesmo com a tecla num botão: foi escolha.
        com_f1.save_to(&path).unwrap();
        assert_eq!(
            Settings::load_from(&path).atalhos.salvar_estado,
            Atalhos::SALVAR_ESTADO_PADRAO
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_combinacao_vai_ao_texto_e_volta() {
        let shift_f2 = Combinacao::le("Shift+F2").unwrap();
        assert_eq!(shift_f2.tecla, egui::Key::F2);
        assert!(shift_f2.modificadores.shift && !shift_f2.modificadores.cmd);
        assert_eq!(shift_f2.nome(), "Shift+F2");

        let so_a_tecla = Combinacao::le("F9").unwrap();
        assert!(so_a_tecla.modificadores.nenhum());
        assert_eq!(so_a_tecla.nome(), "F9");

        // A ordem de quem escreveu à mão não importa; a gravada é sempre a mesma.
        assert_eq!(
            Combinacao::le("Cmd+Shift+]").unwrap().nome(),
            "Shift+Cmd+CloseBracket"
        );
        assert_eq!(Combinacao::le("Ctrl+Plus").unwrap().tecla, egui::Key::Plus);

        assert_eq!(Combinacao::le(""), None);
        assert_eq!(Combinacao::le("Hyper+F1"), None);
        assert_eq!(Combinacao::le("Shift+"), None);

        for padrao in [
            Atalhos::SALVAR_ESTADO_PADRAO,
            Atalhos::CARREGAR_ESTADO_PADRAO,
            Atalhos::PROXIMO_SLOT_PADRAO,
            Atalhos::SLOT_ANTERIOR_PADRAO,
            Atalhos::DESFAZER_CARREGAR_PADRAO,
        ] {
            assert_eq!(
                Combinacao::le(padrao).map(|c| c.nome()).as_deref(),
                Some(padrao)
            );
        }
    }

    #[test]
    fn arquivo_ausente_ou_estragado_devolve_o_padrao() {
        // Abrir com as opções de fábrica é sempre melhor que não abrir.
        let ausente = std::env::temp_dir().join("zeebx-nao-existe-mesmo/settings.json");
        assert_eq!(Settings::load_from(&ausente), Settings::default());

        let dir = std::env::temp_dir().join("zeebx-testes-estragado");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ isto não é json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn um_controle_salvo_antes_dos_eixos_ganha_os_eixos_ao_abrir() {
        // Este é o caminho que o emulador usa de verdade: um arquivo escrito por uma versão
        // que não conhecia os eixos deixaria os manches mudos, e o padrão de um campo novo nem
        // sempre é o vazio.
        let dir = std::env::temp_dir().join("zeebx-testes-eixos");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"controls": {"players": [{"device": "PS4 Controller", "buttons": {}}]}}"#,
        )
        .unwrap();

        let loaded = Settings::load_from(&path);
        let player = loaded.controls.player(0).unwrap();
        assert_eq!(player.axes, crate::input::bindings::Player::default_axes());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn campo_que_falta_no_arquivo_usa_o_padrao() {
        // Um arquivo escrito por uma versão anterior não tem os campos novos, e ele precisa
        // continuar valendo.
        let dir = std::env::temp_dir().join("zeebx-testes-parcial");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"language": "pt-BR"}"#).unwrap();

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.language.as_deref(), Some("pt-BR"));
        assert_eq!(loaded.graphics, Graphics::default());
        assert_eq!(loaded.audio, Audio::default());
        assert_eq!(loaded.controls, crate::input::bindings::Controls::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// O jogo personalizado leva as quatro seções dele, e o resto — inclusive a janela, que mora
    /// nos gráficos — continua sendo o da global.
    #[test]
    fn as_do_jogo_trocam_so_as_secoes_do_jogo() {
        let mut do_jogo = Settings::default();
        do_jogo.audio.volume = 20;
        do_jogo.graphics.resolucao_interna = 3;
        do_jogo.graphics.janela = ModoDaJanela::TelaCheia;
        do_jogo.velocidade.limite_fps = LimiteFps::Trinta;
        do_jogo.controls.players[0].ligada = false;
        let config = ConfigDoJogo::das(&do_jogo);

        let mut global = Settings::default();
        global.language = Some("es".into());
        global.graphics.tamanho_da_janela = Some([800.0, 600.0]);
        let vale = config.sobre(&global);

        assert_eq!(vale.audio.volume, 20);
        assert_eq!(vale.graphics.resolucao_interna, 3);
        assert_eq!(vale.velocidade.limite_fps, LimiteFps::Trinta);
        assert!(!vale.controls.players[0].ligada);
        assert_eq!(vale.language.as_deref(), Some("es"), "o idioma é da global");
        assert_eq!(vale.graphics.janela, global.graphics.janela, "o modo da janela é da global");
        assert_eq!(vale.graphics.tamanho_da_janela, Some([800.0, 600.0]));
        // E as do jogo voltam inteiras de uma configuração já montada.
        assert_eq!(ConfigDoJogo::das(&vale).audio, config.audio);
    }

    /// O arquivo do jogo vai e volta do disco; sem arquivo, ou ilegível, o jogo não tem as dele.
    #[test]
    fn as_do_jogo_vao_e_voltam_e_sem_arquivo_nao_ha() {
        let dir = std::env::temp_dir().join("zeebx-testes-config-do-jogo");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("jogos").join("01234567.json");
        assert_eq!(ConfigDoJogo::carrega_de(&path), None);

        let mut config = ConfigDoJogo::default();
        config.audio.volume = 55;
        config.velocidade.frameskip = Frameskip::Fixo(1);
        config.grava_em(&path).unwrap();
        assert_eq!(ConfigDoJogo::carrega_de(&path), Some(config));

        std::fs::write(&path, "{ isto não é json").unwrap();
        assert_eq!(ConfigDoJogo::carrega_de(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
