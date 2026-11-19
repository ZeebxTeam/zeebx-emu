//! IEGL: contexts, surfaces and the buffer swap that closes the frame.

use super::*;

/// The key of the pbuffer color buffer's watch, in [`CpuBackend::watch_dirty`].
///
/// The other keys are bitmap addresses, which are never zero: the color buffer belongs to
/// none of them and takes the value that is left.
const COLOR_BUFFER_WATCH: u32 = 0;

impl<C: CpuBackend> Machine<C> {
    /// Writes `EGL_TRUE` to the `AEEEGLBoolean *ret` of argument `slot` and returns `SUCCESS`.
    /// The address of the EGL color buffer in guest memory, ready for the game to read.
    ///
    /// **Two paths ask for the same thing and now share the computation**: the
    /// `eglGetColorBufferQUALCOMM` function, which the engine used to serve on its own, and the
    /// `IEGLGetColorBuffer` interface, which is how Prey Evil asks. Zero is still failure.
    pub(super) fn egl_screen_color(&mut self) -> Result<u32, CpuError> {
            self.sync_egl_color_from_guest()?;
            let (width, height) = match self.egl_surfaces.get(&self.egl_surface) {
                Some(&(l, a)) => (l as usize, a as usize),
                None => {
                    let target = self.screen();
                    (target.width() as usize, target.height() as usize)
                }
            };
            // The byte vector is the same from one call to the next: it is four hundred
            // kilobytes per read, two reads per frame, and allocating that sixty times a
            // second buys nothing.
            //
            // Keeping the converted frame to serve the second read does **not** work:
            // measured, zero out of one thousand two hundred and ninety-eight reads could be
            // reused, because the game draws between one and the other.
            let mut bytes = std::mem::take(&mut self.egl_color_bytes);
            self.gl.frame_rgb565(width, height, &mut bytes);
            if self.egl_color_buffer.1 < bytes.len() {
                // The buffer that became too small goes back to the region before asking for
                // another.
                if self.egl_color_buffer.0 != 0 {
                    self.release_surface(self.egl_color_buffer.0);
                    self.egl_color_buffer = (0, 0);
                }
                match self.reserve_surface(bytes.len() as u32) {
                    Some((location, _)) => {
                        self.egl_color_buffer = (location, bytes.len());
                        // The range moved: the watchpoint follows it.
                        self.cpu
                            .watch_dirty(COLOR_BUFFER_WATCH, location, bytes.len() as u32)?;
                    }
                    None => {
                        self.egl_color_bytes = bytes;
                        return Ok(0);
                    }
                }
            }
            self.cpu.write_mem(self.egl_color_buffer.0, &bytes)?;
            self.egl_color_bytes = bytes;
            self.egl_color_dimensions = Some((width, height));
            Ok(self.egl_color_buffer.0)
    }

    pub(super) fn write_egl_true(&mut self, slot: usize) -> Result<u32, CpuError> {
        let out = self.arg(slot);
        if out != 0 {
            self.cpu.write_u32(out, gles::EGL_TRUE)?;
        }
        Ok(SUCCESS)
    }

