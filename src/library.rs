//! The list of games the emulator finds in a folder.
//!
//! Two layouts coexist. The console installs each title as
//! `<Title>/mod/<id>/<name>.mod`, with the `.mif` in a sibling `mif/`; the SDK samples keep the
//! `.mod` and the `.mif` side by side. Since both show up in practice, the scan looks for
//! `.mod` files at any reasonable depth and infers the title from where the file is.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::loader::archive;
use crate::video::icon::{self, Image};
use crate::loader::miffile::MifFile;
use crate::config;

/// Extensions accepted for a cover image placed next to the game.
const COVER_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "bmp"];

/// How deep to descend from the chosen folder. Three levels cover `<Title>/mod/<id>/` with room
/// to spare; anything more would only serve to scan the whole disk by mistake.
const MAX_DEPTH: usize = 4;

/// Cap on games per scan, so that a folder picked by accident does not freeze the interface.
const MAX_GAMES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    /// How the game appears in the list.
    pub title: String,
    /// The `.mod` to load, or the `.zip` that contains it.
    pub path: PathBuf,
    /// The applet's ClassID, read from the manifest, including inside `.zip` packages.
    pub clsid: Option<u32>,
    /// Whether the path is a package that needs to be extracted before running.
    pub packed: bool,
    /// The image that represents the game in the library. It comes from a cover placed next to
    /// the file or, failing that, from the largest icon the `.mif` holds.
    pub art: Option<Image>,
}

/// Where the information about a title in the console catalog came from.
///
/// The NAND describes the device as it left the factory; local ROMs complete that catalog with
/// titles installed by the user, homebrews included. The source keeps a new scan from erasing
/// records we extracted from the firmware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSource {
    Nand,
    Rom,
}

/// A title the console can show or start.
///
/// The ClassID is the key: file and ZIP names are arbitrary, but this value from the `.mif` is
/// what `ISHELL_StartApplet` receives and what the Z-Wheel writes into its tables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub applet_class: u32,
    pub title: String,
    pub rom_path: PathBuf,
    pub source: CatalogSource,
}

/// Persistent index that joins the NAND inventory with the ROMs chosen by the user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogIndex {
    pub titles: Vec<CatalogEntry>,
}

impl CatalogIndex {
    /// Merges in the current ROM list without touching the titles that came from the NAND.
    pub fn refresh_roms(&mut self, games: &[Game]) {
        self.titles.retain(|entry| entry.source != CatalogSource::Rom);
        self.titles.extend(games.iter().filter_map(|game| {
            Some(CatalogEntry {
                applet_class: game.clsid?,
                title: game.title.clone(),
                rom_path: game.path.clone(),
                source: CatalogSource::Rom,
            })
        }));
        // Two packages may announce the same applet. Keeping the first, in title and path
        // order, makes the result repeatable and leaves the user to sort out the duplicate in
        // the folder.
        self.titles.sort_by(|a, b| {
            a.applet_class
                .cmp(&b.applet_class)
                .then(a.source.cmp(&b.source))
                .then(a.rom_path.cmp(&b.rom_path))
        });
        self.titles.dedup_by_key(|entry| entry.applet_class);
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }
}

/// Stable path of the catalog, kept apart from the ZIPs and their extraction caches.
pub fn catalog_path() -> PathBuf {
    config::config_dir().join("catalog.json")
}

/// Updates and persists the titles found in the configured folder.
pub fn sync_catalog(games: &[Game]) -> std::io::Result<CatalogIndex> {
    let mut index = CatalogIndex::load_from(&catalog_path());
    index.refresh_roms(games);
    index.save_to(&catalog_path())?;
    Ok(index)
}

/// Looks for games in `root`, in title order.
pub fn scan(root: &Path) -> Vec<Game> {
    let mut found = Vec::new();
    collect(root, 0, &mut found);
    let mut games: Vec<Game> = found.into_iter().filter_map(describe).collect();
    games.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    games
}

