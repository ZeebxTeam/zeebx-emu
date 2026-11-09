//! The interface themes: color palettes on top of the Fusion style.
//!
//! A theme does not change how the controls are drawn, only the colors, like PCSX2's. Changing the
//! style (Material, Universal) would require restarting the program, and those styles do not read
//! the palette that the library grid and slider use. See `docs/implementation/26-themes.md`.
//!
//! Each theme is a JSON with the `QPalette` roles. The factory ones are embedded in the binary,
//! like the languages; any other comes in as a file in a `temas/` folder, and a file with the same
//! name as a factory one takes its place. **A bad file is ignored**, with the reason in the log:
//! the program opens with the others, and whoever had chosen that one falls back to the system
//! palette. The contrast check lives in `tools/check_themes.py`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The `QPalette` roles a theme defines, with the name QML uses (`palette.windowText`).
/// All are required: a missing role would be the system's, and a light theme text on top of a
/// light system background is unreadable.
pub const ROLES: [&str; 20] = [
    "window",
    "windowText",
    "base",
    "alternateBase",
    "text",
    "placeholderText",
    "button",
    "buttonText",
    "brightText",
    "light",
    "midlight",
    "mid",
    "dark",
    "shadow",
    "highlight",
    "highlightedText",
    "link",
    "linkVisited",
    "toolTipBase",
    "toolTipText",
];

/// The roles the theme also gives to disabled controls.
pub const DISABLED: [&str; 3] = ["windowText", "text", "buttonText"];

/// The format version this reader understands.
const FORMAT: u32 = 1;

/// The themes that come in the binary, as `(key_id, JSON contents)`, in selector order.
const FACTORY_THEMES: [(&str, &str); 7] = [
    ("claro", include_str!("../../assets/themes/claro.json")),
    ("escuro", include_str!("../../assets/themes/escuro.json")),
    ("amoled", include_str!("../../assets/themes/amoled.json")),
    ("laranja", include_str!("../../assets/themes/laranja.json")),
    ("azul", include_str!("../../assets/themes/azul.json")),
    ("rubi", include_str!("../../assets/themes/rubi.json")),
    (
        "esmeralda",
        include_str!("../../assets/themes/esmeralda.json"),
    ),
];

/// A `#rrggbb` color, already parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    fn read(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#').filter(|h| h.len() == 6)?;
        u32::from_str_radix(hex, 16).ok().map(Self)
    }

    /// `#rrggbb`, as QML accepts in a color.
    pub fn hex(self) -> String {
        format!("#{:06x}", self.0)
    }
}

/// A theme read and checked.
#[derive(Debug, Clone)]
pub struct Theme {
    /// The file name without the `.json`. It is what is stored in `settings.json`.
    pub key_id: String,
    names: BTreeMap<String, String>,
    pub dark: bool,
    /// The [`ROLES`], in the same order.
    pub colors: Vec<(&'static str, Color)>,
    /// The [`DISABLED`], in the same order.
    pub disabled: Vec<(&'static str, Color)>,
    /// The warning text (a refused key, a missing folder) and the error text (the game that did
    /// not open), which the interface draws on top of the window.
    pub warning: Color,
    pub error: Color,
}

#[derive(Deserialize)]
struct File {
    #[serde(rename = "formato")]
    format: u32,
    #[serde(rename = "nome")]
    name: BTreeMap<String, String>,
    #[serde(rename = "escuro")]
    dark: bool,
    #[serde(rename = "cores")]
    colors: BTreeMap<String, String>,
    #[serde(rename = "desabilitado")]
    disabled: BTreeMap<String, String>,
    #[serde(rename = "aviso")]
    warning: String,
    #[serde(rename = "erro")]
    error: String,
}

impl Theme {
    /// Reads a theme. The `Err` says what was wrong, to go to the log.
    pub fn read(key_id: &str, json: &str) -> Result<Self, String> {
        let file: File = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if file.format != FORMAT {
            return Err(format!(
                "format {}, and this reader understands {FORMAT}",
                file.format
            ));
        }
        if file.name.is_empty() {
            return Err("no name".to_string());
        }
        let color = |group: &str, values: &BTreeMap<String, String>, role: &'static str| {
            let text = values
                .get(role)
                .ok_or_else(|| format!("{group}.{role} missing"))?;
            Color::read(text)
                .map(|color| (role, color))
                .ok_or_else(|| format!("{group}.{role}: {text:?} is not #rrggbb"))
        };
        let colors = ROLES
            .into_iter()
            .map(|role| color("cores", &file.colors, role))
            .collect::<Result<_, _>>()?;
        let disabled = DISABLED
            .into_iter()
            .map(|role| color("desabilitado", &file.disabled, role))
            .collect::<Result<_, _>>()?;
        let release = |field: &str, text: &str| {
            Color::read(text).ok_or_else(|| format!("{field}: {text:?} is not #rrggbb"))
        };
        Ok(Self {
            key_id: key_id.to_string(),
            names: file.name,
            dark: file.dark,
            colors,
            disabled,
            warning: release("aviso", &file.warning)?,
            error: release("erro", &file.error)?,
        })
    }

    /// The name in language `code`; without it, the catalog's fallback language's; without that,
    /// the first one.
    pub fn name(&self, code: &str) -> &str {
        self.names
            .get(code)
            .or_else(|| self.names.get(crate::ui::i18n::FALLBACK))
            .or_else(|| self.names.values().next())
            .map(String::as_str)
            .unwrap_or(&self.key_id)
    }
}

/// Where to look for themes besides the factory ones: next to the executable and in the
/// configuration folder.
///
/// The first serves a portable copy of the emulator; the second, whoever wants to add a theme
/// without touching the installation. The same rule as [`crate::ui::settings::language_dirs`].
pub fn dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        dirs.push(dir.join("temas"));
    }
    dirs.push(crate::config::config_dir().join("temas"));
    dirs
}

