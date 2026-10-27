//! Os temas da interface: paletas de cor por cima do estilo Fusion.
//!
//! Um tema não troca o desenho dos controles, só as cores, como os do PCSX2. Trocar o estilo
//! (Material, Universal) pediria reiniciar o programa, e esses estilos não leem a paleta que a
//! grade e o slider da biblioteca usam. Ver `docs/implementacao/26-temas.md`.
//!
//! Cada tema é um JSON com os papéis do `QPalette`. Os de fábrica vêm embutidos no binário, como
//! os idiomas; qualquer outro entra como arquivo numa pasta `temas/`, e um arquivo com o mesmo
//! nome de um de fábrica toma o lugar dele. **Um arquivo ruim é ignorado**, com o motivo no
//! registro: o programa abre com os outros, e quem tinha escolhido aquele cai na paleta do
//! sistema. A conferência do contraste mora em `ferramentas/confere_temas.py`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Os papéis do `QPalette` que um tema define, com o nome que o QML usa (`palette.windowText`).
/// Todos são obrigatórios: um papel faltando seria o do sistema, e um texto claro do tema por cima
/// de um fundo claro do sistema é ilegível.
pub const PAPEIS: [&str; 20] = [
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

/// Os papéis que o tema também dá aos controles desabilitados.
pub const DESABILITADOS: [&str; 3] = ["windowText", "text", "buttonText"];

/// A versão do formato que este leitor entende.
const FORMATO: u32 = 1;

/// Os temas que vêm no binário, como `(chave, conteúdo do JSON)`, na ordem do seletor.
const DE_FABRICA: [(&str, &str); 7] = [
    ("claro", include_str!("../../assets/temas/claro.json")),
    ("escuro", include_str!("../../assets/temas/escuro.json")),
    ("amoled", include_str!("../../assets/temas/amoled.json")),
    ("laranja", include_str!("../../assets/temas/laranja.json")),
    ("azul", include_str!("../../assets/temas/azul.json")),
    ("rubi", include_str!("../../assets/temas/rubi.json")),
    (
        "esmeralda",
        include_str!("../../assets/temas/esmeralda.json"),
    ),
];

/// Uma cor `#rrggbb`, já lida.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cor(pub u32);

impl Cor {
    fn le(texto: &str) -> Option<Self> {
        let hexa = texto.strip_prefix('#').filter(|h| h.len() == 6)?;
        u32::from_str_radix(hexa, 16).ok().map(Self)
    }

    /// `#rrggbb`, como o QML aceita numa cor.
    pub fn hexa(self) -> String {
        format!("#{:06x}", self.0)
    }
}

/// Um tema lido e conferido.
#[derive(Debug, Clone)]
pub struct Tema {
    /// O nome do arquivo sem o `.json`. É o que se grava no `settings.json`.
    pub chave: String,
    nomes: BTreeMap<String, String>,
    pub escuro: bool,
    /// Os [`PAPEIS`], na mesma ordem.
    pub cores: Vec<(&'static str, Cor)>,
    /// Os [`DESABILITADOS`], na mesma ordem.
    pub desabilitado: Vec<(&'static str, Cor)>,
    /// O texto de aviso (uma tecla recusada, uma pasta que falta) e o de erro (o jogo que não
    /// abriu), que a interface desenha por cima da janela.
    pub aviso: Cor,
    pub erro: Cor,
}

#[derive(Deserialize)]
struct Arquivo {
    formato: u32,
    nome: BTreeMap<String, String>,
    escuro: bool,
    cores: BTreeMap<String, String>,
    desabilitado: BTreeMap<String, String>,
    aviso: String,
    erro: String,
}

impl Tema {
    /// Lê um tema. O `Err` diz o que estava errado, para ir ao registro.
    pub fn le(chave: &str, json: &str) -> Result<Self, String> {
        let arquivo: Arquivo = serde_json::from_str(json).map_err(|erro| erro.to_string())?;
        if arquivo.formato != FORMATO {
            return Err(format!(
                "formato {}, e este leitor entende o {FORMATO}",
                arquivo.formato
            ));
        }
        if arquivo.nome.is_empty() {
            return Err("sem nome".to_string());
        }
        let cor = |grupo: &str, valores: &BTreeMap<String, String>, papel: &'static str| {
            let texto = valores
                .get(papel)
                .ok_or_else(|| format!("{grupo}.{papel} faltando"))?;
            Cor::le(texto)
                .map(|cor| (papel, cor))
                .ok_or_else(|| format!("{grupo}.{papel}: {texto:?} não é #rrggbb"))
        };
        let cores = PAPEIS
            .into_iter()
            .map(|papel| cor("cores", &arquivo.cores, papel))
            .collect::<Result<_, _>>()?;
        let desabilitado = DESABILITADOS
            .into_iter()
            .map(|papel| cor("desabilitado", &arquivo.desabilitado, papel))
            .collect::<Result<_, _>>()?;
        let solta = |campo: &str, texto: &str| {
            Cor::le(texto).ok_or_else(|| format!("{campo}: {texto:?} não é #rrggbb"))
        };
        Ok(Self {
            chave: chave.to_string(),
            nomes: arquivo.nome,
            escuro: arquivo.escuro,
            cores,
            desabilitado,
            aviso: solta("aviso", &arquivo.aviso)?,
            erro: solta("erro", &arquivo.erro)?,
        })
    }

    /// O nome no idioma `codigo`; sem ele, o do idioma reserva do catálogo; sem esse, o primeiro.
    pub fn nome(&self, codigo: &str) -> &str {
        self.nomes
            .get(codigo)
            .or_else(|| self.nomes.get(crate::ui::i18n::FALLBACK))
            .or_else(|| self.nomes.values().next())
            .map(String::as_str)
            .unwrap_or(&self.chave)
    }
}

/// Onde procurar temas além dos de fábrica: ao lado do executável e na pasta de configuração.
///
/// A primeira serve para uma cópia portátil do emulador; a segunda, para quem quer acrescentar um
/// tema sem mexer na instalação. A mesma regra do [`crate::ui::settings::language_dirs`].
pub fn pastas() -> Vec<PathBuf> {
    let mut pastas = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        pastas.push(dir.join("temas"));
    }
    pastas.push(crate::config::config_dir().join("temas"));
    pastas
}

/// Os de fábrica, na ordem do seletor, e depois os das `pastas`, pela chave. Um arquivo com a
/// chave de um de fábrica fica no lugar dele.
pub fn carrega(pastas: &[PathBuf]) -> Vec<Tema> {
    let mut temas: Vec<Tema> = DE_FABRICA
        .iter()
        .filter_map(|(chave, json)| Tema::le(chave, json).ok())
        .collect();
    let mut de_fora: BTreeMap<String, Tema> = BTreeMap::new();
    for pasta in pastas {
        let Ok(entradas) = std::fs::read_dir(pasta) else {
            continue;
        };
        for caminho in entradas.flatten().map(|entrada| entrada.path()) {
            if caminho.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(chave) = caminho.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let lido = std::fs::read_to_string(&caminho)
                .map_err(|erro| erro.to_string())
                .and_then(|json| Tema::le(chave, &json));
            match lido {
                Ok(tema) => {
                    de_fora.insert(chave.to_string(), tema);
                }
                Err(motivo) => crate::registro!(
                    crate::registro::Nivel::Aviso,
                    "temas",
                    "{} ignorado: {motivo}",
                    caminho.display()
                ),
            }
        }
    }
    for tema in &mut temas {
        if let Some(trocado) = de_fora.remove(&tema.chave) {
            *tema = trocado;
        }
    }
    temas.extend(de_fora.into_values());
    temas
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_de_fabrica_sao_lidos_na_ordem() {
        let temas = carrega(&[]);
        let chaves: Vec<&str> = temas.iter().map(|t| t.chave.as_str()).collect();
        assert_eq!(chaves, DE_FABRICA.map(|(chave, _)| chave));
        for tema in &temas {
            assert_eq!(tema.cores.len(), PAPEIS.len());
            for idioma in ["pt-BR", "en", "es", "es-MX"] {
                assert!(
                    tema.nomes.contains_key(idioma),
                    "{} sem nome em {idioma}",
                    tema.chave
                );
            }
        }
        assert!(!temas[0].escuro, "o primeiro é o claro");
    }

    #[test]
    fn o_nome_cai_no_ingles_e_depois_no_primeiro() {
        let rubi = carrega(&[])
            .into_iter()
            .find(|t| t.chave == "rubi")
            .unwrap();
        assert_eq!(rubi.nome("pt-BR"), "Rubi");
        assert_eq!(rubi.nome("fr"), "Ruby");
        let mut so_alemao: serde_json::Value = serde_json::from_str(DE_FABRICA[0].1).unwrap();
        so_alemao["nome"] = serde_json::json!({ "de": "Hell" });
        let tema = Tema::le("x", &so_alemao.to_string()).unwrap();
        assert_eq!(tema.nome("pt-BR"), "Hell");
    }

    #[test]
    fn um_tema_ruim_diz_o_que_falta() {
        let sem_papel = DE_FABRICA[1].1.replace(r##""mid": "#4b5056","##, "");
        assert_eq!(Tema::le("x", &sem_papel).unwrap_err(), "cores.mid faltando");
        let cor_errada = DE_FABRICA[1].1.replace("#4b5056", "cinza");
        assert!(
            Tema::le("x", &cor_errada)
                .unwrap_err()
                .contains("cores.mid")
        );
        let outro_formato = DE_FABRICA[1]
            .1
            .replace(r#""formato": 1"#, r#""formato": 2"#);
        assert!(
            Tema::le("x", &outro_formato)
                .unwrap_err()
                .contains("formato 2")
        );
    }

    /// Um arquivo na pasta acrescenta um tema, ou toma o lugar do de fábrica com o mesmo nome; um
    /// arquivo ruim é ignorado sem levar os outros junto.
    #[test]
    fn a_pasta_acrescenta_e_troca() {
        let pasta = std::env::temp_dir().join(format!("zeebx-testes-temas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&pasta);
        std::fs::create_dir_all(&pasta).unwrap();
        let roxo = DE_FABRICA[1]
            .1
            .replace(r#""pt-BR": "Escuro""#, r#""pt-BR": "Roxo""#);
        std::fs::write(pasta.join("roxo.json"), &roxo).unwrap();
        let rubi_meu = DE_FABRICA[5].1.replace("#cb3854", "#aa0000");
        std::fs::write(pasta.join("rubi.json"), rubi_meu).unwrap();
        std::fs::write(pasta.join("quebrado.json"), "{").unwrap();
        std::fs::write(pasta.join("leia-me.txt"), "não é tema").unwrap();

        let temas = carrega(std::slice::from_ref(&pasta));
        assert_eq!(temas.len(), DE_FABRICA.len() + 1);
        assert_eq!(temas.last().unwrap().nome("pt-BR"), "Roxo");
        let rubi = temas.iter().find(|t| t.chave == "rubi").unwrap();
        let destaque = rubi
            .cores
            .iter()
            .find(|(papel, _)| *papel == "highlight")
            .unwrap()
            .1;
        assert_eq!(destaque.hexa(), "#aa0000");
        let _ = std::fs::remove_dir_all(&pasta);
    }
}
