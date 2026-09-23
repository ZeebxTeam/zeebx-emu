//! Raízes persistentes e identidade imutável de conteúdo.
//!
//! O frontend Libretro vai fornecer a raiz de saves. O motor não deve espalhar cache, dados do
//! aparelho e saves de cada título dentro da ROM nem depender de `HOME`: todos nascem desta
//! estrutura. Por enquanto os caminhos são nativos; o adaptador `StorageFs` futuro trocará o
//! transporte, sem mudar este layout lógico.

use std::io::Read;
use std::path::{Path, PathBuf};

/// Nome da pasta do perfil criada por um frontend dentro dos diretórios dele.
///
/// É **fixo e minúsculo de propósito**. O frontend já cria as pastas dele com o nome de exibição
/// do core — o RetroArch grava `states/Zeebx` e `saves/Zeebx.srm` —, e derivar este nome da mesma
/// fonte espalharia os dados por dois caminhos que só coincidem em sistema sem diferença de caixa.
/// Num host Linux, `zeebx` e `Zeebx` são pastas diferentes, e o jogo passaria a ter dois perfis.
pub const PROFILE_DIR: &str = "zeebx";
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoragePaths {
    /// Raiz `.../zeebx` criada sob o diretório de saves do frontend.
    pub root: PathBuf,
    /// Conteúdo extraído, descartável e somente-leitura lógica.
    pub cache: PathBuf,
    /// O sistema de arquivos compartilhado do aparelho (`fs:/`).
    pub device: PathBuf,
    /// Overlay gravável de cada conteúdo.
    pub saves: PathBuf,
    /// Manifestos e metadados de conteúdo.
    pub metadata: PathBuf,
    /// Se o perfil usa overlay gravável por título.
    ///
    /// Falso no desktop histórico, onde o jogo grava ao lado do `.mod`; verdadeiro quando o
    /// frontend delimita a raiz de saves e o pacote precisa ficar intacto.
    pub overlay: bool,
}

impl StoragePaths {
    /// Cria o layout a partir de uma raiz de perfil já escolhida.
    ///
    /// A UI desktop atual usa sua configuração diretamente como raiz. O frontend Libretro usa
    /// [`StoragePaths::from_save_dir`] para acrescentar a pasta `zeebx` sem escrever na ROM.
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            cache: root.join("cache"),
            device: root.join("aparelho"),
            saves: root.join("saves"),
            metadata: root.join("metadata"),
            root,
            // O desktop histórico grava dentro do próprio jogo; só um frontend que delimita a
            // raiz pede overlay.
            overlay: false,
        }
    }

    /// Cria o layout lógico do perfil só com o diretório de saves do frontend.
    pub fn from_save_dir(save_dir: impl AsRef<Path>) -> Self {
        Self::for_frontend(save_dir, None::<&Path>)
    }

    /// O layout que um frontend Libretro recebe, separando o que é do aparelho do que é do jogo.
    ///
    /// A divisão segue o que os cores grandes fazem — o PPSSPP põe o `flash0` do sistema no
    /// **system directory** e o memory stick no de saves —, e ela existe por um motivo prático:
    ///
    /// | Peça | Onde | Por quê |
    /// |---|---|---|
    /// | `aparelho/` (a NAND `fs:/`) | `system` | é da máquina, não do título, e sobrevive a tudo |
    /// | `cache/` (conteúdo extraído) | `system` | é descartável e é o que enche o disco |
    /// | `saves/<conteúdo>/` | `save` | é o que o jogador quer guardar, e o frontend o sincroniza |
    /// | `metadata/` | `save` | descreve o save, então anda junto dele |
    ///
    /// Sem `system_dir` tudo cai no diretório de saves, que é o mínimo que a ABI garante.
    pub fn for_frontend(
        save_dir: impl AsRef<Path>,
        system_dir: Option<impl AsRef<Path>>,
    ) -> Self {
        let save_dir = save_dir.as_ref();
        let system_dir = system_dir.as_ref().map_or(save_dir, AsRef::as_ref);
        let perfil_save = save_dir.join(PROFILE_DIR);
        let perfil_sistema = system_dir.join(PROFILE_DIR);
        Self {
            cache: perfil_sistema.join("cache"),
            device: perfil_sistema.join("aparelho"),
            saves: perfil_save.join("saves"),
            metadata: perfil_save.join("metadata"),
            root: perfil_save,
            overlay: true,
        }
    }

    /// Cria as raízes que o motor escreve. Erro aqui é motivo para não carregar o jogo.
    pub fn create_dirs(&self) -> std::io::Result<()> {
        for dir in [&self.saves, &self.device, &self.cache, &self.metadata] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Identidade do conteúdo, para nomear overlay e cache.
    ///
    /// O hash é dos bytes do arquivo escolhido no frontend — o `.zip` quando é pacote, o `.mod`
    /// quando é módulo solto. É o que separa o save de duas versões do mesmo título.
    pub fn content_id(&self, path: &Path) -> std::io::Result<ContentId> {
        ContentId::from_reader(std::fs::File::open(path)?)
    }

    /// Onde fica o overlay privado de um conteúdo.
    pub fn save_for(&self, content: &ContentId) -> PathBuf {
        self.saves.join(content.as_str())
    }

    /// Onde fica o conteúdo extraído e reutilizável.
    pub fn cache_for(&self, content: &ContentId) -> PathBuf {
        self.cache.join(content.as_str())
    }

    /// Onde fica o manifesto do conteúdo.
    pub fn metadata_for(&self, content: &ContentId) -> PathBuf {
        self.metadata.join(format!("{}.json", content.as_str()))
    }
}

