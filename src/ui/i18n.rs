//! Interface texts, in several languages.
//!
//! Each language is a JSON from key to text. The ones that ship with the emulator are embedded
//! in the binary, so that it works on its own; any other comes in as a file in a `lang/`
//! directory, without recompiling anything. That is why the lookup is by directory and not by a
//! fixed list: adding a language is copying a file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Language used when the chosen one lacks the key — or when there is no choice at all.
pub const FALLBACK: &str = "en";

/// The languages that come in the binary, as `(code, JSON contents)`.
const BUILT_IN: [(&str, &str); 4] = [
    ("en", include_str!("../../assets/lang/en.json")),
    ("es", include_str!("../../assets/lang/es.json")),
    ("es-MX", include_str!("../../assets/lang/es-MX.json")),
    ("pt-BR", include_str!("../../assets/lang/pt-BR.json")),
];

/// A loaded language: the code, the name it calls itself, and the texts.
#[derive(Debug, Clone)]
pub struct Language {
    pub code: String,
    /// The language's name **in the language itself** — someone looking for Portuguese looks for
    /// "Português", not "Portuguese".
    pub name: String,
    strings: BTreeMap<String, String>,
}

impl Language {
    fn parse(code: &str, json: &str) -> Option<Self> {
        let strings: BTreeMap<String, String> = serde_json::from_str(json).ok()?;
        let name = strings
            .get("language.name")
            .cloned()
            .unwrap_or_else(|| code.to_string());
        Some(Self {
            code: code.to_string(),
            name,
            strings,
        })
    }
}

/// All available languages and which one is in use.
#[derive(Debug, Clone)]
pub struct Catalog {
    languages: Vec<Language>,
    current: usize,
    fallback: usize,
}

impl Default for Catalog {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl Catalog {
    /// Builds the catalog with the embedded languages plus the `*.json` files of the given dirs.
    ///
    /// A file with the same code as an embedded one **replaces** the embedded one: that is how a
    /// translation can be fixed without waiting for a new version of the emulator.
    pub fn new(extra_dirs: &[PathBuf]) -> Self {
        let mut languages: Vec<Language> = BUILT_IN
            .iter()
            .filter_map(|(code, json)| Language::parse(code, json))
            .collect();
        for dir in extra_dirs {
            for (code, json) in read_dir(dir) {
                let Some(language) = Language::parse(&code, &json) else {
                    continue;
                };
                match languages.iter().position(|l| l.code == code) {
                    Some(at) => languages[at] = language,
                    None => languages.push(language),
                }
            }
        }
        languages.sort_by(|a, b| a.name.cmp(&b.name));
        let fallback = languages
            .iter()
            .position(|l| l.code == FALLBACK)
            .unwrap_or(0);
        Self {
            languages,
            current: fallback,
            fallback,
        }
    }

    /// The available languages, in name order.
    pub fn languages(&self) -> &[Language] {
        &self.languages
    }

    /// The code of the language in use.
    pub fn current(&self) -> &str {
        self.languages
            .get(self.current)
            .map(|l| l.code.as_str())
            .unwrap_or(FALLBACK)
    }

    /// Switches to `code`, if it exists. Returns whether the switch happened.
    pub fn select(&mut self, code: &str) -> bool {
        match self.languages.iter().position(|l| l.code == code) {
            Some(at) => {
                self.current = at;
                true
            }
            None => false,
        }
    }

    /// Chooses the language that best matches `preferred` — the exact `pt-BR` first, and the
    /// first `pt-*` as a second option, which is what makes a system `pt_PT` fall back to
    /// Portuguese instead of English.
    pub fn select_best(&mut self, preferred: &str) -> bool {
        if self.select(preferred) {
            return true;
        }
        let base = preferred.split(['-', '_']).next().unwrap_or(preferred);
        let found = self
            .languages
            .iter()
            .position(|l| l.code.split(['-', '_']).next() == Some(base));
        match found {
            Some(at) => {
                self.current = at;
                true
            }
            None => false,
        }
    }

