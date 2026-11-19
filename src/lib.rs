//! Reusable core of Zeebx.
//!
//! The engine is shared by the desktop, Libretro, headless, Android, iOS and the Vita app.

pub mod audio;
pub mod config;
pub mod cpu;
pub mod input;
pub mod loader;
pub mod library;
pub mod machine;
pub mod bridge;
pub mod net;
pub mod logging;
pub mod session;
pub mod storage;
pub mod ui;
pub mod speed;
pub mod video;

// A plataforma BREW independente de host (heap, VFS, vtables, formato do save state) vem do
// crate `zeebx-core`. Reexportada para que `crate::brew` e `crate::save_state` sigam valendo.
pub use zeebx_core::{brew, save_state};

/// The `egui` the core uses, re-exported for the frontends.
///
/// The core does not open a window with it: what remains are the types the interface shares — the
/// key names of `egui::Key`, which are the format stored in `settings.json`, and the drawing of
/// [`ui::gpu::GlPainter`], which Android and headless use. Re-exporting guarantees a single `egui`
/// in the tree: the Qt bridge compares keys with the same `egui::Key` that read the configuration.
pub use egui;

/// Historical command-line configuration: a Dragon on the first port and the second one free.
pub const DEFAULT_PORTS: [Option<input::bindings::Device>; input::PORTS] =
    [Some(input::bindings::Device::Controller), None];

#[cfg(test)]
pub mod scratch;

#[cfg(test)]
pub mod scan;
