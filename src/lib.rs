//! Zeebx — emulador de Zeebo / Qualcomm BREW.

#[cfg(feature = "desktop")]
mod app;
pub mod audio;
pub mod brew;
pub mod cpu;
pub mod input;
pub mod loader;
pub mod machine;
pub mod ponte;
pub mod rede;
pub mod session;
pub mod ui;
pub mod video;

/// Varredura de ROMs por teste — ver [`varredura`]. Só existe em compilação de teste.
#[cfg(all(test, feature = "desktop"))]
mod varredura;

/// Abre a interface ou atende a linha de comando.
#[cfg(feature = "desktop")]
pub fn run() -> std::process::ExitCode {
    app::cli()
}
