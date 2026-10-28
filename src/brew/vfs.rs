//! Sistema de arquivos visto pelo jogo.
//!
//! O jogo nunca vê um caminho do host. O que ele pede é traduzido, **só como texto**, para um
//! caminho do sistema de arquivos virtual do console; o `..` é resolvido ali e para na raiz
//! virtual. Só depois esse caminho virtual vira caminho do host, por uma de duas camadas:
//!
//! | Camada | Virtual | Host | Acesso |
//! |---|---|---|---|
//! | pacote | `/mod/...` | a pasta `mod/` do pacote — ou só a do módulo, se ele veio solto | leitura |
//! | gravável | `/mod/...` | o store de saves, comum a todos os jogos | leitura e escrita |
//! | gravável | o resto de `/` | o aparelho, comum a todos os jogos | leitura e escrita |
//!
//! A leitura procura primeiro na camada gravável e depois no pacote; a escrita vai sempre para a
//! gravável. É o que o console tem: o pacote instalado embaixo e, por cima, o que os jogos
//! gravaram. Nada que um jogo peça chega a outro pacote, à biblioteca de ROMs ou ao resto da
//! máquina — e o pacote dele mesmo não pode ser alterado nem apagado.
//!
//! Formas de caminho que aparecem nos jogos e no SDK:
//!
//! ```text
//! arquivo.dat          relativo ao diretório do módulo: /mod/<módulo>/arquivo.dat
//! fs:/~/arquivo.dat    o "~" é o diretório do próprio módulo
//! fs:/~/../id1/x       /mod/id1/x — o Quake guarda os dados dele num diretório irmão
//! fs:/~0x01234567/x    diretório de um módulo pelo ClassID
//! fs:/shared/x         área compartilhada entre aplicativos
//! fs:/zeeboiddata/x    **o sistema de arquivos do aparelho**, comum a todos
//! fs:/mod/<pasta>/x    o diretório de um módulo pelo caminho absoluto
//! ```
//!
//! # O `fs:/` é do aparelho, não do jogo
//!
//! No console há **um** sistema de arquivos: o Zeeboids grava os bonecos em
//! `fs:/zeeboiddata/zeeboid.db` e o Zeebo F.C. abre esse mesmo caminho para importá-los. Pelo
//! mesmo motivo o store de saves é um só: um jogo que procura o save de outro em
//! `fs:/mod/<pasta>/` encontra o que esse outro gravou.

use std::path::{Component, Path, PathBuf};

/// Prefixos que o BREW usa e que removemos antes de resolver, com o que cada um diz: se o
/// caminho é do aparelho e se pode trazer o ClassID logo depois do `~`.
const PREFIXES: [(&str, bool); 4] = [
    ("fs:/~/", false),
    ("fs:/~", false),
    ("fs:/", true),
    ("~/", false),
];

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
/// é aqui que se resolve o número para o diretório certo. O nome da pasta não serve de atalho:
/// a da Z-Wheel é `274755` e o ClassID dela é `0x01070798`.
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
/// Onde os módulos ficam instalados, tanto no aparelho quanto no pacote.
const MOD_DIR: &str = "mod";

/// Para que serve uma abertura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenIntent {
    /// Ler um arquivo que precisa existir.
    Read,
    /// Criar um nome novo, sem sobrescrever um existente de caixa diferente.
    Create,
    /// Ler e escrever o mesmo arquivo, preservando o conteúdo atual.
    ReadWrite,
    /// Acrescentar ao fim do arquivo.
    Append,
}

/// Onde abrir, e a cópia que precisa preceder a escrita.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTarget {
    /// Caminho do host a abrir.
    pub path: PathBuf,
    /// Arquivo do pacote a copiar para `path` antes de escrever, no copy-on-write.
    pub copy_from: Option<PathBuf>,
}

/// Um caminho do sistema de arquivos virtual: os componentes a partir de `/`, já sem `.`, `..`
/// nem barra vazia.
type Virtual = Vec<String>;

