//! The file system as the game sees it.
//!
//! The game never sees a host path. What it asks for is translated, **as text only**, into a path
//! in the console's virtual file system; `..` is resolved there and stops at the virtual root.
//! Only then does that virtual path become a host path, through one of two layers:
//!
//! | Layer | Virtual | Host | Access |
//! |---|---|---|---|
//! | package | `/mod/...` | the package's `mod/` folder — or only the module's, if it came loose | read |
//! | writable | `/mod/...` | the save store, shared by every game | read and write |
//! | writable | the rest of `/` | the device, shared by every game | read and write |
//!
//! Reading looks first in the writable layer and then in the package; writing always goes to the
//! writable one. That is what the console has: the installed package underneath and, on top of
//! it, what the games wrote. Nothing a game asks for reaches another package, the ROM library or
//! the rest of the machine — and its own package cannot be modified or deleted.
//!
//! Path forms that appear in the games and in the SDK:
//!
//! ```text
//! file.dat             relative to the module's directory: /mod/<module>/file.dat
//! fs:/~/file.dat       "~" is the module's own directory
//! fs:/~/../id1/x       /mod/id1/x — Quake keeps its data in a sibling directory
//! fs:/~0x01234567/x    directory of a module by ClassID
//! fs:/shared/x         area shared between applications
//! fs:/zeeboiddata/x    **the device's file system**, shared by all
//! fs:/mod/<dir>/x      the directory of a module by absolute path
//! ```
//!
//! # `fs:/` belongs to the device, not the game
//!
//! On the console there is **one** file system: Zeeboids saves the characters to
//! `fs:/zeeboiddata/zeeboid.db` and Zeebo F.C. opens that same path to import them. For the same
//! reason there is a single save store: a game that looks for another's save in `fs:/mod/<dir>/`
//! finds what that other one wrote.

use std::path::{Component, Path, PathBuf};

/// Prefixes BREW uses and that we strip before resolving, with what each one says: whether the
/// path belongs to the device and whether it may carry the ClassID right after the `~`.
const PREFIXES: [(&str, bool); 4] = [
    ("fs:/~/", false),
    ("fs:/~", false),
    ("fs:/", true),
    ("~/", false),
];

/// Strips the `<class_id>/` that may come right after the `~`.
///
/// BREW writes a module's directory as `fs:/~<ClassID>/`, and the Z-Wheel uses that form for its
/// own resources: `fs:/~0x01070798/tectoyli.brf`. Without stripping the number, the path became a
/// **subdirectory** named `0x01070798` inside the module's directory — which does not exist. The
/// file was in the package and still did not open.
///
/// That is what held up the Z-Wheel's z-pad instructions form: it loads a resource from
/// `tectoyli.brf` through that path, did not find it, and returned `EUNABLETOLOAD` — the error 6
/// that showed up thousands of times in the report.
///
/// **Here there is only one module at a time**, so any ClassID leads to its directory. On the
/// console a module can reach another's directory through this path; when that matters, this is
/// where the number gets resolved to the right directory. The folder name is no shortcut: the
/// Z-Wheel's is `274755` and its ClassID is `0x01070798`.
fn without_class(rest: &str) -> String {
    let Some((first, tail)) = rest.split_once('/') else {
        return rest.to_string();
    };
    let number = first
        .strip_prefix("0x")
        .or_else(|| first.strip_prefix("0X"))
        .map(|hex| !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| !first.is_empty() && first.chars().all(|c| c.is_ascii_digit()));
    match number {
        true => tail.to_string(),
        false => rest.to_string(),
    }
}
/// Name of the shared root, which plays the role of the device's file system.
const DEVICE_DIR: &str = "aparelho";
/// Where the modules are installed, both on the device and in the package.
const MOD_DIR: &str = "mod";

/// What an open is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenIntent {
    /// Read a file that must exist.
    Read,
    /// Create a new name, without overwriting an existing one with different case.
    Create,
    /// Read and write the same file, preserving its current contents.
    ReadWrite,
    /// Append to the end of the file.
    Append,
}

/// Where to open, and the copy that must precede the write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTarget {
    /// Host path to open.
    pub path: PathBuf,
    /// Package file to copy to `path` before writing, for copy-on-write.
    pub copy_from: Option<PathBuf>,
}

