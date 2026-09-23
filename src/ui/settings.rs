//! As preferências do usuário, guardadas em disco.
//!
//! Tudo aqui é opcional e tem um padrão razoável: um arquivo faltando, truncado ou de uma
//! versão mais nova do emulador precisa deixar o programa abrir, não impedi-lo. Por isso cada
//! campo é `#[serde(default)]` e a leitura nunca falha — no pior caso, volta o padrão.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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

    /// Aplica o modo a uma janela que ainda vai abrir.
    pub fn no_construtor(self, janela: egui::ViewportBuilder) -> egui::ViewportBuilder {
        match self {
            Self::Janela => janela,
            Self::Maximizada => janela.with_maximized(true),
            Self::TelaCheia => janela.with_fullscreen(true),
        }
    }

    /// Os comandos que levam uma janela aberta a este modo.
    pub fn comandos(self) -> [egui::ViewportCommand; 2] {
        [
            egui::ViewportCommand::Fullscreen(self == Self::TelaCheia),
            egui::ViewportCommand::Maximized(self == Self::Maximizada),
        ]
    }
}

/// O que o painel de depuração mostra durante a execução.
///
/// Tudo desligado por padrão: é ferramenta de quem está caçando um problema, e informação
/// sobre o quadro atrapalha quem só quer jogar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    pub scaling: Scaling,
    /// Interpolar ao ampliar. Desligado, o pixel do console aparece como bloco.
    pub smooth: bool,
    /// Manter os 4:3 da tela do Zeebo.
    pub keep_aspect: bool,
    /// Segurar o emulador no ritmo do console. Desligado, ele corre o quanto o host aguenta e
    /// os jogos ficam acelerados.
    pub speed_limit: bool,
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
            scaling: Scaling::default(),
            smooth: false,
            keep_aspect: true,
            speed_limit: true,
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
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            enabled: true,
            volume: 80,
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
    /// Como a biblioteca mostra os jogos.
    pub biblioteca: ModoDaBiblioteca,
    pub movimento: Movimento,
    pub graphics: Graphics,
    pub debug: DebugView,
    pub audio: Audio,
    pub controls: crate::input::bindings::Controls,
    pub z_wheel: ZWheel,
    pub discord: Discord,
    pub atualizacoes: Atualizacoes,
    /// A versão em que o aviso de abertura foi dispensado de vez. Outra versão mostra de novo.
    pub aviso_dispensado_na_versao: Option<String>,
}

/// A procura por versões novas. Ver [`crate::ui::atualizacao`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Atualizacoes {
    /// Perguntar ao GitHub na abertura.
    pub ao_abrir: bool,
}

impl Default for Atualizacoes {
    fn default() -> Self {
        Self { ao_abrir: true }
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

/// Reexportado pela UI para não quebrar preferências serializadas e chamadas desktop.
pub use crate::config::ZWheel;

impl Settings {
    /// Lê as preferências de `path`. Arquivo ausente ou ilegível devolve o padrão — abrir com
    /// as opções de fábrica é sempre melhor que não abrir.
    pub fn load_from(path: &Path) -> Self {
        let mut settings: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        // Um arquivo de uma versão anterior não traz o que ela não conhecia, e o padrão de um
        // campo novo nem sempre é o vazio.
        settings.controls.adopt();
        settings
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
            biblioteca: ModoDaBiblioteca::Slider,
            movimento: Movimento {
                aviso_de_calibracao: false,
            },
            graphics: Graphics {
                scaling: Scaling::Fit,
                ..Graphics::default()
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
            atualizacoes: Atualizacoes { ao_abrir: false },
            aviso_dispensado_na_versao: Some("0.1.0".into()),
        };
        settings.save_to(&path).unwrap();

        assert_eq!(Settings::load_from(&path), settings);
        let _ = std::fs::remove_dir_all(&dir);
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
}
