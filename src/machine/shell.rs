//! IShell: resources, device information and the applet's startup.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// The three timer methods of `IShell`, all with the signature
    /// `(IShell *, [int32 dwMsecs,] PFNNOTIFY pfn, void *pUser)`.
    ///
    /// `PFNNOTIFY` is `void (*)(void *pUser)`; the `(pfn, pUser)` pair identifies the timer, and
    /// it is by that pair that `CancelTimer` and `GetTimerExpiration` find it.
    pub(super) fn shell_timer_call(&mut self, name: &str) -> Result<u32, CpuError> {
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        Ok(match name {
            "SetTimer" => {
                let callback = Callback {
                    function: a2,
                    context: a3,
                };
                if callback.function == 0 {
                    return Ok(EBADPARM);
                }
                // Rearming the same `(pfn, pUser)` pair replaces the previous timer instead of
                // stacking two — that is what BREW does, and what the game expects when it
                // rearms inside its own callback.
                self.timers.retain(|timer| timer.callback != callback);
                // **The deadline is `int32`, and negative means "already expired".** A game's
                // frame loop arms "frame duration minus what it already spent"; Powerboat
                // Challenge, after loading the whole stage in a single frame, asks for -2 ms.
                // Read as unsigned, that was a 49-day deadline, the timer never expired and the
                // game stayed on the loading screen forever.
                let wait = (a1 as i32).max(0) as u32;
                self.timers.push(Timer {
                    deadline_ms: self.now_ms().saturating_add(wait),
                    callback,
                });
                SUCCESS
            }
            "CancelTimer" => {
                let (function, context) = (a1, a2);
                // A null `pfn` cancels all the timers of that context.
                self.timers.retain(|timer| {
                    timer.callback.context != context
                        || (function != 0 && timer.callback.function != function)
                });
                SUCCESS
            }
            // Returns how much time is left, in milliseconds; zero if no timer is armed.
            _ => {
                let callback = Callback {
                    function: a1,
                    context: a2,
                };
                self.timers
                    .iter()
                    .find(|timer| timer.callback == callback)
                    .map(|timer| timer.deadline_ms.saturating_sub(self.now_ms()))
                    .unwrap_or(0)
            }
        })
    }

    /// `int ISHELL_DetectType(IShell *, const void *cpBuf, uint32 *pdwSize,
    /// const char *cpszName, const char **pcpszMIME)`.
    ///
    /// Finds out the MIME type of some content. The game uses the answer to look up, in the BREW
    /// registry, which class knows how to open that file.
    pub(super) fn shell_detect_type(&mut self) -> Result<u32, CpuError> {
        let (buffer, size_ptr, name_ptr, mime_ptr) =
            (self.arg(1), self.arg(2), self.arg(3), self.arg(4));

        // With no data and no name, the question is "how many bytes do you need?".
        if buffer == 0 && name_ptr == 0 {
            self.write_at(size_ptr, DETECT_TYPE_BYTES)?;
            return Ok(ENEEDMORE);
        }

        let available = if size_ptr == 0 {
            0
        } else {
            self.cpu.read_u32(size_ptr)?
        };
        let bytes = if buffer == 0 {
            Vec::new()
        } else {
            self.read_bytes(buffer, available.min(DETECT_TYPE_BYTES))?
        };
        let name = self.cpu.read_cstring(name_ptr, MAX_STRING);

        match detect_mime(&bytes, &name) {
            Some(mime) => {
                let text = self.intern(mime)?;
                self.write_at(mime_ptr, text)?;
                Ok(SUCCESS)
            }
            None => Ok(ENOTYPE),
        }
    }

    /// `int ISHELL_Resume(IShell *, AEECallback *pcb)` — schedules the callback for the next
    /// turn of the event loop.
    ///
    /// It is the mechanism cooperative threads rely on: the game asks for the resume through
    /// here and only then calls `Suspend`, so that the thread has a way back.
    pub(super) fn shell_resume(&mut self) -> Result<u32, CpuError> {
        let pcb = self.cpu.read_reg(Reg::R1);
        if let Some(&thread) = self.resume_callbacks.get(&pcb) {
            if !self.pending_threads.contains(&thread) {
                self.pending_threads.push(thread);
            }
            return Ok(SUCCESS);
        }
        let call = self.resolve_notify(Callback {
            function: pcb,
            context: pcb,
        })?;
        self.queue_call(call);
        Ok(SUCCESS)
    }

    /// `IBase *ISHELL_LoadResObject(IShell *po, const char *pszResFile, uint16 nResID,
    /// AEECLSID cls)`.
    ///
    /// With `nResID` zero the whole file is the resource — that is how Quake loads
    /// `fs:/~/../id1/splash_title.png`. With a nonzero `nResID` what it names is a `.bar`, and
    /// the image is the entry with that number inside it.
    ///
    /// Ignoring `nResID` cost dearly: Tekken 2 asks for entry 5034 of `tekken2.bar` and we
    /// tried to decode the whole 734 KB `.bar` as a PNG. It failed, returned null, and the game
    /// went on with a sizeless image — which is a division by zero when laying out the screen.
    /// The report said what was happening all along, in the line "a resource requested by
    /// LoadResObject is not a PNG we can read".
    ///
    /// With `cls` zero, BREW infers the class from the content; here the only one we know how to
    /// produce is the image, and that is all the games ask for.
    pub(super) fn shell_load_res_object(&mut self) -> Result<u32, CpuError> {
        let guest_path = self
            .cpu
            .read_cstring(self.cpu.read_reg(Reg::R1), MAX_STRING);
        let id = self.cpu.read_reg(Reg::R2) as u16;
        let cls = self.cpu.read_reg(Reg::R3);
        let Some(path) = self.vfs.resolve(&guest_path) else {
            self.missing_files.insert(guest_path);
            return Ok(0);
        };
        let bytes = match id {
            0 => match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(_) => {
                    self.missing_files.insert(guest_path);
                    return Ok(0);
                }
            },
            _ => {
                let raw = self
                    .resources
                    .open(&path)
                    .and_then(|res| res.get(crate::loader::resfile::RESTYPE_IMAGE, id))
                    .map(<[u8]>::to_vec);
                let Some(raw) = raw else {
                    self.missing_files
                        .insert(format!("{guest_path} (recurso {id})"));
                    return Ok(0);
                };
                self.resources_read.insert(id);
                // The `AEEResBlob` header is ours to skip: what was asked for is an image
                // **object**, not the raw block that `LoadResData` hands out.
                crate::loader::resfile::blob_data(&raw)
                    .unwrap_or(&raw)
                    .to_vec()
            }
        };
        let Some(decoded) = self.decode_resource_image(&bytes) else {
            self.assumptions
                .insert("a resource requested by LoadResObject came in a format we cannot read");
            return Ok(0);
        };

        // The requested class decides what comes out of here. Tekken 2 asks for
        // `AEEIID_IBITMAP` — it wants to draw with `IDISPLAY_BitBlt`, not with `IIMAGE_Draw` —,
        // and returning an `IImage` made it call an `IBitmap` method on an `IImage` vtable:
        // slot 12, which does not exist in `IImage`.
        if cls == AEEIID_IBITMAP {
            return self.bitmap_from_decoded(&decoded);
        }
        let image = self.new_object(Interface::Image)?;
        if image != 0 {
            self.images.insert(image, std::rc::Rc::new(decoded));
        }
        Ok(image)
    }

    /// Decodes a resource image according to what it is.
    ///
    /// PNG goes through its own path, which brings the alpha channel games use to cut out the
    /// sprite. The rest — BMP and JPEG — comes through the icon decoder, which already knows how
    /// to read them and delivers everything opaque, which is what those two formats are.
    pub(super) fn decode_resource_image(&mut self, bytes: &[u8]) -> Option<DecodedImage> {
        if let Some(decoded) = decode_png(bytes) {
            return Some(decoded);
        }
        if let Some(gif) = crate::video::gif::decode(bytes) {
            return Some(frame_strip(&gif));
        }
        let image = crate::video::icon::decode(bytes).ok()?;
        let count = image.width * image.height;
        let mut pixels = Vec::with_capacity(count);
        for at in (0..count * 4).step_by(4) {
            pixels.push(
                Rgb {
                    r: image.rgba[at],
                    g: image.rgba[at + 1],
                    b: image.rgba[at + 2],
                }
                .to_rgb565(),
            );
        }
        Some(DecodedImage {
            width: image.width as u32,
            height: image.height as u32,
            pixels,
            opaque: vec![true; count],
            alpha: Vec::new(),
            frame_width: 0,
        })
    }

    /// Opens the resource file the game named.
    ///
    /// The name may be null, and Peggle sends null: BREW takes that as "this applet's resource
    /// file". Since there is no naming convention that works — Peggle calls its file
    /// `resources.bar` and Pac-Mania calls its own `pacmania.bar` —, what is left is the only
    /// `.bar` that exists next to the module. If there is more than one, there is no way to
    /// choose, and none is opened.
    pub(super) fn open_res_file(
        &mut self,
        pointer: u32,
    ) -> Option<&crate::loader::resfile::ResFile> {
        let path = match pointer {
            0 => self.default_res_file()?,
            _ => {
                let guest_path = self.cpu.read_cstring(pointer, MAX_STRING);
                match self.vfs.resolve(&guest_path) {
                    Some(path) => path,
                    None => {
                        self.missing_files.insert(guest_path);
                        return None;
                    }
                }
            }
        };
        self.resources.open(&path)
    }

    /// The only `.bar` next to the module, if there is exactly one.
    pub(super) fn default_res_file(&self) -> Option<std::path::PathBuf> {
        let mut found = std::fs::read_dir(self.vfs.root())
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("bar"));
        let first = found.next()?;
        match found.next() {
            None => Some(first),
            Some(_) => None,
        }
    }

    /// Picks a language when the preferences database does not have one yet.
    ///
    /// The Z-Wheel stores in `PREFSINFO.Lang` the language **tag** packed into four bytes —
    /// `"pt  "` becomes `0x20207470` —, not an index. Whatever builds the z-pad screen walks the
    /// module's language list comparing each one's tag with that value; when it finds nothing, it
    /// returns zero and the game repeats `Couldn't create z-pad instruction form (6)` forever.
    /// The package ships with `Lang = 0`, which means "nobody has chosen yet": on the console
    /// what fills it in is the first-setup screen, which we have not reached yet.
    ///
    /// Choosing on our own is an assumption, and it is recorded as such. It is Portuguese
    /// because that is the language of the device TecToy sold, and because `tectoy_pt.brf` is in
    /// the package.
    pub(super) fn choose_language(&mut self, db: &crate::brew::sql::Database) {
        /// `"pt  "` read as a 32-bit word, which is how the module compares it.
        const PORTUGUESE: u32 = u32::from_le_bytes(*b"pt  ");
        let no_choice = db
            .exec("SELECT dwValue FROM PREFSINFO WHERE PREFSINFO.name = 'Lang'")
            .ok()
            .and_then(|lines| lines.into_iter().next())
            .and_then(|line| line.values.into_iter().next().flatten())
            .is_some_and(|value| value == "0");
        if !no_choice {
            return;
        }
        let saved = db.exec(&format!(
            "UPDATE PREFSINFO SET dwValue = {PORTUGUESE} WHERE PREFSINFO.name = 'Lang'"
        ));
        if saved.is_ok() {
            self.assumptions
                .insert("the language was not chosen in the database and we assumed Portuguese");
        }
    }

    /// `int ISHELL_LoadResString(IShell *po, const char *pszResFile, int16 nResID,
    /// AECHAR *pBuff, int nSize)`.
    ///
    /// Returns **how many characters** were written, and zero when the resource does not exist —
    /// which is a legitimate answer, not an error.
    pub(super) fn shell_load_res_string(&mut self) -> Result<u32, CpuError> {
        let (file, id) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2) as u16,
        );
        let (buffer, capacity) = (self.cpu.read_reg(Reg::R3), self.stack_arg(0)?);
        let Some(text) = self.open_res_file(file).and_then(|res| res.string(id)) else {
            return Ok(0);
        };
        if buffer == 0 || capacity < 2 {
            return Ok(0);
        }
        // `nSize` is in **bytes**, and each `AECHAR` takes two; the terminator counts too.
        let room = (capacity as usize / 2).saturating_sub(1);
        let written = text.len().min(room);
        let bytes: Vec<u8> = text[..written]
            .iter()
            .chain(std::iter::once(&0))
            .flat_map(|u| u.to_le_bytes())
            .collect();
        self.cpu.write_mem(buffer, &bytes)?;
        Ok(written as u32)
    }

    /// `void *ISHELL_LoadResData(IShell *po, const char *pszResFile, uint16 nResID,
    /// ResType nType)` and the `Ex` variant, which also takes `void *pBuf, uint32 *pnBufSize`.
    ///
    /// The documentation is explicit: what comes out of here is the **raw** content of the
    /// resource, `AEEResBlob` header included. The game is the one that interprets it, with
    /// `RESBLOB_DATA()`.
    pub(super) fn shell_load_res_data(&mut self, with_buffer: bool) -> Result<u32, CpuError> {
        let (file, id) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2) as u16,
        );
        let kind = self.cpu.read_reg(Reg::R3) as u16;
        let Some(data) = self
            .open_res_file(file)
            .and_then(|res| res.get(kind, id))
            .map(<[u8]>::to_vec)
        else {
            return Ok(0);
        };
        let len = data.len() as u32;

        if !with_buffer {
            // Without a caller buffer, BREW allocates — and `FreeResData` is what frees it.
            let ptr = self.malloc(len)?;
            if ptr != 0 {
                self.cpu.write_mem(ptr, &data)?;
            }
            return Ok(ptr);
        }

        let (buffer, size_out) = (self.stack_arg(0)?, self.stack_arg(1)?);
        // A `pBuf` of -1 asks only for the size, without copying anything.
        if buffer == u32::MAX {
            self.write_at(size_out, len)?;
            return Ok(u32::MAX);
        }
        if buffer == 0 {
            let ptr = self.malloc(len)?;
            if ptr != 0 {
                self.cpu.write_mem(ptr, &data)?;
            }
            self.write_at(size_out, len)?;
            return Ok(ptr);
        }
        // With a caller buffer, `*pnBufSize` arrives holding its size. If it does not fit, the
        // documentation says to return null.
        let declared = match size_out {
            0 => 0,
            _ => self.cpu.read_u32(size_out)?,
        };
        // Not every game fills in the input `*pnBufSize`. Peggle queries the resource size,
        // allocates the 64,629 bytes the query returned and calls again with `*pnBufSize` set
        // to 6 — the type number, which was left in the variable. Refusing because of that left
        // the buffer zeroed, and the decoder got sixty thousand bytes of zeros: the game crashed
        // right after, on a null pointer it does not even check.
        //
        // The size of the block on the heap is the honest measure: it is not the number the game
        // said, it is what it actually reserved. That way the copy stays limited to what exists.
        let capacity = declared.max(self.block_size(buffer).unwrap_or(0));
        if capacity < len {
            self.write_at(size_out, len)?;
            return Ok(0);
        }
        self.cpu.write_mem(buffer, &data)?;
        self.write_at(size_out, len)?;
        Ok(buffer)
    }

    /// `uint32 ISHELL_GetClassItemID(IShell *po, AEECLSID cls)`.
    ///
    /// The item ID is the number the BREW store gives the package that installed the module —
    /// and it is literally the name of the directory the `.mod` lives in (`mod/277083/bjt.mod`).
    /// It is not a guess: it is the same numbering that appears in the `.mif` and in the ROM
    /// path.
    ///
    /// The documentation says to return 0 when the class is not from a downloaded module, which
    /// is what we do for any class that is not the loaded applet's.
    pub(super) fn shell_get_class_item_id(&mut self) -> u32 {
        let cls = self.cpu.read_reg(Reg::R1);
        self.item_id_of(cls)
    }

    /// The item ID of an installed class. See [`Machine::shell_get_class_item_id`].
    ///
    /// **This fix was not the cause of the Z-Wheel gesture, and it is worth saying so.** I got
    /// to it reading the disassembly of the wheel's event handler, which calls a method through
    /// a vtable offset — and the instrumentation showed that `GetClassItemID` **is never
    /// dispatched**. The offset belonged to another object. What stays is what the API says: the
    /// item ID belongs to the installed module, and returning zero for an applet that exists is
    /// the wrong answer, with or without the Z-Wheel.
    fn item_id_of(&self, cls: u32) -> u32 {
        // **The item ID applies to any installed applet, not just the loaded one.** This is where
        // the Z-Wheel stopped: it asks for the id of the **chosen** game, which by definition is
        // not its own class, and the answer was always zero. Without the id, the launch never
        // happens — it navigates, the screen changes, and `StartApplet` is never called. Measured
        // in the `IShell` trace, with the call showing up on every key and the value always zero.
        //
        // The number comes from the `.mif` name, which is the module's folder number — the same
        // numbering as the path and as the console's `ISHELL_GetClassItemID`.
        if let Some((_, mif_name)) = self
            .installed_modules
            .iter()
            .find(|(class_id, _)| *class_id == cls)
        {
            if let Ok(id) = mif_name.parse() {
                return id;
            }
        }
        // The loaded applet's class answers with its own directory, as before: that is the case
        // of a game that runs on its own and asks for its own id.
        if cls == self.applet_class {
            return self
                .vfs
                .root()
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.parse().ok())
                .unwrap_or(0);
        }
        0
    }

    /// `ISHELL_GetDeviceInfo(IShell *po, AEEDeviceInfo *pi)`.
    ///
    /// We fill in only the fields at the start of the struct, whose position is unambiguous: the
    /// first eight `uint16`. What comes after depends on how the compiler packs bitfields, and
    /// guessing a layout here would be worse than leaving it zeroed. The fields from
    /// `wStructSize` on are filled in by the caller itself and are not touched.
    pub(super) fn shell_get_device_info(&mut self) -> Result<u32, CpuError> {
        let info = self.cpu.read_reg(Reg::R1);
        if info == 0 {
            return Ok(SUCCESS);
        }

        // The caller writes `wStructSize` **before** the call to ask for the extended fields,
        // and Bejeweled Twist does ask: it sends 64. While we only filled in the first 44 bytes,
        // it got `wMaxPath = 0` — no file path would fit.
        let mut requested = [0u8; 2];
        self.cpu.read_mem(info + 44, &mut requested)?;
        let requested = u16::from_le_bytes(requested) as u32;

        // Zero up to `dwLang`, the last field before the part the caller fills in.
        self.cpu.write_mem(info, &[0u8; 44])?;

        let (screen_width, screen_height) = self.reported_screen(self.applet_class);
        let fields: [u16; 8] = [
            screen_width,
            screen_height,
            0, // cxAltScreen: the Zeebo has no second screen
            0, // cyAltScreen
            8, // cxScrollBar
            AEE_ENC_ISOLATIN1,
            0, // wMenuTextScroll
            COLOR_DEPTH,
        ];
        for (i, value) in fields.iter().enumerate() {
            self.cpu
                .write_mem(info + i as u32 * 2, &value.to_le_bytes())?;
        }
        // `dwRAM` — the documentation calls it "initial size of the BREW heap".
        self.cpu.write_u32(info + 24, loader::HEAP_SIZE as u32)?;

        if requested >= DEVICE_INFO_SIZE {
            self.cpu
                .write_mem(info + 44, &(DEVICE_INFO_SIZE as u16).to_le_bytes())?;
            self.cpu.write_u32(info + 48, 0)?; // dwNetLinger: no network, nothing to keep open
            self.cpu.write_u32(info + 52, 0)?; // dwSleepDefer: the console does not sleep
            self.cpu
                .write_mem(info + 56, &(AEE_MAX_FILE_NAME as u16).to_le_bytes())?;
            self.cpu.write_u32(info + 60, 0)?; // dwPlatformID: we do not know the Zeebo's
        }
        Ok(SUCCESS)
    }

    /// `ISHELL_GetDeviceInfoEx(IShell *po, AEEDeviceItem nItem, void *pBuff, int *pnSize)`.
    ///
    /// `pnSize` is both input and output: it comes in with the buffer size and goes out with
    /// what the item needs. With a null `pBuff` the call only asks for that size, and that is how
    /// Zeebo Sports Peteca asks for the IMEI — returning success without writing there left the
    /// game allocating with garbage and reading outside memory.
    pub(super) fn shell_get_device_info_ex(&mut self) -> Result<u32, CpuError> {
        let (item, buffer, size) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        if size == 0 {
            return Ok(EBADPARM);
        }
        // **The requested item goes to the serial capture**, and that is what was missing to
        // answer "the Z-Wheel does not build the store" with a measurement instead of a guess: an
        // unrecorded `EUNSUPPORTED` is indistinguishable from never having been asked. The item
        // that matters here is `AEE_DEVICEITEM_ADS` (29, from `AEEDeviceItems.h`): the name of the
        // server the store downloads the catalog from.
        if self.serial.is_some() {
            let outcome = match item {
                DEVICEITEM_IMEI => "served",
                _ => "not served",
            };
            self.log_serial(format!("<device item {item} -> {outcome}>"));
        }
        let value = match item {
            DEVICEITEM_IMEI => IMEI,
            _ => return Ok(EUNSUPPORTED),
        };
        let capacity = self.cpu.read_u32(size)? as usize;
        self.cpu.write_u32(size, value.len() as u32)?;
        if buffer != 0 {
            // Whether it fits or not, whatever fits is written: the documentation allows for
            // partial filling, with `pnSize` saying how much was missing.
            self.cpu
                .write_mem(buffer, &value[..capacity.min(value.len())])?;
        }
        Ok(SUCCESS)
    }

    /// Sends `EVT_APP_START` to the applet, which is what starts the game.
    ///
    /// `boolean IAPPLET_HandleEvent(IApplet *po, AEEEvent evt, uint16 wParam, uint32 dwParam)`
    /// — slot 2 of the `IApplet` vtable. On `EVT_APP_START`, `dwParam` points to an
    /// `AEEAppStart`, which we build on the heap.
    pub fn start_applet(
        &mut self,
        applet: u32,
        clsid: u32,
        budget: u64,
    ) -> Result<Outcome, CpuError> {
        let vtable = self.cpu.read_u32(applet)?;
        let handle_event = self.cpu.read_u32(vtable + 2 * 4)?;
        // From here on `GetAppInstance` has something to return — and starts answering without
        // leaving the CPU.
        self.current_applet = applet;
        self.install_app_instance_stub(applet)?;

        let display = self.objects.create(Interface::Display).unwrap_or(0);
        if display != 0 {
            self.cpu
                .write_u32(display, loader::vtable_addr(Interface::Display))?;
        }

        // AEEAppStart: error, clsApp, pDisplay, rc (4 × int16), pszArgs = 24 bytes.
        let start = self.alloc_internal(24)?;
        if start != 0 {
            self.cpu.write_mem(start, &[0u8; 24])?;
            self.cpu.write_u32(start + 4, clsid)?;
            self.cpu.write_u32(start + 8, display)?;
            let (width, height) = self.reported_screen(clsid);
            for (i, value) in [0i16, 0, width as i16, height as i16]
                .iter()
                .enumerate()
            {
                self.cpu
                    .write_mem(start + 12 + i as u32 * 2, &value.to_le_bytes())?;
            }
        }

        self.call_guest(handle_event, [applet, EVT_APP_START, 0, start], budget)
    }

    /// Queues a keyboard key, by its BREW virtual code.
    ///
    /// It does not go straight in: the keyboard reaches the game as an **event**, and an event
    /// can only be delivered at the boundary between two API calls — calling the game's handler
    /// in the middle of a dispatch is the path that already brought Zeeboids down. The queue is
    /// drained in [`Machine::deliver_signals`].
    /// The installed applets, each with the module id — the `.mif` name without extension.
    pub fn set_installed_applets(&mut self, applets: impl IntoIterator<Item = (u32, String)>) {
        self.installed_modules = applets.into_iter().collect();
        self.installed_applets = self.installed_modules.iter().map(|(c, _)| *c).collect();
    }

    /// Delivers an event with `wParam` to the applet, as a widget would when activated.
    ///
    /// **It is a test instrument, and only that.** Event `0x7000` with `w = 0x4ea` is what the
    /// Z-Wheel's handler reads to call `IShell::StartApplet` (measured in the disassembly, at
    /// `0x7b4c0` and `0x7b51c`): on the console a widget sends it, and in the emulator the
    /// wheel's widget list is empty. This makes it possible to exercise **the rest of the cycle**
    /// — the launch request, the session switch and the return — without depending on the
    /// missing widget.
    pub fn deliver_event_to_applet(&mut self, evt: u32, w: u16) -> Result<u32, CpuError> {
        self.send_applet_event(self.applet_class, evt, w, 0)
    }

    /// The host switches sessions after the guest call has finished.
    pub fn take_launch_request(&mut self) -> Option<u32> {
        self.pending_launch.take()
    }

    /// `ISHELL_CreateInstance(IShell *po, AEECLSID ClsId, void **ppobj)`.
    ///
    /// It is the object factory for all of BREW. Each known ClassID becomes one of our objects
    /// with the vtable of the matching interface; what we do not know returns
    /// `ECLASSNOTSUPPORT`, which is a legitimate answer — the game treats a missing class as a
    /// platform without that feature, instead of crashing.
    pub(super) fn shell_create_instance(&mut self) -> Result<u32, CpuError> {
        let clsid = self.cpu.read_reg(Reg::R1);
        let out = self.cpu.read_reg(Reg::R2);
        // Every requested class goes into the report, not just the ones we could not serve:
        // without the full list there is no way to see that the game asked for a class we
        // answered with the **wrong interface** — which was the case with the system fonts.
        *self.requested_classes.entry(clsid).or_insert(0) += 1;
        if self.serial.is_some() {
            self.log_serial(format!("<class {clsid:#010x}>"));
        }

        let iface = match clsid {
            AEECLSID_DISPLAY | AEECLSID_DISPLAY1 => Interface::Display,
            AEECLSID_FILEMGR => Interface::FileMgr,
            AEECLSID_HID => Interface::Hid,
            AEECLSID_SIGNAL_CB_FACTORY => Interface::SignalCbFactory,
            AEECLSID_GRAPHICS => Interface::Graphics,
            AEECLSID_SOUND => Interface::Sound,
            AEECLSID_SOUNDPLAYER => Interface::SoundPlayer,
            AEECLSID_HEAP => Interface::Heap,
            AEECLSID_UNZIPSTREAM => Interface::UnzipStream,
            AEECLSID_LICENSE => Interface::License,
            AEECLSID_MEMASTREAM => Interface::MemAStream,
            // On the console there is one decoder per format, and they are all `IImage` fed by
            // `IAStream`. Ours looks at the byte signature, so all four land in the same place —
            // and with JPEG missing from the list, Zuma's Revenge got a refusal and went on with a
            // null pointer until it crashed.
            AEECLSID_PNG | AEECLSID_BMP | AEECLSID_JPEG | AEECLSID_GIF => Interface::Image,
            AEECLSID_PNGDECODER | AEECLSID_PNGDECODER_BREW | AEECLSID_JPEGDECODER_BREW => {
                Interface::ImageDecoder
            }
            AEECLSID_THREAD => Interface::Thread,
            AEECLSID_QEGL => Interface::Egl,
            AEECLSID_GLES11EXT => Interface::Gles11Ext,
            // The other five graphics extensions, for whoever asks for them through
            // `CreateInstance` instead of `QueryInterface` — Prey Evil uses the second path, but
            // the factory is BREW's documented door.
            AEEIID_GLES10EXT => Interface::Gles10Ext,
            AEEIID_GLES11EXTPAK => Interface::Gles11ExtPak,
            AEEIID_EGLGETCOLORBUFFER => Interface::EglGetColorBuffer,
            AEEIID_EGLGETPOWERLEVEL => Interface::EglGetPowerLevel,
            AEEIID_EGLOESSWAPINTERVAL => Interface::EglOesSwapInterval,
            AEECLSID_IJOYSTICK1 | AEECLSID_IJOYSTICK2 => Interface::Joystick,
            AEECLSID_EGL => Interface::EglLegacy,
            AEECLSID_GL => Interface::GlLegacy,
            AEECLSID_MEDIAUTIL => Interface::MediaUtil,
            AEECLSID_WEB => Interface::Web,
            AEECLSID_COLLECTION => Interface::Collection,
            AEECLSID_SQLMGR => Interface::SqlMgr,
            AEECLSID_SOURCEUTIL => Interface::SourceUtil,
            _ if WIDGET_FAMILY.contains(&clsid) => Interface::Widget,
            AEECLSID_CONTROL => Interface::Control,
            AEECLSID_ZEEBOMCP => Interface::ZeeboMcp,
            AEECLSID_CONFIG => Interface::Config,
            AEECLSID_VECTOR => Interface::Vector,
            AEECLSID_28E3C => Interface::Class28e3c,
            AEECLSID_CM => Interface::Cm,
            AEECLSID_SYSTEMCTL => Interface::SystemCtl,
            // **`0x01006c01` was left out**, and the Z-Wheel itself states the cost: it logs
            // `ERROR: Unable to create instance of AEECLSID_LCT_SIMCARDCTL, cannot do SIM check`
            // (`tectoymain.c:1668`). The interface exists and has always been served
            // ([`Interface::SimCardCtl`]); what was missing was for the factory to know the class,
            // as it already knew its neighbor `0x01006c02`. A `CreateInstance` that returns null
            // becomes a null pointer inside the applet, and that is how "a class is missing"
            // reappears further on as "invalid access to 0x0", three layers later.
            // **`AEECLSID_SIMCARDCTL` stays OUT, and it was measured twice.** `268d6fb` put it
            // here to silence `tectoymain.c:1668 ERROR: Unable to create instance of
            // AEECLSID_LCT_SIMCARDCTL, cannot do SIM check`, and the long comment on
            // [`Interface::SimCardCtl`](crate::brew::aee::Interface::SimCardCtl) already said that
            // this log **is the game taking the right path**: when refused, the Z-Wheel sets the
            // state to `0x27` and `0x82464` calls `0x1f7b4`, which advances the interface; when
            // offered, slot 3 (the check) answers zero, the state becomes `0x28`, and `0x82464`
            // **does nothing with it** — the screen stays where it is, silent. Measured in the
            // windowless harness on 2026-09-22: with the class offered, the wheel stops after 7 s
            // of virtual clock, with three widgets in the tree (the root and two empty objects),
            // never creates the stage (`0x01028e05`) or the roller (`0x01028e14`), and seven keys
            // from the script do not change a single pixel. It is the `0x28` the comment
            // describes.
            // AEECLSID_SIMCARDCTL => Interface::SimCardCtl,
            // **`IFont`, not `ITypeface`.** `AEECLSID_ROLLER_FONT` (0x0102f67c) is
            // `FONT_STANDARD18B`, a system font — it was mapped to `ITypeface`, which is another
            // interface, with other methods.
            _ if crate::machine::font::FONT_CLASSES.contains(&clsid) => {
                let result = self.create_font(clsid, out)?;
                if result != SUCCESS {
                    self.unknown_classes.insert(clsid);
                }
                return Ok(result);
            }
            AEECLSID_TYPEFACE => Interface::Typeface,
            AEECLSID_MD5 => Interface::Hash,
            AEECLSID_MD5CTX => Interface::HashCtx,
            AEECLSID_CIPHER_FACTORY => Interface::CipherFactory,
            AEECLSID_MEDIA | AEECLSID_MEDIAMIDI | AEECLSID_MEDIAMP3 | AEECLSID_MEDIAMIDIOUTMSG
            | AEECLSID_MEDIAADPCM
            | AEECLSID_MEDIAPCM => Interface::Media,
            _ if AEECLSID_MEDIA_FAMILY.contains(&clsid) => Interface::Media,
            // The extension module comes before the probe and before the refusal, because it is
            // the **right** answer: the class exists, implemented in ARM by the game's own
            // package, and that is how the console serves it.
            _ if self.has_extension_for(clsid) => {
                let ext_object = self.create_via_extension(clsid)?;
                if ext_object == 0 {
                    self.unknown_classes.insert(clsid);
                    if out != 0 {
                        self.cpu.write_u32(out, 0)?;
                    }
                    return Ok(ECLASSNOTSUPPORT);
                }
                if out != 0 {
                    self.cpu.write_u32(out, ext_object)?;
                }
                return Ok(SUCCESS);
            }
            // **`AEECLSID_DOWNLOAD`.** See [`AEECLSID_DOWNLOAD`]: the `IDownload` class, which
            // the SDK lists as `AEECLSID_PRIV` (= `QVERSION` = `0x01000000`). Served by the probe
            // for now — what the Z-Wheel calls on it shows up in the report with
            // `ZEEBX_ROM_PROBE`, and that measurement is what says which slots are worth
            // implementing.
            AEECLSID_DOWNLOAD => {
                let object = self.new_object(Interface::Probe)?;
                if object == 0 {
                    return Ok(ENOMEMORY);
                }
                self.assumptions.insert(
                    "the download class is served by an object that answers success to everything",
                );
                self.probe_objects.insert(object, clsid);
                if out != 0 {
                    self.cpu.write_u32(out, object)?;
                }
                return Ok(SUCCESS);
            }
            // A firmware class the game uses without checking: see [`CLASSES_UNDER_OBSERVATION`].
            _ if CLASSES_UNDER_OBSERVATION.contains(&clsid) => {
                let object = self.new_object(Interface::Probe)?;
                if object == 0 {
                    return Ok(ENOMEMORY);
                }
                self.assumptions.insert(
                    "a firmware class is served by an object that answers success to everything",
                );
                self.probe_objects.insert(object, clsid);
                if out != 0 {
                    self.cpu.write_u32(out, object)?;
                }
                return Ok(SUCCESS);
            }
            // The probe comes before the refusal: the game gets an object that does nothing and
            // moves on, and whatever it calls on it goes to the report. That is how one finds out
            // which interface the class is, without a header and without guessing.
            _ if self.probe_classes.contains(&clsid) => {
                let object = self.new_object(Interface::Probe)?;
                if object == 0 {
                    return Ok(ENOMEMORY);
                }
                self.probe_objects.insert(object, clsid);
                if out != 0 {
                    self.cpu.write_u32(out, object)?;
                }
                return Ok(SUCCESS);
            }
            _ => {
                self.unknown_classes.insert(clsid);
                if out != 0 {
                    self.cpu.write_u32(out, 0)?;
                }
                return Ok(ECLASSNOTSUPPORT);
            }
        };

        let Some(obj) = self.objects.create(iface) else {
            return Ok(ENOMEMORY);
        };
        self.cpu.write_u32(obj, loader::vtable_addr(iface))?;
        self.transparency.remove(&obj);
        // A collection starts empty with the cursor at the beginning. Without this record it
        // would not exist for the methods, and an `AtEnd` on an unknown collection would answer
        // "finished" by accident — the right answer for the wrong reason.
        if iface == Interface::Collection {
            self.collections.insert(obj, (Vec::new(), 0));
        }
        // For the same reason as the collection: an unrecorded widget would answer "I do not
        // have that child" because it does not exist, not because it lacks the child.
        // Same reason as the collection: without the record, a `Tamanho` on an unknown list
        // would answer zero because the list does not exist, not because it is empty.
        if iface == Interface::Vector {
            self.vectors.insert(obj, (Vec::new(), 0));
        }
        // An `ISoundPlayer` keeps its `GetVolume` data and its source in its own slot (see
        // `sound_player_call`). The address may be a recycled one: it starts with no source, not
        // with the previous object's bytes.
        if iface == Interface::SoundPlayer {
            self.cpu.write_mem(obj + 4, &[0u8; SOUND_PLAYER_SOURCE as usize + 12 - 4])?;
        }
        if iface == Interface::Widget {
            // A widget starts out visible: the game only calls slot 6 to **hide** it.
            self.next_serial += 1;
            self.widgets.insert(
                obj,
                Widget {
                    visible: true,
                    class_id: clsid,
                    serial: self.next_serial,
                    ..Widget::default()
                },
            );
        }
        if out != 0 {
            self.cpu.write_u32(out, obj)?;
        }
        Ok(SUCCESS)
    }

    /// Creates the applet instance by calling `IModule::CreateInstance` on the loaded module.
    ///
    /// Signature, from `AEEModGen.c`:
    /// `int AEEMod_CreateInstance(IModule *po, IShell *pIShell, AEECLSID ClsId, void **ppObj)`.
    /// `CreateInstance` is slot 2 of the `IModule` vtable (after `AddRef` and `Release`).
    /// Whether any extension module in the package declares that it provides this class.
    fn has_extension_for(&self, clsid: u32) -> bool {
        self.module
            .extensions
            .iter()
            .any(|ext| ext.classes.contains(&clsid))
    }

    /// Creates an object by asking the extension module that provides it.
    ///
    /// There are two steps, the same ones the console takes: `AEEMod_Load` the first time, so
    /// the extension hands over its `IModule*`, and then `IModule::CreateInstance` with the
    /// requested ClassID. The object that comes back is implemented in ARM by the extension —
    /// from then on the game talks to it directly, and we do not need to know which interface it
    /// is.
    ///
    /// Returns zero when it did not work, and the caller turns that into `ECLASSNOTSUPPORT`.
    fn create_via_extension(&mut self, clsid: u32) -> Result<u32, CpuError> {
        let budget = self.budget.max(1);
        let Some(i) = self
            .module
            .extensions
            .iter()
            .position(|ext| ext.classes.contains(&clsid))
        else {
            return Ok(0);
        };
        let (entry, out_module) = (
            self.module.extensions[i].entry,
            self.module.extensions[i].out_module,
        );

        // First time: load the module. `Some(0)` marks the attempt that failed, so that a game
        // that asks for the class every frame does not pay for the load every time nor flood it
        // with logs.
        if self.ext_modules[i].is_none() {
            self.cpu.write_u32(out_module, 0)?;
            let outcome = self.call_guest_nested(
                entry,
                [self.module.shell, self.module.helpers, out_module, 0],
                budget,
            )?;
            let ptr = if matches!(outcome, Outcome::Returned { code: 0 }) {
                self.cpu.read_u32(out_module).unwrap_or(0)
            } else {
                0
            };
            if ptr == 0 {
                self.assumptions
                    .insert("an extension module in the package did not load");
            }
            self.ext_modules[i] = Some(ptr);
        }
        let modulo = self.ext_modules[i].unwrap_or(0);
        if modulo == 0 {
            return Ok(0);
        }

        // `IModule::CreateInstance(po, shell, clsid, &output)` — slot 2, the same as the applet.
        let vtable = self.cpu.read_u32(modulo)?;
        let create_instance = self.cpu.read_u32(vtable + 2 * 4)?;
        let output = out_module + 4;
        self.cpu.write_u32(output, 0)?;
        let outcome = self.call_guest_nested(
            create_instance,
            [modulo, self.module.shell, clsid, output],
            budget,
        )?;
        if !matches!(outcome, Outcome::Returned { code: 0 }) {
            return Ok(0);
        }
        self.cpu.read_u32(output)
    }

    pub fn create_applet(&mut self, clsid: u32, budget: u64) -> Result<AppletResult, CpuError> {
        self.applet_class = clsid;
        let module_ptr = self.cpu.read_u32(self.module.out_module)?;
        if module_ptr == 0 {
            return Ok(AppletResult::NoModule);
        }
        let vtable = self.cpu.read_u32(module_ptr)?;
        let create_instance = self.cpu.read_u32(vtable + 2 * 4)?;

        // The output pointer sits right after the `IModule*`, in the object area.
        let out_applet = self.module.out_module + 4;
        self.cpu.write_u32(out_applet, 0)?;

        let outcome = self.call_guest(
            create_instance,
            [module_ptr, self.module.shell, clsid, out_applet],
            budget,
        )?;
        match outcome {
            Outcome::Returned { code } => Ok(AppletResult::Called {
                code,
                applet: self.cpu.read_u32(out_applet)?,
            }),
            other => Ok(AppletResult::Stopped(other)),
        }
    }
}