/// The path that opens the Z-Wheel, given what the user pointed at: the `.zip`, the `.mod` or a
/// folder that contains it. `None` when there is no Z-Wheel there — the `.mif`'s ClassID decides.
pub fn z_wheel_in(path: &Path) -> Option<PathBuf> {
    let z_wheel = Some(crate::session::Z_WHEEL);
    if path.is_dir() {
        let mut found = Vec::new();
        collect(path, 0, &mut found);
        return found
            .into_iter()
            .filter_map(describe)
            .find(|game| game.clsid == z_wheel)
            .map(|game| game.path);
    }
    describe(path.to_path_buf())
        .filter(|game| game.clsid == z_wheel)
        .map(|game| game.path)
}

/// Where to open the Z-Wheel from: the configured path wins; without it, a Z-Wheel that is among
/// the games in the ROM folder works just as well.
///
/// This is the rule the windows use to serve a request to go back to the Z-Wheel, which is why it
/// lives here and not in each of them.
pub fn z_wheel_for(configured: Option<&Path>, games: &[Game]) -> Option<PathBuf> {
    configured.and_then(z_wheel_in).or_else(|| {
        games
            .iter()
            .find(|game| game.clsid == Some(crate::session::Z_WHEEL))
            .map(|game| game.path.clone())
    })
}

/// Looks for the Z-Wheel without asking: first among the games in the ROM folder, then next to
/// it, in `Downloads` and in the home folder, in anything with "wheel" or "tectoy" in its name.
/// Looking only at those names avoids scanning the disk.
pub fn detect_z_wheel(roms: Option<&Path>, games: &[Game]) -> Option<PathBuf> {
    if let Some(game) = games.iter().find(|game| game.clsid == Some(crate::session::Z_WHEEL)) {
        return Some(game.path.clone());
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let dirs = [
        roms.map(Path::to_path_buf),
        roms.and_then(Path::parent).map(Path::to_path_buf),
        home.as_ref().map(|home| home.join("Downloads")),
        home,
    ];
    dirs.into_iter().flatten().find_map(|dir| {
        let mut candidates: Vec<PathBuf> = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name().and_then(|n| n.to_str()).is_some_and(|name| {
                    let name = name.to_lowercase();
                    name.contains("wheel") || name.contains("tectoy")
                })
            })
            .collect();
        candidates.sort();
        candidates.iter().find_map(|path| z_wheel_in(path))
    })
}

/// Turns a file that was found into a game. `None` for what is not a game — a `.zip` with no
/// module inside is just a zip.
fn describe(path: PathBuf) -> Option<Game> {
    let packed = crate::loader::archive::is_packed(&path);
    if !packed {
        return Some(Game {
            title: title_for(&path),
            clsid: applet_clsid(&path),
            art: cover(&path).or_else(|| manifest_art(&manifest(&path)?)),
            path,
            packed,
        });
    }
    let module = archive::find_module(&path)?;
    let manifest = archive::find_manifest(&path, &module);
    Some(Game {
        title: archive::title_of(&path, &module),
        clsid: manifest
            .as_deref()
            .and_then(|data| MifFile::parse(data).ok())
            .and_then(|mif| mif.main_applet()),
        art: cover(&path).or_else(|| manifest_art(manifest.as_deref()?)),
        path,
        packed,
    })
}

/// An image placed next to the game, with the same name as the game.
///
/// This is the way out for anyone who wants a real cover: the `.mif` files only hold menu icons,
/// which are no larger than 65×42, and no ROM ships art bigger than that.
pub fn cover(path: &Path) -> Option<Image> {
    COVER_EXTENSIONS
        .iter()
        .map(|extension| path.with_extension(extension))
        .filter(|candidate| candidate != path)
        .filter_map(|candidate| std::fs::read(candidate).ok())
        .find_map(|data| icon::decode(&data).ok())
}