#[derive(Debug)]
pub struct Vfs {
    /// Diretório do módulo no host: pacote extraído ou pasta do `.mod` solto. Nunca recebe
    /// escrita.
    root: PathBuf,
    /// Nome com que o módulo aparece em `/mod/<nome>/`: a pasta dele.
    module: String,
    /// A pasta `mod/` do pacote, quando o módulo está numa.
    ///
    /// É ela que deixa o Quake ler `mod/id1/` e o NFS `mod/nfsresources/`, que vêm no mesmo
    /// zip. Um `.mod` solto não tem pacote em volta, e as pastas vizinhas dele são do usuário:
    /// nesse caso só o diretório do próprio módulo fica visível.
    package_mods: Option<PathBuf>,
    /// O store de saves, que recebe tudo o que os jogos gravam em `/mod/`. Sem ele, a pasta
    /// `mod/` do aparelho.
    saves: Option<PathBuf>,
    /// A raiz comum, que faz o papel do sistema de arquivos do aparelho.
    device: PathBuf,
}

impl Vfs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root: PathBuf = root.into();
        let module = root
            .file_name()
            .map(|nome| nome.to_string_lossy().into_owned())
            .unwrap_or_else(|| "modulo".to_string());
        let pai = root.parent().map(Path::to_path_buf);
        let package_mods = pai
            .clone()
            .filter(|pai| pai.file_name().is_some_and(|n| n.eq_ignore_ascii_case(MOD_DIR)));
        let device = pai.unwrap_or_else(|| root.clone()).join(DEVICE_DIR);
        Self {
            root,
            module,
            package_mods,
            saves: None,
            device,
        }
    }

    /// Aponta o store de saves, que recebe o que os jogos gravam em `/mod/`.
    pub fn set_save_root(&mut self, save: impl Into<PathBuf>) {
        self.saves = Some(save.into());
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

    fn saves_dir(&self) -> PathBuf {
        self.saves
            .clone()
            .unwrap_or_else(|| self.device.join(MOD_DIR))
    }

    /// Traduz o caminho do guest para o caminho virtual, sem tocar no disco.
    ///
    /// Devolve `None` para o que sai da raiz virtual e para o que não é caminho do console:
    /// componente com `:` (raiz do Windows, outro prefixo) ou com NUL. `allow_root` aceita o
    /// próprio diretório do módulo e o `/mod/`, que se listam mas não se abrem como arquivo.
    fn virtual_path(&self, guest_path: &str, allow_root: bool) -> Option<Virtual> {
        let normalizado = guest_path.replace('\\', "/");
        let (resto, do_aparelho) = PREFIXES
            .iter()
            .copied()
            .find_map(|(prefixo, do_aparelho)| {
                let resto = normalizado.strip_prefix(prefixo)?;
                Some(match do_aparelho {
                    true => (resto.to_string(), true),
                    false => (sem_classe(resto), false),
                })
            })
            .unwrap_or((normalizado.clone(), false));
        let inicio: Virtual = match do_aparelho {
            true => Vec::new(),
            false => vec![MOD_DIR.to_string(), self.module.clone()],
        };
        // Da pasta do módulo sobe-se até `/mod/`, que é como o Quake alcança os dados dele, e
        // nem um passo além. Do aparelho não se sobe: ele já é a raiz.
        let piso = match do_aparelho {
            true => 0,
            false => 1,
        };
        let mut caminho = inicio.clone();
        // Uma barra vazia — `/` no começo ou `//` no meio — ancora o caminho na pasta do
        // módulo, e dali o `..` não sobe. Os Zeebo Extreme gravam o recorde das pistas em
        // `./udata/trackinfo.txt` e o releem pelo empacotador, que monta `"%s/%s"` com a raiz
        // `./`: `.//../udata/trackinfo.txt`. Subindo, a releitura caía em `mod/udata`, a lista
        // de pistas ficava sem vetor e a largada do Bóia Cross lia o ponteiro nulo.
        let mut ancorado = false;
        for parte in resto.split('/') {
            match parte {
                "" => ancorado = !do_aparelho,
                "." => continue,
                ".." if ancorado && caminho == inicio => continue,
                ".." => {
                    if caminho.len() <= piso {
                        return None;
                    }
                    caminho.pop();
                }
                _ if parte.contains(|c| c == ':' || c == '\0') => return None,
                _ => caminho.push(parte.to_string()),
            }
        }
        let raiz_de_modulos = caminho.len() == 1 && caminho[0].eq_ignore_ascii_case(MOD_DIR);
        if !allow_root && (caminho.is_empty() || raiz_de_modulos || caminho == inicio) {
            return None;
        }
        Some(caminho)
    }

    /// A camada gravável de um caminho virtual: a raiz dela e o caminho dentro.
    fn upper(&self, caminho: &[String]) -> (PathBuf, PathBuf) {
        match caminho.first() {
            Some(primeiro) if primeiro.eq_ignore_ascii_case(MOD_DIR) => {
                let raiz = self.saves_dir();
                let dentro = junta(&raiz, &caminho[1..]);
                (raiz, dentro)
            }
            _ => (self.device.clone(), junta(&self.device, caminho)),
        }
    }

    /// A camada do pacote, quando o caminho virtual cai nela.
    fn lower(&self, caminho: &[String]) -> Option<(PathBuf, PathBuf)> {
        if !caminho.first()?.eq_ignore_ascii_case(MOD_DIR) {
            return None;
        }
        match &self.package_mods {
            Some(mods) => Some((mods.clone(), junta(mods, &caminho[1..]))),
            None if caminho.get(1)?.eq_ignore_ascii_case(&self.module) => {
                Some((self.root.clone(), junta(&self.root, &caminho[2..])))
            }
            None => None,
        }
    }

    /// O caminho gravável, com a caixa que o disco já tem.
    fn upper_existente(&self, caminho: &[String]) -> Option<PathBuf> {
        let (raiz, dentro) = self.upper(caminho);
        na_camada(&raiz, match_case(dentro))
    }

    /// O caminho no pacote, se ele existe lá.
    fn lower_existente(&self, caminho: &[String]) -> Option<PathBuf> {
        let (raiz, dentro) = self.lower(caminho)?;
        na_camada(&raiz, match_case(dentro)).filter(|c| c.exists())
    }

    /// Traduz um caminho do guest para um caminho do host, para ler.
    ///
    /// O que o jogo gravou esconde o que veio no pacote: é o que faz o save relido ser o save, e
    /// não o recurso original. Um caminho que não existe em nenhuma das camadas volta como o
    /// lugar onde ele nasceria, que é sempre a gravável.
    pub fn resolve(&self, guest_path: &str) -> Option<PathBuf> {
        let caminho = self.virtual_path(guest_path, false)?;
        let gravavel = self.upper_existente(&caminho)?;
        match gravavel.exists() {
            true => Some(gravavel),
            false => self.lower_existente(&caminho).or(Some(gravavel)),
        }
    }

    /// Onde abrir um caminho, e de onde copiar antes de escrever.
    ///
    /// Ver [`OpenIntent`]. Toda escrita cai na camada gravável; um recurso do pacote aberto para
    /// escrita é copiado antes, e as escritas seguintes já o encontram lá.
    pub fn open_target(&self, guest_path: &str, intent: OpenIntent) -> Option<OpenTarget> {
        let caminho = self.virtual_path(guest_path, false)?;
        let gravavel = self.upper_existente(&caminho)?;
        let ja_gravado = gravavel.exists();
        let do_pacote = || self.lower_existente(&caminho);
        Some(match intent {
            OpenIntent::Read => OpenTarget {
                path: match ja_gravado {
                    true => gravavel,
                    false => do_pacote().unwrap_or(gravavel),
                },
                copy_from: None,
            },
            OpenIntent::Create => OpenTarget {
                path: gravavel,
                copy_from: None,
            },
            OpenIntent::ReadWrite | OpenIntent::Append => OpenTarget {
                copy_from: match ja_gravado {
                    true => None,
                    false => do_pacote(),
                },
                path: gravavel,
            },
        })
    }

    /// Como [`Vfs::resolve`], mas para **criar** um nome, sempre na camada gravável.
    ///
    /// O nome novo fica exatamente como o guest escreveu: renomear para `save.dat` quando existe
    /// um `SAVE.DAT` tem de criar o primeiro, não sobrescrever o segundo. Os diretórios que já
    /// existem continuam seguindo a semântica sem caixa do console; sem isto, `UDATA/save.dat`
    /// criaria uma segunda pasta ao lado de `udata/` num host Linux.
    pub fn resolve_new(&self, guest_path: &str) -> Option<PathBuf> {
        let caminho = self.virtual_path(guest_path, false)?;
        let (raiz, dentro) = self.upper(&caminho);
        na_camada(&raiz, match_case_parent(dentro))
    }

    /// O caminho na camada gravável, exista ele ou não.
    ///
    /// É para quem vai **remover**, **renomear** ou criar e apagar diretório: o pacote é imutável,
    /// então só o que está na camada gravável pode ser mexido.
    pub fn writable(&self, guest_path: &str) -> Option<PathBuf> {
        self.upper_existente(&self.virtual_path(guest_path, false)?)
    }

    /// O diretório para listar, no pacote quando ele o tem.
    ///
    /// Listar `fs:/~/` é legítimo — é o diretório do jogo —, ainda que abri-lo como arquivo não
    /// seja. O que o jogo gravou no mesmo diretório vem de [`Vfs::writable_dir`].
    pub fn resolve_dir(&self, guest_path: &str) -> Option<PathBuf> {
        let caminho = self.virtual_path(guest_path, true)?;
        let gravavel = self.upper_existente(&caminho)?;
        self.lower_existente(&caminho)
            .filter(|c| c.is_dir())
            .or(Some(gravavel))
    }

    /// O mesmo diretório na camada gravável, quando ele existe lá.
    ///
    /// Serve à listagem: o jogo precisa enxergar os arquivos que ele mesmo gravou, e não só os
    /// que vieram no pacote.
    pub fn writable_dir(&self, guest_path: &str) -> Option<PathBuf> {
        let caminho = self.virtual_path(guest_path, true)?;
        self.upper_existente(&caminho).filter(|c| c.is_dir())
    }

    /// Se o caminho do host está na camada gravável.
    pub fn is_writable_path(&self, path: &Path) -> bool {
        path.starts_with(self.saves_dir()) || path.starts_with(&self.device)
    }

    /// Se um caminho do host está dentro do que este jogo alcança.
    ///
    /// Para o que vem de fora do VFS e aponta para o host: um save state guarda o caminho dos
    /// arquivos abertos, e um estado adulterado não pode reabrir um arquivo qualquer da máquina.
    pub fn contains_host(&self, path: &Path) -> bool {
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return false;
        }
        let raizes = [
            Some(self.saves_dir()),
            Some(self.device.clone()),
            Some(self.root.clone()),
            self.package_mods.clone(),
        ];
        raizes
            .into_iter()
            .flatten()
            .any(|raiz| path.starts_with(&raiz) && sem_ligacao(&raiz, path))
    }
}

