//! HTPC visual themes (QML skins) and per-theme color palette resolution.
//!
//! Unlike [`crate::ui::themes`], which only swaps the `QPalette` colors of the desktop interface,
//! an HTPC theme defines the entire 10-foot library experience in QML — while keeping the host
//! shell (game viewport, Quick Menu, gamepad settings, folder picker and fallback recovery) inside
//! the `zeebx-htpc` binary so a broken or outdated community theme can never lock the user out.
//!
//! Discovery follows the same search order as [`crate::ui::themes::dirs`]:
//!
//! ```text
//! <builtin>                         zwheel-hd (always available, fallback on error)
//! <executable folder>/htpc-themes/  portable installation
//! <config>/htpc-themes/             user / community themes
//! ```
//!
//! Each external theme lives in `<dir>/<id>/` with a `theme.json` manifest (format 1) pointing to
//! a relative QML entry file (typically `Main.qml`) and optional custom color palettes.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::ui::i18n;
use crate::ui::settings::HtpcThemeColors;
use crate::ui::themes::{Color, Theme};

/// The built-in HTPC theme id, always present and used as fallback if an external theme fails.
pub const DEFAULT_THEME_ID: &str = "zwheel-hd";

/// Special palette key meaning "follow the global Zeebx color theme (`settings.theme`)".
pub const GLOBAL_PALETTE_KEY: &str = "global";

/// Special palette key meaning "use this HTPC theme's declared default palette".
pub const THEME_DEFAULT_PALETTE_KEY: &str = "";

/// The manifest format version this reader understands.
const FORMAT: u32 = 1;

/// QRC entry URL for the built-in Z-Wheel HD theme.
pub const BUILTIN_ZWHEEL_HD_URL: &str = "qrc:/qt/qml/zeebx/htpc/qml/themes/zwheel-hd/Main.qml";

/// Built-in accent presets offered in the HTPC Appearance settings for quick gamepad selection.
pub const ACCENT_PRESETS: [(&str, &str); 9] = [
    ("", "htpc.accent.palette_default"),
    ("#e8531e", "htpc.accent.zeebo_orange"),
    ("#4b6acf", "htpc.accent.zeebo_blue"),
    ("#3172d2", "htpc.accent.cobalt"),
    ("#cb3854", "htpc.accent.ruby"),
    ("#259c63", "htpc.accent.emerald"),
    ("#9333ea", "htpc.accent.violet"),
    ("#d97706", "htpc.accent.amber"),
    ("#06b6d4", "htpc.accent.cyan"),
];

/// A color palette declared directly inside an HTPC theme's `theme.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtpcPalette {
    pub key_id: String,
    pub names: BTreeMap<String, String>,
    pub dark: bool,
    pub background: Color,
    pub background_alt: Color,
    pub surface: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub warning: Color,
    pub error: Color,
}

impl HtpcPalette {
    pub fn name(&self, lang: &str) -> &str {
        localized(&self.names, lang, &self.key_id)
    }
}

/// An HTPC theme discovered either built-in or on disk.
#[derive(Debug, Clone)]
pub struct HtpcTheme {
    pub id: String,
    pub names: BTreeMap<String, String>,
    pub descriptions: BTreeMap<String, String>,
    pub author: String,
    pub version: String,
    /// `qrc:/...` for built-in themes, or `file://...` for external themes on disk.
    pub entry_url: String,
    /// Root directory on disk, `None` for built-in themes.
    pub root_dir: Option<PathBuf>,
    pub builtin: bool,
    /// Palette key used when the user hasn't picked a custom palette for this theme.
    pub default_palette: Option<String>,
    /// Optional theme-specific color palettes declared in `theme.json`.
    pub palettes: Vec<HtpcPalette>,
}

impl HtpcTheme {
    pub fn name(&self, lang: &str) -> &str {
        localized(&self.names, lang, &self.id)
    }

    pub fn description(&self, lang: &str) -> &str {
        localized(&self.descriptions, lang, "")
    }
}

