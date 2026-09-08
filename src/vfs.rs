//! Sistema de arquivos visto pelo jogo.
//!
//! O BREW dá a cada módulo um diretório próprio, e é para lá que os caminhos do jogo apontam.
//! Aqui traduzimos os caminhos do guest para o diretório do módulo no host, recusando qualquer
//! coisa que tente sair dele.
//!
//! Formas de caminho que aparecem nos jogos e no SDK:
//!
//! ```text
//! arquivo.dat          relativo ao diretório do módulo
//! fs:/~/arquivo.dat    o "~" é o diretório do próprio módulo
//! fs:/~/../id1/x       sobe para a raiz de módulos — o Quake guarda os dados dele assim
//! fs:/~0x01234567/x    diretório de outro módulo, pelo ClassID
//! fs:/shared/x         área compartilhada entre aplicativos
//! ```

use std::path::{Path, PathBuf};

/// Prefixos que o BREW usa e que removemos antes de resolver.
const PREFIXES: [&str; 4] = ["fs:/~/", "fs:/~", "fs:/", "~/"];
/// Nome do diretório usado para a área compartilhada, dentro da raiz do módulo.
const SHARED_DIR: &str = "shared";

#[derive(Debug)]
pub struct Vfs {
    /// Diretório do módulo — onde caem os caminhos relativos e o `~`.
    root: PathBuf,
    /// Até onde o `..` pode subir. No console é a raiz de módulos: o Quake mantém os dados
    /// dele em `mod/id1/`, ao lado do diretório do próprio módulo, e chega lá por
    /// `fs:/~/../id1/`.
    boundary: PathBuf,
}

impl Vfs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root: PathBuf = root.into();
        let boundary = root.parent().map(Path::to_path_buf).unwrap_or(root.clone());
        Self { root, boundary }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Traduz um caminho do guest para um caminho do host.
    ///
    /// Devolve `None` para qualquer caminho que escape da raiz — `..`, caminho absoluto do
    /// host ou raiz do Windows. O jogo não tem por que sair do diretório dele, e um `.mod` de
    /// origem desconhecida não deveria conseguir ler o resto da máquina.
    pub fn resolve(&self, guest_path: &str) -> Option<PathBuf> {
        self.resolve_inner(guest_path, false).map(match_case)
    }

    /// Como [`Vfs::resolve`], mas sem a busca sem caixa.
    ///
    /// É para quem vai **criar** um nome, não abrir um existente: renomear para `save.dat`
    /// quando existe um `SAVE.DAT` tem de criar o primeiro, não sobrescrever o segundo.
    pub fn resolve_new(&self, guest_path: &str) -> Option<PathBuf> {
        self.resolve_inner(guest_path, false)
    }

    /// Como [`Vfs::resolve`], mas aceita o próprio diretório do módulo.
    ///
    /// Listar `fs:/~/` é legítimo — é o diretório do jogo —, ainda que abri-lo como arquivo não
    /// seja. A diferença está só nisso.
    pub fn resolve_dir(&self, guest_path: &str) -> Option<PathBuf> {
        self.resolve_inner(guest_path, true).map(match_case)
    }

    fn resolve_inner(&self, guest_path: &str, allow_root: bool) -> Option<PathBuf> {
        let mut path = guest_path.replace('\\', "/");
        for prefix in PREFIXES {
            if let Some(rest) = path.strip_prefix(prefix) {
                path = rest.to_string();
                break;
            }
        }
        // `fs:/shared/x` vira `shared/x` dentro da raiz do módulo.
        let path = path
            .strip_prefix("shared/")
            .map_or(path.clone(), |rest| format!("{SHARED_DIR}/{rest}"));

        let mut resolved = self.root.clone();
        for part in path.split('/') {
            match part {
                "" | "." => continue,
                // Subir é permitido até a raiz de módulos e nem um passo além.
                ".." => {
                    if resolved == self.boundary || !resolved.pop() {
                        return None;
                    }
                }
                _ if part.contains(':') => return None,
                _ => resolved.push(part),
            }
        }
        if !allow_root && (resolved == self.root || resolved == self.boundary) {
            return None;
        }
        Some(resolved)
    }
}

