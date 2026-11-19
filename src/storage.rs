//! Persistent roots and immutable content identity.
//!
//! The Libretro frontend will provide the saves root. The engine must not scatter cache, device
//! data and each title's saves inside the ROM nor depend on `HOME`: they all come from this
//! structure. For now the paths are native; the future `StorageFs` adapter will swap the
//! transport, without changing this logical layout.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Name of the profile folder a frontend creates inside its directories.
///
/// It is **fixed and lowercase on purpose**. The frontend already creates its folders with the
/// core's display name — RetroArch writes `states/Zeebx` and `saves/Zeebx.srm` —, and deriving this
/// name from the same source would scatter the data across two paths that only coincide on a
/// case-insensitive system. On a Linux host, `zeebx` and `Zeebx` are different folders, and the
/// game would end up with two profiles.
pub const PROFILE_DIR: &str = "zeebx";
/// The `saves/` folder that plays the role of a writable `/mod/`, and the `aparelho/` one that
/// did before.
const SHARED_SAVES_DIR: &str = "mod";
/// Marks, in the source, that the migration to the shared store has already been done.
pub const MIGRATED_MARKER: &str = ".migrado-para-saves-mod";
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoragePaths {
    /// `.../zeebx` root created under the frontend's saves directory.
    pub root: PathBuf,
    /// Extracted content, disposable and logically read-only.
    pub cache: PathBuf,
    /// The device's shared file system (`fs:/`).
    pub device: PathBuf,
    /// Writable overlay for each content.
    pub saves: PathBuf,
    /// Content manifests and metadata.
    pub metadata: PathBuf,
    /// Whether the profile uses a writable overlay per title.
    ///
    /// True in every profile the engine builds: the package stays intact and the save survives
    /// pruning or re-extracting the cache. False only when someone builds the profile by hand,
    /// without overlay, and then the game writes next to the `.mod`.
    pub overlay: bool,
}

