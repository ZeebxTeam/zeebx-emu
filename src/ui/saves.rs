//! What the games wrote, and how to delete it.
//!
//! A Zeebo game writes in the middle of its own files: Z-Wheel puts `tt_prefs.db` next to
//! `tectoy.mod`, and Zeeboids writes outside, in `fs:/zeeboiddata`, which belongs to the
//! **device** and not to it — Zeebo F.C. reads the characters from there. That is why the list
//! has two sides, and deleting one does not touch the other.
//!
//! # How a save is told apart from package content
//!
//! By date. The package is extracted all at once, so all its files are born at the same instant;
//! what the game writes later is newer than the `.mod`. It is not guesswork: in the Zeeboids
//! cache the `.mod`, the `.sig` and `resources.pakz` are all from `09-07 16:59`, and the
//! `zeeboiddata` folder the game created is from two days later.
//!
//! The `.mod` is the reference, not the folder: the folder's date changes with every file created
//! inside it, so it would always be "newer than itself".

use std::path::{Path, PathBuf};

/// A save: from a game, or from the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    /// What to show — the game's name, or the name of the device folder.
    pub title: String,
    /// What to delete. More than one when the game scattered its files.
    pub items: Vec<PathBuf>,
    /// How many files in all, counting what is inside the folders.
    pub files: usize,
    /// The total size.
    pub bytes: u64,
}

impl Save {
    fn from(title: String, items: Vec<PathBuf>) -> Option<Self> {
        if items.is_empty() {
            return None;
        }
        let (files, bytes) = items
            .iter()
            .map(weigh)
            .fold((0, 0), |(a, b), (c, d)| (a + c, b + d));
        Some(Self {
            title,
            items,
            files,
            bytes,
        })
    }
}

/// How many files and how many bytes there are in `path`, going into the folders.
fn weigh(path: &PathBuf) -> (usize, u64) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return (0, 0);
    };
    if !meta.is_dir() {
        return (1, meta.len());
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return (0, 0);
    };
    entries
        .flatten()
        .map(|e| weigh(&e.path()))
        .fold((0, 0), |(a, b), (c, d)| (a + c, b + d))
}

