//! Frontend-neutral engine configuration.
//!
//! These types cannot depend on Eframe or on the desktop preferences folder, because they also
//! describe behavior observed by the guest and will be received by frontends such as Libretro.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Environment variables that had Portuguese names before the codebase moved to English. The
/// old spelling is still read, so scripts and shell profiles written for earlier versions keep
/// working: `(current name, legacy name)`.
const LEGACY_ENV: &[(&str, &str)] = &[
    ("ZEEBX_CORE_FRAMES", "ZEEBX_CORE_QUADROS"),
    ("ZEEBX_CORE_KEYS", "ZEEBX_CORE_TECLAS"),
    ("ZEEBX_CORE_SELECTORS", "ZEEBX_CORE_SELETORES"),
    ("ZEEBX_CORE_SYSTEM", "ZEEBX_CORE_SISTEMA"),
    ("ZEEBX_NO_BRIDGE", "ZEEBX_SEM_PONTE"),
    ("ZEEBX_ROM_CAP", "ZEEBX_ROM_TETO"),
    ("ZEEBX_ROM_DOWNSCALE", "ZEEBX_ROM_REDUCAO"),
    ("ZEEBX_ROM_DUMP", "ZEEBX_ROM_DESPEJO"),
    ("ZEEBX_ROM_INSTALLED", "ZEEBX_ROM_INSTALADOS"),
    ("ZEEBX_ROM_KEYS", "ZEEBX_ROM_TECLAS"),
    ("ZEEBX_ROM_OUT", "ZEEBX_ROM_SAIDA"),
    ("ZEEBX_ROM_PROBE", "ZEEBX_ROM_SONDA"),
    ("ZEEBX_ROM_PROFILE", "ZEEBX_ROM_PERFIL"),
    ("ZEEBX_ROM_SELECTORS", "ZEEBX_ROM_SELETORES"),
    ("ZEEBX_ROM_TRACE", "ZEEBX_ROM_TRACO"),
    ("ZEEBX_SCOREBOARD_CAP", "ZEEBX_PLACAR_TETO"),
    ("ZEEBX_SCOREBOARD_MS", "ZEEBX_PLACAR_MS"),
    ("ZEEBX_SCOREBOARD_OUT", "ZEEBX_PLACAR_SAIDA"),
    ("ZEEBX_SERVER", "ZEEBX_SERVIDOR"),
    ("ZEEBX_TEST_DEVICE", "ZEEBX_TESTE_APARELHO"),
    ("ZEEBX_TEST_MANIFEST", "ZEEBX_TESTE_MANIFESTO"),
    ("ZEEBX_TEST_MS", "ZEEBX_TESTE_MS"),
    ("ZEEBX_TEST_ROM", "ZEEBX_TESTE_ROM"),
    ("ZEEBX_TEST_WAIT", "ZEEBX_TESTE_ESPERA"),
];

/// `std::env::var`, falling back to the variable's legacy Portuguese name (see [`LEGACY_ENV`]).
pub fn env_var(name: &str) -> Result<String, std::env::VarError> {
    std::env::var(name).or_else(|err| {
        match LEGACY_ENV.iter().find(|(current, _)| *current == name) {
            Some((_, legacy)) => std::env::var(legacy),
            None => Err(err),
        }
    })
}

/// `std::env::var_os`, falling back to the variable's legacy Portuguese name.
pub fn env_var_os(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).or_else(|| {
        LEGACY_ENV
            .iter()
            .find(|(current, _)| *current == name)
            .and_then(|(_, legacy)| std::env::var_os(legacy))
    })
}

/// Configuration root of the desktop frontend.
///
/// Libretro does not use this path: it provides [`crate::storage::StoragePaths`] explicitly. It
/// lives here only so the legacy wrappers don't pull `ui::settings` into the engine.
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

/// How the Z-Wheel behaves inside the guest.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZWheel {
    /// The lower end-of-life wheel ("Jogar" and "Ajuda").
    #[serde(rename = "fim_de_vida")]
    pub end_of_life: bool,
    /// The sliding transition on every screen change.
    #[serde(rename = "transicoes_sempre")]
    pub always_transitions: bool,
}

impl Default for ZWheel {
    fn default() -> Self {
        Self {
            end_of_life: true,
            always_transitions: true,
        }
    }
}
