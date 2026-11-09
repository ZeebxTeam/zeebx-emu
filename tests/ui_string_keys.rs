//! Every text key the interface uses exists in the catalog.
//!
//! A missing key breaks nothing: the `Catalog` returns it as itself, and the screen shows
//! `saves.confirm.yes` instead of "Delete". The test in `i18n.rs` checks that the languages have the
//! same keys as each other; this one checks the other side, that the code only asks for keys that
//! exist.
//!
//! In QML the keys reach `tr()` by many paths — directly, in a `label:`, in a `model` list, in a
//! ternary —, so every literal that looks like a key counts, except the settings ones (`keyId:`,
//! `v()`, `define()`, `options()`), which also have dots. In Rust, the first argument of calls to
//! the catalog counts, only where there is interface code: `src/qt/`, `src/ui/` and the Android
//! frontend. Outside of those, a `.get("a.b")` is something else, and `i18n.rs` asks for a key that
//! doesn't exist on purpose.
//!
//! It lives in the root package, not in the standalone's, which is where the QML is: with the Qt
//! interface enabled, cxx-qt links the QML module into every target of that package, and an
//! integration test doesn't have the Rust bridges, which belong to the binary — it didn't link.

use std::path::{Path, PathBuf};

use zeebx::ui::i18n::Catalog;

/// `settings.discord.on`: lowercase, digits and `_`, with at least one dot. Not a file name.
fn looks_like_key(text: &str) -> bool {
    const FILE_EXTENSIONS: [&str; 7] = [".h", ".png", ".log", ".qml", ".json", ".rs", ".svg"];
    text.contains('.')
        && !text.starts_with('.')
        && !text.ends_with('.')
        && !text.contains("..")
        && text.starts_with(|c: char| c.is_ascii_lowercase())
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
        && !FILE_EXTENSIONS.iter().any(|ext| text.ends_with(ext))
}

/// The quoted literals of a line.
fn literals(line: &str) -> Vec<&str> {
    line.split('"').skip(1).step_by(2).collect()
}

fn files(dir: &Path, extension: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, extension, found);
        } else if path.extension().is_some_and(|e| e == extension) {
            found.push(path);
        }
    }
}

/// The keys requested by the QML, with the file and line.
fn qml_keys(dir: &Path) -> Vec<(String, String)> {
    const SETTINGS_MARKERS: [&str; 5] = ["keyId:", "v(", "define(", "options(", "value("];
    let mut qml = Vec::new();
    files(dir, "qml", &mut qml);
    let mut key_ids = Vec::new();
    for file in qml {
        let text = std::fs::read_to_string(&file).unwrap();
        for (n, line) in text.lines().enumerate() {
            if SETTINGS_MARKERS.iter().any(|c| line.contains(c)) {
                continue;
            }
            for literal in literals(line).into_iter().filter(|l| looks_like_key(l)) {
                key_ids.push((literal.to_string(), format!("{}:{}", file.display(), n + 1)));
            }
        }
    }
    key_ids
}

/// The keys the Rust code requests from the catalog, with the file and line.
fn rust_keys(dir: &Path) -> Vec<(String, String)> {
    const CALLS: [&str; 4] = [".get(\"", ".format(\"", ".tr(\"", "texto(\""];
    let mut rust = Vec::new();
    files(dir, "rs", &mut rust);
    let mut key_ids = Vec::new();
    for file in rust {
        if file.ends_with("i18n.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for (n, line) in text.lines().enumerate() {
            for call in CALLS {
                for (i, _) in line.match_indices(call) {
                    let rest = &line[i + call.len()..];
                    let Some(end) = rest.find('"') else { continue };
                    if looks_like_key(&rest[..end]) {
                        key_ids.push((
                            rest[..end].to_string(),
                            format!("{}:{}", file.display(), n + 1),
                        ));
                    }
                }
            }
        }
    }
    key_ids
}

#[test]
fn every_key_the_ui_uses_exists_in_the_catalog() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut key_ids = qml_keys(&root.join("frontends/qt-standalone/qml"));
    key_ids.extend(rust_keys(&root.join("frontends/qt-standalone/src")));
    key_ids.extend(rust_keys(&root.join("src/ui")));
    key_ids.extend(rust_keys(&root.join("frontends/android/src")));
    // With nothing found, the test would pass by not looking: the folder moved, or the extraction
    // broke.
    assert!(key_ids.len() > 100, "only {} keys found", key_ids.len());

    let catalog = Catalog::default();
    let missing: Vec<String> = key_ids
        .iter()
        .filter(|(key_id, _)| catalog.get(key_id) == key_id)
        .map(|(key_id, location)| format!("{location}: {key_id}"))
        .collect();
    assert!(missing.is_empty(), "keys missing from the catalog:\n{}", missing.join("\n"));
}

#[test]
fn extraction_recognizes_keys_and_ignores_the_rest() {
    assert!(looks_like_key("saves.confirm.yes"));
    assert!(looks_like_key("settings.z_wheel_eol"));
    assert!(!looks_like_key("zeebx.log"));
    assert!(!looks_like_key("quadro.h"));
    assert!(!looks_like_key("Zeebx"));
    assert!(!looks_like_key("qrc:/zeebx/boomerang.png"));
    assert_eq!(literals(r#"tr(a ? "x.y" : "z.w")"#), ["x.y", "z.w"]);
}