/// What the package brought, read from the manifest the extraction writes.
///
/// **No manifest, no list**: an old cache is ignored, not guessed at. The first version of this
/// compared dates — whatever was newer than the `.mod` would be a save — and that falls apart:
/// the files of one extraction differ by milliseconds, and Alice's 15 MB `resources.pakz`
/// finished being written after the `.mod`. In a list with a delete button, a mistake like that
/// deletes the game.
///
/// See [`crate::loader::archive::complete_manifest`], which rebuilds an old cache's manifest from
/// the zip — the source of truth about what came in the package.
fn from_package(dir: &Path) -> Option<std::collections::HashSet<String>> {
    let text = std::fs::read_to_string(dir.join(crate::loader::archive::MANIFEST)).ok()?;
    Some(
        text
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Walks down the tree collecting what is not in the manifest.
///
/// A folder that **is** in the manifest can still have a save inside: Zeeboids writes to
/// `mod/<id>/zeeboiddata`, and `mod` came from the package. That is why the descent continues
/// into the known ones and stops at the unknown ones, which go in whole.
fn join_components(
    dir: &Path,
    package: &std::collections::HashSet<String>,
    prefix: &str,
    found: &mut Vec<PathBuf>,
) {
    /// How deep to go. The cache tree is shallow; the cap is against circular links.
    const BACKGROUND: usize = 8;

    if prefix.matches('/').count() >= BACKGROUND {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == crate::loader::archive::MANIFEST {
            continue;
        }
        let path = format!("{prefix}{name}");
        let is_dir = entry.path().is_dir();
        // The zip lists folders with a trailing slash; files, without. And not every zip lists
        // the folders: Crash's only has `mod/274214/cnk2.mod`, and without looking at the prefix
        // the whole `mod` folder passed for a save — deleting it took the game along.
        let dir_prefix = format!("{path}/");
        let known = package.contains(&path)
            || (is_dir && package.iter().any(|entry| entry.starts_with(&dir_prefix)));
        match (known, is_dir) {
            (true, true) => join_components(&entry.path(), package, &format!("{path}/"), found),
            (true, false) => {}
            (false, _) => found.push(entry.path()),
        }
    }
}

/// What a game wrote in the extraction folder: what did not come in the package.
fn written_files(root: &Path) -> Option<Vec<PathBuf>> {
    let package = from_package(root)?;
    let mut items: Vec<PathBuf> = Vec::new();
    join_components(root, &package, "", &mut items);
    Some(items)
}

fn for_game(root: &Path, title: &str) -> Option<Save> {
    let mut items = written_files(root)?;
    items.sort();
    Save::from(title.to_string(), items)
}

/// Whether the extraction in `root` holds something the game wrote — or whether there is no way
/// to know.
///
/// Without a manifest the answer is `true`: an old cache may have a save, and the one asking is
/// the pruning, which would delete the whole folder.
pub fn may_have_save(root: &Path) -> bool {
    let Some(package) = from_package(root) else {
        return true;
    };
    let mut items = Vec::new();
    join_components(root, &package, "", &mut items);
    !items.is_empty()
}

/// All saves, the games' before the device's, with `true` on the device's.
///
/// Caches made before the manifest existed do not know what came from the package. Before
/// listing, it rebuilds each one's manifest from the zip — which is still in the ROMs folder.
/// Without that the old game would simply not appear in the list.
///
/// Touches the disk: call it when opening the list and after each deletion, not every frame.
pub fn all(roms: Option<&Path>) -> Vec<(bool, Save)> {
    if let Some(roms) = roms {
        for entry in std::fs::read_dir(roms).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
                let _ = crate::loader::archive::complete_manifest(&path);
            }
        }
    }
    let profile = crate::storage::StoragePaths::from_root(crate::config::config_dir());
    let games = game_saves_in_profile(&profile);
    let device = from_device(&crate::loader::archive::device_dir());
    games
        .into_iter()
        .map(|save| (false, save))
        .chain(device.into_iter().map(|save| (true, save)))
        .collect()
}

/// The saves of all the games in the cache.
///
/// The name shown is the cache folder's — `Zeeboids-6518125-1788761080` —, with the numbering
/// removed: it is the title the player recognizes.
pub fn game_saves(cache: &Path) -> Vec<Save> {
    let mut found = from_cache(cache, &std::collections::HashSet::new());
    found.sort_by(|a, b| a.title.cmp(&b.title));
    found
}

/// The cache's extractions, minus those of the contents in `skip`.
fn from_cache(cache: &Path, skip: &std::collections::HashSet<String>) -> Vec<Save> {
    let Ok(entries) = std::fs::read_dir(cache) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if skip.iter().any(|id| name.ends_with(id.as_str())) {
                return None;
            }
            for_game(&e.path(), &title_of(&name))
        })
        .collect()
}

/// A content's extractions in the cache: the folder is named `<label>-<id>`.
fn extractions_of(cache: &Path, id: &str) -> Vec<PathBuf> {
    std::fs::read_dir(cache)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir() && e.file_name().to_string_lossy().ends_with(id))
        .map(|e| e.path())
        .collect()
}

/// The name to show for a content's overlay.
///
/// What the session wrote in `metadata/<id>.titulo`; without it, the extraction's label; without
/// either, the start of the hash, which at least tells one entry from another.
fn content_title(profile: &crate::storage::StoragePaths, id: &str) -> String {
    let recorded = std::fs::read_to_string(profile.metadata.join(format!("{id}.titulo")))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    recorded
        .or_else(|| {
            extractions_of(&profile.cache, id)
                .first()
                .and_then(|p| p.file_name())
                .map(|n| title_of(&n.to_string_lossy()))
        })
        .unwrap_or_else(|| id.chars().take(12).collect())
}