/// Fully resolved colors passed to the HTPC Shell and active theme via `htpc.colors`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedColors {
    pub dark: bool,
    pub background: String,
    pub background_alt: String,
    pub surface: String,
    pub surface_solid: String,
    pub border: String,
    pub text: String,
    pub muted: String,
    pub accent: String,
    pub accent_text: String,
    pub accent_soft: String,
    pub accent_glow: String,
    pub reel_bg: String,
    pub warning: String,
    pub error: String,
}

/// A selectable palette choice for the currently active HTPC theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteOption {
    /// Stored value in `HtpcThemeColors::palette` (`""` means `None` / theme default).
    pub key: String,
    /// Human-readable label (already localized or key for catalog lookup if `is_catalog_key`).
    pub label: String,
    pub is_catalog_key: bool,
}

#[derive(Deserialize)]
struct ManifestFile {
    format: u32,
    name: BTreeMap<String, String>,
    #[serde(default)]
    description: BTreeMap<String, String>,
    #[serde(default)]
    author: String,
    #[serde(default)]
    version: String,
    #[serde(default = "default_entry")]
    entry: String,
    #[serde(default)]
    default_palette: Option<String>,
    #[serde(default)]
    palettes: BTreeMap<String, PaletteFile>,
}

fn default_entry() -> String {
    "Main.qml".to_string()
}

#[derive(Deserialize)]
struct PaletteFile {
    name: BTreeMap<String, String>,
    #[serde(default = "default_true")]
    dark: bool,
    background: String,
    background_alt: String,
    surface: String,
    border: String,
    text: String,
    muted: String,
    accent: String,
    #[serde(default = "default_warning")]
    warning: String,
    #[serde(default = "default_error")]
    error: String,
}

fn default_true() -> bool {
    true
}
fn default_warning() -> String {
    "#e5a33b".to_string()
}
fn default_error() -> String {
    "#f07067".to_string()
}

fn parse_hex(text: &str) -> Option<Color> {
    let hex = text.strip_prefix('#').filter(|h| h.len() == 6)?;
    u32::from_str_radix(hex, 16).ok().map(Color)
}

fn localized<'a>(map: &'a BTreeMap<String, String>, lang: &str, fallback: &'a str) -> &'a str {
    map.get(lang)
        .or_else(|| map.get(i18n::FALLBACK))
        .or_else(|| map.values().next())
        .map(String::as_str)
        .unwrap_or(fallback)
}

/// Validates that `rel` is a safe relative path inside the theme directory (no `..`, no root).
fn safe_relative_path(rel: &str) -> Option<PathBuf> {
    let path = Path::new(rel);
    if path.is_absolute() || rel.is_empty() {
        return None;
    }
    let mut clean = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!clean.as_os_str().is_empty()).then_some(clean)
}

fn file_url(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

/// The built-in Z-Wheel HD theme, which also declares a "Zeebo Classic" light/white preset
/// alongside all standard Zeebx color palettes.
pub fn builtin_zwheel_hd() -> HtpcTheme {
    let mut names = BTreeMap::new();
    names.insert("pt-BR".into(), "Z-Wheel HD".into());
    names.insert("en".into(), "Z-Wheel HD".into());
    names.insert("es".into(), "Z-Wheel HD".into());
    names.insert("es-MX".into(), "Z-Wheel HD".into());

    let mut descriptions = BTreeMap::new();
    descriptions.insert(
        "pt-BR".into(),
        "Vitrine em alta definição inspirada na Z-Wheel do Zeebo, com carrossel de logos e caixas em perspectiva.".into(),
    );
    descriptions.insert(
        "en".into(),
        "High-definition showcase inspired by the Zeebo Z-Wheel, with a cylindrical logo reel and perspective box stage.".into(),
    );
    descriptions.insert(
        "es".into(),
        "Vitrina en alta definición inspirada en la Z-Wheel de Zeebo, con carrusel de logos y cajas en perspectiva.".into(),
    );
    descriptions.insert(
        "es-MX".into(),
        "Vitrina en alta definición inspirada en la Z-Wheel de Zeebo, con carrusel de logos y cajas en perspectiva.".into(),
    );

    let mut classic_names = BTreeMap::new();
    classic_names.insert("pt-BR".into(), "Zeebo Clássico (Claro)".into());
    classic_names.insert("en".into(), "Zeebo Classic (Light)".into());
    classic_names.insert("es".into(), "Zeebo Clásico (Claro)".into());
    classic_names.insert("es-MX".into(), "Zeebo Clásico (Claro)".into());

    let classic = HtpcPalette {
        key_id: "zeebo_classic".into(),
        names: classic_names,
        dark: false,
        background: Color(0xeef2f6),
        background_alt: Color(0xdce4ee),
        surface: Color(0xffffff),
        border: Color(0xc7d2e0),
        text: Color(0x18202c),
        muted: Color(0x536073),
        accent: Color(0xe8531e),
        warning: Color(0xc97d10),
        error: Color(0xc8322b),
    };

    HtpcTheme {
        id: DEFAULT_THEME_ID.into(),
        names,
        descriptions,
        author: "ZeebxTeam".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        entry_url: BUILTIN_ZWHEEL_HD_URL.into(),
        root_dir: None,
        builtin: true,
        default_palette: Some("laranja".into()),
        palettes: vec![classic],
    }
}

/// Where to look for external HTPC themes: next to the executable (portable) and in the user's
/// configuration directory.
pub fn dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        dirs.push(dir.join("htpc-themes"));
    }
    dirs.push(crate::config::config_dir().join("htpc-themes"));
    dirs
}

