//! Procura uma versão nova nas releases do GitHub.
//!
//! A pergunta é a lista de releases, e a resposta chega numa thread, para a janela não esperar a
//! rede. A tag pode vir como `0.1.0` ou `v0.1.0`.
//!
//! **Não é `releases/latest`.** Todas as releases do Zeebx saem como pré-lançamento, e o GitHub
//! deixa pré-lançamentos de fora do `latest`: a resposta era 404, que virava "em dia", e ninguém
//! nunca foi avisado de versão nova. Da lista, sai a maior versão que não é rascunho.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// Onde as releases são publicadas.
pub const REPOSITORIO: &str = "ZeebxTeam/zeebx-emu";

/// A versão deste binário.
pub const VERSAO_ATUAL: &str = env!("CARGO_PKG_VERSION");

/// Uma release mais nova que esta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lancamento {
    /// Sem o `v`: `0.2.0`.
    pub versao: String,
    /// A página da release, com as notas e os instaladores.
    pub pagina: String,
}

/// O que a procura respondeu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resposta {
    Nova(Lancamento),
    EmDia,
    Falhou(String),
}

/// Começa a procura numa thread. A resposta chega pelo canal, uma vez.
pub fn procura() -> Receiver<Resposta> {
    let (envio, recebe) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("atualizacao".into())
        .spawn(move || {
            let resposta = match consulta() {
                Ok(Some(lancamento)) => Resposta::Nova(lancamento),
                Ok(None) => Resposta::EmDia,
                Err(erro) => Resposta::Falhou(erro),
            };
            let _ = envio.send(resposta);
        });
    recebe
}

fn consulta() -> Result<Option<Lancamento>, String> {
    let url = format!("https://api.github.com/repos/{REPOSITORIO}/releases?per_page=20");
    let agente = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut resposta = agente
        .get(&url)
        .header("User-Agent", &format!("zeebx/{VERSAO_ATUAL}"))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|erro| erro.to_string())?;
    // Sem release nenhuma a lista vem vazia, com 200. Um 404 aqui é repositório errado, e tem de
    // aparecer como falha, não como "em dia".
    let codigo = resposta.status().as_u16();
    if codigo != 200 {
        return Err(format!("o GitHub respondeu {codigo}"));
    }
    let texto = resposta
        .body_mut()
        .read_to_string()
        .map_err(|erro| erro.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&texto).map_err(|erro| erro.to_string())?;
    Ok(escolhe(&json, VERSAO_ATUAL))
}

/// A maior release da lista, se ela for mais nova que `atual`. Pré-lançamento conta: é como todas
/// as versões do Zeebx saem. Rascunho não conta — a API só os mostra a quem tem acesso de escrita,
/// mas o filtro não custa nada.
fn escolhe(json: &serde_json::Value, atual: &str) -> Option<Lancamento> {
    let maior = json
        .as_array()?
        .iter()
        .filter(|release| release["draft"].as_bool() != Some(true))
        .filter_map(|release| Some((sem_v(release["tag_name"].as_str()?), release)))
        .max_by_key(|(versao, _)| numeros(versao))?;
    let (versao, release) = maior;
    if !mais_nova(versao, atual) {
        return None;
    }
    let pagina = release["html_url"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://github.com/{REPOSITORIO}/releases/tag/{versao}"));
    Some(Lancamento { versao: versao.to_string(), pagina })
}

fn sem_v(tag: &str) -> &str {
    let tag = tag.trim();
    tag.strip_prefix(['v', 'V']).unwrap_or(tag)
}

/// Os números de uma versão, `1.2.3` → `[1, 2, 3]`. O que vem depois de `-` ou `+` não conta, e
/// uma parte que não é número vale zero.
fn numeros(versao: &str) -> [u64; 3] {
    let nucleo = sem_v(versao).split(['-', '+']).next().unwrap_or("");
    let mut partes = nucleo.split('.').map(|p| p.trim().parse::<u64>().unwrap_or(0));
    [
        partes.next().unwrap_or(0),
        partes.next().unwrap_or(0),
        partes.next().unwrap_or(0),
    ]
}

/// Se `candidata` é mais nova que `atual`.
pub fn mais_nova(candidata: &str, atual: &str) -> bool {
    numeros(candidata) > numeros(atual)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pergunta ao GitHub de verdade: `cargo test --release consulta_de_verdade -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn consulta_de_verdade() {
        println!("{:?}", consulta());
    }

    #[test]
    fn compara_as_versoes_com_e_sem_v() {
        assert!(mais_nova("v0.2.0", "0.1.0"));
        assert!(mais_nova("0.1.1", "0.1.0"));
        assert!(mais_nova("1.0.0", "0.9.9"));
        assert!(mais_nova("0.10.0", "0.9.0"));
        assert!(!mais_nova("v0.1.0", "0.1.0"));
        assert!(!mais_nova("0.0.9", "0.1.0"));
    }

    fn release(tag: &str, rascunho: bool, pre: bool) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("https://github.com/{REPOSITORIO}/releases/tag/{tag}"),
            "draft": rascunho,
            "prerelease": pre,
        })
    }

    #[test]
    fn escolhe_a_maior_da_lista_mesmo_em_pre_lancamento() {
        // A ordem da API é por data, não por versão: a maior pode não vir primeiro.
        let lista = serde_json::json!([
            release("v0.4.0", false, true),
            release("v0.4.1", false, true),
            release("v0.3.0", false, true),
        ]);
        assert_eq!(
            escolhe(&lista, "0.4.0"),
            Some(Lancamento {
                versao: "0.4.1".into(),
                pagina: format!("https://github.com/{REPOSITORIO}/releases/tag/v0.4.1"),
            })
        );
        assert_eq!(escolhe(&lista, "0.4.1"), None);
    }

    #[test]
    fn ignora_rascunho_e_lista_vazia() {
        let lista = serde_json::json!([
            release("v0.5.0", true, false),
            release("v0.4.1", false, true),
        ]);
        assert_eq!(escolhe(&lista, "0.4.1"), None);
        assert_eq!(escolhe(&serde_json::json!([]), "0.1.0"), None);
    }
}