/// `raiz` seguida dos componentes de um caminho virtual.
fn junta(raiz: &Path, componentes: &[String]) -> PathBuf {
    let mut caminho = raiz.to_path_buf();
    caminho.extend(componentes);
    caminho
}

/// O caminho, se nada entre a raiz da camada e ele for ligação simbólica.
///
/// Os componentes já vêm limpos do caminho virtual, mas um link dentro da camada levaria o jogo
/// para onde ele apontasse. O zip e o 7z já recusam links ao extrair; isto cobre o `.mod` solto,
/// cuja pasta é do usuário.
fn na_camada(raiz: &Path, caminho: PathBuf) -> Option<PathBuf> {
    sem_ligacao(raiz, &caminho).then_some(caminho)
}

fn sem_ligacao(raiz: &Path, caminho: &Path) -> bool {
    let Ok(relativo) = caminho.strip_prefix(raiz) else {
        return false;
    };
    let mut atual = raiz.to_path_buf();
    for parte in relativo.components() {
        if !matches!(parte, Component::Normal(_)) {
            return false;
        }
        atual.push(parte);
        match std::fs::symlink_metadata(&atual) {
            Ok(meta) if meta.file_type().is_symlink() => return false,
            Ok(_) => {}
            // Daqui para a frente o caminho ainda não existe, e o que não existe não é link.
            Err(_) => break,
        }
    }
    true
}

