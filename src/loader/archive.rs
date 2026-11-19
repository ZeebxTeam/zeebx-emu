//! Games stored in `.zip` files.
//!
//! A Zeebo title is a tree of files — the `.mod`, the `.mif` and the data — and it commonly
//! circulates compressed. The emulator does not read from inside the archive: it extracts to a
//! cache folder and runs from there. Games write files (Peteca has a `.sav`), and writing back
//! into a zip is not something anyone wants to do; extracting solves that for free and leaves the
//! rest of the emulator unaware that the zip exists.

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config;
use crate::loader::miffile::MifFile;
use crate::loader::seven_z;
use crate::storage::ContentId;

/// Limits applied before decompressing untrusted content. They are generous so as not to reject
/// legitimate games, but they keep a ZIP with malicious metadata from consuming unbounded
/// space/memory.
pub(crate) const MAX_ENTRIES: usize = 20_000;
pub(crate) const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// A `.mif` is small metadata; its own cap avoids allocating hundreds of MiB just to pick a module.
pub(crate) const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct ArchiveLimits {
    pub(crate) entries: usize,
    pub(crate) file_bytes: u64,
    pub(crate) total_bytes: u64,
}

impl ArchiveLimits {
    /// The usual limits, for callers that need to extract outside the `extract_in` path.
    #[cfg(test)]
    pub(crate) fn fallback() -> Self {
        ARCHIVE_LIMITS
    }
}

const ARCHIVE_LIMITS: ArchiveLimits = ArchiveLimits {
    entries: MAX_ENTRIES,
    file_bytes: MAX_FILE_BYTES,
    total_bytes: MAX_TOTAL_BYTES,
};

/// Keeps two openings in the same process from deleting each other's partial directory.
static NEXT_PARTIAL: AtomicU64 = AtomicU64::new(0);

fn partial_dir(target: &Path) -> PathBuf {
    let serial = NEXT_PARTIAL.fetch_add(1, Ordering::Relaxed);
    target.with_extension(format!("partial-{}-{serial}", std::process::id()))
}

/// Checks metadata before reading any payload. `enclosed_name` is checked again at the point of
/// extraction, because it is what decides the output path.
fn validate_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    limits: ArchiveLimits,
) -> std::io::Result<()> {
    if archive.len() > limits.entries {
        return Err(std::io::Error::other("the zip has too many entries"));
    }
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(std::io::Error::other)?;
        if entry.enclosed_name().is_none() {
            return Err(std::io::Error::other("the zip contains an unsafe path"));
        }
        if entry.is_symlink() {
            return Err(std::io::Error::other("the zip contains a symbolic link"));
        }
        if entry.is_dir() {
            continue;
        }
        if entry.size() > limits.file_bytes {
            return Err(std::io::Error::other("the zip contains a file that is too large"));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| std::io::Error::other("the zip has an invalid size"))?;
        if total > limits.total_bytes {
            return Err(std::io::Error::other("the zip decompresses to too many bytes"));
        }
    }
    Ok(())
}
/// Where extractions are kept.
pub fn cache_dir() -> PathBuf {
    config::config_dir().join("cache")
}

/// The root of the device's file system, shared by all games.
///
/// On the console there is **one** file system: Zeeboids saves its characters to
/// `fs:/zeeboiddata/zeeboid.db` and Zeebo F.C. opens that same path to import them. One root per
/// installation would make each game see a folder of its own, and F.C. would conclude — rightly,
/// from its point of view — that Zeeboids is not installed.
pub fn device_dir() -> PathBuf {
    config::config_dir().join("aparelho")
}

/// The system font, where the console keeps it: `fs:/shared/fonts/tectoy.ttf`.
///
/// The path is in the firmware. It is the font `IDisplay` writes with when the game asks for a
/// device font (`AEE_FONT_NORMAL` and its siblings) — and most games ship no font of their own,
/// because on the console they did not need to.
///
/// The file does **not** ship with the emulator: it belongs to TecToy. But it comes in the
/// Z-Wheel package, which is the console's system, so when the device does not have it yet and
/// the Z-Wheel has been opened at some point, it is installed from there — the same thing the
/// Z-Wheel did on the console.
pub fn system_font() -> Option<PathBuf> {
    system_font_in(&cache_dir(), &device_dir())
}

