//! Núcleo reutilizável do Zeebx.
//!
//! O motor é compartilhado pelo desktop, Libretro, headless, Android e iOS.

pub mod audio;
pub mod brew;
pub mod config;
pub mod cpu;
pub mod input;
pub mod loader;
pub mod library;
pub mod machine;
pub mod ponte;
pub mod rede;
pub mod registro;
pub mod save_state;
pub mod session;
pub mod storage;
pub mod ui;
pub mod velocidade;
pub mod video;

/// O `egui` que o núcleo usa, reexportado para os frontends.
///
/// O núcleo não abre janela com ele: o que fica são os tipos que a interface compartilha — os
/// nomes de tecla do `egui::Key`, que são o formato gravado no `settings.json`, e o desenho do
/// [`ui::gpu::Pintor`], que o Android e o headless usam. Reexportar garante um `egui` só na árvore:
/// a ponte Qt compara teclas com o mesmo `egui::Key` que leu a configuração.
pub use egui;

/// Configuração histórica da linha de comando: um Dragon na primeira porta e segunda livre.
pub const PORTAS_PADRAO: [Option<input::bindings::Aparelho>; input::PORTAS] =
    [Some(input::bindings::Aparelho::Controle), None];

#[cfg(test)]
pub mod scratch;

#[cfg(test)]
pub mod varredura;
