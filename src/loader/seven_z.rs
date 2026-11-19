//! Games stored in `.7z`.
//!
//! The Zeebo collection circulates in both formats, and a `.7z` the emulator rejects is a game
//! that **"does not load" with no explanation** — the same symptom as a corrupted file, with no
//! clue at all for whoever tried to open it. This module does the minimum the emulator needs from
//! a `.7z`: list the names, read a file and extract everything to the cache.
//!
//! An archive is **untrusted** content, whatever format it comes in, so the limits are the same
//! as the zip's: a cap on entries, a cap per file and a cap on the total uncompressed size. The
//! only thing that changes is who decompresses.
//!
//! The format is not decided by the extension, but by the **signature** `7z¼¯'`: a `.7z` renamed
//! to `.zip` is still a `.7z`, and that is how it shows up in a collection shuffled by hand.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::archive::ArchiveLimits;

/// The signature of a `.7z`: `7z` and the four bytes `BC AF 27 1C`.
pub const MAGIC: [u8; 6] = [0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c];

/// The Windows attribute that marks a reparse point — what a `.7z` symlink carries.
const LINK_ATTRIBUTE: u32 = 0x400;

/// Does the file start with the `.7z` signature?
pub fn is_seven_z(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; MAGIC.len()];
    file.read_exact(&mut header).is_ok() && header == MAGIC
}

/// Converts an entry name into a safe relative path.
///
/// Rejects what a malicious archive would use to write outside the destination folder: a `..`
/// component, an absolute path, a Windows drive root (`C:`) and an empty name. The separator is
/// accepted in both forms because a `.7z` made on Windows carries `\`.
fn safe_path(name: &str) -> Option<PathBuf> {
    let mut output = PathBuf::new();
    for part in name.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        // `C:` and the like: on Windows this is absolute; on Linux it would pass as an ordinary
        // folder name, and it is better to reject it on both than to accept it differently on each.
        if part.contains(':') {
            return None;
        }
        output.push(part);
    }
    (!output.as_os_str().is_empty()).then_some(output)
}

/// The names inside the `.7z`, with which ones are folders.
///
/// Comes from the **header**, without decompressing any data: choosing which `.mod` to open costs
/// a metadata read, and not the whole extraction. This is what lets a `.7z` be treated the way
/// the zip is treated today — the difference stays in the decompressor, not in the rest of the
/// emulator.
pub fn list(path: &Path) -> Option<Vec<(String, bool)>> {
    let file = sevenz_rust2::Archive::open(path).ok()?;
    if file.files.len() > super::archive::MAX_ENTRIES {
        return None;
    }
    Some(
        file
            .files
            .iter()
            .map(|entry| (entry.name.clone(), entry.is_directory))
            .collect(),
    )
}

/// Opens the `.7z` for reading.
///
/// On `wasm32` the crate's `open` does not exist: it was hidden on the assumption that the target
/// has no files. Emscripten has them, so opening the `File` happens here and `new` does the rest —
/// it is the same function, without the `cfg`.
fn open_reader(path: &Path) -> std::io::Result<sevenz_rust2::ArchiveReader<std::fs::File>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
            .map_err(std::io::Error::other)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let file = std::fs::File::open(path)?;
        sevenz_rust2::ArchiveReader::new(file, sevenz_rust2::Password::empty())
            .map_err(std::io::Error::other)
    }
}

/// Reads a file from inside the `.7z`, with a byte cap.
///
/// A solid block forces decompressing whatever comes before it, so the cost can exceed the size
/// of the requested file — the cap still applies to what we **keep**, and it stops right away
/// once the entry arrives.
pub fn read(path: &Path, target: &str, cap: u64) -> Option<Vec<u8>> {
    let mut reader = open_reader(path).ok()?;
    let mut found: Option<Vec<u8>> = None;
    let mut failed = false;
    let _ = reader.for_each_entries(|entry, stream| {
        if entry.name != target {
            return Ok(true);
        }
        let mut data = Vec::new();
        match stream.take(cap + 1).read_to_end(&mut data) {
            Ok(_) if data.len() as u64 <= cap => found = Some(data),
            _ => failed = true,
        }
        Ok(false)
    });
    if failed { None } else { found }
}

/// Extracts the whole `.7z` into `dest`, with the zip's limits.
pub(crate) fn extract(path: &Path, dest: &Path, limits: ArchiveLimits) -> std::io::Result<()> {
    let mut reader = open_reader(path)?;
    let mut entries = 0usize;
    let mut total = 0u64;
    let mut failure: Option<std::io::Error> = None;
    let result = reader.for_each_entries(|entry, stream| {
        if let Err(error) = extract_one(entry, stream, dest, &mut entries, &mut total, limits) {
            failure = Some(error);
            // Stops at the first rejection: continuing to extract an archive that was already
            // rejected only spends time getting to the same error.
            return Ok(false);
        }
        Ok(true)
    });
    if let Some(error) = failure {
        return Err(error);
    }
    result.map_err(std::io::Error::other)
}