/// A path in the virtual file system: the components from `/`, already without `.`, `..` or
/// empty slashes.
type Virtual = Vec<String>;

#[derive(Debug)]
pub struct Vfs {
    /// The module's directory on the host: extracted package or folder of the loose `.mod`.
    /// Never written to.
    root: PathBuf,
    /// Name the module appears under in `/mod/<name>/`: its folder.
    module: String,
    /// The package's `mod/` folder, when the module is in one.
    ///
    /// It is what lets Quake read `mod/id1/` and NFS `mod/nfsresources/`, which come in the same
    /// zip. A loose `.mod` has no package around it, and its neighboring folders belong to the
    /// user: in that case only the module's own directory is visible.
    package_mods: Option<PathBuf>,
    /// The save store, which receives everything the games write under `/mod/`. Without it, the
    /// device's `mod/` folder.
    saves: Option<PathBuf>,
    /// The shared root, which plays the role of the device's file system.
    device: PathBuf,
}

impl Vfs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root: PathBuf = root.into();
        let module = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "modulo".to_string());
        let parent = root.parent().map(Path::to_path_buf);
        let package_mods = parent
            .clone()
            .filter(|parent| parent.file_name().is_some_and(|n| n.eq_ignore_ascii_case(MOD_DIR)));
        let device = parent.unwrap_or_else(|| root.clone()).join(DEVICE_DIR);
        Self {
            root,
            module,
            package_mods,
            saves: None,
            device,
        }
    }

    /// Sets the save store, which receives what the games write under `/mod/`.
    pub fn set_save_root(&mut self, save: impl Into<PathBuf>) {
        self.saves = Some(save.into());
    }

    /// Points the shared root somewhere else.
    ///
    /// It exists so that **every game** shares the same file system, and not one copy per
    /// installation: that is how Zeebo F.C. finds the characters Zeeboids saved.
    pub fn set_device_root(&mut self, device: impl Into<PathBuf>) {
        self.device = device.into();
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// State file created by the emulator for a console application.
    ///
    /// It does not go through the module's path: it is device state, so it survives a new
    /// extraction of the ZIP and never modifies the original package.
    pub fn profile_file(&self, app: &str, name: &str) -> PathBuf {
        self.device.join(app).join(name)
    }

    fn saves_dir(&self) -> PathBuf {
        self.saves
            .clone()
            .unwrap_or_else(|| self.device.join(MOD_DIR))
    }

    /// Translates the guest path into the virtual path, without touching the disk.
    ///
    /// Returns `None` for whatever leaves the virtual root and for what is not a console path: a
    /// component with `:` (a Windows root, another prefix) or with NUL. `allow_root` accepts the
    /// module's own directory and `/mod/`, which can be listed but not opened as a file.
    fn virtual_path(&self, guest_path: &str, allow_root: bool) -> Option<Virtual> {
        let normalized = guest_path.replace('\\', "/");
        let (rest, from_device) = PREFIXES
            .iter()
            .copied()
            .find_map(|(prefix, from_device)| {
                let rest = normalized.strip_prefix(prefix)?;
                Some(match from_device {
                    true => (rest.to_string(), true),
                    false => (without_class(rest), false),
                })
            })
            .unwrap_or((normalized.clone(), false));
        let start: Virtual = match from_device {
            true => Vec::new(),
            false => vec![MOD_DIR.to_string(), self.module.clone()],
        };
        // From the module's folder you can go up to `/mod/`, which is how Quake reaches its
        // data, and not one step further. From the device you cannot go up: it is already the
        // root.
        let floor = match from_device {
            true => 0,
            false => 1,
        };
        let mut path = start.clone();
        // An empty slash — `/` at the start or `//` in the middle — anchors the path in the
        // module's folder, and from there `..` does not go up. The Zeebo Extreme games save the
        // track records to `./udata/trackinfo.txt` and read them back through the packer, which
        // builds `"%s/%s"` with the root `./`: `.//../udata/trackinfo.txt`. Going up, the reread
        // landed in `mod/udata`, the track list was left without a vector and Bóia Cross's start
        // read the null pointer.
        let mut anchored = false;
        for part in rest.split('/') {
            match part {
                "" => anchored = !from_device,
                "." => continue,
                ".." if anchored && path == start => continue,
                ".." => {
                    if path.len() <= floor {
                        return None;
                    }
                    path.pop();
                }
                _ if part.contains(|c| c == ':' || c == '\0') => return None,
                _ => path.push(part.to_string()),
            }
        }
        let modules_root = path.len() == 1 && path[0].eq_ignore_ascii_case(MOD_DIR);
        if !allow_root && (path.is_empty() || modules_root || path == start) {
            return None;
        }
        Some(path)
    }

    /// The writable layer of a virtual path: its root and the path inside it.
    fn upper(&self, path: &[String]) -> (PathBuf, PathBuf) {
        match path.first() {
            Some(first) if first.eq_ignore_ascii_case(MOD_DIR) => {
                let root = self.saves_dir();
                let inside = join_components(&root, &path[1..]);
                (root, inside)
            }
            _ => (self.device.clone(), join_components(&self.device, path)),
        }
    }

    /// The package layer, when the virtual path falls in it.
    fn lower(&self, path: &[String]) -> Option<(PathBuf, PathBuf)> {
        if !path.first()?.eq_ignore_ascii_case(MOD_DIR) {
            return None;
        }
        match &self.package_mods {
            Some(mods) => Some((mods.clone(), join_components(mods, &path[1..]))),
            None if path.get(1)?.eq_ignore_ascii_case(&self.module) => {
                Some((self.root.clone(), join_components(&self.root, &path[2..])))
            }
            None => None,
        }
    }

    /// The writable path, with the case the disk already has.
    fn upper_existing(&self, path: &[String]) -> Option<PathBuf> {
        let (root, inside) = self.upper(path);
        in_layer(&root, match_case(inside))
    }

    /// The path in the package, if it exists there.
    fn lower_existing(&self, path: &[String]) -> Option<PathBuf> {
        let (root, inside) = self.lower(path)?;
        in_layer(&root, match_case(inside)).filter(|c| c.exists())
    }

    /// Translates a guest path into a host path, for reading.
    ///
    /// What the game wrote hides what came in the package: that is what makes the reread save be
    /// the save, and not the original resource. A path that exists in neither layer comes back as
    /// the place where it would be created, which is always the writable one.
    pub fn resolve(&self, guest_path: &str) -> Option<PathBuf> {
        let path = self.virtual_path(guest_path, false)?;
        let writable_path = self.upper_existing(&path)?;
        match writable_path.exists() {
            true => Some(writable_path),
            false => self.lower_existing(&path).or(Some(writable_path)),
        }
    }

    /// Where to open a path, and where to copy from before writing.
    ///
    /// See [`OpenIntent`]. Every write lands in the writable layer; a package resource opened for
    /// writing is copied first, and later writes already find it there.
    pub fn open_target(&self, guest_path: &str, intent: OpenIntent) -> Option<OpenTarget> {
        let resolved_path = self.virtual_path(guest_path, false)?;
        let writable_path = self.upper_existing(&resolved_path)?;
        let already_written = writable_path.exists();
        let from_package = || self.lower_existing(&resolved_path);
        Some(match intent {
            OpenIntent::Read => OpenTarget {
                path: match already_written {
                    true => writable_path,
                    false => from_package().unwrap_or(writable_path),
                },
                copy_from: None,
            },
            OpenIntent::Create => OpenTarget {
                path: writable_path,
                copy_from: None,
            },
            OpenIntent::ReadWrite | OpenIntent::Append => OpenTarget {
                copy_from: match already_written {
                    true => None,
                    false => from_package(),
                },
                path: writable_path,
            },
        })
    }

    /// Like [`Vfs::resolve`], but to **create** a name, always in the writable layer.
    ///
    /// The new name stays exactly as the guest wrote it: renaming to `save.dat` when a `SAVE.DAT`
    /// exists has to create the former, not overwrite the latter. Directories that already exist
    /// keep following the console's case-insensitive semantics; without this, `UDATA/save.dat`
    /// would create a second folder next to `udata/` on a Linux host.
    pub fn resolve_new(&self, guest_path: &str) -> Option<PathBuf> {
        let path = self.virtual_path(guest_path, false)?;
        let (root, inside) = self.upper(&path);
        in_layer(&root, match_case_parent(inside))
    }

    /// The path in the writable layer, whether it exists or not.
    ///
    /// It is for callers that will **remove**, **rename** or create and delete a directory: the
    /// package is immutable, so only what is in the writable layer can be touched.
    pub fn writable(&self, guest_path: &str) -> Option<PathBuf> {
        self.upper_existing(&self.virtual_path(guest_path, false)?)
    }

    /// The directory to list, in the package when it has it.
    ///
    /// Listing `fs:/~/` is legitimate — it is the game's directory —, even though opening it as a
    /// file is not. What the game wrote in the same directory comes from [`Vfs::writable_dir`].
    pub fn resolve_dir(&self, guest_path: &str) -> Option<PathBuf> {
        let path = self.virtual_path(guest_path, true)?;
        let writable_path = self.upper_existing(&path)?;
        self.lower_existing(&path)
            .filter(|c| c.is_dir())
            .or(Some(writable_path))
    }

    /// The same directory in the writable layer, when it exists there.
    ///
    /// It serves the listing: the game needs to see the files it wrote itself, and not only the
    /// ones that came in the package.
    pub fn writable_dir(&self, guest_path: &str) -> Option<PathBuf> {
        let path = self.virtual_path(guest_path, true)?;
        self.upper_existing(&path).filter(|c| c.is_dir())
    }

    /// Whether the host path is in the writable layer.
    pub fn is_writable_path(&self, path: &Path) -> bool {
        path.starts_with(self.saves_dir()) || path.starts_with(&self.device)
    }

    /// Whether a host path is within what this game can reach.
    ///
    /// For whatever comes from outside the VFS and points at the host: a save state stores the
    /// path of the open files, and a tampered state must not reopen an arbitrary file on the
    /// machine.
    pub fn contains_host(&self, path: &Path) -> bool {
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return false;
        }
        let roots = [
            Some(self.saves_dir()),
            Some(self.device.clone()),
            Some(self.root.clone()),
            self.package_mods.clone(),
        ];
        roots
            .into_iter()
            .flatten()
            .any(|root| path.starts_with(&root) && no_symlink(&root, path))
    }
}

