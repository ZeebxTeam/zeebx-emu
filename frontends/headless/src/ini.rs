//! Um leitor de INI, só o que o arquivo de configuração usa.
//!
//! Escrito à mão, e não trazido de uma caixa, pela mesma razão que o [`config_dir`] do núcleo é:
//! são três regras — seção entre colchetes, `chave = valor`, e `#` ou `;` começam comentário —
//! e cada uma cabe numa linha. A dependência custaria mais para auditar do que este arquivo
//! inteiro.
//!
//! [`config_dir`]: zeebx::ui::settings::config_dir

use std::collections::BTreeMap;

/// O arquivo lido: uma tabela de seções, cada uma com as suas chaves.
///
/// A ordem não importa para quem lê, e a `BTreeMap` dá de graça o que importa: procurar por
/// nome, e listar o que sobrou para avisar sobre chave escrita errada.
#[derive(Debug, Default)]
pub struct Ini {
    secoes: BTreeMap<String, BTreeMap<String, Valor>>,
}

/// Um valor e a linha em que ele estava, para a mensagem de erro poder dizer onde.
#[derive(Debug, Clone)]
pub struct Valor {
    pub texto: String,
    pub linha: usize,
}

impl Ini {
    /// Lê o texto inteiro. Nunca falha: uma linha que não é seção nem `chave = valor` vira um
    /// aviso, não uma recusa — um arquivo escrito por uma versão mais nova precisa deixar o
    /// emulador abrir, como o `settings.json` já faz.
    pub fn ler(texto: &str) -> (Self, Vec<String>) {
        let mut ini = Self::default();
        let mut avisos = Vec::new();
        // Antes da primeira seção, as chaves caem numa seção sem nome. Serve para um arquivo de
        // uma linha só, sem cerimônia.
        let mut secao = String::new();

        for (numero, linha) in texto.lines().enumerate() {
            let numero = numero + 1;
            let limpa = sem_comentario(linha).trim();
            if limpa.is_empty() {
                continue;
            }
            if let Some(nome) = limpa.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                secao = nome.trim().to_lowercase();
                ini.secoes.entry(secao.clone()).or_default();
                continue;
            }
            let Some((chave, valor)) = limpa.split_once('=') else {
                avisos.push(format!("line {numero}: neither a section nor `key = value`: {limpa}"));
                continue;
            };
            let chave = chave.trim().to_lowercase();
            if chave.is_empty() {
                avisos.push(format!("line {numero}: empty key"));
                continue;
            }
            ini.secoes.entry(secao.clone()).or_default().insert(
                chave,
                Valor {
                    texto: valor.trim().to_string(),
                    linha: numero,
                },
            );
        }
        (ini, avisos)
    }

    /// O valor de uma chave, ou `None` se a seção ou a chave não existem.
    ///
    /// Consultar **consome**: o que sobrar no fim é chave que ninguém leu, e é isso que o
    /// [`Ini::sobras`] avisa. Um `resolucao_intern` escrito sem o `a` ficaria silenciosamente
    /// no padrão, e quem configurou não teria como descobrir.
    pub fn pega(&mut self, secao: &str, chave: &str) -> Option<Valor> {
        self.secoes.get_mut(secao)?.remove(chave)
    }

    /// Se a seção existe no arquivo, mesmo vazia.
    pub fn tem_secao(&self, secao: &str) -> bool {
        self.secoes.contains_key(secao)
    }

    /// As seções presentes cujo nome começa com um prefixo, em ordem. É como as portas são
    /// achadas: `porta1`, `porta2`.
    pub fn secoes_com(&self, prefixo: &str) -> Vec<String> {
        self.secoes
            .keys()
            .filter(|nome| nome.starts_with(prefixo))
            .cloned()
            .collect()
    }

    /// O que ninguém leu, com a linha de cada um. Chave escrita errada aparece aqui.
    pub fn sobras(&self) -> Vec<String> {
        let mut sobras = Vec::new();
        for (secao, chaves) in &self.secoes {
            for (chave, valor) in chaves {
                let onde = match secao.is_empty() {
                    true => chave.clone(),
                    false => format!("[{secao}] {chave}"),
                };
                sobras.push(format!("line {}: nobody uses `{onde}`", valor.linha));
            }
        }
        sobras
    }
}

/// Corta o comentário, respeitando o que está entre aspas.
///
/// As aspas importam: o nome de um controle pode ter `#` — e, mais comum, um caminho de
/// Windows não pode perder nada. Fora das aspas, `#` e `;` começam comentário em qualquer
/// posição, que é como todo INI se comporta.
fn sem_comentario(linha: &str) -> &str {
    let mut aspas = false;
    for (i, c) in linha.char_indices() {
        match c {
            '"' => aspas = !aspas,
            '#' | ';' if !aspas => return &linha[..i],
            _ => {}
        }
    }
    linha
}

/// Tira as aspas de um valor, se ele estiver inteiro entre elas.
pub fn sem_aspas(texto: &str) -> &str {
    match texto.len() >= 2 && texto.starts_with('"') && texto.ends_with('"') {
        true => &texto[1..texto.len() - 1],
        false => texto,
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn seccoes_chaves_e_comentarios() {
        let (mut ini, avisos) = Ini::ler(
            "# um comentário\n\
             [Video]\n\
             modo = tela_cheia   ; e outro\n\
             \n\
             [audio]\n\
             volume=80\n",
        );
        assert!(avisos.is_empty(), "{avisos:?}");
        assert_eq!(ini.pega("video", "modo").unwrap().texto, "tela_cheia");
        assert_eq!(ini.pega("audio", "volume").unwrap().texto, "80");
        assert!(ini.pega("audio", "volume").is_none());
    }

    /// O `#` de um nome de controle não pode virar comentário.
    #[test]
    fn aspas_seguram_o_comentario() {
        let (mut ini, _) = Ini::ler("[porta1]\ncontrole = \"Pad #2\"\n");
        let valor = ini.pega("porta1", "controle").unwrap();
        assert_eq!(sem_aspas(&valor.texto), "Pad #2");
    }

    /// Chave escrita errada precisa aparecer, senão ela fica no padrão em silêncio.
    #[test]
    fn a_chave_nao_lida_sobra() {
        let (mut ini, _) = Ini::ler("[grafico]\nresolucao_intern = 2\n");
        assert!(ini.pega("grafico", "resolucao_interna").is_none());
        let sobras = ini.sobras();
        assert_eq!(sobras.len(), 1);
        assert!(sobras[0].contains("resolucao_intern"), "{sobras:?}");
    }

    #[test]
    fn linha_estragada_vira_aviso_e_o_resto_vale() {
        let (mut ini, avisos) = Ini::ler("[audio]\nisto não é chave\nvolume = 50\n");
        assert_eq!(avisos.len(), 1);
        assert_eq!(ini.pega("audio", "volume").unwrap().texto, "50");
    }
}