/// Reads a single external HTPC theme directory containing `theme.json`.
pub fn read_theme_dir(dir: &Path) -> Result<HtpcTheme, String> {
    let id = dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "invalid theme folder name".to_string())?
        .to_string();

    let manifest_path = dir.join("theme.json");
    let raw = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("cannot read theme.json: {e}"))?;
    let file: ManifestFile =
        serde_json::from_str(&raw).map_err(|e| format!("invalid theme.json: {e}"))?;

    if file.format != FORMAT {
        return Err(format!(
            "format {}, and this reader understands {FORMAT}",
            file.format
        ));
    }
    if file.name.is_empty() {
        return Err("name is empty".to_string());
    }

    let rel_entry = safe_relative_path(&file.entry)
        .ok_or_else(|| format!("entry {:?} must be a relative path inside the theme", file.entry))?;
    let entry_path = dir.join(&rel_entry);
    if !entry_path.is_file() {
        return Err(format!("entry file {} does not exist", entry_path.display()));
    }

    let mut palettes = Vec::new();
    for (key_id, p) in file.palettes {
        let color = |field: &str, v: &str| {
            parse_hex(v).ok_or_else(|| format!("palettes.{key_id}.{field}: {v:?} is not #rrggbb"))
        };
        palettes.push(HtpcPalette {
            key_id: key_id.clone(),
            names: p.name,
            dark: p.dark,
            background: color("background", &p.background)?,
            background_alt: color("background_alt", &p.background_alt)?,
            surface: color("surface", &p.surface)?,
            border: color("border", &p.border)?,
            text: color("text", &p.text)?,
            muted: color("muted", &p.muted)?,
            accent: color("accent", &p.accent)?,
            warning: color("warning", &p.warning)?,
            error: color("error", &p.error)?,
        });
    }

    Ok(HtpcTheme {
        id,
        names: file.name,
        descriptions: file.description,
        author: file.author,
        version: file.version,
        entry_url: file_url(&entry_path),
        root_dir: Some(dir.to_path_buf()),
        builtin: false,
        default_palette: file.default_palette,
        palettes,
    })
}

/// Loads all built-in HTPC themes plus valid external themes from `dirs`.
///
/// An external folder with the same id as a built-in theme replaces it (allowing customization),
/// while invalid folders are logged and skipped without preventing startup.
pub fn load(dirs: &[PathBuf]) -> Vec<HtpcTheme> {
    let mut themes = vec![builtin_zwheel_hd()];
    let mut external: BTreeMap<String, HtpcTheme> = BTreeMap::new();

    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if !path.join("theme.json").is_file() {
                continue;
            }
            match read_theme_dir(&path) {
                Ok(theme) => {
                    external.insert(theme.id.clone(), theme);
                }
                Err(reason) => crate::logging!(
                    crate::logging::Level::Warning,
                    "temas",
                    "htpc theme {} ignored: {reason}",
                    path.display()
                ),
            }
        }
    }

    for theme in &mut themes {
        if let Some(override_theme) = external.remove(&theme.id) {
            *theme = override_theme;
        }
    }
    themes.extend(external.into_values());
    themes
}