/// Resolve só os diretórios de um caminho novo, preservando o último componente para criação.
fn match_case_parent(path: PathBuf) -> PathBuf {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return path;
    };
    match_case(parent.to_path_buf()).join(name)
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

    fn vfs() -> Vfs {
        Vfs::new("/jogos/mod/bjt")
    }

    /// O caminho virtual como texto, para os testes lerem como o console escreve.
    fn virtual_de(vfs: &Vfs, guest: &str) -> Option<String> {
        vfs.virtual_path(guest, false)
            .map(|c| format!("/{}", c.join("/")))
    }

    #[test]
    fn o_diretorio_do_modulo_pode_ser_listado_mas_nao_aberto() {
        // Listar `fs:/~/` é legítimo — é a pasta do jogo. Abri-la como arquivo não é, e a
        // diferença entre as duas respostas é justamente essa.
        let vfs = Vfs::new("/roms/jogo");
        assert_eq!(vfs.resolve("fs:/~/"), None);
        assert!(vfs.resolve_dir("fs:/~/").is_some());
        // Sair da raiz continua proibido nos dois.
        assert_eq!(vfs.resolve_dir("fs:/~/../../etc"), None);
    }

    #[test]
    fn caminho_relativo_cai_na_pasta_do_modulo() {
        assert_eq!(virtual_de(&vfs(), "dados.dat").as_deref(), Some("/mod/bjt/dados.dat"));
    }

    #[test]
    fn o_til_e_do_modulo_e_o_resto_e_do_aparelho() {
        let vfs = vfs();
        // Com `~`, e sem prefixo nenhum, é a pasta do jogo.
        let do_jogo = Some("/mod/bjt/dados.dat");
        assert_eq!(virtual_de(&vfs, "fs:/~/dados.dat").as_deref(), do_jogo);
        assert_eq!(virtual_de(&vfs, "~/dados.dat").as_deref(), do_jogo);
        assert_eq!(virtual_de(&vfs, "fs:/~0x01070798/dados.dat").as_deref(), do_jogo);

        // Sem o `~`, é o sistema de arquivos do aparelho — comum a todos os jogos. É o que
        // permite ao Zeebo F.C. abrir `fs:/zeeboiddata/zeeboid.db`, que o Zeeboids gravou.
        assert_eq!(virtual_de(&vfs, "fs:/dados.dat").as_deref(), Some("/dados.dat"));
        assert_eq!(
            virtual_de(&vfs, "fs:/zeeboiddata/zeeboid.db").as_deref(),
            Some("/zeeboiddata/zeeboid.db")
        );
        // E `fs:/mod/<pasta>/` é a mesma pasta que o `~`.
        assert_eq!(virtual_de(&vfs, "fs:/mod/bjt/dados.dat").as_deref(), do_jogo);
    }

    #[test]
    fn aceita_barra_invertida_como_separador() {
        assert_eq!(
            virtual_de(&vfs(), "imagens\\fundo.bmp").as_deref(),
            Some("/mod/bjt/imagens/fundo.bmp")
        );
    }

    #[test]
    fn do_aparelho_nao_se_sobe() {
        // A raiz do aparelho é raiz: `..` a partir dela sairia para a máquina do usuário.
        let vfs = vfs();
        assert_eq!(virtual_de(&vfs, "fs:/../x"), None);
        assert_eq!(virtual_de(&vfs, "fs:/zeeboiddata/../../x"), None);
    }

    #[test]
    fn sobe_ate_a_raiz_de_modulos_mas_nao_alem() {
        let vfs = Vfs::new("/jogos/mod/274802");
        // O Quake guarda os dados dele num diretório irmão.
        assert_eq!(
            virtual_de(&vfs, "fs:/~/../id1/splash_title.png").as_deref(),
            Some("/mod/id1/splash_title.png")
        );
        // Mais um nível já sairia da árvore de módulos.
        assert_eq!(virtual_de(&vfs, "fs:/~/../../outro/x"), None);
        assert_eq!(virtual_de(&vfs, "../../../etc/passwd"), None);
        assert_eq!(virtual_de(&vfs, "dados/../../../fora.txt"), None);
    }

    #[test]
    fn recusa_o_que_nao_e_caminho_do_console() {
        let vfs = vfs();
        assert_eq!(virtual_de(&vfs, "C:/Windows/system32"), None);
        assert_eq!(virtual_de(&vfs, "fs:/~/../C:/x"), None);
        assert_eq!(virtual_de(&vfs, "a\0b"), None);
    }

    #[test]
    fn depois_de_uma_barra_vazia_o_ponto_ponto_nao_sai_do_modulo() {
        let vfs = Vfs::new("/jogos/mod/278285");
        // O que o Bóia Cross grava e o caminho por onde ele relê.
        assert_eq!(
            virtual_de(&vfs, "./udata/trackinfo.txt"),
            virtual_de(&vfs, ".//../udata/trackinfo.txt")
        );
        assert_eq!(
            virtual_de(&vfs, "/../udata/trackinfo.txt").as_deref(),
            Some("/mod/278285/udata/trackinfo.txt")
        );
        // Sem a barra vazia, continua subindo até a raiz de módulos, como o NFS precisa.
        assert_eq!(
            virtual_de(&vfs, "../nfsresources/x.bar").as_deref(),
            Some("/mod/nfsresources/x.bar")
        );
    }

    #[test]
    fn caminho_vazio_nao_resolve_para_a_propria_raiz() {
        let vfs = vfs();
        assert_eq!(vfs.resolve(""), None);
        assert_eq!(vfs.resolve("fs:/~"), None);
        assert_eq!(vfs.resolve("fs:/"), None);
        assert_eq!(vfs.resolve("fs:/mod"), None);
        assert_eq!(vfs.resolve("fs:/~/.."), None);
    }

    #[test]
    fn so_o_modulo_em_pacote_ve_as_pastas_vizinhas() {
        // Num pacote, `mod/` é do pacote. Fora dele, a pasta de cima é do usuário.
        assert!(Vfs::new("/cache/abc/mod/274802").package_mods.is_some());
        assert!(Vfs::new("/cache/abc/MOD/274802").package_mods.is_some());
        assert!(Vfs::new("/home/kaio/Downloads/jogo").package_mods.is_none());
        let solto = Vfs::new("/home/kaio/Downloads/jogo");
        let vizinha = ["mod".to_string(), "outro".to_string(), "x".to_string()];
        assert_eq!(solto.lower(&vizinha), None);
    }

    #[test]
    fn o_save_state_so_reabre_o_que_o_jogo_alcanca() {
        let mut vfs = Vfs::new("/cache/abc/mod/274802");
        vfs.set_device_root("/perfil/aparelho");
        vfs.set_save_root("/perfil/saves/mod");
        assert!(vfs.contains_host(Path::new("/perfil/saves/mod/274802/save.dat")));
        assert!(vfs.contains_host(Path::new("/cache/abc/mod/id1/pak0.pak")));
        assert!(vfs.contains_host(Path::new("/perfil/aparelho/zeeboiddata/zeeboid.db")));
        assert!(!vfs.contains_host(Path::new("/etc/passwd")));
        assert!(!vfs.contains_host(Path::new("/cache/outro/mod/1/x")));
        assert!(!vfs.contains_host(Path::new("/perfil/saves/mod/../../segredo")));
    }
}

