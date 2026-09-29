//! Raízes persistentes e identidade imutável de conteúdo.
//!
//! O frontend Libretro vai fornecer a raiz de saves. O motor não deve espalhar cache, dados do
//! aparelho e saves de cada título dentro da ROM nem depender de `HOME`: todos nascem desta
//! estrutura. Por enquanto os caminhos são nativos; o adaptador `StorageFs` futuro trocará o
//! transporte, sem mudar este layout lógico.

use std::collections::HashSet;
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

    /// Copia para o overlay os arquivos que uma versao antiga gravou dentro do cache do pacote.
    ///
    /// O manifesto da extracao e a fonte de verdade: o que veio do pacote e ignorado; o que
    /// apareceu depois, dentro da raiz do modulo, e copiado. A origem nunca e apagada e um
    /// arquivo que ja existe no overlay nunca e sobrescrito.
    pub fn migrate_legacy_package_writes(
        &self,
        package: &Path,
        content: &ContentId,
    ) -> std::io::Result<u64> {
        if !self.overlay {
            return Ok(0);
        }
        let ext = package.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("zip" | "7z")) {
            return Ok(0);
        }
        let Some(module) = crate::loader::archive::find_module(package) else {
            return Ok(0);
        };
        let mut copied = 0u64;
        let Ok(entries) = std::fs::read_dir(&self.cache) else {
            return Ok(0);
        };
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| {
                entry.path().is_dir()
                    && entry
                        .file_name()
                        .to_string_lossy()
                        .ends_with(content.as_str())
            })
            .map(|entry| entry.path())
            .collect();
        if let Ok(legacy) = crate::loader::archive::legacy_cache_in(package, &self.cache) {
            if legacy.is_dir() && !candidates.contains(&legacy) {
                candidates.push(legacy);
            }
        }

        for cache_root in candidates {
            let manifest_path = cache_root.join(crate::loader::archive::MANIFESTO);
            let had_manifest = manifest_path.is_file();
            if !had_manifest {
                crate::loader::archive::escrever_manifesto(package, &cache_root)?;
            }
            let Ok(manifest) = std::fs::read_to_string(&manifest_path) else {
                continue;
            };
            // Se o manifesto acabou de ser reconstruido, sua data e posterior a qualquer save
            // legado e portanto nao serve como atalho. Nesse caso forca comparacao byte a byte.
            let manifest_modified = had_manifest
                .then(|| {
                    std::fs::metadata(&manifest_path)
                        .and_then(|meta| meta.modified())
                        .ok()
                })
                .flatten();
            let package_entries: HashSet<String> = manifest
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect();
            let module_path = cache_root.join(&module);
            let Some(module_root) = module_path.parent().filter(|parent| parent.is_dir()) else {
                continue;
            };
            let save_root = self.save_for(content);
            migrate_unknown_tree(
                module_root,
                module_root,
                &cache_root,
                &save_root,
                &package_entries,
                package,
                manifest_modified,
                &mut copied,
            )?;
        }
        Ok(copied)
    }
}

fn migrate_unknown_tree(
    dir: &Path,
    module_root: &Path,
    cache_root: &Path,
    save_root: &Path,
    package_entries: &HashSet<String>,
    package: &Path,
    manifest_modified: Option<std::time::SystemTime>,
    copied: &mut u64,
) -> std::io::Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        if entry.file_name() == crate::loader::archive::MANIFESTO {
            continue;
        }
        let path = entry.path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_symlink() => metadata,
            _ => continue,
        };
        let relative_cache = match path.strip_prefix(cache_root) {
            Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        let dir_prefix = format!("{relative_cache}/");
        let in_package = package_entries.contains(&relative_cache)
            || (metadata.is_dir()
                && (package_entries.contains(&dir_prefix)
                    || package_entries
                        .iter()
                        .any(|entry| entry.starts_with(&dir_prefix))));

        if metadata.is_dir() {
            if in_package {
                migrate_unknown_tree(
                    &path,
                    module_root,
                    cache_root,
                    save_root,
                    package_entries,
                    package,
                    manifest_modified,
                    copied,
                )?;
            } else {
                copy_unknown_tree(&path, module_root, save_root, copied)?;
            }
        } else if !in_package {
            copy_unknown_file(&path, module_root, save_root, copied)?;
        } else {
            let destination = path
                .strip_prefix(module_root)
                .ok()
                .map(|relative| save_root.join(relative));
            if destination.as_ref().is_some_and(|path| path.exists()) {
                continue;
            }
            // O manifesto e escrito somente depois de a extracao terminar. Um arquivo do pacote
            // mais novo (ou indistinguivel pela resolucao do timestamp) pode ter sido alterado
            // pelo jogo; nesse caso comparamos com os bytes originais antes de decidir.
            let possibly_modified = match (metadata.modified().ok(), manifest_modified) {
                (Some(file), Some(manifest)) => file >= manifest,
                _ => true,
            };
            if possibly_modified
                && !crate::loader::archive::entrada_igual_ao_arquivo(
                    package,
                    &relative_cache,
                    &path,
                )?
            {
                copy_unknown_file(&path, module_root, save_root, copied)?;
            }
        }
    }
    Ok(())
}