/// Lists the selectable palette choices for a given HTPC theme:
/// 1. Theme default
/// 2. Follow global Zeebx theme
/// 3. Any custom palettes declared by this HTPC theme
/// 4. All Zeebx color themes from `temas/` (factory + user JSONs)
pub fn palette_options(
    htpc_theme: &HtpcTheme,
    zeebx_themes: &[Theme],
    lang: &str,
) -> Vec<PaletteOption> {
    let mut out = vec![
        PaletteOption {
            key: THEME_DEFAULT_PALETTE_KEY.to_string(),
            label: "htpc.palette.theme_default".to_string(),
            is_catalog_key: true,
        },
        PaletteOption {
            key: GLOBAL_PALETTE_KEY.to_string(),
            label: "htpc.palette.follow_global".to_string(),
            is_catalog_key: true,
        },
    ];
    for p in &htpc_theme.palettes {
        out.push(PaletteOption {
            key: p.key_id.clone(),
            label: p.name(lang).to_string(),
            is_catalog_key: false,
        });
    }
    for t in zeebx_themes {
        if !htpc_theme.palettes.iter().any(|p| p.key_id == t.key_id) {
            out.push(PaletteOption {
                key: t.key_id.clone(),
                label: t.name(lang).to_string(),
                is_catalog_key: false,
            });
        }
    }
    out
}

fn role(theme: &Theme, name: &str, fallback: Color) -> Color {
    theme
        .colors
        .iter()
        .find(|(r, _)| *r == name)
        .map(|(_, c)| *c)
        .unwrap_or(fallback)
}

fn rgb_channels(color: Color) -> (u8, u8, u8) {
    (
        ((color.0 >> 16) & 0xff) as u8,
        ((color.0 >> 8) & 0xff) as u8,
        (color.0 & 0xff) as u8,
    )
}

fn rgba_hex(color: Color, alpha: u8) -> String {
    let (r, g, b) = rgb_channels(color);
    // QML `#AARRGGBB` notation works natively for every `color` property.
    format!("#{alpha:02x}{r:02x}{g:02x}{b:02x}")
}

fn blend(a: Color, b: Color, t: f32) -> Color {
    let (ar, ag, ab) = rgb_channels(a);
    let (br, bg, bb) = rgb_channels(b);
    let mix = |x: u8, y: u8| ((x as f32) * (1.0 - t) + (y as f32) * t).round().clamp(0.0, 255.0) as u8;
    let r = mix(ar, br) as u32;
    let g = mix(ag, bg) as u32;
    let bl = mix(ab, bb) as u32;
    Color((r << 16) | (g << 8) | bl)
}

/// Picks white or near-black text for maximum legibility over `bg` using relative luminance.
fn contrasting_text(bg: Color) -> &'static str {
    let (r, g, b) = rgb_channels(bg);
    let lum = 0.2126 * (r as f32) + 0.7152 * (g as f32) + 0.0722 * (b as f32);
    if lum > 158.0 { "#141619" } else { "#ffffff" }
}

fn from_htpc_palette(p: &HtpcPalette, accent_override: Option<Color>) -> ResolvedColors {
    let accent = accent_override.unwrap_or(p.accent);
    let reel_bg = if p.dark {
        rgba_hex(blend(p.background, Color(0x000000), 0.35), 0xe0)
    } else {
        rgba_hex(Color(0x18202c), 0xe0)
    };
    ResolvedColors {
        dark: p.dark,
        background: p.background.hex(),
        background_alt: p.background_alt.hex(),
        surface: rgba_hex(p.surface, if p.dark { 0xd8 } else { 0xe6 }),
        surface_solid: p.surface.hex(),
        border: rgba_hex(p.border, if p.dark { 0x66 } else { 0x99 }),
        text: p.text.hex(),
        muted: p.muted.hex(),
        accent: accent.hex(),
        accent_text: contrasting_text(accent).to_string(),
        accent_soft: rgba_hex(accent, if p.dark { 0x38 } else { 0x26 }),
        accent_glow: rgba_hex(accent, 0x88),
        reel_bg,
        warning: p.warning.hex(),
        error: p.error.hex(),
    }
}

