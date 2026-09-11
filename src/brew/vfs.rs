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
//! fs:/zeeboiddata/x    **o sistema de arquivos do aparelho**, comum a todos
//! ```
//!
//! # O `fs:/` é do aparelho, não do jogo
//!
//! Esta distinção parece detalhe e não é. No console há **um** sistema de arquivos: o Zeeboids
//! grava os bonecos em `fs:/zeeboiddata/zeeboid.db` e o Zeebo F.C. abre esse mesmo caminho para
//! importá-los. Enquanto `fs:/` caía dentro da pasta do módulo, cada jogo via uma pasta só sua
//! e vazia — e o F.C. concluía, corretamente do ponto de vista dele, que o Zeeboids não estava
//! instalado.
//!
//! Então: caminho relativo e `fs:/~/` vão para a pasta do módulo; `fs:/` vai para uma raiz
//! comum, que é o que o console tem.

use std::path::{Path, PathBuf};

/// Prefixos que o BREW usa e que removemos antes de resolver.
const PREFIXES: [&str; 4] = ["fs:/~/", "fs:/~", "fs:/", "~/"];

/// Tira o `<classe>/` que pode vir logo depois do `~`.
///
/// O BREW escreve o diretório de um módulo como `fs:/~<ClassID>/`, e a Z-Wheel usa essa forma
/// para os próprios recursos: `fs:/~0x01070798/tectoyli.brf`. Sem tirar o número, o caminho
/// virava um **subdiretório** com nome `0x01070798` dentro do diretório do módulo — que não
/// existe. O arquivo estava no pacote e mesmo assim não abria.
///
/// Foi o que segurou o formulário de instruções do z-pad da Z-Wheel: ele carrega um recurso do
/// `tectoyli.brf` por esse caminho, não achava, e devolvia `EUNABLETOLOAD` — o erro 6 que
/// aparecia milhares de vezes no relatório.
///
/// **Aqui só existe um módulo por vez**, então qualquer ClassID leva ao diretório dele. No
/// console um módulo pode alcançar o diretório de outro por este caminho; quando isso importar,
/// é aqui que se resolve o número para o diretório certo.
fn sem_classe(resto: &str) -> String {
    let Some((primeiro, cauda)) = resto.split_once('/') else {
        return resto.to_string();
    };
    let numero = primeiro
        .strip_prefix("0x")
        .or_else(|| primeiro.strip_prefix("0X"))
        .map(|hex| !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| !primeiro.is_empty() && primeiro.chars().all(|c| c.is_ascii_digit()));
    match numero {
        true => cauda.to_string(),
        false => resto.to_string(),
    }
}
/// Nome da raiz comum, que faz o papel do sistema de arquivos do aparelho.
const DEVICE_DIR: &str = "aparelho";

#[derive(Debug)]
pub struct Vfs {
    /// Diretório do módulo — onde caem os caminhos relativos e o `~`.
    root: PathBuf,
    /// A raiz comum, que faz o papel do sistema de arquivos do aparelho.
    device: PathBuf,
    /// Até onde o `..` pode subir. No console é a raiz de módulos: o Quake mantém os dados
    /// dele em `mod/id1/`, ao lado do diretório do próprio módulo, e chega lá por
    /// `fs:/~/../id1/`.
    boundary: PathBuf,
}