/// The largest icon in the `.mif` — the one that gives the best image on screen.
fn manifest_art(data: &[u8]) -> Option<Image> {
    let mif = MifFile::parse(data).ok()?;
    mif.images(data)
        .into_iter()
        .filter_map(|image| icon::decode(image).ok())
        .max_by_key(Image::pixels)
}

/// The contents of the `.mif` that goes with a loose `.mod` on disk.
fn manifest(mod_path: &Path) -> Option<Vec<u8>> {
    manifest_paths(mod_path)
        .into_iter()
        .find_map(|path| std::fs::read(path).ok())
}

fn collect(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || found.len() >= MAX_GAMES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // Sorting makes the scan repeatable: `read_dir` promises no order at all, and a list that
    // shuffles between two openings is confusing for the user.
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(&path, depth + 1, found);
        } else if path.extension().and_then(|e| e.to_str()) == Some("mod")
            || crate::loader::archive::is_packed(&path)
        {
            found.push(path);
        }
    }
}

/// The name to show for a `.mod`.
///
/// In the console layout the file name is a dull numeric identifier, and what holds the game's
/// name is the title folder, two levels up. Outside that layout, the file name is all there
/// is.
/// A game's module id: the folder the `.mod` lives in, `mod/<id>/`. It is also the name of its
/// `.mif`. In a package, it comes from the internal path, without extracting.
pub fn module_id(path: &Path) -> Option<String> {
    let internal = match crate::loader::archive::is_packed(path) {
        true => PathBuf::from(crate::loader::archive::find_module(path)?),
        false => path.to_path_buf(),
    };
    Some(internal.parent()?.file_name()?.to_str()?.to_string())
}

/// The name under which things belonging to a single game are stored, such as its settings
/// (see [`crate::ui::settings::GameConfig`]).
///
/// The applet's ClassID, when the manifest gives it: it belongs to the game, not to the file —
/// renaming the package or moving it to another folder loses nothing. Without it, the module
/// id, and lastly the file name. Only characters that are valid in a file name on any system.
pub fn game_key(path: &Path) -> Option<String> {
    let key_id = match applet_clsid(path) {
        Some(class_id) => format!("{class_id:08x}"),
        None => module_id(path)
            .or_else(|| Some(path.file_stem()?.to_str()?.to_string()))?,
    };
    let clear: String = key_id
        .chars()
        .map(|c| match c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            true => c,
            false => '_',
        })
        .collect();
    (!clear.is_empty()).then_some(clear)
}

pub fn title_for(mod_path: &Path) -> String {
    let stem = mod_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let parent = mod_path.parent();
    let is_console_layout = parent
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        == Some("mod");
    if is_console_layout {
        if let Some(title) = parent
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
        {
            return title.to_string();
        }
    }
    stem.to_string()
}

/// The name of an extraction folder without the package's fingerprint.
///
/// `Zeebo-Extreme-Boia-Cross-21503726-1788761080` becomes `Zeebo Extreme Boia Cross`: the two
/// trailing numbers are the `.zip`'s size and date, and the hyphens took the place of the
/// spaces. Today's key is the BLAKE3 digest of the contents, 64 hex digits
/// (`Crash-Bandicoot-Nitro-Kart-3D-0c9aef…`), and it is stripped the same way.
pub fn strip_fingerprint(name: &str) -> String {
    let mut parts: Vec<&str> = name.split('-').collect();
    if parts.len() > 1
        && parts
            .last()
            .is_some_and(|p| p.len() == 64 && p.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        parts.pop();
    }
    for _ in 0..2 {
        if parts.len() > 1 && parts.last().is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
            parts.pop();
        }
    }
    parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Looks for the title's `.mif` and returns the applet's ClassID.