fn from_zeebx_theme(theme: &Theme, accent_override: Option<Color>) -> ResolvedColors {
    let window = role(theme, "window", Color(0x25282c));
    let base = role(theme, "base", Color(0x1c1e21));
    let dark_c = role(theme, "dark", Color(0x121417));
    let button = role(theme, "button", Color(0x2f3337));
    let mid = role(theme, "mid", Color(0x4b5056));
    let text = role(theme, "windowText", Color(0xe6e8eb));
    let muted = role(theme, "placeholderText", Color(0x8e949c));
    let highlight = role(theme, "highlight", Color(0xca4014));
    let accent = accent_override.unwrap_or(highlight);

    let background = if theme.dark { dark_c } else { window };
    let background_alt = if theme.dark { window } else { base };
    let surface = if theme.dark { button } else { base };
    let reel_bg = if theme.dark {
        rgba_hex(blend(dark_c, Color(0x000000), 0.4), 0xe4)
    } else {
        rgba_hex(Color(0x1c222b), 0xe0)
    };

    ResolvedColors {
        dark: theme.dark,
        background: background.hex(),
        background_alt: background_alt.hex(),
        surface: rgba_hex(surface, if theme.dark { 0xd8 } else { 0xec }),
        surface_solid: surface.hex(),
        border: rgba_hex(mid, if theme.dark { 0x80 } else { 0xaa }),
        text: text.hex(),
        muted: muted.hex(),
        accent: accent.hex(),
        accent_text: contrasting_text(accent).to_string(),
        accent_soft: rgba_hex(accent, if theme.dark { 0x3a } else { 0x24 }),
        accent_glow: rgba_hex(accent, 0x8c),
        reel_bg,
        warning: theme.warning.hex(),
        error: theme.error.hex(),
    }
}

