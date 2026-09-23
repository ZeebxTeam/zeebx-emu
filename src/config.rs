//! Configuração neutra do motor.
//!
//! Estes tipos não podem depender de Eframe nem da pasta de preferências desktop, pois também
//! descrevem comportamento observado pelo guest e serão recebidos por frontends como Libretro.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Raiz de configuração do frontend desktop.
///
/// Libretro não usa este caminho: fornece [`crate::storage::StoragePaths`] explicitamente. Ele
/// fica aqui apenas para os wrappers legados não puxarem `ui::settings` para dentro do motor.
pub fn config_dir() -> PathBuf {
    const APP_DIR: &str = "Zeebx";
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

/// Como a Z-Wheel se comporta dentro do guest.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZWheel {
    /// A roda inferior de fim de vida ("Jogar" e "Ajuda").
    pub fim_de_vida: bool,
    /// A transição deslizante em toda troca de tela.
    pub transicoes_sempre: bool,
}

impl Default for ZWheel {
    fn default() -> Self {
        Self {
            fim_de_vida: true,
            transicoes_sempre: true,
        }
    }
}