/// Like [`system_font`], but with the cache and device root the frontend provided.
///
/// The version without arguments is the desktop path, which reads the user's configuration. A
/// frontend like Libretro extracts the package into **its own cache** and writes `fs:/` into its
/// own root: looking in the desktop's location returned `None` even with `tectoy.ttf` inside the
/// Z-Wheel package — and without a font the game draws no text at all, which was the case with
/// Double Dragon.
pub fn system_font_in(cache: &Path, device: &Path) -> Option<PathBuf> {
    let dest = device.join("shared").join("fonts").join("tectoy.ttf");
    if dest.is_file() {
        return Some(dest);
    }
    let origin = std::fs::read_dir(cache)
        .ok()?
        .filter_map(Result::ok)
        .flat_map(|package| {
            std::fs::read_dir(package.path().join("mod"))
                .into_iter()
                .flatten()
                .flatten()
        })
        .map(|modulo| modulo.path().join("tectoy.ttf"))
        .find(|ttf| ttf.is_file())?;
    std::fs::create_dir_all(dest.parent()?).ok()?;
    std::fs::copy(origin, &dest).ok()?;
    Some(dest)
}

/// Installs the `tectoy.ttf` that comes **inside the Z-Wheel package** into the device root.
///
/// This is the path for someone who opens a game without ever having opened the Z-Wheel: the
/// system font lives in its package, and without a font the game draws no system text — the
/// Double Dragon report listed "text on screen (no font to draw it yet)".
///
/// **It is not the cause of its white screen.** Measured: with the font installed, the Double
/// Dragon frame stayed white and uniform. What was missing there was something else in the
/// execution path.
///
/// Extracts **only** the font file: materializing the whole package for 190 KB is not worth it.
pub fn install_package_font(package: &Path, device: &Path) -> Option<PathBuf> {
    let dest = device.join("shared").join("fonts").join("tectoy.ttf");
    if dest.is_file() {
        return Some(dest);
    }
    // Two forms of the Z-Wheel show up in the same collection: the package's `.zip` and an
    // already-extracted copy, where the font sits **next to** the module (`mod/274755/tectoy.ttf`).
    // Handling only the zip left the extracted collection without a font.
    let bytes = if seven_z::is_seven_z(package) {
        // In a `.7z` the font is looked up by name, and the format is decided by the signature:
        // the package in the collection may have been renamed.
        let name = seven_z::list(package)?
            .into_iter()
            .map(|(name, _)| name)
            .find(|name| name.to_ascii_lowercase().ends_with(".ttf"))?;
        seven_z::read(package, &name, MAX_MANIFEST_BYTES)?
    } else {
        match package.extension().and_then(|e| e.to_str()) {
        Some("zip") => {
            let file = std::fs::File::open(package).ok()?;
            let mut archive = zip::ZipArchive::new(file).ok()?;
            let index = (0..archive.len()).find(|&i| {
                archive.by_index(i).is_ok_and(|entry| {
                    entry
                        .name()
                        .is_ok_and(|name| name.replace('\\', "/").ends_with("/tectoy.ttf"))
                })
            })?;
            let mut entry = archive.by_index(index).ok()?;
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).ok()?;
            bytes
        }
        _ => std::fs::read(package.parent()?.join("tectoy.ttf")).ok()?,
        }
    };
    std::fs::create_dir_all(dest.parent()?).ok()?;
    std::fs::write(&dest, bytes).ok()?;
    Some(dest)
}

/// The file is a package: `.zip` or `.7z`.
///
/// **The extension does not decide the format** — the signature does, at open time — but it does
/// decide whether the file is worth treating as a package, and that is the question the rest of
/// the emulator asks. A title circulates in both forms, and refusing `.7z` here gave the worst
/// possible symptom: the game simply "does not load", without saying why.
pub fn is_packed(path: &Path) -> bool {
    matches!(
        path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("zip" | "7z")
    )
}