/// The game saves of a profile.
///
/// Each folder in `saves/mod/` is a module's folder in the shared store, and everything in it is
/// a save.
///
/// The other folders in `saves/` are per-content overlays, from before the store. Until the game
/// opens again, the overlay has not been migrated and shows up as before, together with what an
/// older version wrote inside the extraction. Once migrated it stays on disk as a copy, but it
/// leaves the list: the VFS no longer reads it, and deleting it would not delete the save the
/// game uses.
pub fn game_saves_in_profile(profile: &crate::storage::StoragePaths) -> Vec<Save> {
    let mut with_overlay = std::collections::HashSet::new();
    let mut found: Vec<Save> = Vec::new();
    let store = profile.mod_saves();
    for entry in std::fs::read_dir(&store).into_iter().flatten().flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let dir = entry.file_name().to_string_lossy().into_owned();
        let items: Vec<PathBuf> = std::fs::read_dir(entry.path())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        found.extend(Save::from(dir_title(profile, &dir), items));
    }
    for entry in std::fs::read_dir(&profile.saves).into_iter().flatten().flatten() {
        if !entry.path().is_dir() || entry.path() == store {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if entry.path().join(crate::storage::MIGRATED_MARKER).exists() {
            with_overlay.insert(id);
            continue;
        }
        let mut items: Vec<PathBuf> = std::fs::read_dir(entry.path())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        for extraction in extractions_of(&profile.cache, &id) {
            items.extend(written_files(&extraction).unwrap_or_default());
        }
        items.sort();
        found.extend(Save::from(content_title(profile, &id), items));
        with_overlay.insert(id);
    }
    found.extend(from_cache(&profile.cache, &with_overlay));
    found.sort_by(|a, b| a.title.cmp(&b.title));
    found
}

/// The name to show for a store folder: that of the content that opened it first, or the folder.
fn dir_title(profile: &crate::storage::StoragePaths, dir: &str) -> String {
    std::fs::read_to_string(profile.metadata.join(crate::storage::module_title_file(dir)))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| dir.to_string())
}

/// The title without the numbering the cache adds.
///
/// `Zeeboids-6518125-1788761080` becomes `Zeeboids`, and so does `Zeeboids-<hash>`. The trailing
/// numbers are the package identifier and the timestamp, or the content hash; none of that means
/// anything to whoever is looking at the list.
fn title_of(dir: &str) -> String {
    let mut parts: Vec<&str> = dir.split('-').collect();
    while parts.len() > 1
        && parts
            .last()
            .is_some_and(|p| {
                // The old cache's numbering, or the current one's BLAKE3 hash.
                p.chars().all(|c| c.is_ascii_digit())
                    || (p.len() == 64 && p.chars().all(|c| c.is_ascii_hexdigit()))
            })
    {
        parts.pop();
    }
    parts.join("-").replace('-', " ")
}

/// What is stored in the device's file system, one line per folder.
///
/// Here dates are **not** compared: none of this came from any package. The device only has what
/// the games wrote — Zeeboids' `zeeboiddata`, the system's `shared`.
pub fn from_device(device: &Path) -> Vec<Save> {
    let Ok(entries) = std::fs::read_dir(device) else {
        return Vec::new();
    };
    let mut found: Vec<Save> = entries
        .flatten()
        // The device's old `mod/`, once copied to the saves store, is just a copy.
        .filter(|e| !e.path().join(crate::storage::MIGRATED_MARKER).exists())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            Save::from(name, vec![e.path()])
        })
        .collect();
    found.sort_by(|a, b| a.title.cmp(&b.title));
    found
}

/// Deletes a save. Returns the first error, if any.
pub fn remove(save: &Save) -> std::io::Result<()> {
    for item in &save.items {
        let result = match item.is_dir() {
            true => std::fs::remove_dir_all(item),
            false => std::fs::remove_file(item),
        };
        // Already not existing is not an error: the goal is for it not to exist.
        if let Err(error) = result {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error);
            }
        }
    }
    Ok(())
}

