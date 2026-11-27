//! Zeebx HTPC — dedicated 10-foot gamepad-driven frontend for TVs and Steam Big Picture.
//!
//! Shares the same configuration, ROM library, saves, save states and color palettes as `zeebx-qt`
//! while running in a single fullscreen window with pluggable QML themes (`htpc-themes/`),
//! per-theme color configuration and a gamepad-only Quick Menu and Settings UI.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod htpc_bridge;
mod htpc_input;

use std::process::ExitCode;

const APP_ID: &str = "zeebx-htpc";
const MAIN_QML: &str = "qrc:/qt/qml/zeebx/htpc/qml/HtpcMain.qml";

fn main() -> ExitCode {
    zeebx::logging::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("zeebx-htpc {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        game => zeebx_qt_core::launch(APP_ID, MAIN_QML, game, false),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use zeebx::ui::i18n::Catalog;

    fn collect_qml(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_qml(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("qml") {
                out.push(path);
            }
        }
    }

    #[test]
    fn every_htpc_qml_translation_key_exists() {
        let catalog = Catalog::default();
        let qml_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("qml");
        let mut files = Vec::new();
        collect_qml(&qml_root, &mut files);
        assert!(!files.is_empty(), "expected HTPC QML files");

        for file in files {
            let content = std::fs::read_to_string(&file).unwrap();
            let mut rest = content.as_str();
            while let Some(pos) = rest.find("tr(\"") {
                rest = &rest[pos + 4..];
                let Some(end) = rest.find('"') else {
                    break;
                };
                let key = &rest[..end];
                assert_ne!(
                    catalog.get(key),
                    key,
                    "missing translation key {key:?} in {}",
                    file.display()
                );
                rest = &rest[end + 1..];
            }
        }
    }

    #[test]
    fn example_community_theme_is_valid() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("minimal-grid");
        let theme = zeebx::ui::htpc_themes::read_theme_dir(&dir)
            .expect("example community theme must parse cleanly");
        assert_eq!(theme.id, "minimal-grid");
        assert!(!theme.builtin);
        assert!(theme.palettes.iter().any(|p| p.key_id == "midnight_teal"));
    }
}