/// Hash BLAKE3 completo de bytes que definem um conteúdo.
///
/// O valor é hexadecimal para poder virar parte de caminho sem escapamento. Não é título nem
/// ClassID: estes podem coincidir entre versões ou homebrews diferentes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentId(String);

impl ContentId {
    /// Calcula a identidade de um fluxo sem carregá-lo inteiro na memória.
    pub fn from_reader(mut reader: impl Read) -> std::io::Result<Self> {
        let mut hasher = blake3::Hasher::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(Self(hasher.finalize().to_hex().to_string()))
    }

    /// Aceita somente um hash hexadecimal BLAKE3 completo vindo de metadata já validada.
    pub fn parse(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfil_separa_aparelho_e_cache_no_sistema_e_saves_no_jogo() {
        let paths = StoragePaths::for_frontend("/saves", Some("/sistema"));
        assert_eq!(paths.saves, PathBuf::from("/saves/zeebx/saves"));
        assert_eq!(paths.metadata, PathBuf::from("/saves/zeebx/metadata"));
        assert_eq!(paths.device, PathBuf::from("/sistema/zeebx/aparelho"));
        assert_eq!(paths.cache, PathBuf::from("/sistema/zeebx/cache"));
        assert!(paths.overlay);
    }

    /// Sem diretório de sistema, tudo cai nos saves: é o mínimo que a ABI garante.
    #[test]
    fn sem_sistema_o_perfil_inteiro_cabe_nos_saves() {
        let paths = StoragePaths::for_frontend("/saves", None::<&Path>);
        assert_eq!(paths.device, PathBuf::from("/saves/zeebx/aparelho"));
        assert_eq!(paths.cache, PathBuf::from("/saves/zeebx/cache"));
        assert_eq!(paths.saves, PathBuf::from("/saves/zeebx/saves"));
    }

    /// O nome do perfil é fixo e minúsculo: nunca pode sair do nome de exibição do core, senão
    /// `zeebx` e `Zeebx` viram dois perfis num host que distingue caixa.
    #[test]
    fn o_nome_do_perfil_nao_depende_da_caixa_do_core() {
        assert_eq!(PROFILE_DIR, "zeebx");
        let paths = StoragePaths::for_frontend("/saves", Some("/sistema"));
        // **Componente a componente, e não texto com barra.** O separador do Windows é `\`, e
        // procurar `/zeebx/` na string fazia este teste falhar só ali — acusando o produto por um
        // detalhe do sistema de arquivos de quem roda o teste.
        let partes: Vec<String> = paths
            .saves
            .iter()
            .map(|parte| parte.to_string_lossy().to_string())
            .collect();
        assert!(partes.iter().any(|parte| parte == PROFILE_DIR), "{partes:?}");
        assert!(!partes.iter().any(|parte| parte == "Zeebx"), "{partes:?}");
    }

    #[test]
    fn o_desktop_historico_nao_usa_overlay() {
        assert!(!StoragePaths::from_root("/config/zeebx").overlay);
    }

    #[test]
    fn hash_e_estavel_e_valido_para_caminho() {
        let one = ContentId::from_reader(&b"zeebo"[..]).unwrap();
        let two = ContentId::from_reader(&b"zeebo"[..]).unwrap();
        let other = ContentId::from_reader(&b"zeebo!"[..]).unwrap();
        assert_eq!(one, two);
        assert_ne!(one, other);
        assert_eq!(ContentId::parse(one.as_str()), Some(one));
    }
}