#[cfg(test)]
mod tests_no_disco {
    use super::*;

    /// O sistema de arquivos distingue maiúsculas de minúsculas?
    ///
    /// **O APFS do macOS e o NTFS do Windows, por padrão, não distinguem**: `data` e `DATA` são a
    /// mesma pasta, `font.fnz` e `font.FNZ` são o mesmo arquivo. Nesses sistemas o caminho pedido
    /// já existe, a resposta do VFS é ele mesmo, e não há duas grafias para comparar — foi assim
    /// que dois testes desta seção passaram a falhar **só no macOS** no primeiro run de CI que
    /// chegou até eles. Onde o sistema funde as grafias, o que a busca sem caixa tem de garantir
    /// vem de graça do próprio sistema de arquivos.
    fn distingue_caixa(onde: &Path) -> bool {
        let alta = onde.join("zeebx-CAIXA");
        let baixa = onde.join("zeebx-caixa");
        std::fs::write(&alta, b"x").unwrap();
        let distingue = !baixa.exists();
        let _ = std::fs::remove_file(&alta);
        distingue
    }

    /// Um pacote extraído, com o aparelho e o store de saves ao lado, como o motor monta.
    struct Perfil {
        raiz: PathBuf,
        modulo: PathBuf,
        saves: PathBuf,
        aparelho: PathBuf,
    }

