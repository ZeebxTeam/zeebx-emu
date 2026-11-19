//! Zeebx Qt — the Zeebo emulator with the Qt Quick interface.
//!
//! The emulator's interface: `zeebx-qt` opens the library, and `zeebx-qt <game>` opens the game
//! directly. The debugging command line (`run`, `info`, `bench`...) is `zeebx`, in `frontends/cli`.

// On Windows, the release build opens without the console window behind the interface.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod qt;

use std::process::ExitCode;

/// The name Wayland uses to match the window with the installed `.desktop`, where it gets the icon
/// from. It is the binary's name, which is the name `cargo packager` gives the `.desktop`.
const APP_ID: &str = "zeebx-qt";

fn main() -> ExitCode {
    zeebx::logging::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // Exits before creating the `QGuiApplication`, with no screen: CI runs this to prove the
        // binary finds the Qt libraries, which the system loads before `main`.
        Some("--version") => {
            println!("zeebx-qt {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        game => qt::launch(game),
    }
}