/// The internal path of the `.mod` inside the package, if there is one.
///
/// When there is more than one, it decides in this order:
///
/// 1. **Having an applet in the `.mif`.** A module whose manifest declares no applet cannot be
///    started. The Action Hero 3D package ships two games, and the other one — IMICRO3D —
///    declares only a class, with no applet: it used to win, and the whole package would not
///    open.
/// 2. **The console layout**, `<Title>/mod/<id>/<name>.mod`, even though it is the deepest: a
///    package with the game and some extra has the game's module there, and it is the extra that
///    tends to be loose.
/// 3. The shortest path.
pub fn find_module(package: &Path) -> Option<String> {
    let names = archive_entry_names(package)?;
    // The manifest is only read when there is more than one candidate: with only one, it is the
    // answer anyway, and opening the package again would be wasted work for every game in the
    // collection.
    pick_module(names, |name| {
        find_manifest(package, name)
            .and_then(|data| MifFile::parse(&data).ok())
            .and_then(|mif| mif.main_applet())
            .is_some()
    })
}

/// The names inside the package, whether it is a `.zip` or a `.7z`.
///
/// This is the only question the rest of the file asks about the format: from here down, zip and
/// 7z are the same package. A `.7z` is recognized by its signature, not by its extension — a
/// `.7z` renamed to `.zip` is still a `.7z`.
fn archive_entry_names(package: &Path) -> Option<Vec<String>> {
    if seven_z::is_seven_z(package) {
        return seven_z::list(package).map(|listing| listing.into_iter().map(|(name, _)| name).collect());
    }
    let file = std::fs::File::open(package).ok()?;
    let mut zip = zip::ZipArchive::new(file).ok()?;
    validate_archive(&mut zip, ARCHIVE_LIMITS).ok()?;
    let mut names = Vec::with_capacity(zip.len());
    for index in 0..zip.len() {
        let entry = zip.by_index(index).ok()?;
        // `enclosed_name` refuses paths with `..` or an absolute root, which is how a malicious
        // zip would write outside the destination folder.
        entry.enclosed_name()?;
        names.push(entry.name().ok()?.into_owned());
    }
    Some(names)
}

/// Picks the `.mod`, in the order documented in [`find_module`].
///
/// It is separate because the choice is the same for zip and for 7z — what changes is who lists
/// the names. Two copies of this rule would diverge at the first tweak.
fn pick_module(
    names: Vec<String>,
    has_applet: impl Fn(&str) -> bool,
) -> Option<String> {
    let mut candidates: Vec<(bool, usize, String)> = Vec::new();
    for name in names {
        if Path::new(&name).extension().and_then(|e| e.to_str()) != Some("mod") {
            continue;
        }
        let parts: Vec<&str> = name.split(['/', '\\']).collect();
        let console = parts.len() >= 4 && parts[parts.len() - 3] == "mod";
        candidates.push((console, parts.len(), name));
    }
    let alone = candidates.len() <= 1;
    candidates
        .into_iter()
        .max_by_key(|(console, depth, name)| {
            (alone || has_applet(name), *console, std::cmp::Reverse(*depth))
        })
        .map(|(_, _, name)| name)
}