/// `root` followed by the components of a virtual path.
fn join_components(root: &Path, components: &[String]) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(components);
    path
}

/// The path, if nothing between the layer's root and it is a symbolic link.
///
/// The components already come clean from the virtual path, but a link inside the layer would
/// take the game wherever it pointed. zip and 7z already refuse links when extracting; this
/// covers the loose `.mod`, whose folder belongs to the user.
fn in_layer(root: &Path, path: PathBuf) -> Option<PathBuf> {
    no_symlink(root, &path).then_some(path)
}

fn no_symlink(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let mut current = root.to_path_buf();
    for part in relative.components() {
        if !matches!(part, Component::Normal(_)) {
            return false;
        }
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => return false,
            Ok(_) => {}
            // From here on the path does not exist yet, and what does not exist is not a link.
            Err(_) => break,
        }
    }
    true
}

/// Resolves only the directories of a new path, keeping the last component for creation.
fn match_case_parent(path: PathBuf) -> PathBuf {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return path;
    };
    match_case(parent.to_path_buf()).join(name)
}

/// The same path, with the case the files really have on disk.
///
/// The console's file system is case-insensitive, and the games count on that: the Zeebo's ten
/// arcade ports ask for `font.fnz` and ship `font.FNZ` in the package. None of them got past
/// `EVT_APP_START` — the font did not open and the null pointer came right after. On Windows and
/// macOS this worked by accident; on Linux, it did not.
///
/// A path that does not exist comes back as it came: that is the case of a file the game is
/// creating.
fn match_case(path: PathBuf) -> PathBuf {
    if path.exists() {
        return path;
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return path;
    };
    // The directory may also have different case, so it is resolved first.
    let parent = match_case(parent.to_path_buf());
    let exact = parent.join(name);
    if exact.exists() {
        return exact;
    }
    let Ok(entries) = std::fs::read_dir(&parent) else {
        return exact;
    };
    let target = name.to_string_lossy().to_lowercase();
    // With more than one candidate — two files that differ only in case —, the first in
    // alphabetical order wins, so the choice does not depend on the order the disk returns them.
    entries
        .flatten()
        .map(|entry| entry.file_name())
        .filter(|candidate| candidate.to_string_lossy().to_lowercase() == target)
        .min()
        .map_or(exact, |candidate| parent.join(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vfs() -> Vfs {
        Vfs::new("/jogos/mod/bjt")
    }

    /// The virtual path as text, so the tests read the way the console writes.
    fn virtual_of(vfs: &Vfs, guest: &str) -> Option<String> {
        vfs.virtual_path(guest, false)
            .map(|c| format!("/{}", c.join("/")))
    }

    #[test]
    fn the_module_dir_can_be_listed_but_not_opened() {
        // Listing `fs:/~/` is legitimate — it is the game's folder. Opening it as a file is not,
        // and that is precisely the difference between the two answers.
        let vfs = Vfs::new("/roms/jogo");
        assert_eq!(vfs.resolve("fs:/~/"), None);
        assert!(vfs.resolve_dir("fs:/~/").is_some());
        // Leaving the root is still forbidden in both.
        assert_eq!(vfs.resolve_dir("fs:/~/../../etc"), None);
    }

    #[test]
    fn a_relative_path_lands_in_the_module_dir() {
        assert_eq!(virtual_of(&vfs(), "dados.dat").as_deref(), Some("/mod/bjt/dados.dat"));
    }

    #[test]
    fn the_tilde_is_the_module_and_the_rest_is_the_device() {
        let vfs = vfs();
        // With `~`, and with no prefix at all, it is the game's folder.
        let for_game = Some("/mod/bjt/dados.dat");
        assert_eq!(virtual_of(&vfs, "fs:/~/dados.dat").as_deref(), for_game);
        assert_eq!(virtual_of(&vfs, "~/dados.dat").as_deref(), for_game);
        assert_eq!(virtual_of(&vfs, "fs:/~0x01070798/dados.dat").as_deref(), for_game);

        // Without the `~`, it is the device's file system — shared by every game. That is what
        // lets Zeebo F.C. open `fs:/zeeboiddata/zeeboid.db`, which Zeeboids wrote.
        assert_eq!(virtual_of(&vfs, "fs:/dados.dat").as_deref(), Some("/dados.dat"));
        assert_eq!(
            virtual_of(&vfs, "fs:/zeeboiddata/zeeboid.db").as_deref(),
            Some("/zeeboiddata/zeeboid.db")
        );
        // And `fs:/mod/<dir>/` is the same folder as `~`.
        assert_eq!(virtual_of(&vfs, "fs:/mod/bjt/dados.dat").as_deref(), for_game);
    }

    #[test]
    fn accepts_backslash_as_separator() {
        assert_eq!(
            virtual_of(&vfs(), "imagens\\fundo.bmp").as_deref(),
            Some("/mod/bjt/imagens/fundo.bmp")
        );
    }

    #[test]
    fn cannot_climb_out_of_the_device() {
        // The device's root is a root: `..` from it would leave for the user's machine.
        let vfs = vfs();
        assert_eq!(virtual_of(&vfs, "fs:/../x"), None);
        assert_eq!(virtual_of(&vfs, "fs:/zeeboiddata/../../x"), None);
    }

    #[test]
    fn climbs_to_the_modules_root_but_not_beyond() {
        let vfs = Vfs::new("/jogos/mod/274802");
        // Quake keeps its data in a sibling directory.
        assert_eq!(
            virtual_of(&vfs, "fs:/~/../id1/splash_title.png").as_deref(),
            Some("/mod/id1/splash_title.png")
        );
        // One more level would already leave the module tree.
        assert_eq!(virtual_of(&vfs, "fs:/~/../../outro/x"), None);
        assert_eq!(virtual_of(&vfs, "../../../etc/passwd"), None);
        assert_eq!(virtual_of(&vfs, "dados/../../../fora.txt"), None);
    }

    #[test]
    fn rejects_what_is_not_a_console_path() {
        let vfs = vfs();
        assert_eq!(virtual_of(&vfs, "C:/Windows/system32"), None);
        assert_eq!(virtual_of(&vfs, "fs:/~/../C:/x"), None);
        assert_eq!(virtual_of(&vfs, "a\0b"), None);
    }

    #[test]
    fn after_an_empty_slash_dot_dot_does_not_leave_the_module() {
        let vfs = Vfs::new("/jogos/mod/278285");
        // What Bóia Cross writes and the path it reads it back through.
        assert_eq!(
            virtual_of(&vfs, "./udata/trackinfo.txt"),
            virtual_of(&vfs, ".//../udata/trackinfo.txt")
        );
        assert_eq!(
            virtual_of(&vfs, "/../udata/trackinfo.txt").as_deref(),
            Some("/mod/278285/udata/trackinfo.txt")
        );
        // Without the empty slash, it still goes up to the modules root, as NFS needs.
        assert_eq!(
            virtual_of(&vfs, "../nfsresources/x.bar").as_deref(),
            Some("/mod/nfsresources/x.bar")
        );
    }

    #[test]
    fn an_empty_path_does_not_resolve_to_the_root_itself() {
        let vfs = vfs();
        assert_eq!(vfs.resolve(""), None);
        assert_eq!(vfs.resolve("fs:/~"), None);
        assert_eq!(vfs.resolve("fs:/"), None);
        assert_eq!(vfs.resolve("fs:/mod"), None);
        assert_eq!(vfs.resolve("fs:/~/.."), None);
    }

    #[test]
    fn only_a_packaged_module_sees_the_sibling_dirs() {
        // In a package, `mod/` belongs to the package. Outside one, the folder above belongs to
        // the user.
        assert!(Vfs::new("/cache/abc/mod/274802").package_mods.is_some());
        assert!(Vfs::new("/cache/abc/MOD/274802").package_mods.is_some());
        assert!(Vfs::new("/home/kaio/Downloads/jogo").package_mods.is_none());
        let loose = Vfs::new("/home/kaio/Downloads/jogo");
        let sibling = ["mod".to_string(), "outro".to_string(), "x".to_string()];
        assert_eq!(loose.lower(&sibling), None);
    }

    #[test]
    fn the_save_state_only_reopens_what_the_game_can_reach() {
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
mod disk_tests {
    use super::*;

    /// Is the file system case-sensitive?
    ///
    /// **macOS's APFS and Windows's NTFS are not, by default**: `data` and `DATA` are the same
    /// folder, `font.fnz` and `font.FNZ` are the same file. On those systems the requested path
    /// already exists, the VFS's answer is that path itself, and there are no two spellings to
    /// compare — that is how two tests in this section started failing **only on macOS** in the
    /// first CI run that reached them. Where the system merges the spellings, what the
    /// case-insensitive lookup has to guarantee comes for free from the file system itself.
    fn is_case_sensitive(location: &Path) -> bool {
        let high = location.join("zeebx-CAIXA");
        let low = location.join("zeebx-caixa");
        std::fs::write(&high, b"x").unwrap();
        let case_sensitive = !low.exists();
        let _ = std::fs::remove_file(&high);
        case_sensitive
    }

    /// An extracted package, with the device and the save store next to it, as the engine sets
    /// them up.
    struct Profile {
        root: PathBuf,
        modulo: PathBuf,
        saves: PathBuf,
        device: PathBuf,
    }

    impl Profile {
        fn create(name: &str, modulo: &str) -> Self {
            let root = std::env::temp_dir().join(format!("zeebx-vfs-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let modulo = root.join("cache/abc").join(modulo);
            let saves = root.join("saves/mod");
            let device = root.join("aparelho");
            for dir in [&modulo, &saves, &device] {
                std::fs::create_dir_all(dir).unwrap();
            }
            Self { root, modulo, saves, device }
        }

        fn vfs(&self) -> Vfs {
            let mut vfs = Vfs::new(&self.modulo);
            vfs.set_device_root(&self.device);
            vfs.set_save_root(&self.saves);
            vfs
        }
    }

    impl Drop for Profile {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// The ten arcade ports ask for `font.fnz` and ship `font.FNZ`. On the console it makes no
    /// difference.
    #[test]
    fn finds_the_file_with_different_case() {
        let profile = Profile::create("caixa", "mod/279233");
        let modulo = &profile.modulo;
        std::fs::write(modulo.join("font.FNZ"), b"fonte").unwrap();

        let vfs = profile.vfs();
        let case_sensitive = is_case_sensitive(modulo);
        if case_sensitive {
            assert_eq!(vfs.resolve("font.fnz"), Some(modulo.join("font.FNZ")));
        } else {
            // A system that merges the two spellings: the answer is the requested name, and the
            // file is the same. The check that matters here is that the answer opens the
            // package's contents.
            let found = vfs.resolve("font.fnz").expect("answered with the file");
            assert_eq!(std::fs::read(&found).unwrap(), b"fonte");
        }
        // What does not exist comes back as the place where it would be created, which is the
        // save store.
        assert_eq!(
            vfs.resolve("save.dat"),
            Some(profile.saves.join("279233/save.dat"))
        );
        // A creation keeps the file name, but finds existing directories case-insensitively.
        let udata = profile.saves.join("279233/udata");
        std::fs::create_dir_all(&udata).unwrap();
        if case_sensitive {
            assert_eq!(vfs.resolve_new("UDATA/save.dat"), Some(udata.join("save.dat")));
        } else {
            assert!(vfs.resolve_new("UDATA/save.dat").is_some());
        }
    }

    /// The store keeps what the game writes, without touching the package's contents.
    #[test]
    fn a_write_goes_to_the_store_and_hides_the_package_resource() {
        let profile = Profile::create("overlay", "mod/1");
        let modulo = &profile.modulo;
        let save = profile.saves.join("1");
        std::fs::write(modulo.join("config.ini"), b"do pacote").unwrap();
        let vfs = profile.vfs();

        // Read: the package is found while there is nothing in the store.
        assert_eq!(vfs.resolve("config.ini"), Some(modulo.join("config.ini")));

        // Write to an existing resource: copy first, and from then on the store wins.
        let target = vfs
            .open_target("config.ini", OpenIntent::ReadWrite)
            .unwrap();
        assert_eq!(target.path, save.join("config.ini"));
        assert_eq!(target.copy_from, Some(modulo.join("config.ini")));
        std::fs::create_dir_all(&save).unwrap();
        std::fs::write(&target.path, b"do save").unwrap();
        assert_eq!(vfs.resolve("config.ini"), Some(save.join("config.ini")));
        assert_eq!(
            std::fs::read(modulo.join("config.ini")).unwrap(),
            b"do pacote"
        );

        // New file: created in the store.
        let create = vfs
            .open_target("udata/recorde.dat", OpenIntent::Create)
            .unwrap();
        assert_eq!(create.path, save.join("udata/recorde.dat"));
        assert_eq!(create.copy_from, None);

        // The device has its own root.
        let device = vfs
            .open_target("fs:/zeeboiddata/zeeboid.db", OpenIntent::Create)
            .unwrap();
        assert_eq!(device.path, profile.device.join("zeeboiddata/zeeboid.db"));
    }

    /// Quake reads `mod/id1/` from the package, and what it writes there goes to the store.
    #[test]
    fn the_sibling_dir_is_read_from_the_package_and_written_to_the_store() {
        let profile = Profile::create("irmao", "mod/274802");
        let id1 = profile.modulo.parent().unwrap().join("id1");
        std::fs::create_dir_all(&id1).unwrap();
        std::fs::write(id1.join("pak0.pak"), b"pak").unwrap();
        let vfs = profile.vfs();

        assert_eq!(vfs.resolve("fs:/~/../id1/pak0.pak"), Some(id1.join("pak0.pak")));
        let config = vfs
            .open_target("fs:/~/../id1/config.cfg", OpenIntent::Create)
            .unwrap();
        assert_eq!(config.path, profile.saves.join("id1/config.cfg"));
        // Deleting the pak targets the store, where it does not exist: the package's stays.
        assert_eq!(
            vfs.writable("fs:/~/../id1/pak0.pak"),
            Some(profile.saves.join("id1/pak0.pak"))
        );
        assert!(!vfs.is_writable_path(&id1.join("pak0.pak")));
    }

    /// What the game wrote can be deleted and renamed; the package cannot.
    #[test]
    fn the_store_protects_the_package_from_removal_and_rename() {
        let profile = Profile::create("rm", "mod/1");
        let modulo = &profile.modulo;
        std::fs::write(modulo.join("recurso.pak"), b"do pacote").unwrap();
        let vfs = profile.vfs();

        // The removal target of a package resource is the path in the store, not the package's.
        let in_store = profile.saves.join("1/recurso.pak");
        assert_eq!(vfs.writable("recurso.pak"), Some(in_store.clone()));
        assert!(!vfs.is_writable_path(&modulo.join("recurso.pak")));

        // Once copied to the store, the target becomes the store's.
        std::fs::create_dir_all(in_store.parent().unwrap()).unwrap();
        std::fs::write(&in_store, b"do save").unwrap();
        assert!(vfs.is_writable_path(&vfs.resolve("recurso.pak").unwrap()));
    }

    /// OpenTyrian opens `fs:/mod/opentyrian_zeebo/data/tyrian1.lvl`: it is its folder on the
    /// console.
    #[test]
    fn fs_mod_finds_the_loose_module_dir() {
        let profile = Profile::create("mod", "tyrian");
        let modulo = &profile.modulo;
        std::fs::create_dir_all(modulo.join("data")).unwrap();
        std::fs::create_dir_all(profile.saves.join("tyrian")).unwrap();
        std::fs::write(modulo.join("data/tyrian1.lvl"), b"fase").unwrap();
        std::fs::write(modulo.join("tyrian.cfg"), b"pacote").unwrap();
        std::fs::write(profile.saves.join("tyrian/tyrian.cfg"), b"gravado").unwrap();
        // A folder next to the loose `.mod` belongs to the user, not to the game.
        let sibling = modulo.parent().unwrap().join("outro");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("segredo"), b"x").unwrap();

        let vfs = profile.vfs();
        if is_case_sensitive(&profile.root) {
            assert_eq!(
                vfs.resolve("fs:/mod/tyrian/DATA/tyrian1.lvl"),
                Some(modulo.join("data/tyrian1.lvl"))
            );
        } else {
            let found = vfs
                .resolve("fs:/mod/tyrian/DATA/tyrian1.lvl")
                .expect("answered with the folder");
            assert_eq!(std::fs::read(&found).unwrap(), b"fase");
        }
        // What the game wrote wins over what came with it.
        assert_eq!(
            vfs.resolve("fs:/mod/tyrian/tyrian.cfg"),
            Some(profile.saves.join("tyrian/tyrian.cfg"))
        );
        assert_eq!(
            vfs.resolve("fs:/mod/tyrian/novo.sav"),
            Some(profile.saves.join("tyrian/novo.sav"))
        );
        assert_eq!(
            vfs.resolve_dir("fs:/mod/tyrian/data"),
            Some(modulo.join("data"))
        );
        // The neighbor does not show up: `../outro/segredo` lands in the store, where there is
        // nothing.
        assert_eq!(
            vfs.resolve("../outro/segredo"),
            Some(profile.saves.join("outro/segredo"))
        );
        assert_eq!(vfs.resolve("fs:/mod/../../segredo"), None);
    }

    /// A link inside the module's folder does not take the game outside it.
    #[cfg(unix)]
    #[test]
    fn a_symlink_does_not_escape_the_layer() {
        let profile = Profile::create("link", "tyrian");
        let outside = profile.root.join("fora");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("segredo"), b"x").unwrap();
        std::os::unix::fs::symlink(&outside, profile.modulo.join("atalho")).unwrap();
        let vfs = profile.vfs();

        // In the package: the link is ignored, and the path lands in the store, where there is
        // nothing.
        assert_eq!(
            vfs.resolve("atalho/segredo"),
            Some(profile.saves.join("tyrian/atalho/segredo"))
        );
        assert!(!vfs.contains_host(&profile.modulo.join("atalho/segredo")));

        // In the store: no reading, no creating, no deleting.
        std::os::unix::fs::symlink(&outside, profile.saves.join("tyrian")).unwrap();
        assert_eq!(vfs.resolve("segredo"), None);
        assert_eq!(vfs.open_target("x.sav", OpenIntent::Create), None);
        assert_eq!(vfs.writable("segredo"), None);
    }
}
