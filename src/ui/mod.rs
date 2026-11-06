//! What the interface keeps, split in two: what needs a host window and what does not.
//!
//! The library, the settings, the saves and the language catalog are read **by the core** — the
//! `machine`, the `loader` and the [`crate::session`] talk to them —, so they exist on every
//! platform. The GL painter and the UI-less window only exist where there is a desktop window:
//! the interface itself belongs to each frontend (Qt on the desktop, another one on Android and
//! iOS), on top of the same core.

/// The project repository.
///
/// It lives here, and not in each frontend, because it is the same project seen from different
/// screens. The first version of the Android "about" screen carried its own copy of these two
/// addresses, and the Discord one was simply wrong — a made-up invite that led nowhere. A
/// duplicated constant does not warn you when the copies diverge; a single constant does.
pub const REPOSITORY: &str = "https://github.com/ZeebxTeam/zeebx-emu";

/// The invite to the chat server.
pub const DISCORD: &str = "https://discord.gg/D96HjsKTPa";

pub mod zwheel_metadata;
// No gate: it is only the rule for when the notice opens and goes away; drawing it is up to each
// window.
pub mod calibration;
// No gate, like the siblings below: the panel only talks to the `session`, the `i18n` and the
// `settings`, and it is the same one the Android frontend draws. There is nothing desktop here.
pub mod debug_panel;
// No gate: the slots only talk to the `session` and to the disk, and Android and Qt save the same.
pub mod save_slots;
// **The painter belongs to the `gl` feature, not to `desktop`.** It is the engine's OpenGL backend:
// the Android frontend draws with it and has no eframe at all. `desktop` still brings it in,
// because it implies `gpu`, which implies `gl`.
#[cfg(feature = "gl")]
pub mod gpu;
pub mod i18n;
pub mod library;
// No gate: it is what turns keyboard and controller into library commands, in egui and in Qt.
pub mod navigation;
// No gate: the running game only talks to the `session` and the `settings`, and it is what the
// egui and Qt windows use to open and run a game the same way.
pub mod running_game;
pub mod saves;
// No gate: the name, the directory and the `.png` of a screenshot need no window, and Android can
// get the button without rewriting anything.
pub mod screenshot;
pub mod settings;
// No gate: a theme is a JSON of colors, and applying them is up to each frontend's window.
pub mod themes;

#[cfg(feature = "desktop")]
pub mod updater;
#[cfg(feature = "desktop")]
pub mod discord;
// Desktop because it is `gilrs`, which only the desktop brings; but no egui window: Qt uses it the
// same way.
#[cfg(feature = "desktop")]
pub mod host_input;
#[cfg(feature = "desktop")]
pub mod window;