/// The contents of the `.mif` that goes with `module` inside the zip.
///
/// The title's `.mif` sits in `<Title>/mif/<id>.mif`, a sibling of the `mod/` folder. When there
/// is more than one, the one matching the module's identifier wins; failing that, the first one
/// will do.
pub fn find_manifest(package: &Path, module: &str) -> Option<Vec<u8>> {
    // `<Title>/mod/<id>/x.mod` -> the identifier is the folder that contains the module.
    let parts: Vec<&str> = module.split('/').collect();
    let id = parts.get(parts.len().checked_sub(2)?).copied();

    if seven_z::is_seven_z(package) {
        let listing = seven_z::list(package)?;
        let name = pick_mif(listing.into_iter().filter(|(_, dir)| !dir).map(|(name, _)| name), id)?;
        return seven_z::read(package, &name, MAX_MANIFEST_BYTES);
    }

    let file = std::fs::File::open(package).ok()?;
    let mut zip = zip::ZipArchive::new(file).ok()?;
    validate_archive(&mut zip, ARCHIVE_LIMITS).ok()?;
    let mut names = Vec::with_capacity(zip.len());
    for index in 0..zip.len() {
        let entry = zip.by_index(index).ok()?;
        entry.enclosed_name()?;
        names.push(entry.name().ok()?.into_owned());
    }
    let name = pick_mif(names.into_iter(), id)?;
    let mut entry = zip.by_name(&name).ok()?;
    if entry.size() > MAX_MANIFEST_BYTES {
        return None;
    }
    let mut data = Vec::with_capacity(entry.size() as usize);
    entry
        .by_ref()
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() as u64 <= MAX_MANIFEST_BYTES).then_some(data)
}

/// Says whether an extracted file is still byte-for-byte identical to the package's original
/// entry.
///
/// It serves the migration of frontends that used to let the game write inside the cache: some
/// titles ship a "seed" save in the ZIP itself and later overwrite it. In that case the manifest
/// alone cannot tell package content from the player's progress.
pub fn entry_matches_file(
    package: &Path,
    name: &str,
    file: &Path,
) -> std::io::Result<bool> {
    let meta = std::fs::metadata(file)?;
    if seven_z::is_seven_z(package) {
        let data = seven_z::read(package, name, MAX_FILE_BYTES)
            .ok_or_else(|| std::io::Error::other("could not read the 7z entry"))?;
        if data.len() as u64 != meta.len() {
            return Ok(false);
        }
        return Ok(std::fs::read(file)? == data);
    }

    let origin = std::fs::File::open(package)?;
    let mut zip = zip::ZipArchive::new(origin).map_err(std::io::Error::other)?;
    validate_archive(&mut zip, ARCHIVE_LIMITS)?;
    let mut entry = zip.by_name(name).map_err(std::io::Error::other)?;
    if entry.size() != meta.len() {
        return Ok(false);
    }
    let mut current = std::fs::File::open(file)?;
    let mut a = [0u8; 64 * 1024];
    let mut b = [0u8; 64 * 1024];
    loop {
        let la = entry.read(&mut a)?;
        let lb = current.read(&mut b)?;
        if la != lb || a[..la] != b[..lb] {
            return Ok(false);
        }
        if la == 0 {
            return Ok(true);
        }
    }
}

/// Picks the title's `.mif` among the package's names.
///
/// The one matching the module's identifier wins; with no match, the first one will do — the
/// same criterion for zip and for 7z.
fn pick_mif(names: impl Iterator<Item = String>, id: Option<&str>) -> Option<String> {
    let mut first: Option<String> = None;
    for name in names {
        if !name.ends_with(".mif") {
            continue;
        }
        if id.is_some_and(|id| name.ends_with(&format!("/{id}.mif"))) {
            return Some(name);
        }
        first.get_or_insert(name);
    }
    first
}

/// The title the zip announces.
///
/// A package made from the console's directory has the title folder at its root, and the name
/// comes from it. Without that folder, the name of the file itself will do.
pub fn title_of(zip: &Path, module: &str) -> String {
    let stem = zip
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let parts: Vec<&str> = module.split('/').collect();
    // `<Title>/mod/<id>/<name>.mod`
    match parts.len() >= 4 && parts[1] == "mod" {
        true => parts[0].to_string(),
        false => stem,
    }
}

/// Extracts the zip into the cache and returns the path of the `.mod`.
///
/// The destination folder is named after the BLAKE3 hash of the file's bytes: changing the
/// contents produces another folder, and an identical copy reuses the earlier extraction without
/// relying on size/date.
pub fn extract(zip: &Path) -> std::io::Result<PathBuf> {
    extract_in(zip, &cache_dir())
}

