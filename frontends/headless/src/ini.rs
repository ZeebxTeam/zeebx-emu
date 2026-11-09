//! An INI reader, only what the configuration file uses.
//!
//! Written by hand, and not pulled in from a crate, for the same reason the core's [`config_dir`]
//! is: there are three rules — section in brackets, `key = value`, and `#` or `;` start a comment
//! — and each one fits on one line. The dependency would cost more to audit than this whole
//! file.
//!
//! [`config_dir`]: zeebx::ui::settings::config_dir

use std::collections::BTreeMap;

/// The file as read: a table of sections, each with its own keys.
///
/// Order does not matter to the reader, and `BTreeMap` gives for free what does matter: lookup by
/// name, and listing what was left over to warn about a misspelled key.
#[derive(Debug, Default)]
pub struct Ini {
    sections: BTreeMap<String, BTreeMap<String, Value>>,
}

/// A value and the line it was on, so the error message can say where.
#[derive(Debug, Clone)]
pub struct Value {
    pub text: String,
    pub line: usize,
}

impl Ini {
    /// Reads the whole text. Never fails: a line that is neither a section nor `key = value`
    /// becomes a warning, not a refusal — a file written by a newer version must still let the
    /// emulator open, as `settings.json` already does.
    pub fn read(text: &str) -> (Self, Vec<String>) {
        let mut ini = Self::default();
        let mut warnings = Vec::new();
        // Before the first section, keys fall into a section with no name. It serves a one-line
        // file, without ceremony.
        let mut section = String::new();

        for (number, line) in text.lines().enumerate() {
            let number = number + 1;
            let clear = strip_comment(line).trim();
            if clear.is_empty() {
                continue;
            }
            if let Some(name) = clear.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                section = name.trim().to_lowercase();
                ini.sections.entry(section.clone()).or_default();
                continue;
            }
            let Some((key_id, value)) = clear.split_once('=') else {
                warnings.push(format!("line {number}: neither a section nor `key = value`: {clear}"));
                continue;
            };
            let key_id = key_id.trim().to_lowercase();
            if key_id.is_empty() {
                warnings.push(format!("line {number}: empty key"));
                continue;
            }
            ini.sections.entry(section.clone()).or_default().insert(
                key_id,
                Value {
                    text: value.trim().to_string(),
                    line: number,
                },
            );
        }
        (ini, warnings)
    }

    /// The value of a key, or `None` if the section or the key does not exist.
    ///
    /// Looking up **consumes**: whatever is left at the end is a key nobody read, and that is what
    /// [`Ini::leftovers`] warns about. An `internal_resolutio` written without the final `n` would
    /// silently stay at the default, and whoever configured it would have no way to find out.
    pub fn get(&mut self, section: &str, key_id: &str) -> Option<Value> {
        self.sections.get_mut(section)?.remove(key_id)
    }

    /// Whether the section exists in the file, even if empty.
    pub fn has_section(&self, section: &str) -> bool {
        self.sections.contains_key(section)
    }

    /// The present sections whose name starts with a prefix, in order. It is how the ports are
    /// found: `port1`, `port2`.
    pub fn sections_with(&self, prefix: &str) -> Vec<String> {
        self.sections
            .keys()
            .filter(|name| name.starts_with(prefix))
            .cloned()
            .collect()
    }

    /// What nobody read, with the line of each one. A misspelled key shows up here.
    pub fn leftovers(&self) -> Vec<String> {
        let mut leftovers = Vec::new();
        for (section, key_ids) in &self.sections {
            for (key_id, value) in key_ids {
                let location = match section.is_empty() {
                    true => key_id.clone(),
                    false => format!("[{section}] {key_id}"),
                };
                leftovers.push(format!("line {}: nobody uses `{location}`", value.line));
            }
        }
        leftovers
    }
}

/// Cuts off the comment, respecting what is inside quotes.
///
/// Quotes matter: a controller name may contain `#` — and, more commonly, a Windows path cannot
/// lose anything. Outside quotes, `#` and `;` start a comment at any position, which is how every
/// INI behaves.
fn strip_comment(line: &str) -> &str {
    let mut quotes = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quotes = !quotes,
            '#' | ';' if !quotes => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Strips the quotes from a value, if it is entirely enclosed in them.
pub fn unquoted(text: &str) -> &str {
    match text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        true => &text[1..text.len() - 1],
        false => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_keys_and_comments() {
        let (mut ini, warnings) = Ini::read(
            "# um comentário\n\
             [Video]\n\
             modo = tela_cheia   ; e outro\n\
             \n\
             [audio]\n\
             volume=80\n",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(ini.get("video", "modo").unwrap().text, "tela_cheia");
        assert_eq!(ini.get("audio", "volume").unwrap().text, "80");
        assert!(ini.get("audio", "volume").is_none());
    }

    /// The `#` in a controller name must not become a comment.
    #[test]
    fn quotes_protect_the_comment() {
        let (mut ini, _) = Ini::read("[porta1]\ncontrole = \"Pad #2\"\n");
        let value = ini.get("porta1", "controle").unwrap();
        assert_eq!(unquoted(&value.text), "Pad #2");
    }

    /// A misspelled key has to show up, otherwise it silently stays at the default.

    #[test]
    fn an_unread_key_is_left_over() {
        let (mut ini, _) = Ini::read("[grafico]\nresolucao_intern = 2\n");
        assert!(ini.get("grafico", "resolucao_interna").is_none());
        let leftovers = ini.leftovers();
        assert_eq!(leftovers.len(), 1);
        assert!(leftovers[0].contains("resolucao_intern"), "{leftovers:?}");
    }

    #[test]
    fn a_broken_line_becomes_a_warning_and_the_rest_still_counts() {
        let (mut ini, warnings) = Ini::read("[audio]\nisto não é chave\nvolume = 50\n");
        assert_eq!(warnings.len(), 1);
        assert_eq!(ini.get("audio", "volume").unwrap().text, "50");
    }
}
