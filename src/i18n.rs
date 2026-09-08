//! Textos da interface, em vários idiomas.
//!
//! Cada idioma é um JSON de chave para texto. Os dois que acompanham o emulador vêm embutidos
//! no binário, para que ele funcione sozinho; qualquer outro entra como arquivo numa pasta
//! `lang/`, sem recompilar nada. É por isso que a busca é por diretório e não por uma lista
//! fixa: acrescentar um idioma é copiar um arquivo.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Idioma usado quando o escolhido não tem a chave — ou quando não há escolha nenhuma.
pub const FALLBACK: &str = "en";

/// Os idiomas que vêm no binário, como `(código, conteúdo do JSON)`.
const BUILT_IN: [(&str, &str); 2] = [
    ("en", include_str!("../assets/lang/en.json")),
    ("pt-BR", include_str!("../assets/lang/pt-BR.json")),
];

/// Um idioma carregado: o código, o nome como ele se chama, e os textos.
#[derive(Debug, Clone)]
pub struct Language {
    pub code: String,
    /// O nome do idioma **no próprio idioma** — quem procura português procura "Português",
    /// não "Portuguese".
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

/// Todos os idiomas disponíveis e qual está em uso.
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
    /// Monta o catálogo com os idiomas embutidos mais os arquivos `*.json` das pastas dadas.
    ///
    /// Um arquivo com o mesmo código de um embutido **substitui** o embutido: é assim que dá
    /// para corrigir uma tradução sem esperar uma versão nova do emulador.
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

    /// Os idiomas disponíveis, em ordem de nome.
    pub fn languages(&self) -> &[Language] {
        &self.languages
    }

    /// O código do idioma em uso.
    pub fn current(&self) -> &str {
        self.languages
            .get(self.current)
            .map(|l| l.code.as_str())
            .unwrap_or(FALLBACK)
    }

    /// Passa a usar `code`, se ele existir. Devolve se a troca aconteceu.
    pub fn select(&mut self, code: &str) -> bool {
        match self.languages.iter().position(|l| l.code == code) {
            Some(at) => {
                self.current = at;
                true
            }
            None => false,
        }
    }

    /// Escolhe o idioma que melhor atende `preferred` — o `pt-BR` exato na frente, e o
    /// primeiro `pt-*` como segunda opção, que é o que faz um `pt_PT` do sistema cair no
    /// português em vez do inglês.
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

    /// O texto de `key`, no idioma em uso.
    ///
    /// Uma chave que falta cai no idioma de reserva e, faltando lá também, aparece como ela
    /// mesma: uma tradução incompleta deixa a interface feia, nunca vazia.
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        [self.current, self.fallback]
            .iter()
            .find_map(|&at| self.languages.get(at)?.strings.get(key))
            .map(String::as_str)
            .unwrap_or(key)
    }

    /// O texto de `key` com os `{nome}` trocados pelos valores dados.
    pub fn format(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut text = self.get(key).to_string();
        for (name, value) in args {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }
}

/// Lê os `*.json` de uma pasta como `(código, conteúdo)`. Pasta que não existe devolve nada —
/// não ter idiomas extras é o caso comum, não um erro.
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

/// O idioma que o sistema pede, no formato `pt-BR`.
///
/// Vem das variáveis de ambiente do POSIX, que o macOS também respeita quando o usuário as
/// define. Sem nenhuma delas, o inglês.
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
    fn os_idiomas_embutidos_carregam_e_se_nomeiam() {
        let catalog = Catalog::default();
        let codes: Vec<&str> = catalog
            .languages()
            .iter()
            .map(|l| l.code.as_str())
            .collect();
        assert!(codes.contains(&"en"));
        assert!(codes.contains(&"pt-BR"));
        let pt = catalog
            .languages()
            .iter()
            .find(|l| l.code == "pt-BR")
            .unwrap();
        assert_eq!(pt.name, "Português (Brasil)");
    }

    #[test]
    fn as_duas_traducoes_tem_exatamente_as_mesmas_chaves() {
        // Uma chave que existe só num idioma é um texto que vai aparecer em inglês no meio do
        // português — o tipo de falha que ninguém percebe até estar na tela.
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
    }

    #[test]
    fn uma_chave_que_falta_cai_no_reserva_e_depois_em_si_mesma() {
        let mut catalog = Catalog::default();
        catalog.select("pt-BR");
        assert_eq!(catalog.get("nav.library"), "Jogos");
        assert_eq!(catalog.get("chave.que.nao.existe"), "chave.que.nao.existe");
    }

    #[test]
    fn o_idioma_do_sistema_cai_no_mesmo_tronco() {
        // Um `pt_PT` do sistema tem que achar o português, não o inglês.
        let mut catalog = Catalog::default();
        assert!(catalog.select_best("pt-PT"));
        assert_eq!(catalog.current(), "pt-BR");
        // E um idioma que não temos deixa o que estava.
        assert!(!catalog.select_best("ja-JP"));
        assert_eq!(catalog.current(), "pt-BR");
    }

    #[test]
    fn os_argumentos_entram_no_lugar_das_chaves() {
        let mut catalog = Catalog::default();
        catalog.select("pt-BR");
        assert_eq!(
            catalog.format("library.count", &[("count", "3")]),
            "3 jogo(s)"
        );
    }
}