    impl Perfil {
        fn novo(nome: &str, modulo: &str) -> Self {
            let raiz = std::env::temp_dir().join(format!("zeebx-vfs-{nome}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&raiz);
            let modulo = raiz.join("cache/abc").join(modulo);
            let saves = raiz.join("saves/mod");
            let aparelho = raiz.join("aparelho");
            for dir in [&modulo, &saves, &aparelho] {
                std::fs::create_dir_all(dir).unwrap();
            }
            Self { raiz, modulo, saves, aparelho }
        }

        fn vfs(&self) -> Vfs {
            let mut vfs = Vfs::new(&self.modulo);
            vfs.set_device_root(&self.aparelho);
            vfs.set_save_root(&self.saves);
            vfs
        }
    }

    impl Drop for Perfil {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.raiz);
        }
    }

    /// Os dez ports de arcade pedem `font.fnz` e trazem `font.FNZ`. No console dá na mesma.
    #[test]
    fn acha_o_arquivo_com_outra_caixa() {
        let perfil = Perfil::novo("caixa", "mod/279233");
        let modulo = &perfil.modulo;
        std::fs::write(modulo.join("font.FNZ"), b"fonte").unwrap();

        let vfs = perfil.vfs();
        let distingue = distingue_caixa(modulo);
        if distingue {
            assert_eq!(vfs.resolve("font.fnz"), Some(modulo.join("font.FNZ")));
        } else {
            // Sistema que funde as duas grafias: a resposta é o nome pedido, e o arquivo é o
            // mesmo. A prova que interessa aqui é que a resposta abre o conteúdo do pacote.
            let achado = vfs.resolve("font.fnz").expect("respondeu pelo arquivo");
            assert_eq!(std::fs::read(&achado).unwrap(), b"fonte");
        }
        // O que não existe volta como o lugar onde nasceria, que é o store de saves.
        assert_eq!(
            vfs.resolve("save.dat"),
            Some(perfil.saves.join("279233/save.dat"))
        );
        // Uma criação preserva o nome do arquivo, mas acha diretórios existentes sem caixa.
        let udata = perfil.saves.join("279233/udata");
        std::fs::create_dir_all(&udata).unwrap();
        if distingue {
            assert_eq!(vfs.resolve_new("UDATA/save.dat"), Some(udata.join("save.dat")));
        } else {
            assert!(vfs.resolve_new("UDATA/save.dat").is_some());
        }
    }

