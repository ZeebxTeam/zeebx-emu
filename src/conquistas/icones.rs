//! Os ícones das conquistas, baixados do servidor de mídia do RA e guardados em disco.
//!
//! O RA dá cada ícone por um endereço (`https://media.retroachievements.org/Badge/00234.png`), e o
//! mesmo ícone aparece no aviso de desbloqueio, na lista e de novo na próxima partida. Guardar o
//! arquivo poupa a rede e deixa o aviso aparecer com o ícone já na primeira vez em que ele está
//! no cache. Quem pede recebe o caminho quando o arquivo existe, e `None` enquanto ele baixa.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

/// Onde os ícones ficam: `<config>/conquistas/icones`.
pub fn pasta_padrao() -> PathBuf {
    crate::config::config_dir()
        .join("conquistas")
        .join("icones")
}

/// Um download que terminou: o endereço, e o arquivo ou o motivo da falha.
type Chegada = (String, Result<PathBuf, String>);

enum Estado {
    Baixando,
    Pronto(PathBuf),
    /// Não baixou. Não se tenta de novo nesta execução: um endereço quebrado não vira um
    /// download por quadro.
    Falhou,
}

pub struct Icones {
    pasta: PathBuf,
    estados: HashMap<String, Estado>,
    chegadas: (Sender<Chegada>, Receiver<Chegada>),
}

impl Icones {
    pub fn novo(pasta: impl Into<PathBuf>) -> Self {
        Self {
            pasta: pasta.into(),
            estados: HashMap::new(),
            chegadas: mpsc::channel(),
        }
    }

    /// O arquivo do ícone de `url`, se ele já está em disco. Se não está, começa a baixar e
    /// devolve `None`; o [`Icones::recolhe`] diz quando chegou.
    pub fn caminho(&mut self, url: &str) -> Option<PathBuf> {
        match self.estados.get(url) {
            Some(Estado::Pronto(caminho)) => return Some(caminho.clone()),
            Some(Estado::Baixando | Estado::Falhou) => return None,
            None => {}
        }
        let destino = self.pasta.join(nome_do_arquivo(url)?);
        if destino.is_file() {
            self.estados
                .insert(url.to_string(), Estado::Pronto(destino.clone()));
            return Some(destino);
        }
        self.estados.insert(url.to_string(), Estado::Baixando);
        let envio = self.chegadas.0.clone();
        let url = url.to_string();
        let _ = std::thread::Builder::new()
            .name("conquistas-icone".into())
            .spawn(move || {
                let resultado = super::http::baixa(&url, &destino).map(|()| destino);
                let _ = envio.send((url, resultado));
            });
        None
    }

    /// Recolhe os downloads que terminaram. `true` se algum ícone novo ficou pronto.
    pub fn recolhe(&mut self) -> bool {
        let mut chegou = false;
        while let Ok((url, resultado)) = self.chegadas.1.try_recv() {
            let estado = match resultado {
                Ok(caminho) => {
                    chegou = true;
                    Estado::Pronto(caminho)
                }
                Err(erro) => {
                    crate::registro!(
                        crate::registro::Nivel::Aviso,
                        "conquistas",
                        "o ícone {url} não baixou: {erro}"
                    );
                    Estado::Falhou
                }
            };
            self.estados.insert(url, estado);
        }
        chegou
    }
}

/// O nome em disco: o último trecho do endereço, só com o que vale em nome de arquivo em qualquer
/// sistema. `None` para um endereço sem nome que preste.
fn nome_do_arquivo(url: &str) -> Option<String> {
    let ultimo = url.split(['?', '#']).next()?.rsplit('/').next()?;
    let limpo: String = ultimo
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    (!limpo.is_empty() && !limpo.starts_with('.')).then_some(limpo)
}

/// O endereço `file://` de um caminho, para o QML.
pub fn endereco(caminho: &Path) -> String {
    let texto = caminho.to_string_lossy().replace('\\', "/");
    match texto.starts_with('/') {
        true => format!("file://{texto}"),
        false => format!("file:///{texto}"),
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn o_nome_do_arquivo_e_o_fim_do_endereco_limpo() {
        assert_eq!(
            nome_do_arquivo("https://media.retroachievements.org/Badge/00234.png").as_deref(),
            Some("00234.png")
        );
        assert_eq!(
            nome_do_arquivo("https://media.retroachievements.org/Badge/00234_lock.png?v=2")
                .as_deref(),
            Some("00234_lock.png")
        );
        assert_eq!(nome_do_arquivo("https://x/../"), None);
        assert_eq!(nome_do_arquivo("https://x/..").as_deref(), None);
    }

    #[test]
    fn o_icone_em_disco_nao_e_baixado_de_novo() {
        let pasta = std::env::temp_dir().join(format!("zeebx-icones-{}", std::process::id()));
        std::fs::create_dir_all(&pasta).unwrap();
        std::fs::write(pasta.join("00234.png"), b"png").unwrap();
        let mut icones = Icones::novo(&pasta);
        assert_eq!(
            icones.caminho("https://media.retroachievements.org/Badge/00234.png"),
            Some(pasta.join("00234.png"))
        );
        std::fs::remove_dir_all(&pasta).unwrap();
    }

    #[test]
    fn o_endereco_de_arquivo_vale_nos_dois_sistemas() {
        assert_eq!(endereco(Path::new("/a/b.png")), "file:///a/b.png");
        assert_eq!(endereco(Path::new("C:\\a\\b.png")), "file:///C:/a/b.png");
    }
}
