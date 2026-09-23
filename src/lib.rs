//! Núcleo reutilizável do Zeebx.
//!
//! O motor é compartilhado pelo desktop, Libretro, headless e Android.

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
pub mod save_state;
pub mod session;
pub mod storage;
pub mod ui;
pub mod video;

/// O `eframe` que o núcleo usa, reexportado para quem monta a janela.
///
/// **Tem de ser este, e não outro.** O `ui::App` daqui implementa `eframe::App`, e um frontend
/// que declarasse o `eframe` por conta própria poderia resolver para outra versão: aí o
/// `Box<dyn eframe::App>` dele seria de um `eframe` e o `App` seria de outro, e o erro sai no
/// ligador, não no compilador. Reexportar é o que garante um `eframe` só na árvore.
#[cfg(feature = "desktop")]
pub use eframe;

/// Configuração histórica da linha de comando: um Dragon na primeira porta e segunda livre.
pub const PORTAS_PADRAO: [Option<input::bindings::Aparelho>; input::PORTAS] =
    [Some(input::bindings::Aparelho::Controle), None];

#[cfg(test)]
pub mod scratch;

#[cfg(test)]
pub mod varredura;