///
/// The installed layout is `<title>/mod/<id>/<name>.mod` with the `.mif` in
/// `<title>/mif/<id>.mif`; the SDK samples keep the `.mif` next to the `.mod`. We cover both
/// cases, plus one more: some packages keep the module in a folder of its own and the manifest
/// **one level above** it.
///
/// A `.mif` that declares no applet is not an answer: the Action Hero 3D package ships two
/// manifests, and one of them declares only a class. That is why the search keeps going through
/// the candidates until it finds one that says which applet to create, instead of stopping at the
/// first one that opens.
pub fn applet_clsid(mod_path: &Path) -> Option<u32> {
    manifest_paths(mod_path)
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .filter_map(|data| MifFile::parse(&data).ok())
        .find_map(|mif| mif.main_applet())
}

/// The **extension** modules that come with a `.mod` in the same package.
///
/// Returns, for each one, its path and the classes it provides. A console package may ship more
/// than one module, and when it does, the ones without an applet exist to export a class: that is
/// how Action Hero 3D gets `IMICRO3D` and Kingdom Hearts gets Superscape's 3D engine. Both ask
/// for a class the console has nowhere except there.
///
/// The search starts one folder **above** the module's and goes down two levels, which is what
/// covers the two real layouts: `mod/<id>/x.mod` next to `mod/<other>/y.mod`, and
/// `<Title>/<Title>/x.mod` next to `<Title>/<Title>_/y.mod`.
pub fn extensions(mod_path: &Path) -> Vec<(PathBuf, Vec<u32>)> {
    let Some(root) = mod_path.parent().and_then(Path::parent) else {
        return Vec::new();
    };
    let mut modules = Vec::new();
    collect_modules(root, 2, &mut modules);
    modules.sort();
    modules
        .into_iter()
        .filter(|path| path != mod_path)
        .filter_map(|path| {
            let mif = mif_in_dir(&path)?;
            let data = std::fs::read(mif).ok()?;
            let classes = MifFile::parse(&data).ok()?.extension().to_vec();
            (!classes.is_empty()).then_some((path, classes))
        })
        .collect()
}

/// The `.mod` files under a directory, down to `depth` levels.
fn collect_modules(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_modules(&path, depth - 1, found);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("mod") {
            found.push(path);
        }
    }
}

/// The `.mif` that matches a `.mod` by the **name of its folder**.
///
/// This is the BREW rule: an installed module lives in `<name>/` and the manifest is
/// `<name>.mif`, next to it. It is narrower than the [`manifest_paths`] search on purpose — here
/// a wrong pairing does not yield "manifest not found", it yields **the game's module offering to
/// serve the class it is itself asking for**, because the two `.mif` files involved cite the
/// same ClassID in the same format.
fn mif_in_dir(mod_path: &Path) -> Option<PathBuf> {
    let dir = mod_path.parent()?;
    let name = dir.file_name()?;
    let candidates = [
        mod_path.with_extension("mif"),
        // `<Title>/<name>/x.mod` -> `<Title>/<name>.mif`
        dir.parent()?.join(name).with_extension("mif"),
        // `<Title>/mod/<id>/x.mod` -> `<Title>/mif/<id>.mif`, the console layout
        dir.parent()?
            .parent()?
            .join("mif")
            .join(name)
            .with_extension("mif"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

/// Where a `.mod`'s `.mif` may be, in the order worth searching.
///
/// The first two candidates are computed names; the last ones come from scanning the folders,
/// which is what covers packages with a layout of their own — Kingdom Hearts keeps the module in
/// `<Title>/<Title>_/swv21brew.mod` and the manifest in `<Title>/<Title>.mif`, and no naming
/// rule finds that. Scanning is cheap: it is two folders, and only until the applet is found.
fn manifest_paths(mod_path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![mod_path.with_extension("mif")];
    let dir = mod_path.parent();
    if let Some(dir) = dir
        && let Some(id) = dir.file_name()
        && let Some(title_dir) = dir.parent().and_then(|p| p.parent())
    {
        // .../mod/<id>/x.mod  ->  .../mif/<id>.mif
        candidates.push(title_dir.join("mif").join(id).with_extension("mif"));
    }
    // The module's folder first, and the one above it after: the manifest closest to the `.mod`
    // is the most likely to be its own, in a package that ships more than one game.
    for search_dir in [dir, dir.and_then(Path::parent)].into_iter().flatten() {
        let Ok(entries) = std::fs::read_dir(search_dir) else {
            continue;
        };
        let mut mifs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("mif"))
            .collect();
        // Stable order: without it, two manifests in the same folder would give different
        // answers depending on how the file system listed them.
        mifs.sort();
        candidates.extend(mifs);
    }
    candidates.dedup();
    candidates
}

/// Whether the title has every word of the search, in any order and anywhere.
///
/// Case, accents and punctuation do not count: "cnk" finds "C.N.K.", "joao" finds "João" and
/// "extreme boia" finds "Zeebo Extreme Bóia Cross".
pub fn matches_search(title: &str, search: &str) -> bool {
    let title = strip_accent(title);
    let compact: String = title.split_whitespace().collect();
    strip_accent(search)
        .split_whitespace()
        .all(|word| title.contains(word) || compact.contains(word))
}

/// The text in lowercase, without accents, with punctuation replaced by spaces and dotted
/// acronyms joined ("C.N.K." becomes "cnk").
fn strip_accent(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            '.' | '\'' | '’' => continue,
            c if c.is_alphanumeric() => c,
            _ => ' ',
        };
        output.push(c);
    }
    output
}

