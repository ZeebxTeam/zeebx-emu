//! IFileMgr and IFile: the VFS as the game sees it.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Root of the file system the game sees.
    pub fn file_root(&self) -> &std::path::Path {
        self.vfs.root()
    }

    /// Files the game asked for that were not found.
    ///
    /// A resource that another file ended up providing does not count as missing. The Z-Wheel
    /// keeps what is language-independent in `tectoyli.brf` and the rest in
    /// `tectoy_<language>.brf`, and asks the language one first: image 5035 is not there, the game
    /// falls back to `li` and finds it. Recording the first attempt as a missing file makes a
    /// normal fallback look like a defect — and it did: I went after that line thinking it was an
    /// image we didn't have.
    pub fn missing_files(&self) -> Vec<String> {
        self.missing_files
            .iter()
            .filter(|remaining| match resource_id(remaining) {
                Some(id) => !self.resources_read.contains(&id),
                None => true,
            })
            .cloned()
            .collect()
    }

    /// `IFileMgr` and `IFile`, over the module directory.
    pub(super) fn file_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        // What the game asked the file system, with the return value: this is the instrument for
        // the class of problem where the game's error screen does not say which call failed —
        // Double Dragon shows "Memory is insufficient" whenever any space or file check does not
        // answer what it expects.
        let log_fs = |machine: &mut Self, line: String| {
            if machine.fs_log.len() >= MAX_FS_LOG {
                machine.fs_log.pop_front();
            }
            machine.fs_log.push_back(line);
        };
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.open_files.remove(&this);
                    self.enumerations.remove(&this);
                }
                remaining
            }
            // IFile *OpenFile(IFileMgr *, const char *pszName, OpenFileMode mode).
            // Returns the file pointer, not an error code.
            "OpenFile" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                self.open_file(&guest_path, a2)?
            }
            // int GetInfo(IFileMgr *, const char *pszName, FileInfo *pInfo)
            "GetInfo" if iface == Interface::FileMgr => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                // The Z-Wheel asks for the preload's metadata before deciding whether to open it.
                // It is console state, not a file shipped inside the package.
                let path = if let Some(cfg) = self.zwheel_cfg(&guest_path) {
                    Some(cfg)
                } else if guest_path.eq_ignore_ascii_case("preloaded.cfg") {
                    let path = self.vfs.profile_file("z-wheel", "preloaded.cfg");
                    if !path.exists() {
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::File::create(&path);
                    }
                    Some(path)
                } else {
                    self.vfs.resolve(&guest_path)
                };
                match path.and_then(|p| std::fs::metadata(&p).ok()) {
                    Some(meta) => {
                        self.write_file_info(a2, &guest_path, &meta)?;
                        SUCCESS
                    }
                    None => EFAILED,
                }
            }
            // int GetInfoEx(IFile *, AEEFileInfoEx *pInfo)
            //
            // **The size lives at `+0xc`**, and this was read, not assumed: at `0x89068` the
            // Z-Wheel calls this method and, on the next instruction, reads `sp[0xc]` as the size
            // and passes it to `malloc` — **without checking the return value**. Refusing is not
            // a neutral answer here: what it read was uninitialized stack, and what was left there
            // was a pointer. Hence the `check_malloc: Malloc failed` that showed up in the log.
            //
            // We write sixteen bytes and nothing more, and the limit has a reason. That function's
            // frame is `0x2c` and it keeps a local at `sp[0x28]`; the previous attempt filled in
            // the `FileInfo` layout, **file name included**, and the name ran over the neighbor.
            // The app read `0x6f6369d5` — text — as the size and asked for 1.8 GB.
            //
            // The first three fields are zeroed because we don't know what they are. Zero is an
            // answer the game knows how to handle; garbage is not.
            "GetInfoEx" if iface == Interface::File => {
                /// Where the size lives in `AEEFileInfoEx`, read at `0x89078`.
                const SIZE_OFFSET: u32 = 0xc;
                /// How much of the struct we fill. See above for why it isn't more.
                const CLEAR_LEN: usize = 0x10;

                let size = self
                    .open_files
                    .get(&this)
                    .and_then(|opened| opened.file.metadata().ok())
                    .map(|meta| meta.len() as u32);
                let Some(size) = size else {
                    return Ok(Some(EBADPARM));
                };
                if a1 != 0 {
                    self.cpu.write_mem(a1, &[0u8; CLEAR_LEN])?;
                    self.cpu.write_u32(a1 + SIZE_OFFSET, size)?;
                }
                SUCCESS
            }
            // int GetInfo(IFile *, FileInfo *pInfo)
            "GetInfo" if iface == Interface::File => match self.open_files.get(&this) {
                Some(open) => {
                    let (path, meta) = (open.guest_path.clone(), open.file.metadata().ok());
                    match meta {
                        Some(meta) => {
                            self.write_file_info(a1, &path, &meta)?;
                            SUCCESS
                        }
                        None => EFAILED,
                    }
                }
                None => EBADPARM,
            },
            "Test" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                // The preload is virtual console state: the official package does not ship it,
                // but the Z-Wheel tests for its existence before opening it. `OpenFile` creates it
                // in the profile right after.
                if guest_path.eq_ignore_ascii_case("preloaded.cfg") {
                    return Ok(Some(SUCCESS));
                }
                match self.vfs.resolve(&guest_path) {
                    Some(path) if path.exists() => SUCCESS,
                    _ => EFAILED,
                }
            }
            "Remove" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                // Only what the game wrote can be deleted: the package resource is immutable, and
                // deleting it would alter the original content.
                let Some(target) = self.vfs.writable(&guest_path) else {
                    return Ok(Some(EFAILED));
                };
                match std::fs::remove_file(&target) {
                    Ok(()) => SUCCESS,
                    Err(_) => EFAILED,
                }
            }
            // int IFILEMGR_Rename(IFileMgr *, const char *pszSrc, const char *pszDest)
            //
            // The destination is resolved by the exact path, without the case-insensitive
            // lookup: renaming creates a new name, and matching an existing file with different
            // case would overwrite the wrong file.
            "Rename" => {
                let origin = self.cpu.read_cstring(a1, MAX_STRING);
                let dest = self.cpu.read_cstring(a2, MAX_STRING);
                // The destination is always created in the writable layer; the source may be in
                // the package.
                let to = self.vfs.resolve_new(&dest);
                match (self.vfs.resolve(&origin), to) {
                    (Some(src_path), Some(to)) => {
                        if let Some(dest_dir) = to.parent() {
                            let _ = std::fs::create_dir_all(dest_dir);
                        }
                        // Source in the package: it is immutable, so the destination is created in
                        // the writable layer and the source stays. Written source: it is actually
                        // moved.
                        let rename_result = match self.vfs.is_writable_path(&src_path) {
                            true => std::fs::rename(&src_path, &to),
                            false => std::fs::copy(&src_path, &to).map(|_| ()),
                        };
                        match rename_result {
                            Ok(()) => SUCCESS,
                            Err(_) => EFAILED,
                        }
                    }
                    _ => EFAILED,
                }
            }
            "MkDir" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                match self.vfs.writable(&guest_path) {
                    Some(path) if std::fs::create_dir_all(&path).is_ok() => SUCCESS,
                    _ => EFAILED,
                }
            }
            "RmDir" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                match self.vfs.writable(&guest_path) {
                    Some(path) if std::fs::remove_dir(&path).is_ok() => SUCCESS,
                    _ => EFAILED,
                }
            }
            // uint32 GetFreeSpace(IFileMgr *, uint32 *pdwTotal)
            //
            // Returns the **free** space and writes the **total** to the pointer, if any.
            "GetFreeSpace" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, FS_TOTAL_BYTES)?;
                }
                FS_FREE_BYTES
            }
            // int GetFreeSpaceEx(IFileMgr *, const char *cpszPath, uint32 *pdwTotal,
            //                    uint32 *pdwFree)
            //
            // **It is not the same method.** It has four arguments, and the first one **is not an
            // output pointer**: it is the path of the file system being asked about. Treating the
            // two the same, the total was written over the game's string and the two output
            // pointers went unanswered — and Double Dragon, which asks for the space before
            // opening its data, concluded there was no memory and showed "Memory is insufficient.
            // Please delete some files." instead of the game.
            "GetFreeSpaceEx" => {
                let path_text = self.cpu.read_cstring(a1, MAX_STRING);
                let out_total = a2;
                let out_free = self.cpu.read_reg(Reg::R3);
                // There is no peripheral card on this console: the Zeebo keeps everything in NAND,
                // and `fs:/card0/` is how the game asks for one. `EUNSUPPORTED` is the documented
                // answer, and it is what makes the game use the main file system.
                if !path_text.is_empty() && !path_text.starts_with("fs:/") {
                    return Ok(Some(EUNSUPPORTED));
                }
                if out_total != 0 {
                    self.cpu.write_u32(out_total, FS_TOTAL_BYTES)?;
                }
                if out_free != 0 {
                    self.cpu.write_u32(out_free, FS_FREE_BYTES)?;
                }
                SUCCESS
            }
            // int EnumInit(IFileMgr *, const char *pszDir, boolean bDirs)
            //
            // The whole listing comes out of here, at once. BREW keeps the state inside the
            // `IFileMgr`, and so do we: the queue belongs to the manager, not a global.
            "EnumInit" => {
                let guest_dir = self.cpu.read_cstring(a1, MAX_STRING);
                let entries = self.list_dir(&guest_dir, a2 != 0);
                self.enumerations.insert(this, entries);
                SUCCESS
            }
            // boolean EnumNext(IFileMgr *, FileInfo *pInfo)
            //
            // False ends the enumeration, and the `GetLastError` that follows returns `EFAILED`
            // even when everything went fine — the documentation calls this compatibility with
            // the 1.0 client, and some games check it.
            //
            // **Each entry is looked up the way `OpenFile` would look it up**: in the overlay
            // before the package. The listing merges the two folders, and an entry that only
            // exists in the overlay — the save the game wrote — has no metadata in the package.
            // And an entry that does not resolve is skipped, not the end of the list. Those two
            // things together made Ultimate Chess 3D, with a saved match (`gamestd.sv2`), see the
            // list end at the save: the piece files, which come after it in the order, vanished,
            // and the game read field 0x14 of a null pointer when opening.
            "EnumNext" => {
                let mut next = None;
                while let Some(guest) = self
                    .enumerations
                    .get_mut(&this)
                    .and_then(std::collections::VecDeque::pop_front)
                {
                    let meta = self
                        .vfs
                        .resolve(&guest)
                        .filter(|p| p.exists())
                        .or_else(|| self.vfs.resolve_dir(&guest))
                        .and_then(|p| p.metadata().ok());
                    if let Some(meta) = meta {
                        next = Some((guest, meta));
                        break;
                    }
                }
                match next {
                    Some((guest, meta)) => {
                        self.write_file_info(a1, &guest, &meta)?;
                        TRUE
                    }
                    None => {
                        self.file_error = EFAILED;
                        FALSE
                    }
                }
            }
            "GetLastError" => self.file_error,
            // int ResolvePath(IFileMgr *, const char *cpszIn, char *pszOut, int *pnOutLen)
            "ResolvePath" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                let limit = if a3 != 0 {
                    self.cpu.read_u32(a3)? as usize
                } else {
                    0
                };
                self.write_cstring_limited(a2, &guest_path, limit)?;
                if a3 != 0 {
                    self.cpu.write_u32(a3, guest_path.len() as u32 + 1)?;
                }
                SUCCESS
            }
            // int32 Read(IFile *, void *pBuffer, uint32 dwCount)
            // uint32 Read(IFile *, void *pBuffer, uint32 dwCount)
            //
            // **A short read is not the end of the file.** Rust's `read` may return less than was
            // asked without anything having ended, and BREW's `IFILE_Read` delivers what was asked
            // for as long as there is file left. By retrying until the real end only, the Iron
            // Sight package — 16 MB read in large chunks — stops arriving truncated.
            "Read" => {
                // The size comes from the game: checked before allocating, or an absurd request
                // brings down the process instead of becoming an API error.
                let count = guest_size(a2 as usize)?;
                let mut buffer = vec![0u8; count];
                let read = match self.open_files.get_mut(&this) {
                    Some(open) => {
                        let mut got = 0;
                        while got < count {
                            match std::io::Read::read(&mut open.file, &mut buffer[got..]) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => got += n,
                            }
                        }
                        got
                    }
                    None => 0,
                };
                if read > 0 {
                    self.cpu.write_mem(a1, &buffer[..read])?;
                }
                read as u32
            }
            // uint32 Write(IFile *, const void *pBuffer, uint32 dwCount)
            "Write" => {
                let bytes = self.read_bytes(a1, a2)?;
                match self.open_files.get_mut(&this) {
                    Some(open) => std::io::Write::write(&mut open.file, &bytes).unwrap_or(0) as u32,
                    None => 0,
                }
            }
            // int32 Seek(IFile *, FileSeekType seek, int32 position)
            "Seek" => {
                let position = a2 as i32 as i64;
                let from = match a1 {
                    SEEK_END => std::io::SeekFrom::End(position),
                    SEEK_CURRENT => std::io::SeekFrom::Current(position),
                    _ => std::io::SeekFrom::Start(position.max(0) as u64),
                };
                match self.open_files.get_mut(&this) {
                    Some(open) => match std::io::Seek::seek(&mut open.file, from) {
                        // Documented in `IFILE_Seek.htm`: the return is `SUCCESS`, except in the
                        // special case of `_SEEK_CURRENT` with a zero offset, which returns the
                        // current position.
                        Ok(offset) if a1 == SEEK_CURRENT && a2 == 0 => offset as u32,
                        Ok(_) => SUCCESS,
                        Err(_) => EFAILED,
                    },
                    None => EFAILED,
                }
            }
            "Truncate" => match self.open_files.get_mut(&this) {
                Some(open) if open.file.set_len(a1 as u64).is_ok() => SUCCESS,
                _ => EFAILED,
            },
            // No cache of our own and no memory-mapped files.
            "SetCacheSize" => 0,
            "Map" => 0,
            "Cancel" => SUCCESS,
            _ => return Ok(None),
        };
        // The path goes into the line when the call has one: `Test`, `OpenFile`, `MkDir`,
        // `Remove`, `Rename` and `EnumInit` are the ones that decide where the game writes.
        let has_path = matches!(
            name,
            "Test" | "OpenFile" | "MkDir" | "Remove" | "Rename" | "EnumInit" | "GetFreeSpaceEx"
        );
        let path_text = match has_path && a1 != 0 {
            true => format!(" \"{}\"", self.cpu.read_cstring(a1, MAX_STRING)),
            false => String::new(),
        };
        log_fs(
            self,
            format!("{name}{path_text}  ({a1:#x} {a2:#x} {a3:#x}) -> {result}"),
        );
        Ok(Some(result))
    }

    /// The adjusted copy of `tectoy.cfg`, when that is what the Z-Wheel should read.
    ///
    /// `None` for any other file, another applet, or with the factory options — then the package
    /// file applies. See [`Machine::configure_z_wheel`].
    fn zwheel_cfg(&self, guest_path: &str) -> Option<std::path::PathBuf> {
        let factory_default = self.z_wheel.end_of_life && !self.z_wheel.always_transitions;
        if factory_default || self.applet_class != crate::session::Z_WHEEL {
            return None;
        }
        let name = guest_path.rsplit(['/', '\\']).next().unwrap_or(guest_path);
        if !name.eq_ignore_ascii_case("tectoy.cfg") {
            return None;
        }
        let original = std::fs::read_to_string(self.vfs.resolve(guest_path)?).ok()?;
        let copy = self.vfs.profile_file("z-wheel", "tectoy.cfg");
        std::fs::create_dir_all(copy.parent()?).ok()?;
        std::fs::write(&copy, adjust_cfg(&original, self.z_wheel)).ok()?;
        Some(copy)
    }

    /// Opens a game file, returning the `IFile*` or zero if it failed.
    pub(super) fn open_file(&mut self, guest_path: &str, mode: u32) -> Result<u32, CpuError> {
        let preloaded = guest_path.eq_ignore_ascii_case("preloaded.cfg");
        let path = if let Some(cfg) = self.zwheel_cfg(guest_path) {
            Some(cfg)
        } else if preloaded {
            let path = self.vfs.profile_file("z-wheel", "preloaded.cfg");
            if !path.exists() {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                // Without a NAND, the faithful answer is an empty list of preinstalled games. The
                // local ROM catalog comes in through the profile database, not through this file.
                let _ = std::fs::File::create(&path);
            }
            Some(path)
        } else {
            None
        };
        if let Some(cfg) = path {
            // Emulator profile file: same mode semantics as the regular path.
            return self.open_resolved(guest_path, cfg, mode);
        }
        // From here on the VFS resolution applies, which picks between content and overlay and
        // says whether the package file has to be copied before it is written to.
        let intent = match (mode & OFM_CREATE != 0, mode & (OFM_READWRITE | OFM_APPEND) != 0) {
            (true, _) => crate::brew::vfs::OpenIntent::Create,
            (false, true) if mode & OFM_APPEND != 0 => crate::brew::vfs::OpenIntent::Append,
            (false, true) => crate::brew::vfs::OpenIntent::ReadWrite,
            (false, false) => crate::brew::vfs::OpenIntent::Read,
        };
        let Some(target) = self.vfs.open_target(guest_path, intent) else {
            self.file_error = EFAILED;
            return Ok(0);
        };
        let path = target.path;
        // Only writes go into the log: they say when the game writes its save, and there are
        // hundreds of reads per run. Because of issue #70, where Ridge Racer's progress
        // disappears and we didn't even know whether the game got as far as writing.
        if !matches!(intent, crate::brew::vfs::OpenIntent::Read) {
            crate::logging!(
                crate::logging::Level::Info,
                "arquivo",
                "{guest_path} opened for writing at {}{}",
                path.display(),
                match target.copy_from.is_some() {
                    true => ", copied from the package",
                    false => "",
                }
            );
        }
        // The package is never altered: the first write copies the resource to the overlay.
        if let Some(origin) = target.copy_from {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::copy(&origin, &path).is_err() {
                self.file_error = EFAILED;
                return Ok(0);
            }
        }
        // **On the console the module directory already exists; here it doesn't.** Several games
        // open `udata/something` with `OFM_CREATE` without calling `MkDir` first — Double Dragon
        // is one of them, and when the directory creation was lost in a refactor, it failed to
        // open the save and showed the "Memory is insufficient. Please delete some files." screen
        // instead of the game.
        if mode & OFM_CREATE != 0
            && let Some(parent) = path.parent()
        {
            let _ = std::fs::create_dir_all(parent);
        }
        self.open_resolved(guest_path, path, mode)
    }

    /// Whether the game has any file open for writing right now.
    ///
    /// **A rewind point is never taken in the middle of a write.** The state stores the open file
    /// by path and offset, not by content (see `save.rs`); going back to the middle of a write
    /// would make the game keep writing from an old offset over a file that has already changed —
    /// the player's save. See `docs/implementation/24-speed.md`.
    pub fn has_writable_file(&self) -> bool {
        self.open_files.values().any(|opened| opened.for_writing)
    }

    /// The open options equivalent to the BREW mode.
    fn open_options(mode: u32) -> std::fs::OpenOptions {
        let mut options = std::fs::OpenOptions::new();
        if mode & OFM_CREATE != 0 {
            options.create(true).read(true).write(true);
        } else if mode & (OFM_READWRITE | OFM_APPEND) != 0 {
            options.read(true).write(true);
        } else {
            options.read(true);
        }
        if mode & OFM_APPEND != 0 {
            options.append(true);
        }
        options
    }

    /// Opens the already resolved `path`, in the BREW mode, and registers the `IFile`.
    fn open_resolved(
        &mut self,
        guest_path: &str,
        path: std::path::PathBuf,
        mode: u32,
    ) -> Result<u32, CpuError> {
        let options = Self::open_options(mode);
        let Ok(file) = options.open(&path) else {
            // The host path and the system error go into the log: "the game couldn't open the
            // save" does not say whether the problem is the resolved path, the permission or the
            // mode.
            let error = options.open(&path).err().map(|e| e.to_string());
            if self.fs_log.len() >= MAX_FS_LOG {
                self.fs_log.pop_front();
            }
            self.fs_log.push_back(format!(
                "open failed: {} ({})",
                path.display(),
                error.unwrap_or_else(|| "no reason".into())
            ));
            self.file_error = EFAILED;
            self.missing_files.insert(guest_path.to_string());
            return Ok(0);
        };
        let handle = self.new_object(Interface::File)?;
        if handle == 0 {
            return Ok(0);
        }
        self.open_files.insert(
            handle,
            OpenFile {
                file,
                guest_path: guest_path.to_string(),
                path,
                for_writing: mode & (OFM_CREATE | OFM_READWRITE | OFM_APPEND) != 0,
            },
        );
        self.file_error = SUCCESS;
        Ok(handle)
    }

    /// Fills a `FileInfo`: `char attrib` (with three bytes of padding), `uint32
    /// dwCreationDate`, `uint32 dwSize` and `char szName[64]`.
    /// The files (or the directories) of a guest directory, already with the path the game
    /// understands.
    ///
    /// The returned name is the full path, with the same prefix the game passed: it is what goes
    /// back to `OpenFile` right after, and a bare name would open nothing.
    pub(super) fn list_dir(
        &self,
        guest_dir: &str,
        want_dirs: bool,
    ) -> std::collections::VecDeque<String> {
        let Some(dir) = self.vfs.resolve_dir(guest_dir) else {
            return Default::default();
        };
        // Package and writable layer add up: the game needs to see what came in the package
        // **and** what it wrote itself. `read_dir` promises no order, and a game that builds a
        // menu from it would change order between two openings; that's why the list is sorted.
        let mut dirs = vec![dir];
        if let Some(writable_path) = self.vfs.writable_dir(guest_dir)
            && !dirs.contains(&writable_path)
        {
            dirs.push(writable_path);
        }
        let mut names: Vec<String> = Vec::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            names.extend(
                entries
                    .flatten()
                    .filter(|entry| entry.path().is_dir() == want_dirs)
                    // **The package manifest does not belong to the game.** It is *our* file,
                    // written in the folder the guest sees as its root, and a game that enumerates
                    // its own folder must not see in it a file the console doesn't have. See
                    // [`crate::loader::archive::MANIFEST`].
                    .filter(|entry| entry.file_name() != crate::loader::archive::MANIFEST)
                    .filter_map(|entry| entry.file_name().into_string().ok()),
            );
        }
        names.sort();
        names.dedup();
        let prefix = guest_dir.trim_end_matches('/');
        names
            .into_iter()
            .map(|name| match prefix.is_empty() {
                true => name,
                false => format!("{prefix}/{name}"),
            })
            .collect()
    }

    pub(super) fn write_file_info(
        &mut self,
        addr: u32,
        guest_path: &str,
        meta: &std::fs::Metadata,
    ) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        let attrib = if meta.is_dir() { FA_DIR } else { FA_NORMAL };
        self.cpu.write_u32(addr, attrib)?;
        self.cpu.write_u32(addr + 4, 0)?;
        self.cpu.write_u32(addr + 8, meta.len() as u32)?;
        let mut name = guest_path.as_bytes().to_vec();
        name.truncate(MAX_FILE_NAME - 1);
        name.resize(MAX_FILE_NAME, 0);
        self.cpu.write_mem(addr + 12, &name)
    }
}