/// O mesmo caminho, com a caixa que os arquivos têm de verdade no disco.
///
/// O sistema de arquivos do console não distingue maiúsculas de minúsculas, e os jogos contam
/// com isso: os dez ports de arcade do Zeebo pedem `font.fnz` e trazem `font.FNZ` no pacote.
/// Nenhum deles passava do `EVT_APP_START` — a fonte não abria e o ponteiro nulo vinha logo
/// depois. No Windows e no macOS isso funcionava por acaso; no Linux, não.
///
/// Um caminho que não existe volta como veio: é o caso de um arquivo que o jogo está criando.
fn match_case(path: PathBuf) -> PathBuf {
    if path.exists() {
        return path;
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return path;
    };
    // O diretório também pode estar com outra caixa, então ele é resolvido primeiro.
    let parent = match_case(parent.to_path_buf());
    let exact = parent.join(name);
    if exact.exists() {
        return exact;
    }
    let Ok(entries) = std::fs::read_dir(&parent) else {
        return exact;
    };
    let alvo = name.to_string_lossy().to_lowercase();
    // Havendo mais de um candidato — dois arquivos que só diferem na caixa —, fica o primeiro
    // em ordem alfabética, para a escolha não depender da ordem em que o disco os devolve.
    entries
        .flatten()
        .map(|entry| entry.file_name())
        .filter(|nome| nome.to_string_lossy().to_lowercase() == alvo)
        .min()
        .map_or(exact, |nome| parent.join(nome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_diretorio_do_modulo_pode_ser_listado_mas_nao_aberto() {
        // Listar `fs:/~/` é legítimo — é a pasta do jogo. Abri-la como arquivo não é, e a
        // diferença entre as duas respostas é justamente essa.
        let vfs = Vfs::new("/roms/jogo");
        assert_eq!(vfs.resolve("fs:/~/"), None);
        assert_eq!(
            vfs.resolve_dir("fs:/~/"),
            Some(std::path::PathBuf::from("/roms/jogo"))
        );
        // Sair da raiz continua proibido nos dois.
        assert_eq!(vfs.resolve_dir("fs:/~/../../etc"), None);
    }

    fn vfs() -> Vfs {
        Vfs::new("/jogos/bjt")
    }

    #[test]
    fn caminho_relativo_cai_na_raiz_do_modulo() {
        assert_eq!(
            vfs().resolve("dados.dat"),
            Some(PathBuf::from("/jogos/bjt/dados.dat"))
        );
    }

    #[test]
    fn remove_os_prefixos_do_brew() {
        let vfs = vfs();
        let esperado = Some(PathBuf::from("/jogos/bjt/dados.dat"));
        assert_eq!(vfs.resolve("fs:/~/dados.dat"), esperado);
        assert_eq!(vfs.resolve("fs:/dados.dat"), esperado);
        assert_eq!(vfs.resolve("~/dados.dat"), esperado);
    }

    #[test]
    fn aceita_barra_invertida_como_separador() {
        assert_eq!(
            vfs().resolve("imagens\\fundo.bmp"),
            Some(PathBuf::from("/jogos/bjt/imagens/fundo.bmp"))
        );
    }

    #[test]
    fn area_compartilhada_fica_dentro_da_raiz() {
        assert_eq!(
            vfs().resolve("fs:/shared/save.dat"),
            Some(PathBuf::from("/jogos/bjt/shared/save.dat"))
        );
    }

    #[test]
    fn sobe_ate_a_raiz_de_modulos_mas_nao_alem() {
        let vfs = Vfs::new("/jogos/mod/274802");
        // O Quake guarda os dados dele num diretório irmão.
        assert_eq!(
            vfs.resolve("fs:/~/../id1/splash_title.png"),
            Some(PathBuf::from("/jogos/mod/id1/splash_title.png"))
        );
        // Mais um nível já sairia da árvore de módulos.
        assert_eq!(vfs.resolve("fs:/~/../../outro/x"), None);
        assert_eq!(vfs.resolve("../../../etc/passwd"), None);
    }

    #[test]
    fn recusa_qualquer_tentativa_de_sair_da_raiz() {
        let vfs = vfs();
        // Um `..` chega à raiz de módulos, que é legítimo; dois já sairiam dela.
        assert_eq!(
            vfs.resolve("dados/../../fora.txt"),
            Some(PathBuf::from("/jogos/fora.txt"))
        );
        assert_eq!(vfs.resolve("../../etc/passwd"), None);
        assert_eq!(vfs.resolve("fs:/~/../../etc/passwd"), None);
        assert_eq!(vfs.resolve("C:/Windows/system32"), None);
    }

    #[test]
    fn caminho_vazio_nao_resolve_para_a_propria_raiz() {
        let vfs = vfs();
        assert_eq!(vfs.resolve(""), None);
        assert_eq!(vfs.resolve("fs:/~"), None);
    }
}

#[cfg(test)]
mod tests_no_disco {
    use super::*;

    /// Os dez ports de arcade pedem `font.fnz` e trazem `font.FNZ`. No console dá na mesma.
    #[test]
    fn acha_o_arquivo_com_outra_caixa() {
        let raiz = std::env::temp_dir().join(format!("zeebx-vfs-{}", std::process::id()));
        let modulo = raiz.join("mod/279233");
        std::fs::create_dir_all(&modulo).unwrap();
        std::fs::write(modulo.join("font.FNZ"), b"fonte").unwrap();

        let vfs = Vfs::new(&modulo);
        assert_eq!(vfs.resolve("font.fnz"), Some(modulo.join("font.FNZ")));
        // O nome exato continua ganhando de qualquer outro.
        std::fs::write(modulo.join("font.fnz"), b"outra").unwrap();
        assert_eq!(vfs.resolve("font.fnz"), Some(modulo.join("font.fnz")));
        // O que não existe volta como veio: é assim que um arquivo novo é criado.
        assert_eq!(vfs.resolve("save.dat"), Some(modulo.join("save.dat")));

        std::fs::remove_dir_all(&raiz).unwrap();
    }
}
