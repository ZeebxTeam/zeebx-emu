//! As preferências do usuário, guardadas em disco.
//!
//! Tudo aqui é opcional e tem um padrão razoável: um arquivo faltando, truncado ou de uma
//! versão mais nova do emulador precisa deixar o programa abrir, não impedi-lo. Por isso cada
//! campo é `#[serde(default)]` e a leitura nunca falha — no pior caso, volta o padrão.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Nome da pasta do emulador dentro do diretório de configuração do sistema.
const APP_DIR: &str = "Zeebx";
const FILE_NAME: &str = "settings.json";

/// Como a imagem do console preenche a janela.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scaling {
    /// Só múltiplos inteiros do tamanho original. Nunca borra, mas sobra borda.
    #[default]
    Integer,
    /// O maior tamanho que cabe na janela, mantendo a proporção.
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
    pub scaling: Scaling,
    /// Interpolar ao ampliar. Desligado, o pixel do console aparece como bloco.
    pub smooth: bool,
    /// Manter os 4:3 da tela do Zeebo.
    pub keep_aspect: bool,
    /// Segurar o emulador no ritmo do console. Desligado, ele corre o quanto o host aguenta e
    /// os jogos ficam acelerados.
    pub speed_limit: bool,
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            scaling: Scaling::default(),
            smooth: false,
            keep_aspect: true,
            speed_limit: true,
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
    pub graphics: Graphics,
    pub debug: DebugView,
    pub audio: Audio,
    pub controls: crate::input::bindings::Controls,
}

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

/// A pasta onde o sistema espera que um programa guarde configuração.
///
/// Escrito à mão em vez de vir de uma dependência porque são três regras conhecidas e cada uma
/// cabe numa linha. Sem nenhuma das variáveis, o diretório corrente serve.
pub fn config_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "windows") {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join(APP_DIR);
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = &home {
            return home.join("Library/Application Support").join(APP_DIR);
        }
    } else {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join(APP_DIR.to_lowercase());
        }
        if let Some(home) = &home {
            return home.join(".config").join(APP_DIR.to_lowercase());
        }
    }
    PathBuf::from(".")
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
