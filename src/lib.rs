//! Zeebx — emulador de Zeebo / Qualcomm BREW.

mod app;
mod audio;
mod brew;
mod cpu;
mod input;
mod loader;
mod machine;
mod ponte;
mod rede;
mod session;
mod ui;
mod video;

/// Varredura de ROMs por teste — ver [`varredura`]. Só existe em compilação de teste.
#[cfg(test)]
mod varredura;

/// O núcleo Libretro: o RetroArch (e frontends compatíveis) carregam o `cdylib`.
mod libretro;

/// Abre a interface ou atende a linha de comando.
pub fn run() -> std::process::ExitCode {
    app::cli()
}