fn copy_unknown_tree(
    dir: &Path,
    module_root: &Path,
    save_root: &Path,
    copied: &mut u64,
) -> std::io::Result<()> {
    let Ok(relative) = dir.strip_prefix(module_root) else {
        return Ok(());
    };
    std::fs::create_dir_all(save_root.join(relative))?;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_symlink() => metadata,
            _ => continue,
        };
        if metadata.is_dir() {
            copy_unknown_tree(&path, module_root, save_root, copied)?;
        } else {
            copy_unknown_file(&path, module_root, save_root, copied)?;
        }
    }
    Ok(())
}

fn copy_unknown_file(
    source: &Path,
    module_root: &Path,
    save_root: &Path,
    copied: &mut u64,
) -> std::io::Result<()> {
    let Ok(relative) = source.strip_prefix(module_root) else {
        return Ok(());
    };
    let destination = save_root.join(relative);
    if destination.exists() {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    *copied += std::fs::copy(source, destination)?;
    Ok(())
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
    use std::io::Write;

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

    #[test]
    fn migra_save_legado_do_cache_sem_copiar_o_pacote() {
        let root = crate::scratch::TempDir::new("zeebx-storage-migrate");
        let package = root.join("jogo.zip");
        let file = std::fs::File::create(&package).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in [
            ("mod/123/jogo.mod", b"mod".as_slice()),
            ("mod/123/recurso.dat", b"pacote".as_slice()),
            ("mod/123/udata/base.dat", b"base".as_slice()),
            ("mod/123/udata/seed.sav", b"0000".as_slice()),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();

        let paths = StoragePaths::for_frontend(root.join("save"), Some(root.join("system")));
        paths.create_dirs().unwrap();
        let module = crate::loader::archive::extract_in(&package, &paths.cache).unwrap();
        let module_root = module.parent().unwrap();
        std::fs::write(module_root.join("udata/save.dat"), b"progresso").unwrap();
        // Alguns jogos, como RE4 e Double Dragon, trazem um save semente no pacote e o
        // sobrescrevem depois. Mesmo continuando no manifesto, os bytes diferentes sao save.
        std::fs::write(module_root.join("udata/seed.sav"), b"1111").unwrap();
        std::fs::create_dir_all(module_root.join("perfil")).unwrap();
        std::fs::write(module_root.join("perfil/opcoes.bin"), b"opcoes").unwrap();

        let content = paths.content_id(&package).unwrap();
        let copied = paths
            .migrate_legacy_package_writes(&package, &content)
            .unwrap();
        let save = paths.save_for(&content);
        assert_eq!(
            std::fs::read(save.join("udata/save.dat")).unwrap(),
            b"progresso"
        );
        assert_eq!(
            std::fs::read(save.join("perfil/opcoes.bin")).unwrap(),
            b"opcoes"
        );
        assert_eq!(std::fs::read(save.join("udata/seed.sav")).unwrap(), b"1111");
        assert!(!save.join("recurso.dat").exists());
        assert!(!save.join("udata/base.dat").exists());
        assert_eq!(
            copied,
            b"progresso".len() as u64 + b"opcoes".len() as u64 + b"1111".len() as u64
        );

        // Uma segunda migracao nunca pisa no overlay ja estabelecido.
        std::fs::write(save.join("udata/save.dat"), b"novo").unwrap();
        assert_eq!(
            paths
                .migrate_legacy_package_writes(&package, &content)
                .unwrap(),
            0
        );
        assert_eq!(std::fs::read(save.join("udata/save.dat")).unwrap(), b"novo");
    }

    #[test]
    fn migra_o_cache_com_nome_antigo_e_reconstroi_o_manifesto() {
        let root = crate::scratch::TempDir::new("zeebx-storage-migrate-legacy");
        let package = root.join("velho.zip");
        let file = std::fs::File::create(&package).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in [
            ("mod/7/jogo.mod", b"mod".as_slice()),
            ("mod/7/recurso.bin", b"pacote".as_slice()),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();

        let paths = StoragePaths::for_frontend(root.join("save"), Some(root.join("system")));
        paths.create_dirs().unwrap();
        let module = crate::loader::archive::extract_in(&package, &paths.cache).unwrap();
        let relative = module.strip_prefix(&paths.cache).unwrap();
        let first = relative.components().next().unwrap().as_os_str();
        let strong = paths.cache.join(first);
        let legacy = crate::loader::archive::legacy_cache_in(&package, &paths.cache).unwrap();
        std::fs::rename(&strong, &legacy).unwrap();
        std::fs::remove_file(legacy.join(crate::loader::archive::MANIFESTO)).unwrap();
        std::fs::write(legacy.join("mod/7/progresso.sav"), b"legado").unwrap();

        let content = paths.content_id(&package).unwrap();
        assert_eq!(
            paths
                .migrate_legacy_package_writes(&package, &content)
                .unwrap(),
            b"legado".len() as u64
        );
        assert_eq!(
            std::fs::read(paths.save_for(&content).join("progresso.sav")).unwrap(),
            b"legado"
        );
        assert!(legacy.join(crate::loader::archive::MANIFESTO).is_file());
        assert!(legacy.join("mod/7/progresso.sav").is_file());
    }
}