#[cfg(test)]
mod item_id_tests {
    use super::*;

    /// **The item ID applies to any installed applet**, not just the loaded one.
    ///
    /// The number is the module's folder number, which is the `.mif` name. Returning zero for an
    /// applet that exists is the wrong answer: the documentation says zero only for a class that
    /// is **not** from a downloaded module.
    #[test]
    fn the_item_id_comes_out_for_an_installed_applet() {
        // The smallest module the loader accepts: the target here is the applet table, not the
        // code.
        let code = [
            0xe3a0_0010u32.to_le_bytes(), // mov r0, #16
            0xe12f_ff1eu32.to_le_bytes(), // bx lr
        ]
        .concat();
        let image = crate::loader::modfile::ModImage::parse(code).unwrap();
        let module = crate::loader::load(&image).unwrap();
        let mut machine = Machine::new(crate::cpu::DefaultBackend::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        machine.set_installed_applets([
            (0x0102_8e35u32, "274755".to_string()),
            (0x0102_8e36, "279888".to_string()),
        ]);
        assert_eq!(machine.item_id_of(0x0102_8e35), 274755);
        assert_eq!(machine.item_id_of(0x0102_8e36), 279888);
        // A class that is not from an installed module still returns zero, as the API says.
        assert_eq!(machine.item_id_of(0x0100_0001), 0);
    }
}