/// Resolves the active color set for `htpc_theme` given its per-theme color overrides, the global
/// Zeebx theme key, and all loaded Zeebx color themes.
pub fn resolve_colors(
    htpc_theme: &HtpcTheme,
    per_theme: Option<&HtpcThemeColors>,
    global_theme_key: Option<&str>,
    zeebx_themes: &[Theme],
) -> ResolvedColors {
    let accent_override = per_theme
        .and_then(|c| c.accent.as_deref())
        .and_then(parse_hex);

    let wanted_key: Option<&str> = match per_theme.and_then(|c| c.palette.as_deref()) {
        Some(GLOBAL_PALETTE_KEY) => global_theme_key.or(Some("escuro")),
        Some(k) if !k.is_empty() => Some(k),
        _ => htpc_theme
            .default_palette
            .as_deref()
            .or(global_theme_key)
            .or(Some("laranja")),
    };

    if let Some(key) = wanted_key {
        if let Some(p) = htpc_theme.palettes.iter().find(|p| p.key_id == key) {
            return from_htpc_palette(p, accent_override);
        }
        if let Some(t) = zeebx_themes.iter().find(|t| t.key_id == key) {
            return from_zeebx_theme(t, accent_override);
        }
    }

    if let Some(fallback) = zeebx_themes
        .iter()
        .find(|t| t.key_id == "laranja")
        .or_else(|| zeebx_themes.first())
    {
        return from_zeebx_theme(fallback, accent_override);
    }

    // Ultimate fallback if no themes were loaded at all.
    let accent = accent_override.unwrap_or(Color(0xe8531e));
    ResolvedColors {
        dark: true,
        background: "#161210".into(),
        background_alt: "#231e1c".into(),
        surface: "#d828221f".into(),
        surface_solid: "#28221f".into(),
        border: "#55ffffff".into(),
        text: "#f5efe9".into(),
        muted: "#b3a598".into(),
        accent: accent.hex(),
        accent_text: contrasting_text(accent).into(),
        accent_soft: rgba_hex(accent, 0x38),
        accent_glow: rgba_hex(accent, 0x88),
        reel_bg: "#e0120d0b".into(),
        warning: "#f0c75a".into(),
        error: "#ff7a6e".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::themes;

    #[test]
    fn builtin_zwheel_hd_is_always_loaded() {
        let loaded = load(&[]);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, DEFAULT_THEME_ID);
        assert!(loaded[0].builtin);
        assert_eq!(loaded[0].name("pt-BR"), "Z-Wheel HD");
        assert_eq!(loaded[0].default_palette.as_deref(), Some("laranja"));
        assert!(loaded[0].palettes.iter().any(|p| p.key_id == "zeebo_classic"));
    }

    #[test]
    fn external_theme_loads_and_invalid_ones_are_ignored() {
        let dir = std::env::temp_dir().join(format!("zeebx-htpc-themes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // Valid community theme with a custom palette.
        let good = dir.join("neon-arcade");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(good.join("Main.qml"), "import QtQuick\nItem {}\n").unwrap();
        std::fs::write(
            good.join("theme.json"),
            r##"{
                "format": 1,
                "name": { "en": "Neon Arcade", "pt-BR": "Arcade Neon" },
                "author": "Community",
                "version": "1.0",
                "entry": "Main.qml",
                "default_palette": "synth",
                "palettes": {
                    "synth": {
                        "name": { "en": "Synthwave", "pt-BR": "Synthwave" },
                        "dark": true,
                        "background": "#0b0718",
                        "background_alt": "#180d33",
                        "surface": "#221347",
                        "border": "#5b3a99",
                        "text": "#f8f4ff",
                        "muted": "#b4a3d9",
                        "accent": "#ff2e97"
                    }
                }
            }"##,
        )
        .unwrap();

        // Path traversal attempt must be rejected.
        let evil = dir.join("evil-theme");
        std::fs::create_dir_all(&evil).unwrap();
        std::fs::write(
            evil.join("theme.json"),
            r#"{"format": 1, "name": {"en": "Evil"}, "entry": "../escape.qml"}"#,
        )
        .unwrap();

        // Missing entry file must be rejected.
        let broken = dir.join("missing-qml");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(
            broken.join("theme.json"),
            r#"{"format": 1, "name": {"en": "Missing"}, "entry": "DoesNotExist.qml"}"#,
        )
        .unwrap();

        let loaded = load(std::slice::from_ref(&dir));
        assert_eq!(loaded.len(), 2);
        let neon = loaded.iter().find(|t| t.id == "neon-arcade").unwrap();
        assert_eq!(neon.name("pt-BR"), "Arcade Neon");
        assert!(neon.entry_url.starts_with("file://"));

        // Resolves custom palette and accent override cleanly.
        let zeebx_themes = themes::load(&[]);
        let resolved_default = resolve_colors(neon, None, None, &zeebx_themes);
        assert_eq!(resolved_default.background, "#0b0718");
        assert_eq!(resolved_default.accent, "#ff2e97");

        let override_cfg = HtpcThemeColors {
            palette: Some("azul".into()),
            accent: Some("#00ffaa".into()),
        };
        let resolved_override = resolve_colors(neon, Some(&override_cfg), None, &zeebx_themes);
        assert_eq!(resolved_override.accent, "#00ffaa");
        assert_eq!(resolved_override.accent_text, "#141619");
        assert_ne!(resolved_override.background, "#0b0718");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn per_theme_palette_can_follow_global_or_pick_builtin_classic() {
        let zeebx_themes = themes::load(&[]);
        let zwheel = builtin_zwheel_hd();

        let classic_cfg = HtpcThemeColors {
            palette: Some("zeebo_classic".into()),
            accent: None,
        };
        let classic = resolve_colors(&zwheel, Some(&classic_cfg), Some("escuro"), &zeebx_themes);
        assert!(!classic.dark);
        assert_eq!(classic.background, "#eef2f6");

        let global_cfg = HtpcThemeColors {
            palette: Some(GLOBAL_PALETTE_KEY.into()),
            accent: None,
        };
        let global = resolve_colors(&zwheel, Some(&global_cfg), Some("rubi"), &zeebx_themes);
        assert!(global.dark);
        let rubi = zeebx_themes.iter().find(|t| t.key_id == "rubi").unwrap();
        let rubi_hl = role(rubi, "highlight", Color(0));
        assert_eq!(global.accent, rubi_hl.hex());
    }
}