#[cfg(test)]
mod tests {

    #[test]
    fn search_ignores_case_accents_and_punctuation() {
        assert!(matches_search("Zeebo Extreme Bóia Cross", "boia"));
        assert!(matches_search("Zeebo Extreme Bóia Cross", "EXTREME boia"));
        assert!(matches_search("Zeebo Extreme Bóia Cross", "cross zeebo"));
        assert!(matches_search("C.N.K. 3D", "cnk"));
        assert!(matches_search("Resident Evil 4", "resident evil"));
        assert!(matches_search("Double Dragon", "doubledragon"));
        assert!(!matches_search("Double Dragon", "rolima"));
        assert!(!matches_search("Zeebo Extreme Rolimã", "rolima boia"));
    }

    #[test]
    fn empty_search_shows_everything() {
        assert!(matches_search("Tênis", ""));
        assert!(matches_search("Tênis", "   "));
    }
    use super::*;

    #[test]
    fn the_cache_digest_comes_from_the_title() {
        let digest = "0c9aef377a62f6211acb6e1dd1d79ce97c39d5fa2a3db82d36fcf25de2f2222f";
        assert_eq!(
            strip_fingerprint(&format!("Crash-Bandicoot-Nitro-Kart-3D-{digest}")),
            "Crash Bandicoot Nitro Kart 3D"
        );
        assert_eq!(
            strip_fingerprint("Zeebo-Extreme-Boia-Cross-21503726-1788761080"),
            "Zeebo Extreme Boia Cross"
        );
        // A name that is only the digest stays as it is, since that is all there is.
        assert_eq!(strip_fingerprint(digest), digest);
    }

    /// Builds a tree of empty files and returns its root.
    fn tree(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("zeebx-testes-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for file in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, []).unwrap();
        }
        root
    }

    #[test]
    fn catalog_refreshes_roms_without_erasing_what_came_from_the_nand() {
        let mut index = CatalogIndex {
            titles: vec![CatalogEntry {
                applet_class: 7,
                title: "Jogo de fábrica".into(),
                rom_path: PathBuf::from("/nand/jogo.mod"),
                source: CatalogSource::Nand,
            }],
        };
        let games = vec![Game {
            title: "Homebrew".into(),
            path: PathBuf::from("/roms/homebrew.mod"),
            clsid: Some(9),
            packed: false,
            art: None,
        }];

        index.refresh_roms(&games);
        assert_eq!(index.titles.len(), 2);
        assert!(index.titles.iter().any(|entry| entry.applet_class == 7));
        assert!(index.titles.iter().any(|entry| entry.applet_class == 9));

        index.refresh_roms(&[]);
        assert_eq!(index.titles.len(), 1);
        assert_eq!(index.titles[0].source, CatalogSource::Nand);
    }