    /// O store guarda o que o jogo grava, sem tocar no conteúdo do pacote.
    #[test]
    fn a_escrita_vai_para_o_store_e_esconde_o_recurso_do_pacote() {
        let perfil = Perfil::novo("overlay", "mod/1");
        let modulo = &perfil.modulo;
        let save = perfil.saves.join("1");
        std::fs::write(modulo.join("config.ini"), b"do pacote").unwrap();
        let vfs = perfil.vfs();

        // Leitura: o pacote é encontrado enquanto não há nada no store.
        assert_eq!(vfs.resolve("config.ini"), Some(modulo.join("config.ini")));

        // Escrita de recurso existente: copia antes e depois o store vence.
        let alvo = vfs
            .open_target("config.ini", OpenIntent::ReadWrite)
            .unwrap();
        assert_eq!(alvo.path, save.join("config.ini"));
        assert_eq!(alvo.copy_from, Some(modulo.join("config.ini")));
        std::fs::create_dir_all(&save).unwrap();
        std::fs::write(&alvo.path, b"do save").unwrap();
        assert_eq!(vfs.resolve("config.ini"), Some(save.join("config.ini")));
        assert_eq!(
            std::fs::read(modulo.join("config.ini")).unwrap(),
            b"do pacote"
        );

        // Arquivo novo: nasce no store.
        let novo = vfs
            .open_target("udata/recorde.dat", OpenIntent::Create)
            .unwrap();
        assert_eq!(novo.path, save.join("udata/recorde.dat"));
        assert_eq!(novo.copy_from, None);

        // O aparelho tem a raiz dele.
        let dispositivo = vfs
            .open_target("fs:/zeeboiddata/zeeboid.db", OpenIntent::Create)
            .unwrap();
        assert_eq!(dispositivo.path, perfil.aparelho.join("zeeboiddata/zeeboid.db"));
    }

    /// O Quake lê `mod/id1/` do pacote, e o que ele grava ali vai para o store.
    #[test]
    fn o_diretorio_irmao_e_lido_do_pacote_e_gravado_no_store() {
        let perfil = Perfil::novo("irmao", "mod/274802");
        let id1 = perfil.modulo.parent().unwrap().join("id1");
        std::fs::create_dir_all(&id1).unwrap();
        std::fs::write(id1.join("pak0.pak"), b"pak").unwrap();
        let vfs = perfil.vfs();

        assert_eq!(vfs.resolve("fs:/~/../id1/pak0.pak"), Some(id1.join("pak0.pak")));
        let config = vfs
            .open_target("fs:/~/../id1/config.cfg", OpenIntent::Create)
            .unwrap();
        assert_eq!(config.path, perfil.saves.join("id1/config.cfg"));
        // Apagar o pak mira o store, onde ele não existe: o do pacote fica.
        assert_eq!(
            vfs.writable("fs:/~/../id1/pak0.pak"),
            Some(perfil.saves.join("id1/pak0.pak"))
        );
        assert!(!vfs.is_writable_path(&id1.join("pak0.pak")));
    }