    /// EGL, in the two forms in which BREW exposes it.
    ///
    /// The new one (`IEGL11`, from `sdk/inc/AEEEGL10.h` and `AEEEGL11.h`) receives `this` in the
    /// first argument and delivers the result through an output pointer — always the last
    /// argument — reserving the return value for the error code. The old one (`IEGL`, from
    /// `sdk/inc/AEEGL.h`) does not receive `this` and returns the result directly.
    ///
    /// Apart from that the two are the same API, in the same order: dropping the `egl` prefix from
    /// the old names, the methods match. That is why a single translator serves both, with an
    /// offset in the arguments and a decision at the end about where to put the answer.
    pub(super) fn egl_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(full) = iface.method(slot) else {
            return Ok(None);
        };
        let legacy = iface == Interface::EglLegacy;
        let name = full.strip_prefix("egl").unwrap_or(full);
        if self.serial.is_some() {
            let args: Vec<String> = (0..6).map(|i| format!("{:#x}", self.arg(i))).collect();
            let msg = format!("<egl {full} {}>", args.join(" "));
            self.log_serial(msg);
        }
        // The arguments, already without `this` when there is one. Reading a few more than the
        // method uses is harmless: `arg` answers zero for whatever it cannot read.
        let base = usize::from(!legacy);
        let a: [u32; 8] = std::array::from_fn(|i| self.arg(base + i));
        let this = self.arg(0);
        // The screen is the "device bitmap" surface when the game asked for one; `self.screen`
        // becomes a 1×1 placeholder in that case, and answering with its dimensions made Crash
        // set up a one-pixel viewport and draw the whole game inside it.
        let (width, height) = {
            let target = self.screen();
            (target.width(), target.height())
        };
        let (out, value) = match name {
            "AddRef" => return Ok(Some(self.objects.add_ref(this))),
            "Release" => return Ok(Some(self.objects.release(this))),
            "QueryInterface" => return Ok(Some(self.egl_query_interface(iface)?)),
            // Reading the error also clears it, as the EGL spec says.
            "GetError" => (0, std::mem::replace(&mut self.egl_error, gles::EGL_SUCCESS)),
            // There is only one display, and it is the console's screen.
            "GetDisplay" => (1, EGL_DISPLAY),
            "Initialize" => {
                self.write_at(a[1], 1)?;
                self.write_at(a[2], 0)?;
                (3, gles::EGL_TRUE)
            }
            "Terminate" => (1, gles::EGL_TRUE),
            "QueryString" => {
                let text = match a[1] {
                    gles::EGL_VENDOR => "Zeebx",
                    gles::EGL_VERSION => "1.1",
                    // The console's surface scale. Games look for the name by substring, with the
                    // trailing space as the delimiter — that is how the string appears in their
                    // binary.
                    // Games look for the name by substring, with the trailing space as the
                    // delimiter — that is how the string appears in their binary.
                    // `get_color_buffer` was added because the Z-Wheel looks for it before asking
                    // for the function pointer: the literal is at `0x43a5c` in `tectoy.mod`.
                    gles::EGL_EXTENSIONS => {
                        "EGL_QUALCOMM_surface_scale EGL_QUALCOMM_get_color_buffer "
                    }
                    _ => {
                        self.egl_error = gles::EGL_BAD_ATTRIBUTE;
                        return Ok(Some(if legacy { 0 } else { SUCCESS }));
                    }
                };
                let pointer = self.intern(text)?;
                if self.serial.is_some() {
                    let loaded = self.cpu.read_cstring(pointer, 200);
                    self.log_serial(format!("<egl string {pointer:#x} = {loaded:?}>"));
                }
                (2, pointer)
            }
            // void (*eglGetProcAddress(const char *procname))()
            //
            // The trampoline range **is** a function pointer: the address encodes interface and
            // slot, and that is how every API call gets here. So it is enough to find the method
            // by name, dropping the leading `gl` that OpenGL uses and the vtable does not.
            "GetProcAddress" => {
                let name = self.cpu.read_cstring(a[0], MAX_STRING);
                // The search covers the whole table, not just the real vtable's slots: the
                // extension functions sit at its end and this is the only way the game reaches
                // them. **A `gl*` name also searches the old table, `IGL`**, for the same reason as
                // `egl*` just below: what is returned is a C function, without `this`. Handing out
                // the `IGLES11` slot made the function read its arguments shifted by one — the QX
                // engine's `glGenBuffers(1, &name)` took the pointer for the count and wrote
                // outside memory. The functions `AEEGL.h` does not have sit at the end of the
                // table.
                let search = |method_name: &str| {
                    (0..crate::brew::aee_slots::GL_LEGACY.len() as u32)
                        .find(|&s| Interface::GlLegacy.method(s) == Some(method_name))
                };
                // **The extension name falls through to the core function.** `vertex_buffer_object`
                // is core in OpenGL ES 1.1, and we announce the `ARB` one because that is the name
                // games look for in the list; whoever finds it asks for the functions with the
                // suffix. The SDK's QX engine (Dragon Vs Chicken) asks for `glBindBufferARB` and
                // the other five, and with a null pointer the characters, which it draws from
                // buffers, did not show up.
                let slot = name.starts_with("gl").then(|| {
                    search(&name).or_else(|| {
                        ["ARB", "OES"]
                            .iter()
                            .find_map(|suffix| name.strip_suffix(suffix))
                            .and_then(search)
                    })
                }).flatten();
                // An `egl*` name searches the **old** table, `AEEGL.h`'s `IEGL`, and not the
                // `IEGL11` the game uses through the vtable. It is not a matter of taste: what
                // `eglGetProcAddress` returns is a C function, without `this` in the first
                // argument, and that is exactly the old table's convention. Searching `IEGL11`,
                // besides eating the first argument, never matched any name — there they have no
                // `egl` prefix, and here the search uses it.
                let egl = (name.starts_with("egl") && slot.is_none())
                    .then(|| {
                        (0..crate::brew::aee_slots::EGL_LEGACY.len() as u32)
                            .find(|&s| Interface::EglLegacy.method(s) == Some(name.as_str()))
                    })
                    .flatten();
                match (slot, egl) {
                    (Some(slot), _) => (1, aee::encode(Interface::GlLegacy, slot)),
                    (_, Some(slot)) => (1, aee::encode(Interface::EglLegacy, slot)),
                    _ => {
                        self.bad_pointers
                            .insert(format!("the game asked for the address of {name}, which we do not have"));
                        (1, 0)
                    }
                }
            }
            // `GetConfigs` lists the configurations and `ChooseConfig` filters by attributes. Since
            // there is only one, the screen's native one, both answer the same.
            "GetConfigs" | "ChooseConfig" => {
                let first = usize::from(name == "ChooseConfig");
                let (configs, size, num) = (a[1 + first], a[2 + first], a[3 + first]);
                let fits = size >= 1;
                if configs != 0 && fits {
                    self.cpu.write_u32(configs, EGL_CONFIG)?;
                }
                self.write_at(num, u32::from(configs == 0 || fits))?;
                (4 + first, gles::EGL_TRUE)
            }
            "GetConfigAttrib" => match gles::config_attrib(a[2], width, height) {
                Some(answer) => {
                    self.write_at(a[3], answer)?;
                    (4, gles::EGL_TRUE)
                }
                None => {
                    self.egl_error = gles::EGL_BAD_ATTRIBUTE;
                    (4, gles::EGL_FALSE)
                }
            },
            "CreateWindowSurface" | "CreatePixmapSurface" | "CreatePbufferSurface" => {
                let pbuffer = name == "CreatePbufferSurface";
                let handle = self.new_egl_handle();
                // **The pbuffer has the size the attribute list says, not the screen's.** The
                // Z-Wheel draws the stage into a 640×330 one — the area of the wheel widget — and
                // that crop is what `eglGetColorBufferQUALCOMM` returns. While the size here was
                // the screen's, the drawing came out shifted: the cylinder went past the edge and
                // the bottom panel was cut off.
                //
                // The window and the pixmap carry no size in the list; for them the screen still
                // applies.
                let attributes = a[if pbuffer { 2 } else { 3 }];
                let measured = match pbuffer {
                    true => self.size_from_attribs(attributes, (width, height))?,
                    false => (width, height),
                };
                self.egl_surfaces.insert(handle, measured);
                (if pbuffer { 3 } else { 4 }, handle)
            }
            "DestroySurface" => {
                self.egl_surfaces.remove(&a[1]);
                (2, gles::EGL_TRUE)
            }
            "QuerySurface" => {
                let (w, h) = self
                    .egl_surfaces
                    .get(&a[1])
                    .copied()
                    .unwrap_or((width, height));
                let answer = match a[2] {
                    gles::EGL_WIDTH => Some(w),
                    gles::EGL_HEIGHT => Some(h),
                    attribute => gles::config_attrib(attribute, width, height),
                };
                match answer {
                    Some(answer) => {
                        self.write_at(a[3], answer)?;
                        (4, gles::EGL_TRUE)
                    }
                    None => {
                        self.egl_error = gles::EGL_BAD_ATTRIBUTE;
                        (4, gles::EGL_FALSE)
                    }
                }
            }
            "CreateContext" => {
                let handle = self.new_egl_handle();
                self.egl_context = handle;
                (4, handle)
            }
            "DestroyContext" => (2, gles::EGL_TRUE),
            "MakeCurrent" => {
                self.egl_surface = a[1];
                self.egl_context = a[3];
                // **The GL surface is the activated pbuffer's.** Without this the rasterizer stayed
                // at 640×480 while the Z-Wheel draws into a 640×330 pbuffer, and both ends of
                // `eglGetColorBufferQUALCOMM` went through scaling: the export reads row
                // `y·480/330` and the import writes row `y·330/480`, which do not come back to the
                // same place. The background the game copies into the buffer every frame never
                // reached a third of the rows, and what the character's sword painted there stayed
                // forever — dotted streaks trailing from it. Qualcomm's scale, when the game asks
                // for it, still decides the size.
                //
                // The viewport goes along on the first activation, which is what OpenGL defines: it
                // is born with the surface's size. The Z-Wheel never calls `glViewport` — its
                // frustum already has the 640×330 aspect — and with a 640×480 viewport the stage
                // came out stretched. It only applies to a surface of a size other than the
                // screen's: in a window the default viewport is already that, and games like Quake
                // infer the surface from the viewport they ask for later.
                let measured = self.egl_surfaces.get(&a[1]).copied();
                if let (None, Some((surf_width, surf_height))) = (self.scale_source, measured) {
                    let screen_sized = (surf_width, surf_height) == (SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32);
                    if !screen_sized && !self.egl_initial_viewport {
                        self.egl_initial_viewport = true;
                        self.gl.set_surface(surf_width as usize, surf_height as usize);
                        self.gl.set_viewport(0, 0, surf_width as i32, surf_height as i32);
                    }
                }
                (4, gles::EGL_TRUE)
            }
            "GetCurrentContext" => (0, self.egl_context),
            "GetCurrentSurface" => (1, self.egl_surface),
            "GetCurrentDisplay" => (0, EGL_DISPLAY),
            "QueryContext" => {
                self.write_at(a[3], 0)?;
                (4, gles::EGL_TRUE)
            }
            "WaitGL" => (0, gles::EGL_TRUE),
            "WaitNative" => (1, gles::EGL_TRUE),
            // Presenting the frame: the back buffer becomes the front one.
            "SwapBuffers" => {
                self.sync_egl_color_from_guest()?;
                self.egl_swaps += 1;
                self.present_gl();
                self.wait_for_vsync();
                (2, gles::EGL_TRUE)
            }
            // The four from `EGL_QUALCOMM_surface_scale`, in C function form: the arguments
            // arrive without `this` and the result is the return value, not an output pointer. The
            // behavior is the same one already implemented in [`Interface::EglSurfaceManip`] — see
            // `extension_call` — and it is here because the Z-Wheel reaches it through a function
            // pointer.
            //
            // **Announcing the extension and not delivering the functions is worse than not
            // announcing it**: `eglQueryString` already said `EGL_QUALCOMM_surface_scale`, and the
            // game, finding the pointers null, discarded the group.
            // `EGLBoolean eglSwapIntervalOES(EGLDisplay dpy, EGLint interval)`. The frame pace
            // here is the virtual clock's, in `wait_for_vsync`; accepting and not storing it is what
            // lets the game go on without promising an interval we do not control.
            "SwapIntervalOES" => (2, gles::EGL_TRUE),
            "SurfaceScaleEnableQUALCOMM" => (3, gles::EGL_TRUE),
            // `EGLBoolean eglSetSurfaceScaleQUALCOMM(dpy, surf, const rect *src, const rect *dst)`
            "SetSurfaceScaleQUALCOMM" => {
                let origin = a[2];
                if origin != 0 {
                    let surf_width = self.cpu.read_u32(origin + 8)? as i32;
                    let surf_height = self.cpu.read_u32(origin + 12)? as i32;
                    if surf_width > 0 && surf_height > 0 {
                        self.scale_source = Some((surf_width, surf_height));
                        self.gl.set_stretched_surface(surf_width as usize, surf_height as usize);
                    }
                }
                (4, gles::EGL_TRUE)
            }
            // `EGLBoolean eglGetSurfaceScaleQUALCOMM(dpy, surf, EGLBoolean *on, rect *src, *dst)`
            "GetSurfaceScaleQUALCOMM" => {
                let (enabled, origin, dest) = (a[2], a[3], a[4]);
                self.write_at(enabled, u32::from(self.scale_source.is_some()))?;
                let (surf_width, surf_height) = match self.scale_source {
                    Some(rect_size) => rect_size,
                    None => {
                        let (w, h) = self.gl.surface();
                        (w as i32, h as i32)
                    }
                };
                for (rect, rect_size) in [
                    (origin, (surf_width, surf_height)),
                    (dest, (SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32)),
                ] {
                    if rect != 0 {
                        self.cpu.write_u32(rect, 0)?;
                        self.cpu.write_u32(rect + 4, 0)?;
                        self.cpu.write_u32(rect + 8, rect_size.0 as u32)?;
                        self.cpu.write_u32(rect + 12, rect_size.1 as u32)?;
                    }
                }
                (5, gles::EGL_TRUE)
            }
            // `EGLBoolean eglGetSurfaceScaleCapsQUALCOMM(dpy, surf, AEEEGLSurfaceScaleCaps *)`
            //
            // Same values as the interface: scale up from the game's surface to the screen, with
            // the factors in 16.16 fixed point.
            "GetSurfaceScaleCapsQUALCOMM" => {
                let caps = a[2];
                if caps != 0 {
                    let fields: [u32; 12] = [
                        1 << 16,
                        8 << 16,
                        1 << 16,
                        8 << 16,
                        1,
                        SCREEN_WIDTH as u32,
                        1,
                        SCREEN_HEIGHT as u32,
                        1,
                        SCREEN_WIDTH as u32,
                        1,
                        SCREEN_HEIGHT as u32,
                    ];
                    for (index, cap_value) in fields.iter().enumerate() {
                        self.cpu.write_u32(caps + index as u32 * 4, *cap_value)?;
                    }
                }
                (3, gles::EGL_TRUE)
            }
            // `void *eglGetColorBufferQUALCOMM(void)` — **with no argument at all**.
            //
            // The call is a plain `blx r0`, at `0x76cbc`, with the pointer read from `[r4+0x34]`:
            // the registers that arrive here are leftovers, and the only one that matters is the
            // one that goes out. Zero is failure — the `cmp r0,#0` right after branches to
            // `0x76e30` and the stage gives up.
            //
            // What goes out is the raw address of the pixels. The game already knows the
            // dimensions: it keeps width and height in `[r4+0x18]` and `[r4+0x1c]` and builds the
            // drawing's descriptor with them. That is why there is no size output here, and why the
            // format must be the screen's — RGB565, the same one `present_gl` delivers.
            //
            // This function exists because the Z-Wheel draws the stage into a **pbuffer** and not
            // into a window: without a pointer to the result, there is no way to composite the 3D
            // with the 2D.
            "GetColorBufferQUALCOMM" => (0, self.egl_screen_color()?),
            "CopyBuffers" => (3, gles::EGL_TRUE),
            "SurfaceAttrib" => (4, gles::EGL_TRUE),
            "BindTexImage" | "ReleaseTexImage" => (3, gles::EGL_TRUE),
            "SwapInterval" => (2, gles::EGL_TRUE),
            _ => return Ok(None),
        };
        if legacy {
            return Ok(Some(value));
        }
        self.write_at(a[out], value)?;
        Ok(Some(SUCCESS))
    }

    /// Reads width and height from an EGL attribute list, with the default for whatever is
    /// missing.
    ///
    /// The list is a vector of `(attrib, value)` pairs terminated by `EGL_NONE`. The pair cap
    /// exists because the list comes from the game: one without a terminator must not become an
    /// endless loop.
    pub(super) fn size_from_attribs(
        &mut self,
        list: u32,
        fallback: (u32, u32),
    ) -> Result<(u32, u32), CpuError> {
        /// How many pairs to read before giving up.
        const CAP: u32 = 64;

        let (mut width, mut height) = fallback;
        if list == 0 {
            return Ok(fallback);
        }
        for pair in 0..CAP {
            let attrib = self.cpu.read_u32(list + pair * 8)?;
            if attrib == gles::EGL_NONE {
                break;
            }
            let value = self.cpu.read_u32(list + pair * 8 + 4)?;
            match attrib {
                gles::EGL_WIDTH => width = value,
                gles::EGL_HEIGHT => height = value,
                _ => {}
            }
        }
        Ok((width, height))
    }

    /// Writes a word to an output pointer, ignoring null.
    pub(super) fn write_at(&mut self, pointer: u32, value: u32) -> Result<(), CpuError> {
        if pointer != 0 {
            self.cpu.write_u32(pointer, value)?;
        }
        Ok(())
    }

    /// `QueryInterface` of the EGL object.
    ///
    /// `AEECLSID_QEGL` is a single object that answers for several interfaces: EGL proper
    /// and OpenGL ES. Returning `this` for everything, as we used to, handed the EGL vtable
    /// to whoever asked for the GL one — and the first call landed in a slot that does not exist.
    pub(super) fn egl_query_interface(&mut self, iface: Interface) -> Result<u32, CpuError> {
        let (iid, out) = (self.arg(1), self.arg(2));
        // A `QueryInterface` in the old form returns the old form of OpenGL: the two
        // conventions do not mix within the same object.
        let gl = match iface {
            Interface::EglLegacy => Interface::GlLegacy,
            _ => Interface::Gles,
        };
        let object = match iid {
            AEEIID_GLES10 | AEEIID_GLES11 => {
                if self.gles_object == 0 {
                    self.gles_object = self.new_object(gl)?;
                }
                self.gles_object
            }
            AEEIID_EGL10 | AEEIID_EGL11 => self.arg(0),
            // The console's extensions. V2 is a superset of V1 with the same vtable prefix, so
            // the same object serves both IIDs.
            AEEIID_EGL_SURFACE_MANIP | AEEIID_EGL_SURFACE_MANIP_V1 => {
                if self.surface_manip == 0 {
                    self.surface_manip = self.new_object(Interface::EglSurfaceManip)?;
                }
                self.surface_manip
            }
            AEEIID_GLES_IMAGEON_EXT | AEEIID_GLES_IMAGEON_EXT_V1 => {
                if self.imageon_ext == 0 {
                    self.imageon_ext = self.new_object(Interface::GlesImageonExt)?;
                }
                self.imageon_ext
            }
            // `IGLES11Ext`: the OpenGL ES 1.1 OES extensions. **This is how Prey Evil asks**, and
            // the request fell into the generic branch below — `ECLASSNOTSUPPORT` — from where it
            // went on building matrices and textures without ever drawing.
            AEECLSID_GLES11EXT => {
                if self.gles11_ext == 0 {
                    self.gles11_ext = self.new_object(Interface::Gles11Ext)?;
                }
                self.gles11_ext
            }
            // `IGLES10Ext`: `QueryMatrixxOES`. One extension at a time is what learned the lesson
            // from yesterday — each registration moves the game through one gate, and each gate is
            // measured.
            AEEIID_GLES10EXT => {
                if self.gles10_ext == 0 {
                    self.gles10_ext = self.new_object(Interface::Gles10Ext)?;
                }
                self.gles10_ext
            }
            AEEIID_EGLGETPOWERLEVEL => {
                if self.egl_get_power_level == 0 {
                    self.egl_get_power_level = self.new_object(Interface::EglGetPowerLevel)?;
                }
                self.egl_get_power_level
            }
            AEEIID_GLES11EXTPAK => {
                if self.gles11_ext_pak == 0 {
                    self.gles11_ext_pak = self.new_object(Interface::Gles11ExtPak)?;
                }
                self.gles11_ext_pak
            }
            AEEIID_EGLGETCOLORBUFFER => {
                if self.egl_get_color_buffer == 0 {
                    self.egl_get_color_buffer = self.new_object(Interface::EglGetColorBuffer)?;
                }
                self.egl_get_color_buffer
            }
            AEEIID_EGLOESSWAPINTERVAL => {
                if self.egl_oes_swap_interval == 0 {
                    self.egl_oes_swap_interval =
                        self.new_object(Interface::EglOesSwapInterval)?;
                }
                self.egl_oes_swap_interval
            }
            _ => {
                self.unknown_classes.insert(iid);
                if out != 0 {
                    self.cpu.write_u32(out, 0)?;
                }
                return Ok(ECLASSNOTSUPPORT);
            }
        };
        self.objects.add_ref(object);
        if out != 0 {
            self.cpu.write_u32(out, object)?;
        }
        Ok(SUCCESS)
    }

    /// Imports only pixels modified by the guest; the rest may have been
    /// updated by GL since the last exposure and must not be overwritten.
    pub(super) fn sync_egl_color_from_guest(&mut self) -> Result<(), CpuError> {
        let Some((width, height)) = self.egl_color_dimensions else {
            return Ok(());
        };
        // **It only reads when the game wrote.** This function is called on every `Draw*`, `Clear`
        // and `ReadPixels` — 93 thousand times in thirteen seconds of the Z-Wheel — and the
        // previous version read 400 KB from the guest and compared them byte by byte on each of
        // them, only to find out there was almost never a change. The write watchpoint answers
        // the same question for free.
        if !self.cpu.take_dirty(COLOR_BUFFER_WATCH) {
            return Ok(());
        }
        self.egl_color_readback
            .resize(self.egl_color_bytes.len(), 0);
        self.cpu
            .read_mem(self.egl_color_buffer.0, &mut self.egl_color_readback)?;
        if self.egl_color_readback != self.egl_color_bytes {
            self.gl.import_rgb565_changes(
                width,
                height,
                &self.egl_color_bytes,
                &self.egl_color_readback,
            );
            std::mem::swap(&mut self.egl_color_bytes, &mut self.egl_color_readback);
        }
        Ok(())
    }

    /// Next EGL surface or context identifier.
    pub(super) fn new_egl_handle(&mut self) -> u32 {
        let handle = self.egl_next_handle;
        self.egl_next_handle += 1;
        handle
    }
}
