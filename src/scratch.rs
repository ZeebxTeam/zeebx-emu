//! A temporary directory that deletes itself, for the tests.
//!
//! There was a small, constant leak in the suite: the tests created `/tmp/zeebx-sql-*` and
//! `/tmp/zeebx-saves-*` and cleaned up **at the start** — in case the previous run had failed —
//! and never at the end. A session with several runs leaves dozens of directories behind. With this
//! guard, the directory goes away when the test ends, whether it passes or not.

use std::path::{Path, PathBuf};

/// A folder in `temp_dir()` that removes itself when it goes out of scope.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Creates the folder, deleting whatever exists with the same name.
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temporary directory");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A path inside the folder.
    pub fn join(&self, rest: &str) -> PathBuf {
        self.0.join(rest)
    }

    /// The path as a `PathBuf`, for callers that need an owned value.
    pub fn to_path_buf(&self) -> PathBuf {
        self.0.clone()
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

/// Checks that there is still room to write on the volume of `dir`.
///
/// **Why try instead of asking.** There is no portable way to ask "how much space is there"
/// without pulling in a dependency, and what the scan needs to know is not the number: it is
/// whether things still fit. The probe writes and deletes a file, and returns the system error
/// when the system says no.
///
/// This exists because running out of space **lies**: running the scan of the 62 ROMs with the disk
/// full, two games showed up as "does not load" with `No space left on device`, and the report
/// blamed a defect where there was an environment problem. Extracting each ROM to the cache is what
/// uses the space, and the whole scan takes close to 1 GB.
pub fn ensure_room(dir: &Path, bytes: usize) -> Result<(), String> {
    const STEP: usize = 1024 * 1024;
    let probe_file = dir.join("zeebx-prova-de-espaco");
    let for_writing = std::fs::create_dir_all(dir).and_then(|()| {
        let mut file = std::fs::File::create(&probe_file)?;
        let block = vec![0u8; STEP];
        let mut remaining = bytes;
        while remaining > 0 {
            let step = remaining.min(STEP);
            std::io::Write::write_all(&mut file, &block[..step])?;
            remaining -= step;
        }
        std::io::Write::flush(&mut file)
    });
    let _ = std::fs::remove_file(&probe_file);
    for_writing.map_err(|error| {
        format!(
            "{error} ({} MB probe in {})",
            bytes / STEP,
            dir.display()
        )
    })
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