/// Writes one entry, applying the caps and rejecting unsafe paths.
fn extract_one(
    entry: &sevenz_rust2::ArchiveEntry,
    stream: &mut dyn Read,
    dest: &Path,
    entries: &mut usize,
    total: &mut u64,
    limits: ArchiveLimits,
) -> std::io::Result<()> {
    use super::archive::{MAX_FILE_BYTES, MAX_TOTAL_BYTES};

    *entries += 1;
    if *entries > limits.entries {
        return Err(std::io::Error::other("the 7z has too many entries"));
    }
    let Some(relative) = safe_path(&entry.name) else {
        return Err(std::io::Error::other("the 7z contains an unsafe path"));
    };
    if entry.windows_attributes & LINK_ATTRIBUTE != 0 {
        return Err(std::io::Error::other("the 7z contains a symbolic link"));
    }
    let output = dest.join(&relative);
    if entry.is_directory {
        std::fs::create_dir_all(&output)?;
        return Ok(());
    }
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // The per-file cap comes from the zip; the total cap is recomputed for each entry so that a
    // lying header does not write past it during decompression.
    let remaining = MAX_TOTAL_BYTES
        .checked_sub(*total)
        .ok_or_else(|| std::io::Error::other("the 7z decompresses too many bytes"))?;
    let allowed = MAX_FILE_BYTES.min(remaining);
    let mut file = std::fs::File::create(&output)?;
    let written = std::io::copy(&mut stream.take(allowed + 1), &mut file)?;
    if written > allowed {
        return Err(std::io::Error::other("the 7z decompresses too many bytes"));
    }
    *total += written;
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::archive;
    use crate::scratch::TempDir;

    /// Writes a real `.7z`, so the test does not depend on a file in the repository.
    fn write_7z(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = sevenz_rust2::ArchiveWriter::create(path).expect("create 7z");
        for (name, data) in entries {
            writer
                .push_archive_entry(
                    sevenz_rust2::ArchiveEntry::new_file(name),
                    Some(std::io::Cursor::new(data.to_vec())),
                )
                .expect("write entry");
        }
        writer.finish().expect("close the 7z");
    }

    /// The signature decides, not the extension: a `.7z` named `.zip` is still a `.7z`.
    #[test]
    fn recognizes_by_signature_not_extension() {
        let dir = TempDir::new("zeebx-7z-magia");
        let package = dir.join("jogo.zip");
        write_7z(&package, &[("Titulo/mod/1/a.mod", b"mod")]);
        assert!(is_seven_z(&package));
        std::fs::write(dir.join("outro.zip"), b"PK\x03\x04nao e 7z").unwrap();
        assert!(!is_seven_z(&dir.join("outro.zip")));
    }

    /// The game's whole path works: find the `.mod`, find the `.mif` and extract.
    ///
    /// It is what a `.7z` owner needs to work, and each step goes through a different part of the
    /// archive: the choice of module comes from the header, the manifest from an entry that is
    /// read, and the extraction from the whole archive.
    #[test]
    fn finds_the_module_and_manifest_inside_the_7z() {
        let dir = TempDir::new("zeebx-7z-jogo");
        let package = dir.join("Jogo.7z");
        write_7z(
            &package,
            &[
                ("Jogo/leia-me.txt", b"texto"),
                ("Jogo/mod/279233/jogo.mod", b"codigo"),
                ("Jogo/mif/279233.mif", b"manifesto"),
            ],
        );

        assert_eq!(archive::find_module(&package).as_deref(), Some("Jogo/mod/279233/jogo.mod"));
        assert_eq!(
            archive::find_manifest(&package, "Jogo/mod/279233/jogo.mod").as_deref(),
            Some(&b"manifesto"[..])
        );

        let dest = dir.join("extraido");
        std::fs::create_dir_all(&dest).unwrap();
        let limits = crate::loader::archive::ArchiveLimits::fallback();
        extract(&package, &dest, limits).expect("extracts");
        assert_eq!(
            std::fs::read(dest.join("Jogo/mod/279233/jogo.mod")).unwrap(),
            b"codigo"
        );
    }

    /// A path with `..` is rejected, and nothing is written outside the destination.
    #[test]
    fn rejects_path_escaping_the_destination() {
        let dir = TempDir::new("zeebx-7z-inseguro");
        let package = dir.join("malicioso.7z");
        write_7z(&package, &[("../fora.txt", b"nao devia estar aqui")]);

        let dest = dir.join("extraido");
        std::fs::create_dir_all(&dest).unwrap();
        let error = extract(&package, &dest, crate::loader::archive::ArchiveLimits::fallback())
            .expect_err("should reject");
        assert!(error.to_string().contains("unsafe path"), "{error}");
        assert!(!dir.join("fora.txt").exists());
    }
}