/// `tectoy.cfg` with the changes from [`Machine::configure_z_wheel`], the rest as it came.
fn adjust_cfg(original: &str, options: crate::config::ZWheel) -> String {
    original
        .split_inclusive('\n')
        .map(|line| {
            let key_id = line.split('=').next().unwrap_or("").trim();
            let end = &line[line.trim_end_matches(['\r', '\n']).len()..];
            let zeroed = match key_id {
                "EOL" | "zeebomenu_hide" => !options.end_of_life,
                "SlideOnceToForm" => options.always_transitions,
                _ => false,
            };
            match zeroed && line.contains('=') {
                true => format!("{key_id}=0{end}"),
                false => line.to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod cfg_tests {
    use super::adjust_cfg;
    use crate::config::ZWheel;

    const ORIGINAL: &str = "; EOL\r\nEOL=1\r\n#zeebomenu_hide - x\r\nzeebomenu_hide=1\r\nEOLX=1\r\nSlideOnceToForm=31\n";

    #[test]
    fn without_end_of_life_only_the_two_keys_change() {
        let options = ZWheel { end_of_life: false, always_transitions: false };
        assert_eq!(
            adjust_cfg(ORIGINAL, options),
            "; EOL\r\nEOL=0\r\n#zeebomenu_hide - x\r\nzeebomenu_hide=0\r\nEOLX=1\r\nSlideOnceToForm=31\n"
        );
    }

    #[test]
    fn always_transitions_zero_the_slide_once() {
        let options = ZWheel { end_of_life: true, always_transitions: true };
        assert_eq!(
            adjust_cfg(ORIGINAL, options),
            "; EOL\r\nEOL=1\r\n#zeebomenu_hide - x\r\nzeebomenu_hide=1\r\nEOLX=1\r\nSlideOnceToForm=0\n"
        );
    }
}