    /// O que o jogo gravou pode ser apagado e renomeado; o pacote não.
    #[test]
    fn o_store_protege_o_pacote_de_remocao_e_renomeio() {
        let perfil = Perfil::novo("rm", "mod/1");
        let modulo = &perfil.modulo;
        std::fs::write(modulo.join("recurso.pak"), b"do pacote").unwrap();
        let vfs = perfil.vfs();

        // O alvo de remoção de um recurso do pacote é o caminho no store, não o do pacote.
        let no_store = perfil.saves.join("1/recurso.pak");
        assert_eq!(vfs.writable("recurso.pak"), Some(no_store.clone()));
        assert!(!vfs.is_writable_path(&modulo.join("recurso.pak")));

        // Depois de copiado para o store, o alvo passa a ser dele.
        std::fs::create_dir_all(no_store.parent().unwrap()).unwrap();
        std::fs::write(&no_store, b"do save").unwrap();
        assert!(vfs.is_writable_path(&vfs.resolve("recurso.pak").unwrap()));
    }

    /// O OpenTyrian abre `fs:/mod/opentyrian_zeebo/data/tyrian1.lvl`: é a pasta dele no console.
    #[test]
    fn fs_mod_acha_a_pasta_do_modulo_solto() {
        let perfil = Perfil::novo("mod", "tyrian");
        let modulo = &perfil.modulo;
        std::fs::create_dir_all(modulo.join("data")).unwrap();
        std::fs::create_dir_all(perfil.saves.join("tyrian")).unwrap();
        std::fs::write(modulo.join("data/tyrian1.lvl"), b"fase").unwrap();
        std::fs::write(modulo.join("tyrian.cfg"), b"pacote").unwrap();
        std::fs::write(perfil.saves.join("tyrian/tyrian.cfg"), b"gravado").unwrap();
        // Uma pasta vizinha do `.mod` solto é do usuário, não do jogo.
        let vizinha = modulo.parent().unwrap().join("outro");
        std::fs::create_dir_all(&vizinha).unwrap();
        std::fs::write(vizinha.join("segredo"), b"x").unwrap();

        let vfs = perfil.vfs();
        if distingue_caixa(&perfil.raiz) {
            assert_eq!(
                vfs.resolve("fs:/mod/tyrian/DATA/tyrian1.lvl"),
                Some(modulo.join("data/tyrian1.lvl"))
            );
        } else {
            let achado = vfs
                .resolve("fs:/mod/tyrian/DATA/tyrian1.lvl")
                .expect("respondeu pela pasta");
            assert_eq!(std::fs::read(&achado).unwrap(), b"fase");
        }
        // O que o jogo gravou vence o que veio com ele.
        assert_eq!(
            vfs.resolve("fs:/mod/tyrian/tyrian.cfg"),
            Some(perfil.saves.join("tyrian/tyrian.cfg"))
        );
        assert_eq!(
            vfs.resolve("fs:/mod/tyrian/novo.sav"),
            Some(perfil.saves.join("tyrian/novo.sav"))
        );
        assert_eq!(
            vfs.resolve_dir("fs:/mod/tyrian/data"),
            Some(modulo.join("data"))
        );
        // A vizinha não aparece: `../outro/segredo` cai no store, onde não há nada.
        assert_eq!(
            vfs.resolve("../outro/segredo"),
            Some(perfil.saves.join("outro/segredo"))
        );
        assert_eq!(vfs.resolve("fs:/mod/../../segredo"), None);
    }

    /// Um link dentro da pasta do módulo não leva o jogo para fora dela.
    #[cfg(unix)]
    #[test]
    fn ligacao_simbolica_nao_sai_da_camada() {
        let perfil = Perfil::novo("link", "tyrian");
        let fora = perfil.raiz.join("fora");
        std::fs::create_dir_all(&fora).unwrap();
        std::fs::write(fora.join("segredo"), b"x").unwrap();
        std::os::unix::fs::symlink(&fora, perfil.modulo.join("atalho")).unwrap();
        let vfs = perfil.vfs();

        // No pacote: o link é ignorado, e o caminho cai no store, onde não há nada.
        assert_eq!(
            vfs.resolve("atalho/segredo"),
            Some(perfil.saves.join("tyrian/atalho/segredo"))
        );
        assert!(!vfs.contains_host(&perfil.modulo.join("atalho/segredo")));

        // No store: nem ler, nem criar, nem apagar.
        std::os::unix::fs::symlink(&fora, perfil.saves.join("tyrian")).unwrap();
        assert_eq!(vfs.resolve("segredo"), None);
        assert_eq!(vfs.open_target("x.sav", OpenIntent::Create), None);
        assert_eq!(vfs.writable("segredo"), None);
    }
}