    /// The text of `key`, in the language in use.
    ///
    /// A missing key falls back to the fallback language and, if it is missing there too, shows up
    /// as itself: an incomplete translation leaves the interface ugly, never empty.
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        [self.current, self.fallback]
            .iter()
            .find_map(|&at| self.languages.get(at)?.strings.get(key))
            .map(String::as_str)
            .unwrap_or(key)
    }

    /// The text of `key` with the `{name}`s replaced by the given values.
    pub fn format(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut text = self.get(key).to_string();
        for (name, value) in args {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }
}

/// Reads the `*.json` files of a directory as `(code, contents)`. A directory that does not exist
/// returns nothing — having no extra languages is the common case, not an error.
fn read_dir(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(code) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(json) = std::fs::read_to_string(&path) {
            found.push((code.to_string(), json));
        }
    }
    found
}

/// The language the system asks for, in the `pt-BR` format.
///
/// It comes from the POSIX environment variables, which macOS also honors when the user sets
/// them. Without any of them, English.
pub fn system_language() -> String {
    for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let Ok(value) = std::env::var(name) else {
            continue;
        };
        // `pt_BR.UTF-8` -> `pt-BR`
        let code = value.split('.').next().unwrap_or("").replace('_', "-");
        if !code.is_empty() && code != "C" && code != "POSIX" {
            return code;
        }
    }
    FALLBACK.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_languages_load_and_name_themselves() {
        let catalog = Catalog::default();
        let codes: Vec<&str> = catalog
            .languages()
            .iter()
            .map(|l| l.code.as_str())
            .collect();
        assert!(codes.contains(&"en"));
        assert!(codes.contains(&"es"));
        assert!(codes.contains(&"es-MX"));
        assert!(codes.contains(&"pt-BR"));
        let pt = catalog
            .languages()
            .iter()
            .find(|l| l.code == "pt-BR")
            .unwrap();
        assert_eq!(pt.name, "Português (Brasil)");
        let es = catalog
            .languages()
            .iter()
            .find(|l| l.code == "es")
            .unwrap();
        assert_eq!(es.name, "Español");
        let es_mx = catalog
            .languages()
            .iter()
            .find(|l| l.code == "es-MX")
            .unwrap();
        assert_eq!(es_mx.name, "Español (México)");
    }

    #[test]
    fn the_built_in_translations_have_exactly_the_same_keys() {
        // A key that exists in only one language is a text that will show up in English in the
        // middle of Portuguese — the kind of failure nobody notices until it is on screen.
        let catalog = Catalog::default();
        let keys = |code: &str| -> Vec<String> {
            catalog
                .languages()
                .iter()
                .find(|l| l.code == code)
                .map(|l| l.strings.keys().cloned().collect())
                .unwrap_or_default()
        };
        assert_eq!(keys("en"), keys("pt-BR"));
        assert_eq!(keys("en"), keys("es"));
        assert_eq!(keys("en"), keys("es-MX"));
    }

    #[test]
    fn a_missing_key_falls_back_to_the_fallback_then_to_itself() {
        let mut catalog = Catalog::default();
        catalog.select("pt-BR");
        assert_eq!(catalog.get("nav.library"), "Jogos");
        assert_eq!(catalog.get("chave.que.nao.existe"), "chave.que.nao.existe");
    }

    #[test]
    fn the_system_language_falls_back_to_the_same_base() {
        // A system `pt_PT` has to find Portuguese, not English.
        let mut catalog = Catalog::default();
        assert!(catalog.select_best("pt-PT"));
        assert_eq!(catalog.current(), "pt-BR");
        // A system `es_MX` lands on Mexican Spanish; an `es_AR` on generic Spanish.
        assert!(catalog.select_best("es-MX"));
        assert_eq!(catalog.current(), "es-MX");
        assert!(catalog.select_best("es-AR"));
        assert_eq!(catalog.current(), "es");
        // And a language we do not have leaves what was there.
        assert!(!catalog.select_best("ja-JP"));
        assert_eq!(catalog.current(), "es");
    }

    #[test]
    fn the_arguments_replace_the_placeholders() {
        let mut catalog = Catalog::default();
        catalog.select("pt-BR");
        assert_eq!(
            catalog.format("library.count", &[("count", "3")]),
            "3 jogo(s)"
        );
    }
}