    /// Kingdom Hearts' manifest sits one level above the module's folder.
    ///
    /// `<Title>/<Title>_/swv21brew.mod` with the `.mif` in `<Title>/<Title>.mif`: no naming
    /// rule finds that, and the game would not open because we did not know which applet to
    /// create. This is why the search scans the folders instead of only computing names.
    #[test]
    fn the_manifest_may_be_one_level_above_the_module_dir() {
        let root = tree(
            "mif-acima",
            &[
                "Kingdon Hearts/Kingdon Hearts_/swv21brew.mod",
                "Kingdon Hearts/Kingdon Hearts.mif",
            ],
        );
        let modulo = root.join("Kingdon Hearts/Kingdon Hearts_/swv21brew.mod");
        let candidates = manifest_paths(&modulo);
        assert!(
            candidates.contains(&root.join("Kingdon Hearts/Kingdon Hearts.mif")),
            "{candidates:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The module's folder comes before the one above it, and the order does not depend on the
    /// file system.
    ///
    /// In a package with more than one game, the manifest closest to the `.mod` is the most
    /// likely to be its own — and two `.mif` files in the same folder must always be tried in the
    /// same order, or the same game would open differently on two machines.
    #[test]
    fn the_manifest_closest_to_the_module_comes_first() {
        let root = tree(
            "mif-ordem",
            &[
                "jogo/mod/b.mif",
                "jogo/mod/a.mif",
                "jogo/mod/x.mod",
                "jogo/de-cima.mif",
            ],
        );
        let candidates = manifest_paths(&root.join("jogo/mod/x.mod"));
        let inside_only: Vec<&PathBuf> = candidates
            .iter()
            .filter(|p| p.starts_with(root.join("jogo/mod")))
            .collect();
        assert_eq!(
            inside_only,
            [
                &root.join("jogo/mod/x.mif"),
                &root.join("jogo/mod/a.mif"),
                &root.join("jogo/mod/b.mif"),
            ]
        );
        let topmost = candidates
            .iter()
            .position(|p| p == &root.join("jogo/de-cima.mif"));
        assert_eq!(topmost, Some(candidates.len() - 1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_title_comes_from_the_dir_in_the_console_layout() {
        // `<Title>/mod/<id>/<name>.mod` — the file name is an identifier, the title is three
        // levels up.
        let path = Path::new("/roms/Zeebo Sports Peteca/mod/279159/zeebopeteca.mod");
        assert_eq!(title_for(path), "Zeebo Sports Peteca");
    }

    #[test]
    fn outside_the_console_layout_the_title_is_the_file_name() {
        assert_eq!(title_for(Path::new("/roms/helloworld.mod")), "helloworld");
        // A folder named `mod` in the wrong place does not make the grandparent a title.
        assert_eq!(title_for(Path::new("/mod/x.mod")), "x");
    }

    #[test]
    fn the_scan_finds_both_layouts_and_sorts_by_title() {
        let root = tree(
            "biblioteca",
            &[
                "Quake/mod/274802/quake.mod",
                "Quake/mif/274802.mif",
                "Crash/mod/274214/cnk2.mod",
                "avulso.mod",
                "leiame.txt",
                "Quake/mod/274802/dados.bin",
            ],
        );
        let games = scan(&root);
        let titles: Vec<&str> = games.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["avulso", "Crash", "Quake"]);
        // The `.mif` is empty, so there is no ClassID — but the game stays in the list, because
        // whether the module is any good is for the emulator to say, not the scan.
        assert!(games.iter().all(|g| g.clsid.is_none()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_dir_returns_an_empty_list() {
        let games = scan(&std::env::temp_dir().join("zeebx-nao-existe-mesmo"));
        assert!(games.is_empty());
    }
}