/// The factory ones, in selector order, and then those from `dirs`, by key. A file with the key
/// of a factory one takes its place.
pub fn load(dirs: &[PathBuf]) -> Vec<Theme> {
    let mut themes: Vec<Theme> = FACTORY_THEMES
        .iter()
        .filter_map(|(key_id, json)| Theme::read(key_id, json).ok())
        .collect();
    let mut external: BTreeMap<String, Theme> = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(key_id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let loaded = std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|json| Theme::read(key_id, &json));
            match loaded {
                Ok(theme) => {
                    external.insert(key_id.to_string(), theme);
                }
                Err(reason) => crate::logging!(
                    crate::logging::Level::Warning,
                    "temas",
                    "{} ignored: {reason}",
                    path.display()
                ),
            }
        }
    }
    for theme in &mut themes {
        if let Some(changed) = external.remove(&theme.key_id) {
            *theme = changed;
        }
    }
    themes.extend(external.into_values());
    themes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_themes_are_read_in_order() {
        let themes = load(&[]);
        let key_ids: Vec<&str> = themes.iter().map(|t| t.key_id.as_str()).collect();
        assert_eq!(key_ids, FACTORY_THEMES.map(|(key_id, _)| key_id));
        for theme in &themes {
            assert_eq!(theme.colors.len(), ROLES.len());
            for language in ["pt-BR", "en", "es", "es-MX"] {
                assert!(
                    theme.names.contains_key(language),
                    "{} has no name in {language}",
                    theme.key_id
                );
            }
        }
        assert!(!themes[0].dark, "the first one is the light one");
    }

    #[test]
    fn the_name_falls_back_to_english_then_to_the_first() {
        let ruby = load(&[])
            .into_iter()
            .find(|t| t.key_id == "rubi")
            .unwrap();
        assert_eq!(ruby.name("pt-BR"), "Rubi");
        assert_eq!(ruby.name("fr"), "Ruby");
        let mut german_only: serde_json::Value = serde_json::from_str(FACTORY_THEMES[0].1).unwrap();
        german_only["nome"] = serde_json::json!({ "de": "Hell" });
        let theme = Theme::read("x", &german_only.to_string()).unwrap();
        assert_eq!(theme.name("pt-BR"), "Hell");
    }

    #[test]
    fn a_bad_theme_says_what_is_missing() {
        let missing_role = FACTORY_THEMES[1].1.replace(r##""mid": "#4b5056","##, "");
        assert_eq!(Theme::read("x", &missing_role).unwrap_err(), "cores.mid missing");
        let bad_color = FACTORY_THEMES[1].1.replace("#4b5056", "cinza");
        assert!(
            Theme::read("x", &bad_color)
                .unwrap_err()
                .contains("cores.mid")
        );
        let other_format = FACTORY_THEMES[1]
            .1
            .replace(r#""formato": 1"#, r#""formato": 2"#);
        assert!(
            Theme::read("x", &other_format)
                .unwrap_err()
                .contains("format 2")
        );
    }

    /// A file in the folder adds a theme, or takes the place of the factory one with the same
    /// name; a bad file is ignored without taking the others along.
    #[test]
    fn the_dir_adds_and_replaces() {
        let dir = std::env::temp_dir().join(format!("zeebx-testes-temas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let purple = FACTORY_THEMES[1]
            .1
            .replace(r#""pt-BR": "Escuro""#, r#""pt-BR": "Roxo""#);
        std::fs::write(dir.join("roxo.json"), &purple).unwrap();
        let my_ruby = FACTORY_THEMES[5].1.replace("#cb3854", "#aa0000");
        std::fs::write(dir.join("rubi.json"), my_ruby).unwrap();
        std::fs::write(dir.join("quebrado.json"), "{").unwrap();
        std::fs::write(dir.join("leia-me.txt"), "não é tema").unwrap();

        let themes = load(std::slice::from_ref(&dir));
        assert_eq!(themes.len(), FACTORY_THEMES.len() + 1);
        assert_eq!(themes.last().unwrap().name("pt-BR"), "Roxo");
        let ruby = themes.iter().find(|t| t.key_id == "rubi").unwrap();
        let highlight = ruby
            .colors
            .iter()
            .find(|(role, _)| *role == "highlight")
            .unwrap()
            .1;
        assert_eq!(highlight.hex(), "#aa0000");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