impl Vfs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root: PathBuf = root.into();
        let boundary = root.parent().map(Path::to_path_buf).unwrap_or(root.clone());
        // A raiz comum fica ao lado das pastas de módulo, dentro da instalação do jogo. Um nível
        // acima disso ficaria fora do que cada jogo pode alcançar pelo `..`.
        let device = boundary.join(DEVICE_DIR);
        Self {
            root,
            boundary,
            device,
        }
    }

    /// Aponta a raiz comum para outro lugar.
    ///
    /// Existe para que **todos os jogos** compartilhem o mesmo sistema de arquivos, e não uma
    /// cópia por instalação: é assim que o Zeebo F.C. encontra os bonecos que o Zeeboids gravou.
    pub fn set_device_root(&mut self, device: impl Into<PathBuf>) {
        self.device = device.into();
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Arquivo de estado criado pelo emulador para um aplicativo do console.
    ///
    /// Não passa pelo caminho do módulo: é estado do aparelho, portanto sobrevive a uma nova
    /// extração do ZIP e nunca altera o pacote original.
    pub fn profile_file(&self, app: &str, name: &str) -> PathBuf {
        self.device.join(app).join(name)
    }

    /// Traduz um caminho do guest para um caminho do host.
    ///
    /// Devolve `None` para qualquer caminho que escape da raiz — `..`, caminho absoluto do
    /// host ou raiz do Windows. O jogo não tem por que sair do diretório dele, e um `.mod` de
    /// origem desconhecida não deveria conseguir ler o resto da máquina.
    pub fn resolve(&self, guest_path: &str) -> Option<PathBuf> {
        self.resolve_inner(guest_path, false).map(match_case)
    }

    /// **Sobre o `preloaded.cfg`.**
    ///
    /// Ele lista os jogos que vêm de fábrica no aparelho, e nem o pacote da Z-Wheel nem o
    /// sistema de arquivos do dump o têm. O [`Machine`](crate::machine::Machine) o materializa
    /// no perfil persistente do aparelho quando a Z-Wheel o consulta, em vez de alterar o
    /// pacote original. A lista pode começar vazia: jogos locais pertencem ao banco da
    /// biblioteca, não a uma NAND inventada.
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
                path = sem_classe(rest);
                break;
            }
        }
        // O que sobrou depois de tirar o prefixo diz onde a busca começa. Um caminho que veio
        // com `fs:/` e **não** era `fs:/~` é do aparelho, não do módulo.
        let do_aparelho = guest_path.starts_with("fs:/") && !guest_path.starts_with("fs:/~");
        let mut resolved = match do_aparelho {
            true => self.device.clone(),
            false => self.root.clone(),
        };
        let base = resolved.clone();
        for part in path.split('/') {
            match part {
                "" | "." => continue,
                // Subir é permitido até a raiz de módulos e nem um passo além.
                ".." => {
                    // Do aparelho não se sobe: ele já é a raiz. Da pasta do módulo sobe-se até
                    // a raiz de módulos, que é como o Quake alcança os dados dele.
                    let limite = match do_aparelho {
                        true => &base,
                        false => &self.boundary,
                    };
                    if resolved == *limite || !resolved.pop() {
                        return None;
                    }
                }
                _ if part.contains(':') => return None,
                _ => resolved.push(part),
            }
        }
        if !allow_root && (resolved == base || resolved == self.boundary) {
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
    fn o_til_e_do_modulo_e_o_resto_e_do_aparelho() {
        let vfs = vfs();
        // Com `~`, e sem prefixo nenhum, é a pasta do jogo.
        let do_jogo = Some(PathBuf::from("/jogos/bjt/dados.dat"));
        assert_eq!(vfs.resolve("fs:/~/dados.dat"), do_jogo);
        assert_eq!(vfs.resolve("~/dados.dat"), do_jogo);

        // Sem o `~`, é o sistema de arquivos do aparelho — comum a todos os jogos. É o que
        // permite ao Zeebo F.C. abrir `fs:/zeeboiddata/zeeboid.db`, que o Zeeboids gravou.
        assert_eq!(
            vfs.resolve("fs:/dados.dat"),
            Some(PathBuf::from("/jogos/aparelho/dados.dat"))
        );
        assert_eq!(
            vfs.resolve("fs:/zeeboiddata/zeeboid.db"),
            Some(PathBuf::from("/jogos/aparelho/zeeboiddata/zeeboid.db"))
        );
    }

    #[test]
    fn aceita_barra_invertida_como_separador() {
        assert_eq!(
            vfs().resolve("imagens\\fundo.bmp"),
            Some(PathBuf::from("/jogos/bjt/imagens/fundo.bmp"))
        );
    }

    #[test]
    fn a_area_compartilhada_e_do_aparelho() {
        assert_eq!(
            vfs().resolve("fs:/shared/save.dat"),
            Some(PathBuf::from("/jogos/aparelho/shared/save.dat"))
        );
    }

    #[test]
    fn do_aparelho_nao_se_sobe() {
        // A raiz do aparelho é raiz: `..` a partir dela sairia para a máquina do usuário.
        let vfs = vfs();
        assert_eq!(vfs.resolve("fs:/../x"), None);
        assert_eq!(vfs.resolve("fs:/zeeboiddata/../../x"), None);
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