impl StoragePaths {
    /// Creates the layout from an already chosen profile root.
    ///
    /// The current desktop UI uses its configuration directly as the root. The Libretro frontend
    /// uses [`StoragePaths::from_save_dir`] to add the `zeebx` folder without writing to the ROM.
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            cache: root.join("cache"),
            device: root.join("aparelho"),
            saves: root.join("saves"),
            metadata: root.join("metadata"),
            root,
            // The desktop wrote inside the extraction, in the cache, and pruning the cache took
            // the save with it. The save lives in `saves/mod/<dir>/`, as in the other frontends.
            overlay: true,
        }
    }

    /// Creates the profile's logical layout with only the frontend's saves directory.
    pub fn from_save_dir(save_dir: impl AsRef<Path>) -> Self {
        Self::for_frontend(save_dir, None::<&Path>)
    }

    /// The layout a Libretro frontend receives, separating what belongs to the device from what
    /// belongs to the game.
    ///
    /// The split follows what the big cores do — PPSSPP puts the system's `flash0` in the
    /// **system directory** and the memory stick in the saves one —, and it exists for a practical
    /// reason:
    ///
    /// | Piece | Where | Why |
    /// |---|---|---|
    /// | `aparelho/` (the NAND `fs:/`) | `system` | the machine's, not the title's; survives everything |
    /// | `cache/` (extracted content) | `system` | it is disposable and it is what fills the disk |
    /// | `saves/mod/<dir>/` | `save` | what the player wants to keep; the frontend syncs it |
    /// | `metadata/` | `save` | it describes the save, so it goes along with it |
    ///
    /// Without `system_dir` everything falls into the saves directory, which is the minimum the ABI
    /// guarantees.
    pub fn for_frontend(
        save_dir: impl AsRef<Path>,
        system_dir: Option<impl AsRef<Path>>,
    ) -> Self {
        let save_dir = save_dir.as_ref();
        let system_dir = system_dir.as_ref().map_or(save_dir, AsRef::as_ref);
        let save_profile = save_dir.join(PROFILE_DIR);
        let system_profile = system_dir.join(PROFILE_DIR);
        Self {
            cache: system_profile.join("cache"),
            device: system_profile.join("aparelho"),
            saves: save_profile.join("saves"),
            metadata: save_profile.join("metadata"),
            root: save_profile,
            overlay: true,
        }
    }

    /// Creates the roots the engine writes to. An error here is reason not to load the game.
    pub fn create_dirs(&self) -> std::io::Result<()> {
        for dir in [&self.saves, &self.device, &self.cache, &self.metadata] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Content identity, to name overlay and cache.
    ///
    /// The hash is of the bytes of the file chosen in the frontend — the `.zip` when it is a
    /// package, the `.mod` when it is a loose module. It is what separates the saves of two
    /// versions of the same title.
    pub fn content_id(&self, path: &Path) -> std::io::Result<ContentId> {
        ContentId::from_reader(std::fs::File::open(path)?)
    }

    /// Where a content's private overlay used to be, before the shared store.
    ///
    /// Only the migration reads from here: see [`StoragePaths::migrate_to_shared_saves`].
    pub fn save_for(&self, content: &ContentId) -> PathBuf {
        self.saves.join(content.as_str())
    }

    /// The shared save store: the writable `/mod/` of every game.
    ///
    /// It is a single one because on the console it is a single one: what a game writes to
    /// `fs:/~/` stays in `mod/<dir>/`, and a game that looks for another's save finds it there. The
    /// price is that two versions of the same game, with the same folder, share the save — as
    /// they would on the device.
    pub fn mod_saves(&self) -> PathBuf {
        self.saves.join(SHARED_SAVES_DIR)
    }

    /// Moves into the shared store what the saves kept before it.
    ///
    /// `module` is the module's folder, the `<dir>` of `/mod/<dir>/`.
    ///
    /// Up to 0.5.0 each content had its own overlay in `saves/<content>/`, and what the game wrote
    /// to `fs:/mod/` landed in `aparelho/mod/`. The copy overwrites nothing in the store and does
    /// not delete the source; a marker in the source says it has already been copied, so that a
    /// save the player deleted from the store does not come back on the next open.
    pub fn migrate_to_shared_saves(
        &self,
        content: &ContentId,
        module: &str,
    ) -> std::io::Result<u64> {
        let store = self.mod_saves();
        let from_content = copy_once(&self.save_for(content), &store.join(module))?;
        let from_device = copy_once(&self.device.join(SHARED_SAVES_DIR), &store)?;
        Ok(from_content + from_device)
    }

    /// Where the extracted, reusable content lives.
    pub fn cache_for(&self, content: &ContentId) -> PathBuf {
        self.cache.join(content.as_str())
    }

    /// Where the content's manifest lives.
    pub fn metadata_for(&self, content: &ContentId) -> PathBuf {
        self.metadata.join(format!("{}.json", content.as_str()))
    }

    /// Copies to the overlay the files an old version wrote inside the package cache.
    ///
    /// The extraction manifest is the source of truth: what came from the package is ignored;
    /// what showed up afterwards, inside the module root, is copied. The source is never deleted
    /// and a file that already exists in the overlay is never overwritten. From here the overlay
    /// moves on to the shared store in [`StoragePaths::migrate_to_shared_saves`].
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
            let manifest_path = cache_root.join(crate::loader::archive::MANIFEST);
            let had_manifest = manifest_path.is_file();
            if !had_manifest {
                crate::loader::archive::write_manifest(package, &cache_root)?;
            }
            let Ok(manifest) = std::fs::read_to_string(&manifest_path) else {
                continue;
            };
            // If the manifest was just rebuilt, its date is later than any legacy save and so it
            // does not serve as a shortcut. In that case force a byte-by-byte comparison.
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
            // What the game wrote in a sibling directory of the package — Quake writes in
            // `mod/id1/` — had no overlay at all and stayed in the cache. It goes straight to the
            // shared store.
            let mods = module_root
                .parent()
                .filter(|mods_dir| mods_dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case("mod")));
            if let Some(mods) = mods {
                let store = self.mod_saves();
                for sibling in std::fs::read_dir(mods).into_iter().flatten().flatten() {
                    let path = sibling.path();
                    let dir = sibling.file_type().is_ok_and(|kind| kind.is_dir());
                    if !dir || path.as_path() == module_root {
                        continue;
                    }
                    migrate_unknown_tree(
                        &path,
                        mods,
                        &cache_root,
                        &store,
                        &package_entries,
                        package,
                        manifest_modified,
                        &mut copied,
                    )?;
                }
            }
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
        if entry.file_name() == crate::loader::archive::MANIFEST {
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
            // The manifest is written only after the extraction finishes. A package file that is
            // newer (or indistinguishable at the timestamp's resolution) may have been changed by
            // the game; in that case we compare with the original bytes before deciding.
            let possibly_modified = match (metadata.modified().ok(), manifest_modified) {
                (Some(file), Some(manifest)) => file >= manifest,
                _ => true,
            };
            if possibly_modified
                && !crate::loader::archive::entry_matches_file(
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

/// The `metadata/` file with the title of a save store folder.
pub fn module_title_file(dir: &str) -> String {
    format!("mod-{dir}.titulo")
}

/// Copies the `origin` tree to `dest` without overwriting, only once.
fn copy_once(origin: &Path, dest: &Path) -> std::io::Result<u64> {
    let marker = origin.join(MIGRATED_MARKER);
    let is_symlink = std::fs::symlink_metadata(origin).is_ok_and(|m| m.file_type().is_symlink());
    if is_symlink || !origin.is_dir() || marker.exists() {
        return Ok(0);
    }
    let mut copied = 0;
    copy_unknown_tree(origin, origin, dest, &mut copied)?;
    std::fs::write(marker, b"")?;
    Ok(copied)
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

/// Full BLAKE3 hash of the bytes that define a content.
///
/// The value is hexadecimal so it can become part of a path without escaping. It is not a title
/// nor a ClassID: those can coincide between different versions or homebrews.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentId(String);

impl ContentId {
    /// Computes the identity of a stream without loading it whole into memory.
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

    /// Accepts only a full hexadecimal BLAKE3 hash coming from already validated metadata.
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
    fn profile_puts_device_and_cache_in_system_and_saves_in_game() {
        let paths = StoragePaths::for_frontend("/saves", Some("/sistema"));
        assert_eq!(paths.saves, PathBuf::from("/saves/zeebx/saves"));
        assert_eq!(paths.metadata, PathBuf::from("/saves/zeebx/metadata"));
        assert_eq!(paths.device, PathBuf::from("/sistema/zeebx/aparelho"));
        assert_eq!(paths.cache, PathBuf::from("/sistema/zeebx/cache"));
        assert!(paths.overlay);
    }

    /// Without a system directory, everything falls into the saves: it is the minimum the ABI
    /// guarantees.
    #[test]
    fn without_system_the_whole_profile_fits_in_saves() {
        let paths = StoragePaths::for_frontend("/saves", None::<&Path>);
        assert_eq!(paths.device, PathBuf::from("/saves/zeebx/aparelho"));
        assert_eq!(paths.cache, PathBuf::from("/saves/zeebx/cache"));
        assert_eq!(paths.saves, PathBuf::from("/saves/zeebx/saves"));
    }

    /// The profile name is fixed and lowercase: it can never come from the core's display name,
    /// or else `zeebx` and `Zeebx` become two profiles on a case-sensitive host.
    #[test]
    fn the_profile_name_does_not_depend_on_the_core_letter_case() {
        assert_eq!(PROFILE_DIR, "zeebx");
        let paths = StoragePaths::for_frontend("/saves", Some("/sistema"));
        // **Component by component, not text with a slash.** The Windows separator is `\`, and
        // searching for `/zeebx/` in the string made this test fail only there — blaming the
        // product for a detail of the file system of whoever runs the test.
        let parts: Vec<String> = paths
            .saves
            .iter()
            .map(|part| part.to_string_lossy().to_string())
            .collect();
        assert!(parts.iter().any(|part| part == PROFILE_DIR), "{parts:?}");
        assert!(!parts.iter().any(|part| part == "Zeebx"), "{parts:?}");
    }

    #[test]
    fn the_desktop_uses_an_overlay_outside_the_cache() {
        let paths = StoragePaths::from_root("/config/zeebx");
        assert!(paths.overlay);
        assert!(!paths.saves.starts_with(&paths.cache));
    }

    #[test]
    fn the_hash_is_stable_and_valid_for_a_path() {
        let one = ContentId::from_reader(&b"zeebo"[..]).unwrap();
        let two = ContentId::from_reader(&b"zeebo"[..]).unwrap();
        let other = ContentId::from_reader(&b"zeebo!"[..]).unwrap();
        assert_eq!(one, two);
        assert_ne!(one, other);
        assert_eq!(ContentId::parse(one.as_str()), Some(one));
    }

    #[test]
    fn migrates_a_legacy_cache_save_without_copying_the_package() {
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
        // Some games, like RE4 and Double Dragon, ship a seed save in the package and overwrite
        // it later. Even though they remain in the manifest, the different bytes are a save.
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

        // A second migration never steps on the already established overlay.
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
    fn migrates_the_old_named_cache_and_rebuilds_the_manifest() {
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
        std::fs::remove_file(legacy.join(crate::loader::archive::MANIFEST)).unwrap();
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
        assert!(legacy.join(crate::loader::archive::MANIFEST).is_file());
        assert!(legacy.join("mod/7/progresso.sav").is_file());
    }

    #[test]
    fn moves_old_saves_to_the_shared_store_once() {
        let root = crate::scratch::TempDir::new("zeebx-storage-shared");
        let paths = StoragePaths::for_frontend(root.join("save"), Some(root.join("system")));
        paths.create_dirs().unwrap();
        let content = ContentId::parse("ab".repeat(32)).unwrap();
        let old = paths.save_for(&content);
        std::fs::create_dir_all(old.join("udata")).unwrap();
        std::fs::write(old.join("udata/save.dat"), b"progresso").unwrap();
        let zwheel_dir = paths.device.join("mod/274755");
        std::fs::create_dir_all(&zwheel_dir).unwrap();
        std::fs::write(zwheel_dir.join("tt_prefs.db"), b"prefs").unwrap();

        let copied = paths.migrate_to_shared_saves(&content, "123").unwrap();
        let store = paths.mod_saves();
        assert_eq!(
            std::fs::read(store.join("123/udata/save.dat")).unwrap(),
            b"progresso"
        );
        assert_eq!(std::fs::read(store.join("274755/tt_prefs.db")).unwrap(), b"prefs");
        assert_eq!(copied, (b"progresso".len() + b"prefs".len()) as u64);

        // The source stays, and the next open does not bring back what the player deleted.
        assert!(old.join("udata/save.dat").is_file());
        std::fs::remove_file(store.join("123/udata/save.dat")).unwrap();
        assert_eq!(paths.migrate_to_shared_saves(&content, "123").unwrap(), 0);
        assert!(!store.join("123/udata/save.dat").exists());
    }
}