/// The size as something readable.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes} {}", UNITS[0]),
        _ => format!("{value:.1} {}", UNITS[unit]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Builds a cache with one game: the manifest lists the package, and the save stays outside it.
    fn cache_with_game(root: &Path) -> PathBuf {
        let game = root.join("Zeeboids-6518125-1788761080");
        let dir = game.join("mod/274");
        std::fs::create_dir_all(&dir).unwrap();
        let package = ["zeeboids.mod", "zeeboids.sig", "resources.pakz"];
        for name in package {
            std::fs::File::create(dir.join(name))
                .unwrap()
                .write_all(b"pacote")
                .unwrap();
        }
        let mut lines = vec!["mod/".to_string(), "mod/274/".to_string()];
        lines.extend(package.iter().map(|n| format!("mod/274/{n}")));
        std::fs::write(
            game.join(crate::loader::archive::MANIFEST),
            lines.join("\n"),
        )
        .unwrap();
        // The save is not in the manifest — it is what the game wrote later.
        std::fs::create_dir_all(dir.join("zeeboiddata")).unwrap();
        std::fs::File::create(dir.join("zeeboiddata/zeeboid.db"))
            .unwrap()
            .write_all(b"save")
            .unwrap();
        root.to_path_buf()
    }

    /// A cache without a manifest does not enter the list: better left out than guessed at.
    #[test]
    fn a_cache_without_a_manifest_is_ignored() {
        let root = scratch_dir("sem-manifesto");
        cache_with_game(&root);
        std::fs::remove_file(
            root.join("Zeeboids-6518125-1788761080")
                .join(crate::loader::archive::MANIFEST),
        )
        .unwrap();
        assert!(game_saves(&root).is_empty());
    }

    fn scratch_dir(name: &str) -> crate::scratch::TempDir {
        crate::scratch::TempDir::new(&format!("zeebx-saves-{name}"))
    }

    #[test]
    fn what_the_game_wrote_later_is_a_save_and_the_package_is_not() {
        let root = scratch_dir("jogo");
        cache_with_game(&root);
        let saves = game_saves(&root);
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].title, "Zeeboids");
        // Only `zeeboiddata`. The `.mod`, the `.sig` and the `.pakz` belong to the package.
        assert_eq!(saves[0].items.len(), 1);
        assert!(saves[0].items[0].ends_with("zeeboiddata"));
        assert_eq!(saves[0].files, 1);
    }

    /// A zip without folder entries: the `mod` folder belongs to the package because one of its
    /// files is in there.
    #[test]
    fn a_manifest_without_dirs_does_not_make_the_package_a_save() {
        let root = scratch_dir("sem-pastas");
        cache_with_game(&root);
        let game = root.join("Zeeboids-6518125-1788761080");
        let lines: Vec<String> = ["zeeboids.mod", "zeeboids.sig", "resources.pakz"]
            .iter()
            .map(|n| format!("mod/274/{n}"))
            .collect();
        std::fs::write(game.join(crate::loader::archive::MANIFEST), lines.join("\n")).unwrap();
        let saves = game_saves(&root);
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].items.len(), 1, "{:?}", saves[0].items);
        assert!(saves[0].items[0].ends_with("zeeboiddata"));
    }

    #[test]
    fn deleting_removes_the_save_and_keeps_the_package() {
        let root = scratch_dir("apagar");
        cache_with_game(&root);
        let saves = game_saves(&root);
        remove(&saves[0]).unwrap();
        assert!(game_saves(&root).is_empty());
        let dir = root.join("Zeeboids-6518125-1788761080/mod/274");
        assert!(dir.join("zeeboids.mod").exists());
        assert!(dir.join("resources.pakz").exists());
    }

    #[test]
    fn deleting_what_is_already_gone_is_not_an_error() {
        let root = scratch_dir("sumiu");
        cache_with_game(&root);
        let saves = game_saves(&root);
        remove(&saves[0]).unwrap();
        remove(&saves[0]).unwrap();
    }

    /// The overlay and the old copy in the extraction of the same content are **one** save:
    /// deleting only one would leave the VFS reading the other.
    #[test]
    fn the_overlay_and_the_old_copy_are_a_single_save() {
        let root = scratch_dir("perfil");
        let profile = crate::storage::StoragePaths::from_root(root.to_path_buf());
        let id = "ab".repeat(32);
        cache_with_game(&profile.cache);
        std::fs::rename(
            profile.cache.join("Zeeboids-6518125-1788761080"),
            profile.cache.join(format!("Zeeboids-{id}")),
        )
        .unwrap();
        let overlay = profile.saves.join(&id);
        std::fs::create_dir_all(&overlay).unwrap();
        std::fs::write(overlay.join("progresso.db"), b"fase 7").unwrap();

        let saves = game_saves_in_profile(&profile);
        assert_eq!(saves.len(), 1, "{saves:?}");
        assert_eq!(saves[0].title, "Zeeboids");
        assert_eq!(saves[0].items.len(), 2, "the overlay and the old zeeboiddata");

        std::fs::create_dir_all(&profile.metadata).unwrap();
        std::fs::write(profile.metadata.join(format!("{id}.titulo")), "Zeeboids (BR)").unwrap();
        assert_eq!(game_saves_in_profile(&profile)[0].title, "Zeeboids (BR)");

        remove(&saves[0]).unwrap();
        assert!(game_saves_in_profile(&profile).is_empty());
        let package = profile.cache.join(format!("Zeeboids-{id}/mod/274/zeeboids.mod"));
        assert!(package.is_file(), "the package stays");
        assert!(
            !may_have_save(&profile.cache.join(format!("Zeeboids-{id}"))),
            "without the old copy, the pruning may take the extraction"
        );
    }

    #[test]
    fn the_device_lists_each_dir() {
        let root = scratch_dir("aparelho");
        std::fs::create_dir_all(root.join("zeeboiddata")).unwrap();
        std::fs::create_dir_all(root.join("shared")).unwrap();
        std::fs::File::create(root.join("zeeboiddata/zeeboid.db"))
            .unwrap()
            .write_all(b"12345")
            .unwrap();
        let saves = from_device(&root);
        assert_eq!(saves.len(), 2);
        assert_eq!(saves[0].title, "shared");
        assert_eq!(saves[1].title, "zeeboiddata");
        assert_eq!(saves[1].bytes, 5);
    }

    #[test]
    fn the_title_loses_the_cache_numbering() {
        assert_eq!(title_of("Zeeboids-6518125-1788761080"), "Zeeboids");
        assert_eq!(
            title_of("Zeebo-F-C--Foot-Camp-17313250-1788761080"),
            "Zeebo F C  Foot Camp"
        );
        // Without numbering, it stays as it is.
        assert_eq!(title_of("Quake"), "Quake");
        assert_eq!(title_of(&format!("Zeeboids-{}", "0f".repeat(32))), "Zeeboids");
    }

    /// Just for looking: lists this machine's real cache.
    #[test]
    #[ignore]
    fn inspect_the_real_cache() {
        let base = config_dirs();
        for s in game_saves(&base.join("cache")) {
            println!(
                "game     {:<28} {} files  {}",
                s.title,
                s.files,
                size(s.bytes)
            );
        }
        for s in from_device(&base.join("aparelho")) {
            println!(
                "device   {:<28} {} files  {}",
                s.title,
                s.files,
                size(s.bytes)
            );
        }
    }

    fn config_dirs() -> PathBuf {
        PathBuf::from(std::env::var("HOME").unwrap()).join(".config/zeebx")
    }

    #[test]
    fn the_size_comes_out_readable() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1536), "1.5 KB");
        assert_eq!(size(5 * 1024 * 1024), "5.0 MB");
    }
}