/// Like [`extract`], but with an explicit cache for the frontend that controls its persistent root.
pub fn extract_in(zip: &Path, cache: &Path) -> std::io::Result<PathBuf> {
    // Selection and extraction still need to open the file more than once. Checking the digest
    // between those phases refuses a ROM swapped mid-operation, instead of putting different bytes
    // under the wrong content key.
    let source_fingerprint = fingerprint(zip)?;
    let module = find_module(zip)
        .ok_or_else(|| std::io::Error::other("the package contains no .mod file"))?;
    if fingerprint(zip)? != source_fingerprint {
        return Err(std::io::Error::other("the zip changed while it was being analyzed"));
    }
    let target = cache.join(&source_fingerprint);
    let extracted = target.join(&module);
    // Already extracted under a strong identity: nothing to do.
    if extracted.is_file() {
        return Ok(extracted);
    }
    // The previous version used name+size+mtime. We keep the old cache as a transitional
    // read-only path so as not to hide saves that still live inside it; we never write new
    // content there, because that key could collide.
    let legacy = cache.join(legacy_fingerprint(zip)?).join(&module);
    if legacy.is_file() {
        return Ok(legacy);
    }

    // We never let a partially extracted cache look usable. A crash in the middle of the copy
    // stays in `.partial`, which the next opening removes before trying again.
    let partial = partial_dir(&target);
    let result = (|| -> std::io::Result<()> {
        // A `.7z` decompresses through its own path, and **from here down the path is the
        // same**: check the digest, write the manifest, require the `.mod` to be present and
        // publish the extraction with a rename. Returning early from here left the contents in a
        // `.partial` folder and returned a path that did not exist — which is what happened in
        // the first version.
        if seven_z::is_seven_z(zip) {
            std::fs::create_dir_all(&partial)?;
            seven_z::extract(zip, &partial, ARCHIVE_LIMITS)?;
        } else {
        let file = std::fs::File::open(zip)?;
        let mut archive = zip::ZipArchive::new(file).map_err(std::io::Error::other)?;
        validate_archive(&mut archive, ARCHIVE_LIMITS)?;
        let mut extracted_bytes = 0u64;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(std::io::Error::other)?;
            let Some(name) = entry.enclosed_name() else {
                continue;
            };
            let out = partial.join(name);
            if entry.is_dir() {
                std::fs::create_dir_all(&out)?;
                continue;
            }
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // Does not allocate a Vec of the size the ZIP declares. On top of the metadata
            // pre-check, it limits the real stream: a lying header cannot write past the cap
            // during decompression.
            let remaining = MAX_TOTAL_BYTES
                .checked_sub(extracted_bytes)
                .ok_or_else(|| std::io::Error::other("the zip decompresses to too many bytes"))?;
            let allowed = MAX_FILE_BYTES.min(remaining);
            let mut output = std::fs::File::create(&out)?;
            let written = std::io::copy(&mut entry.by_ref().take(allowed + 1), &mut output)?;
            if written > allowed {
                return Err(std::io::Error::other(
                    "the zip exceeded the limit while decompressing",
                ));
            }
            extracted_bytes = extracted_bytes
                .checked_add(written)
                .ok_or_else(|| std::io::Error::other("the zip has an invalid size"))?;
        }
        }
        if fingerprint(zip)? != source_fingerprint {
            return Err(std::io::Error::other("the package changed during extraction"));
        }
        write_manifest(zip, &partial)?;
        if fingerprint(zip)? != source_fingerprint {
            return Err(std::io::Error::other("the package changed during extraction"));
        }
        if !partial.join(&module).is_file() {
            return Err(std::io::Error::other(
                "the .mod did not appear after extracting the package",
            ));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Err(error) = std::fs::rename(&partial, &target) {
            // Another process may have published the same hash first. If its publication is
            // complete, it is equivalent to ours; otherwise, we do not hide the error.
            if target.join(&module).is_file() {
                let _ = std::fs::remove_dir_all(&partial);
            } else {
                return Err(error);
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&partial);
    }
    result?;
    // **Pruning happens here, and before it happened nowhere.** The cap has existed as long as
    // the cache has (see [`CACHE_LIMIT_BYTES`]), but `prune_cache` was only called by the tests:
    // in real use the folder grew without end, one opened game at a time, and anyone who opened a
    // whole collection kept the extraction of every one of them forever. Measured on an
    // installation in normal use: 143 MB sitting idle.
    //
    // This is the moment, and not the opening: pruning only after **adding** something is what
    // makes the cost proportional to the growth. Reusing an extraction that already exists (the
    // common path, just above) does not grow the cache and does not need to scan the folder.
    //
    // A pruning error does not bring down the game's opening: the contents are already extracted
    // and usable, and "could not delete old cache" is no reason to turn away someone who just
    // wanted to play.
    match prune_cache(cache, Some(&extracted), CACHE_LIMIT_BYTES) {
        Ok(freed) if freed > 0 => crate::logging!(
            crate::logging::Level::Info,
            "loader",
            "pruning the extraction cache at {} freed {} MiB",
            cache.display(),
            freed / (1024 * 1024)
        ),
        Ok(_) => crate::logging!(
            crate::logging::Level::Debug,
            "loader",
            "extraction cache at {} was already within the cap",
            cache.display()
        ),
        Err(prune_error) => crate::logging!(
            crate::logging::Level::Warning,
            "loader",
            "could not prune the extraction cache at {}: {prune_error}",
            cache.display()
        ),
    }
    Ok(extracted)
}

/// Cap on the cache of extracted content, in bytes.
///
/// Without it, every opened game leaves its extraction behind forever: a test collection of 62
/// titles goes past 1.5 GB and fills the disk of someone who just wanted to play. The value is
/// generous for the game in use and tight for the whole collection.
pub const CACHE_LIMIT_BYTES: u64 = 512 * 1024 * 1024;

/// The name of the file that lists what came from the package.
pub const MANIFEST: &str = ".zeebx-pacote";

/// Deletes old extractions until the cache fits in `limit`, preserving the one that contains
/// `keep`.
///
/// `keep` is a path **inside** the extraction in use — the `.mod`, for example. The entry that
/// contains it is never removed, even if it alone blows the cap: deleting the running game would
/// be worse than a full disk. The others go from oldest to newest, by modification date, except
/// those that hold a save (see [`crate::ui::saves::may_have_save`]).
///
/// Returns how many bytes were freed.
pub fn prune_cache(cache: &Path, keep: Option<&Path>, limit: u64) -> std::io::Result<u64> {
    let Ok(entries) = std::fs::read_dir(cache) else {
        // Without a cache there is nothing to prune.
        return Ok(0);
    };
    let mut items: Vec<(std::time::SystemTime, PathBuf, u64)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if keep.is_some_and(|keep| keep.starts_with(&path)) {
            continue;
        }
        // **The save lives in here.** On the desktop (`StoragePaths::from_root`, no overlay) the
        // game writes in the middle of its own extraction, and deleting the folder took the
        // progress with it — which is what pruning did as soon as it started running for real.
        // An extraction the game touched stays, even if it blows the cap; one left over from an
        // interrupted extraction may go.
        let partial = path
            .extension()
            .is_some_and(|e| e.to_string_lossy().starts_with("partial-"));
        if !partial && crate::ui::saves::may_have_save(&path) {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        items.push((age, path, size_on_disk(&entry.path())));
    }
    let mut total: u64 = items.iter().map(|(_, _, bytes)| bytes).sum();
    // The entry in use is already out of the sum; the cap applies to what may go.
    items.sort_by_key(|(age, _, _)| *age);
    let mut freed = 0u64;
    for (_, path, bytes) in items {
        if total <= limit {
            break;
        }
        if std::fs::remove_dir_all(&path).is_ok() {
            total = total.saturating_sub(bytes);
            freed += bytes;
        }
    }
    Ok(freed)
}

/// Size of a directory tree, in file bytes.
fn size_on_disk(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => size_on_disk(&entry.path()),
            Ok(_) => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// Writes, inside the cache, the list of what the zip brought.
///
/// It exists so the save manager can tell, **without guessing**, what is a save and what is game
/// content. The first attempt compared dates — anything newer than the `.mod` would be a save —
/// and it falls apart: the files of a single extraction differ by milliseconds, and Alice's 15 MB
/// `resources.pakz` finished being written after the `.mod`. In a list with a delete button,
/// getting it wrong like that deletes the game.
///
/// The list comes from the zip, which is the source: what is in it belongs to the package, the
/// rest the game wrote.
pub fn write_manifest(package: &Path, dest: &Path) -> std::io::Result<()> {
    // The names come from the same place as the module choice, so the manifest covers zip and 7z
    // without a separate path. It is what tells the engine which files came from the package —
    // without it, an extracted `.7z` would treat the contents as if they belonged to the device.
    let mut names: Vec<String> = archive_entry_names(package).unwrap_or_default();
    names.sort();
    names.dedup();
    std::fs::write(dest.join(MANIFEST), names.join("\n"))
}

/// Where the zip was extracted, whether or not it exists.
pub fn cache_for(zip: &Path) -> std::io::Result<PathBuf> {
    Ok(cache_dir().join(fingerprint(zip)?))
}

/// Writes the manifest of an already-extracted package that does not have one yet.
///
/// Caches made before the manifest existed have no way to know what belonged to the package.
/// Instead of leaving them out of the save manager — or worse, guessing by date — the manifest is
/// rebuilt from the zip, which is still there.
pub fn complete_manifest(zip: &Path) -> std::io::Result<()> {
    let dest = cache_for(zip)?;
    if dest.is_dir() {
        if !dest.join(MANIFEST).is_file() {
            write_manifest(zip, &dest)?;
        }
        return Ok(());
    }
    // See `extract`: old caches could contain relative saves. Even though the old key is not used
    // for new content, rebuilding its manifest keeps those saves visible in the UI.
    let legacy = cache_dir().join(legacy_fingerprint(zip)?);
    if legacy.is_dir() && !legacy.join(MANIFEST).is_file() {
        write_manifest(zip, &legacy)?;
    }
    Ok(())
}

/// A folder name that identifies the contents, not where/when they were copied.
///
/// The title is only a human label for the save manager; the key that prevents collisions is the
/// full BLAKE3 digest at the end. Two copies with the same name and bytes go to the same cache.
fn fingerprint(zip: &Path) -> std::io::Result<String> {
    let content = ContentId::from_reader(std::fs::File::open(zip)?)?;
    Ok(format!("{}-{}", cache_label(zip), content.as_str()))
}

fn cache_label(zip: &Path) -> String {
    zip.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("jogo")
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// The previous cache key, only to keep reading saves/caches already created by the old version.
fn legacy_fingerprint(zip: &Path) -> std::io::Result<String> {
    let meta = std::fs::metadata(zip)?;
    let stamp = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Ok(format!("{}-{}-{stamp}", cache_label(zip), meta.len()))
}

/// Exact path of the cache from before the content hash, for frontend migrations.
pub(crate) fn legacy_cache_in(zip: &Path, cache: &Path) -> std::io::Result<PathBuf> {
    Ok(cache.join(legacy_fingerprint(zip)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Writes a zip with the given paths, all with dummy contents.
    fn make_zip(name: &str, entries: &[&str]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("zeebx-teste-{name}.zip"));
        let file = std::fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for entry in entries {
            writer.start_file(*entry, options).unwrap();
            writer.write_all(entry.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    #[test]
    fn finds_the_module_closest_to_the_root() {
        let zip = make_zip(
            "achar",
            &[
                "Peteca/mif/279159.mif",
                "Peteca/mod/279159/zeebopeteca.mod",
                "extras/demo/outro.mod",
            ],
        );
        assert_eq!(
            find_module(&zip).as_deref(),
            Some("Peteca/mod/279159/zeebopeteca.mod")
        );
        let _ = std::fs::remove_file(&zip);
    }

    #[test]
    fn a_zip_without_a_module_is_not_a_game() {
        let zip = make_zip("vazio", &["leiame.txt", "capa.png"]);
        assert_eq!(find_module(&zip), None);
        let _ = std::fs::remove_file(&zip);
    }

    #[test]
    fn the_title_comes_from_the_inner_dir_when_it_exists() {
        assert_eq!(
            title_of(
                Path::new("/baixados/279159.zip"),
                "Zeebo Sports Peteca/mod/279159/zeebopeteca.mod"
            ),
            "Zeebo Sports Peteca"
        );
        // Without the title folder, the file name is all there is.
        assert_eq!(
            title_of(Path::new("/baixados/peteca.zip"), "zeebopeteca.mod"),
            "peteca"
        );
    }

    #[test]
    fn fingerprint_comes_from_bytes_not_dates() {
        let first = make_zip("hash-origem", &["mod/1/jogo.mod"]);
        let dir = std::env::temp_dir().join("zeebx-teste-hash-copia");
        std::fs::create_dir_all(&dir).unwrap();
        let second = dir.join("zeebx-teste-hash-origem.zip");
        std::fs::copy(&first, &second).unwrap();
        assert_eq!(fingerprint(&first).unwrap(), fingerprint(&second).unwrap());
        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn entry_limit_is_refused_before_extraction() {
        let zip = make_zip("limite", &["a", "b", "c"]);
        let file = std::fs::File::open(&zip).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let err = validate_archive(
            &mut archive,
            ArchiveLimits {
                entries: 2,
                file_bytes: MAX_FILE_BYTES,
                total_bytes: MAX_TOTAL_BYTES,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("entries"));
        let _ = std::fs::remove_file(zip);
    }

    #[test]
    fn pruning_drops_old_cache_and_keeps_the_game_in_use() {
        let root = std::env::temp_dir().join("zeebx-teste-poda");
        let _ = std::fs::remove_dir_all(&root);
        let cache = root.join("cache");
        let old = cache.join("jogo-antigo");
        let in_use = cache.join("jogo-em-uso");
        let with_save = cache.join("jogo-com-save");
        let without_manifest = cache.join("jogo-sem-manifesto");
        for dir in [&old, &in_use, &with_save, &without_manifest] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("dados.bin"), vec![0u8; 4096]).unwrap();
        }
        for dir in [&old, &with_save] {
            std::fs::write(dir.join(MANIFEST), "dados.bin\n").unwrap();
        }
        std::fs::write(with_save.join("progresso.db"), b"fase 7").unwrap();
        let modulo = in_use.join("mod/1/jogo.mod");
        std::fs::create_dir_all(modulo.parent().unwrap()).unwrap();
        std::fs::write(&modulo, b"mod").unwrap();

        // Zero cap: only pure package content goes; the game in use and the saves stay.
        let freed = prune_cache(&cache, Some(&modulo), 0).unwrap();
        assert!(freed > 0, "should have freed space");
        assert!(!old.exists(), "the old extraction goes");
        assert!(modulo.is_file(), "the extraction in use stays");
        assert!(with_save.join("progresso.db").is_file(), "the extraction with a save stays");
        assert!(without_manifest.exists(), "without a manifest there is no way to know, so it stays");

        // With nothing above the cap, nothing is touched.
        assert_eq!(prune_cache(&cache, Some(&modulo), u64::MAX).unwrap(), 0);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_returns_the_module_and_reuses_it_the_second_time() {
        let zip = make_zip(
            "extrair",
            &["Jogo/mif/1.mif", "Jogo/mod/1/jogo.mod", "Jogo/dados.pak"],
        );
        let module = extract(&zip).unwrap();
        assert!(module.is_file());
        assert!(module.ends_with("Jogo/mod/1/jogo.mod"));
        // The neighboring files come along: they are what the game opens at run time.
        assert!(module.parent().unwrap().join("../../dados.pak").exists());

        // The second extraction points to the same place, without redoing the work.
        assert_eq!(extract(&zip).unwrap(), module);

        let _ = std::fs::remove_dir_all(cache_dir().join(fingerprint(&zip).unwrap()));
        let _ = std::fs::remove_file(&zip);
    }
}
