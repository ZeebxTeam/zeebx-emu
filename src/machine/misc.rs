//! The single-call interfaces: configuration, SIM, power, license and collections.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// The console's graphics extensions: `IEGLSurfaceManip` and `IGLESImageonExt`.
    ///
    /// They exist because ten arcade ports — all built on the same Neo Geo emulator — ask for
    /// both through `QueryInterface` on the EGL object and give up on graphics initialization
    /// without them, writing "InitGLExtensions failed" on the screen.
    ///
    /// Almost everything here answers "done" without doing anything, and that is deliberate:
    /// rotation, transparency and layer overlays do not change what the game draws, only how
    /// the console composites the result. Refusing would make the game give up over a feature
    /// it never even uses.
    ///
    /// The exception is **scaling**: there the game states the size of the surface it draws
    /// on, and that information is worth more than the viewport-based guess we make without it.
    pub(super) fn extension_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            // `IQueryInterface`: one object answers for several interfaces, and whoever asks for
            // one of them gets a pointer to **that** vtable.
            //
            // **This is how Prey Evil asks for the OES extensions**, not through
            // `ISHELL_CreateInstance`: it asks the EGL object whether there is an `IGLES11Ext`,
            // and used to get `ECLASSNOTSUPPORT` — hence it built matrices and textures and never
            // drew. The right answer is an object with the requested vtable; without this,
            // registering the class in the factory changes nothing.
            "QueryInterface" if iface == Interface::Egl && self.arg(1) == AEECLSID_GLES11EXT => {
                let (out, object) = (self.arg(2), self.new_object(Interface::Gles11Ext)?);
                if out != 0 {
                    self.cpu.write_u32(out, object)?;
                }
                match object {
                    0 => ECLASSNOTSUPPORT,
                    _ => SUCCESS,
                }
            }
            "QueryInterface" => {
                let (iid, out) = (self.arg(1), self.arg(2));
                if out != 0 {
                    self.cpu.write_u32(out, 0)?;
                }
                self.unknown_classes.insert(iid);
                ECLASSNOTSUPPORT
            }
            // int SetSurfaceScale(pMe, dpy, surf, AEEEGLSurfaceScaleRect *src, *dst,
            //                     AEEEGLBoolean *ret)
            "SetSurfaceScale" => {
                let source = self.arg(3);
                if source != 0 {
                    let width = self.cpu.read_u32(source + 8)? as i32;
                    let height = self.cpu.read_u32(source + 12)? as i32;
                    if width > 0 && height > 0 {
                        self.scale_source = Some((width, height));
                        self.gl.set_stretched_surface(width as usize, height as usize);
                    }
                }
                self.write_egl_true(4)?
            }
            // int GetSurfaceScale(pMe, dpy, surf, EGLBoolean *enabled, *src, *dst, *ret)
            "GetSurfaceScale" => {
                let (enabled, source, dest) = (self.arg(3), self.arg(4), self.arg(5));
                self.write_at(enabled, u32::from(self.scale_source.is_some()))?;
                let (width, height) = match self.scale_source {
                    Some(size) => size,
                    None => {
                        let (w, h) = self.gl.surface();
                        (w as i32, h as i32)
                    }
                };
                for (rect, size) in [
                    (source, (width, height)),
                    (dest, (SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32)),
                ] {
                    if rect != 0 {
                        self.cpu.write_u32(rect, 0)?;
                        self.cpu.write_u32(rect + 4, 0)?;
                        self.cpu.write_u32(rect + 8, size.0 as u32)?;
                        self.cpu.write_u32(rect + 12, size.1 as u32)?;
                    }
                }
                self.write_egl_true(6)?
            }
            // int GetSurfaceScaleCaps(pMe, dpy, surf, AEEEGLSurfaceScaleCaps *param, *ret)
            //
            // The console scales up from the game's surface to the screen; we advertise exactly
            // that range. The factors are 16.16 fixed point, as `AEEEGLfixed` requires.
            "GetSurfaceScaleCaps" => {
                let caps = self.arg(3);
                if caps != 0 {
                    let fields: [u32; 12] = [
                        1 << 16, // MinXScaleFactor: never shrinks
                        8 << 16, // MaxXScaleFactor
                        1 << 16, // MinYScaleFactor
                        8 << 16, // MaxYScaleFactor
                        1,       // MinSrcWidth
                        SCREEN_WIDTH as u32,
                        1, // MinSrcHeight
                        SCREEN_HEIGHT as u32,
                        1, // MinDstWidth
                        SCREEN_WIDTH as u32,
                        1, // MinDstHeight
                        SCREEN_HEIGHT as u32,
                    ];
                    for (index, value) in fields.iter().enumerate() {
                        self.cpu.write_u32(caps + index as u32 * 4, *value)?;
                    }
                }
                self.write_egl_true(4)?
            }
            // The rest of the surface manipulation: accepting without doing is honest because
            // none of it changes what the game draws. The last argument is always the
            // `EGLBoolean *ret`.
            "SurfaceScaleEnable"
            | "SurfaceRotateEnable"
            | "SetSurfaceRotate"
            | "SurfaceTransparencyEnable"
            | "SetSurfaceTransparency"
            | "SetSurfaceTransparencyMap"
            | "SurfaceColorKeyEnable"
            | "SetSurfaceColorKey"
            | "SurfaceOverlayEnable"
            | "SurfaceOverlayLayerEnable"
            | "SurfaceOverlayBind" => {
                let last = EXTENSION_RESULT_SLOT
                    .iter()
                    .find(|(method, _)| *method == name)
                    .map(|(_, slot)| *slot)
                    .unwrap_or(4);
                self.write_egl_true(last)?
            }
            // The queries we have no way of really answering: they zero the output and say the
            // feature is not enabled, which is the truth.
            "GetSurfaceRotate"
            | "GetSurfaceRotateCaps"
            | "GetSurfaceTransparency"
            | "GetSurfaceTransparencyMap"
            | "GetSurfaceTransparencyCaps"
            | "GetSurfaceColorKey"
            | "GetSurfaceOverlayBinding"
            | "GetSurfaceOverlay"
            | "GetSurfaceOverlayCaps"
            | "CreateCompositeSurface" => {
                for index in 3..8 {
                    let out = self.arg(index);
                    if out != 0 {
                        self.cpu.write_u32(out, 0)?;
                    }
                }
                SUCCESS
            }
            // `IGLESImageonExt` repeats OpenGL ES methods with a different signature: here `this`
            // is the extension, so the arguments come one slot further along.
            "TexEnvi" | "TexEnviv" | "TexParameteri" | "TexParameteriv" | "TexParameterfv"
            | "TexParameterxv" => {
                let (pname, value) = (self.arg(2), self.arg(3));
                match name.ends_with('v') {
                    true if pname == gles::GL_TEXTURE_CROP_RECT_OES => {
                        let mut crop = [0i32; 4];
                        for (index, item) in crop.iter_mut().enumerate() {
                            *item = self.cpu.read_u32(value + index as u32 * 4)? as i32;
                        }
                        self.gl.set_texture_crop(crop);
                    }
                    true => {
                        let value = self.cpu.read_u32(value)?;
                        self.apply_texture_setting(name, pname, value);
                    }
                    false => self.apply_texture_setting(name, pname, value),
                }
                SUCCESS
            }
            "BlendEquationEXT"
            | "BlendEquationSeparateEXT"
            | "BlendFuncSeparateEXT"
            | "PointSizePointerOES" => SUCCESS,
            // **Qualcomm's vertex buffers are the usual buffer objects.** Ridge Racer fills a
            // hundred of them per match — a hundred `GenBuffersQUALCOMM`, three hundred
            // `BufferSubDataQUALCOMM` — and while the extension refused them all that geometry
            // went to waste, with the assumption "used a buffer we don't have" in the report.
            //
            // The extension passes its own object as the first argument, so everything comes
            // one slot further along; the rest is the same name, the same target and the same
            // contents as `glBindBuffer` and friends, and they are stored in the same place.
            "GenBuffersQUALCOMM" => {
                let (count, output) = (self.arg(1), self.arg(2));
                if output != 0 {
                    for i in 0..count {
                        let buffer_id = self.buffer_names.generate();
                        self.cpu.write_u32(output + i * 4, buffer_id)?;
                    }
                }
                SUCCESS
            }
            "BindBufferQUALCOMM" => {
                let (target, buffer_id) = (self.arg(1), self.arg(2));
                if buffer_id != 0 {
                    self.gl_buffers.entry(buffer_id).or_default();
                }
                self.buffer_names.mark_used(buffer_id);
                match target {
                    gles::GL_ARRAY_BUFFER => self.gl_array_buffer = buffer_id,
                    gles::GL_ELEMENT_ARRAY_BUFFER => self.gl_element_buffer = buffer_id,
                    _ => {}
                }
                SUCCESS
            }
            "BufferDataQUALCOMM" => {
                let (target, byte_len, data) = (self.arg(1), self.arg(2), self.arg(3));
                match self.bound_buffer(target) {
                    None => SUCCESS,
                    Some(buffer_id) => {
                        let content = match data {
                            0 => vec![0u8; guest_size(byte_len as usize)?],
                            _ => self.read_bytes(data, byte_len)?,
                        };
                        self.gl_buffers.insert(buffer_id, content);
                        SUCCESS
                    }
                }
            }
            "BufferSubDataQUALCOMM" => {
                let (target, start, byte_len, data) =
                    (self.arg(1), self.arg(2), self.arg(3), self.arg(4));
                match self.bound_buffer(target) {
                    None => SUCCESS,
                    Some(buffer_id) => {
                        let create = self.read_bytes(data, byte_len)?;
                        if let Some(buffer) = self.gl_buffers.get_mut(&buffer_id) {
                            let end = start as usize + create.len();
                            if end <= buffer.len() {
                                buffer[start as usize..end].copy_from_slice(&create);
                            }
                        }
                        SUCCESS
                    }
                }
            }
            "DeleteBuffersQUALCOMM" => {
                let (count, list) = (self.arg(1), self.arg(2));
                for i in 0..count {
                    let buffer_id = self.cpu.read_u32(list + i * 4)?;
                    self.gl_buffers.remove(&buffer_id);
                    if self.gl_array_buffer == buffer_id {
                        self.gl_array_buffer = 0;
                    }
                    if self.gl_element_buffer == buffer_id {
                        self.gl_element_buffer = 0;
                    }
                }
                SUCCESS
            }
            "IsBufferQUALCOMM" => u32::from(self.gl_buffers.contains_key(&self.arg(1))),
            // ATI's buffers, which no console game ever used. Answering success without storing
            // anything would make the next draw come out of garbage — refusing is more honest.
            "BufferDataATI"
            | "MeshListATI"
            | "DrawVertexBufferObjectATI"
            | "GetPointerv"
            | "GetMaterialfv"
            | "GetTexParameteriv"
            | "GetTexParameterfv"
            | "GetTexParameterxv" => {
                self.assumptions
                    .insert("the game used an extension vertex buffer, which we don't have");
                EUNSUPPORTED
            }
            // `int QueryMatrixxOES(pMe, AEEGLfixed *mantissa, AEEGLint *exponent, bitfield *ret)`
            //
            // The current matrix in fixed point, as GLES represents it: one 16.16 `fixed` per
            // element, in `glLoadMatrix` column order, and the exponent of each — zero, because
            // the mantissa already is the value.
            //
            // **Quake 2 draws with it.** `R_SetupGL` sets up the camera and reads the modelview
            // back through here (on PC it was `glGetFloatv(GL_MODELVIEW_MATRIX)`), to cull the
            // world. The identity we used to answer only served callers asking whether the
            // extension exists, like Prey Evil. `ret` is the GL bitfield: zero says "all elements
            // are valid".
            "QueryMatrixxOES" if iface == Interface::Gles10Ext => {
                let (mantissa, exponent, status) = (self.arg(1), self.arg(2), self.arg(3));
                let matrix = self.gl.top_matrix();
                for (i, raw_value) in matrix.iter().enumerate() {
                    let i = i as u32;
                    if mantissa != 0 {
                        let fixed_point = (f64::from(*raw_value) * 65536.0)
                            .round()
                            .clamp(f64::from(i32::MIN), f64::from(i32::MAX))
                            as i32;
                        self.cpu.write_u32(mantissa + i * 4, fixed_point as u32)?;
                    }
                    if exponent != 0 {
                        self.cpu.write_u32(exponent + i * 4, 0)?;
                    }
                }
                if status != 0 {
                    self.cpu.write_u32(status, 0)?;
                }
                SUCCESS
            }
            // `int GetPowerLevel(pMe, int *ret)`. The console is portable; the emulator has no
            // battery to query, and "full" is the answer that doesn't make the game ask to
            // charge — nor open a warning screen nobody asked for.
            "GetPowerLevel" if iface == Interface::EglGetPowerLevel => {
                let out = self.arg(1);
                if out != 0 {
                    self.cpu.write_u32(out, 100)?;
                }
                SUCCESS
            }
            // `int SwapInterval(pMe, dpy, interval, EGLBoolean *ret)` and its `Get`.
            //
            // **Accepting and not storing is what the function path already does**, and it is
            // right: the frame pace here is the virtual clock's, and promising an interval we
            // don't control would be worse than promising nothing.
            "SwapInterval" if iface == Interface::EglOesSwapInterval => self.write_egl_true(3)?,
            "GetSwapInterval" if iface == Interface::EglOesSwapInterval => {
                let out = self.arg(2);
                if out != 0 {
                    self.cpu.write_u32(out, 1)?;
                }
                SUCCESS
            }
            // `int GetColorBuffer(pMe, void **ret)` — the same color buffer that
            // `eglGetColorBufferQUALCOMM` hands out as a function, by the same calculation.
            "GetColorBuffer" if iface == Interface::EglGetColorBuffer => {
                let (out, buffer) = (self.arg(1), self.egl_screen_color()?);
                if out != 0 {
                    self.cpu.write_u32(out, buffer)?;
                }
                SUCCESS
            }
            // `IGLES11ExtPak`: `TexGen`, separate blending and framebuffer objects.
            //
            // **The treatment is the same as for the other graphics extensions** — answer "done"
            // to what doesn't change what the game draws, and give a plausible answer to what it
            // queries. None of these families changes the output of our rasterizer: texture
            // coordinate generation is the only one that would, and no game in the library uses
            // it to draw — Prey Evil asks for it to learn that the pack exists. The framebuffer
            // objects get fake identifiers and answer "complete": there is only one target here,
            // and it is the right one.
            "TexGenf" | "TexGeni" | "TexGenx" | "TexGenfv" | "TexGeniv" | "TexGenxv"
            | "BlendEquation" | "BlendFuncSeparate" | "BlendEquationSeparate"
            | "BindRenderbufferOES" | "DeleteFramebuffersOES" | "DeleteRenderbuffersOES"
            | "FramebufferRenderbufferOES" | "FramebufferTexture2DOES" | "GenerateMipmapOES"
            | "RenderbufferStorageOES"
                if iface == Interface::Gles11ExtPak =>
            {
                self.assumptions.insert(concat!(
                    "the game used the OES extension pack (IGLES11ExtPak); the draw target ",
                    "is still single, and what it asked for was served without changing the output"
                ));
                SUCCESS
            }
            // The `Gen*OES` calls return an identifier through the output pointer: zero means "ran
            // out", and the game gives up on the whole family. **We always return the same `1`**,
            // and that is consistent: there is only one draw target here, so there is one
            // framebuffer and one renderbuffer, and asking for more returns the same one. A
            // counter would give identifiers that lead nowhere.
            "GenFramebuffersOES" | "GenRenderbuffersOES" if iface == Interface::Gles11ExtPak => {
                let out = self.arg(1);
                if out != 0 {
                    self.cpu.write_u32(out, 1)?;
                }
                SUCCESS
            }
            "IsFramebufferOES" | "IsRenderbufferOES" if iface == Interface::Gles11ExtPak => {
                self.write_egl_true(1)?
            }
            // `GL_FRAMEBUFFER_COMPLETE_OES` is 0x8CD5, and that is what a single target always is.
            "CheckFramebufferStatusOES" if iface == Interface::Gles11ExtPak => 0x8cd5,
            "GetTexGenfv" | "GetTexGeniv" | "GetTexGenxv"
            | "GetFramebufferAttachmentParameterivOES" | "GetRenderbufferParameterivOES"
                if iface == Interface::Gles11ExtPak =>
            {
                // Zeroing what is queried is better than leaving garbage in the game's memory, and
                // it is what the rest of the engine does with the `Get`s that have nothing to
                // return.
                self.write_at(self.arg(2), 0)?;
                SUCCESS
            }
            // `IJoystick`: the USB joystick that Qualcomm's joystick manager looks for.
            //
            // `int SetParm(pMe, int16 nParmID, int32 p1, int32 p2)` — calibration and setup.
            // Accepting and not storing is right: the controller we have is the Z-Pad, and it has
            // no parameter the game could change through here.
            "SetParm" if iface == Interface::Joystick => SUCCESS,
            // `int GetParm(pMe, int16 nParmID, int32 *pP1)` — answering zero is better than
            // leaving garbage, which the game would read as calibration.
            "GetParm" if iface == Interface::Joystick => {
                self.write_at(self.arg(2), 0)?;
                SUCCESS
            }
            // `int Read(pMe, int16 *px, int16 *py)` — **the controller state, for real**.
            //
            // It is the same reading `IHIDDevice::GetPositionState` delivers, in the console's
            // range (resting at 128): the joystick and the Z-Pad are the same device to the
            // player, and it is what the game's joystick manager expects to read.
            "Read" if iface == Interface::Joystick => {
                let (px, py) = (self.arg(1), self.arg(2));
                let pad = self.pads[0];
                for (location, axis) in [(px, 0), (py, 1)] {
                    if location != 0 {
                        let raw_value = pad.console_axis(axis) as i16;
                        self.cpu.write_mem(location, &raw_value.to_le_bytes())?;
                    }
                }
                SUCCESS
            }
            // `IGLES11Ext`: the OES extensions of OpenGL ES 1.1.
            //
            // **Prey Evil does not draw without them.** The survey of the 62 ROMs caught it with a
            // single-color screen: eleven GL methods in the report — `MatrixMode`, `PushMatrix`,
            // `BindTexture` sixteen thousand times — and **no** `Draw` or `eglSwapBuffers`. It
            // sets up the state and stops, because it asks for these extensions through
            // `CreateInstance` and used to get null. Without the interface, its drawing path
            // never starts.
            //
            // The `DrawTex*` calls draw a texture rectangle in screen coordinates, bypassing the
            // model matrix. It is the same drawing as `IGLES11`'s: Ridge Racer builds its whole
            // menu and HUD through here.
            "DrawTexsOES" | "DrawTexiOES" | "DrawTexxOES" | "DrawTexsvOES" | "DrawTexivOES"
            | "DrawTexxvOES" | "DrawTexfOES" | "DrawTexfvOES"
                if iface == Interface::Gles11Ext =>
            {
                self.sync_egl_color_from_guest()?;
                self.gles_draw_tex(name, 1)?;
                SUCCESS
            }
            // The matrix palette (`OES_matrix_palette`) still answers "done" with no effect.
            "CurrentPaletteMatrixOES" | "LoadPaletteFromModelViewMatrixOES"
            | "MatrixIndexPointerOES" | "WeightPointerOES"
                if iface == Interface::Gles11Ext =>
            {
                // The method name already shows up in "heaviest calls"; here it is enough to name
                // the cause, because the assumptions are a set of fixed texts.
                self.assumptions.insert(concat!(
                    "the game used the matrix palette (IGLES11Ext), which has no effect yet"
                ));
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    pub(super) fn license_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::License.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "IsExpired" => FALSE,
            // AEELicenseType GetInfo(ILicense *, uint32 *pdwExpire)
            //
            // With `LT_NONE` the documentation says there is no associated value, but the game
            // passes a pointer and will read whatever is there — so we write `BV_UNLIMITED`.
            "GetInfo" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, BV_UNLIMITED)?;
                }
                LT_NONE
            }
            // Only makes sense for a per-use license; the documentation itself says to return
            // `EFAILED` when the type is not `LT_USES`.
            "SetUsesRemaining" => EFAILED,
            // AEEPriceType GetPurchaseInfo(ILicense *, AEELicenseType *plt, uint32 *pdwExpire,
            //                              uint32 *pdSeq)
            "GetPurchaseInfo" => {
                if a1 != 0 {
                    self.cpu.write_mem(a1, &[LT_NONE as u8])?;
                }
                if a2 != 0 {
                    self.cpu.write_u32(a2, BV_UNLIMITED)?;
                }
                if a3 != 0 {
                    self.cpu.write_u32(a3, 0)?;
                }
                PT_PURCHASE
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// The generic collection of the Z-Wheel interface.
    ///
    /// The app walks it like a cursor: `Reset` once, and then `GetCurrent`/`AtEnd` until the
    /// end. While `AtEnd` answered "not yet" — which is what the probe did by returning
    /// success — it spun fifteen million times.
    ///
    /// The unnamed slots have not shown up yet; if they do, the report says so instead of
    /// pretending they were served. That is why they have no name in the table.
    /// Serves `IConfig`. See [`Interface::Config`].
    ///
    /// `int ICONFIG_GetItem(IConfig *pMe, ConfigItem nItem, void *pBuff, int nSize)` and the
    /// `SetItem` of the same shape. The items live while the emulator runs, like the
    /// `ISHELL_GetPrefs` preferences: writing them to disk would mean inventing a format the
    /// console had and we don't know. What must hold is that whoever writes reads back what it
    /// wrote.
    pub(super) fn config_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Config.method(slot) else {
            return Ok(None);
        };
        if aee::is_placeholder(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.config_items.remove(&this);
                }
                remaining
            }
            "GetItem" => {
                let (item, buffer, size) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3) as usize,
                );
                match self
                    .config_items
                    .get(&this)
                    .and_then(|items| items.get(&item))
                {
                    // Returning less than was asked would leave the rest of the buffer with
                    // whatever was already there, and the game would read garbage thinking it
                    // read configuration.
                    Some(data) if data.len() >= size => {
                        let clip = data[..size].to_vec();
                        self.cpu.write_mem(buffer, &clip)?;
                        SUCCESS
                    }
                    _ => EFAILED,
                }
            }
            "SetItem" => {
                let (item, buffer, size) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3) as usize,
                );
                if size == 0 || size > MAX_STRING {
                    return Ok(Some(EFAILED));
                }
                let mut data = vec![0u8; size];
                self.cpu.read_mem(buffer, &mut data)?;
                self.config_items
                    .entry(this)
                    .or_default()
                    .insert(item, data);
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Serves ZEEBOMCP. See [`Interface::ZeeboMcp`].
    ///
    /// Only the three slots read in the firmware are served; the five below fall through and
    /// end up in the report, which is what we want when the Z-Wheel finally uses one of them.
    pub(super) fn zeebo_mcp_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::ZeeboMcp.method(slot) else {
            return Ok(None);
        };
        if aee::is_placeholder(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            // The firmware's accepts two IIDs: the class's own and `0x01000001`. Accepting any
            // of them would be saying this object is every interface in the system.
            "QueryInterface" => {
                let (iid, output) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                if iid != AEECLSID_ZEEBOMCP && iid != 0x0100_0001 {
                    return Ok(Some(ECLASSNOTSUPPORT));
                }
                if output != 0 {
                    self.cpu.write_u32(output, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // **The three slots the Z-Wheel uses to launch a game touch the module's working
            // copy, and here it does not exist.** On the console the game lives in the eNAND, at
            // `fs:/card3/mod/<dir>`, and runs from a copy at `fs:/mcp/mod/<dir>`. The names and
            // paths come from the firmware (`0x110860f0`, `0x11086342`, `0x11086466`) and from
            // the Z-Wheel's own error message, `ModDataCopyFromENAND failure, folder='%s'`:
            //
            // - slot 3 copies from the eNAND to the MCP before running;
            // - slot 7 takes the `udata` — the saves — from the MCP back to the eNAND;
            // - slot 5 deletes the MCP copy.
            //
            // Here each game runs and saves in its own folder, so there is nothing to copy or
            // delete. With all three refused, the Z-Wheel gave up on launching the game picked
            // in the grid.
            "ModDataCopyFromENAND" | "UserDataCopyToENAND" | "ModDataRemoveFromMCP" => SUCCESS,
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Serves the SIM card control. See [`Interface::SimCardCtl`].
    pub(super) fn sim_card_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::SimCardCtl.method(slot) else {
            return Ok(None);
        };
        if aee::is_placeholder(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "QueryInterface" => {
                let (iid, output) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                if iid != AEECLSID_SIMCARDCTL && iid != 0x0100_0001 {
                    return Ok(Some(ECLASSNOTSUPPORT));
                }
                if output != 0 {
                    self.cpu.write_u32(output, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // Stores the pair and tells no one: there is no card to verify, and calling the
            // callback would be claiming there is.
            // Stores the pair and tells no one: there is no card to verify, and calling the
            // callback would be claiming there is. Today it never gets here — the class is not
            // offered.
            "PedirVerificacao" => {
                self.assumptions
                    .insert("a SIM card verification was accepted and never answered");
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Serves `IValueModel` (`0x01028e3c`). See [`Interface::Class28e3c`].
    pub(super) fn value_model_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        /// `EVT_MDL_VALUE`, the "value changed" event. The number is what the grid's listener
        /// checks at `0x37710` before taking the item.
        const EVT_MDL_VALUE: u32 = 0x1000;
        /// The size of `ModelEvent`: `{ evCode, pModel, dwParam }`.
        const MODEL_EVENT: u32 = 12;

        let Some(name) = Interface::Class28e3c.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.value_models.remove(&this);
                }
                remaining
            }
            "QueryInterface" => {
                if a2 != 0 {
                    self.cpu.write_u32(a2, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // void AddListener(IValueModel *, ModelListener *pl)
            //
            // `ModelListener` is `{ pNext, pPrev, pfnListener, pListenerData, pfnCancel,
            // pCancelData }`. The game fills in the function and the context; the rest belongs to
            // the model. The cancel stays null: whoever cancels a listener calls `pfnCancel` if
            // there is one, and without it the listener stays on our list — hence the check at
            // notification time.
            "AddListener" => {
                if a1 != 0 {
                    let callback = self.cpu.read_u32(a1 + 8)?;
                    let context = self.cpu.read_u32(a1 + 12)?;
                    for field in [0, 4, 16, 20] {
                        self.cpu.write_u32(a1 + field, 0)?;
                    }
                    let model = self.value_models.entry(this).or_default();
                    model.listeners.retain(|&(location, _, _)| location != a1);
                    model.listeners.push((a1, callback, context));
                }
                SUCCESS
            }
            // void Notify(IValueModel *, ModelEvent *pev)
            "Notify" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1 + 4, this)?;
                    self.notify_listeners(this, a1)?;
                }
                SUCCESS
            }
            // int SetValue(IValueModel *, void *pvValue, int nLen, PFNVALUEFREE pfn)
            //
            // The previous value's free function is not called: it is game code, and calling it
            // in the middle of a dispatch is the path that has crashed games before.
            "SetValue" => {
                let model = self.value_models.entry(this).or_default();
                model.value = a1;
                model.size = a2;
                let event = self.alloc_internal(MODEL_EVENT)?;
                if event != 0 {
                    self.cpu.write_u32(event, EVT_MDL_VALUE)?;
                    self.cpu.write_u32(event + 4, this)?;
                    self.cpu.write_u32(event + 8, 0)?;
                    self.notify_listeners(this, event)?;
                    self.free(event);
                }
                SUCCESS
            }
            // void *GetValue(IValueModel *, int *pnLen)
            "GetValue" => {
                let model = self.value_models.get(&this).cloned().unwrap_or_default();
                if a1 != 0 {
                    self.cpu.write_u32(a1, model.size)?;
                }
                model.value
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Calls each of the model's listeners with the event, checking it is still the one that was
    /// registered.
    fn notify_listeners(&mut self, model: u32, event: u32) -> Result<(), CpuError> {
        let listeners = self
            .value_models
            .get(&model)
            .map(|m| m.listeners.clone())
            .unwrap_or_default();
        for (location, callback, context) in listeners {
            let live = self.cpu.read_u32(location + 8).ok() == Some(callback)
                && self.cpu.read_u32(location + 12).ok() == Some(context);
            if !live || callback == 0 {
                if let Some(m) = self.value_models.get_mut(&model) {
                    m.listeners.retain(|&(o, _, _)| o != location);
                }
                continue;
            }
            let budget = self.budget.max(1);
            let _ = self.call_guest_nested(callback, [context, event, 0, 0], budget)?;
        }
        Ok(())
    }

    /// Serves the system control. See [`Interface::SystemCtl`].
    ///
    /// The firmware's `QueryInterface` accepts two IIDs, `0x01000001` and the class's own, and
    /// that is what we do here — accepting any of them would be saying this object is every
    /// interface in the system.
    pub(super) fn system_ctl_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::SystemCtl.method(slot) else {
            return Ok(None);
        };
        if aee::is_placeholder(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "QueryInterface" => {
                let (iid, output) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                if iid != AEECLSID_SYSTEMCTL && iid != 0x0100_0001 {
                    return Ok(Some(ECLASSNOTSUPPORT));
                }
                if output != 0 {
                    self.cpu.write_u32(output, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // `slot6(this)` reads the current mode through an OEM function (`0x10e9ff84` in the
            // firmware), and `slot3(this, mode, enabled)` writes it: the firmware accepts modes 0
            // to 3 and sets hardware flags for each (`0x10e9fdb6`). The Z-Wheel reads and writes
            // it back at `0x83594`, right before launching a game — with slot 3 refused, the
            // launch stopped there.
            //
            // There is no hardware to adjust here, so the mode is only stored to be read back.
            // Before any write, the reading is zero, as it always was.
            "Consultar" => {
                self.assumptions
                    .insert("the system control answered zero: there is no device to query");
                self.system_mode
            }
            "DefinirModo" => {
                self.system_mode = self.cpu.read_reg(Reg::R1);
                SUCCESS
            }
            // Slot 5 stores the mode **through the same path** as slot 3 — the disassembled
            // firmware shows the body is the same (`0x10e9fdb6`) — and also takes a third
            // argument, the option. There is no hardware to switch on, so the mode is stored and
            // slot 6 returns it, as with slot 3; the option is recorded in the assumption.
            "DefinirModoComOpcao" => {
                self.system_mode = self.cpu.read_reg(Reg::R1);
                self.assumptions.insert(
                    "the system control received a mode and an option; only the mode is stored",
                );
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Serves `ICM`. See [`Interface::Cm`].
    pub(super) fn cm_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        /// Offset of the service state inside `AEECMSSInfo`.
        const SERVICE_STATE: u32 = 0x0;
        /// Offset of the operating mode, read at `0x87cb0`.
        const OPERATING_MODE: u32 = 0xc;
        /// Offset of the signal strength, read at `0x696f8` as a halfword.
        const STRENGTH: u32 = 0x28;
        /// `AEECM_SRV_STATUS_SRV`. `0x696e8` accepts 1, 2 or 3 and rejects the rest with
        /// `Service status is NOT available!`; 2 is "full service".
        const IN_SERVICE: u32 = 2;
        /// `SYS_OPRT_MODE_ONLINE`. This is the number `0x77564` compares against.
        const ON_AIR: u32 = 5;
        /// `0x69830` turns the strength into bars in bands of nine: `0x45..=0x4d` is four bars,
        /// and that is where this number falls.
        const SIGNAL_LEVEL: u16 = 0x48;
        /// The smallest buffer that answers the three reads we know of.
        const MIN_SIZE: usize = STRENGTH as usize + 2;

        let Some(name) = Interface::Cm.method(slot) else {
            return Ok(None);
        };
        if aee::is_placeholder(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            // int ICM_GetSSInfo(ICM *, AEECMSSInfo *pInfo, uint32 nSize)
            //
            // One call, two readers: `0x87c90` wants the operating mode at `+0xc`, and `0x696a0`
            // wants the service state at `+0` and, when it is 2, the signal strength at `+0x28`.
            // Answering only the first left the second repeating
            // `Service status is NOT available!` forever.
            "GetSSInfo" => {
                let (info, size) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2) as usize,
                );
                if info == 0 || size < MIN_SIZE {
                    return Ok(Some(EBADPARM));
                }
                // Zeroing the rest is part of the answer: the game passes a buffer it zeroed
                // itself, but a caller of this function cannot count on that.
                self.cpu
                    .write_mem(info, &vec![0u8; guest_size(size as usize)?])?;
                self.cpu.write_u32(info + SERVICE_STATE, IN_SERVICE)?;
                self.cpu.write_u32(info + OPERATING_MODE, ON_AIR)?;
                self.cpu
                    .write_mem(info + STRENGTH, &SIGNAL_LEVEL.to_le_bytes())?;
                self.assumptions.insert(
                    "the ICM answered radio on air and full service, with the rest of AEECMSSInfo zeroed",
                );
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Serves the Z-Wheel's generic list. See [`Interface::Vector`].
    pub(super) fn vector_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        /// The index the game passes to say "at the end".
        const AT_END: u32 = u32::MAX;

        let Some(name) = Interface::Vector.method(slot) else {
            return Ok(None);
        };
        if aee::is_placeholder(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.vectors.remove(&this);
                }
                remaining
            }
            "Tamanho" => self
                .vectors
                .get(&this)
                .map_or(0, |(items, _)| items.len() as u32),
            "PegarEm" => {
                let (index, output) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let item = self
                    .vectors
                    .get(&this)
                    .and_then(|(items, _)| items.get(index as usize).copied());
                match item {
                    Some(item) => {
                        if output != 0 {
                            self.cpu.write_u32(output, item)?;
                        }
                        SUCCESS
                    }
                    // Out of range writes nothing: leaving the output as it was is what lets the
                    // caller tell "there is none" from "there is one and it is null".
                    None => EBADPARM,
                }
            }
            // `IVector`'s `ReplaceAt`, and it is BREW's slot 7, between `GetAt` and `InsertAt`.
            // The Z-Wheel uses it in an insertion sort at `0x38e5c`: it shifts each item one
            // position and writes the new one in the right place. It was missing, and the game
            // list that "Jogar" opens stayed unsorted — and never came out.
            "SubstituirEm" => {
                let (index, item) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                match self
                    .vectors
                    .get_mut(&this)
                    .and_then(|(items, _)| items.get_mut(index as usize))
                {
                    Some(entry) => {
                        *entry = item;
                        SUCCESS
                    }
                    None => EBADPARM,
                }
            }
            "InserirEm" => {
                let (index, item) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let Some((items, _)) = self.vectors.get_mut(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let location = match index {
                    AT_END => items.len(),
                    n => (n as usize).min(items.len()),
                };
                items.insert(location, item);
                SUCCESS
            }
            // The `RemoverEm(0)` + `PegarEm(0)` pair at `0x7d788` is a loop that drains the
            // list: the game removes the first, takes the new first and repeats until there are
            // none left. Without a real `RemoverEm` it never ends — it took seven million
            // iterations until the instruction budget ran out.
            "RemoverEm" => {
                let index = self.cpu.read_reg(Reg::R1) as usize;
                let Some((items, _)) = self.vectors.get_mut(&this) else {
                    return Ok(Some(EBADPARM));
                };
                if index >= items.len() {
                    return Ok(Some(EBADPARM));
                }
                items.remove(index);
                SUCCESS
            }
            // The free function is a function pointer in the module, and it is what the
            // console's `Esvaziar` hands each item to. Here it is only stored — see the note on
            // `Esvaziar`.
            "DefinirLiberador" => {
                if let Some((_, free_fn)) = self.vectors.get_mut(&this) {
                    *free_fn = self.cpu.read_reg(Reg::R1);
                }
                SUCCESS
            }
            // Emptying **without** calling each item's free function is a deliberate debt: the
            // game allocated the items, and calling its code in the middle of a dispatch is the
            // path that already crashed Zeeboids once. The cost is memory that does not return
            // to the game's heap while it runs, and that is why the assumption is recorded.
            "Esvaziar" => {
                if let Some((items, free_fn)) = self.vectors.get_mut(&this)
                    && !items.is_empty()
                    && *free_fn != 0
                {
                    items.clear();
                    self.assumptions.insert(
                        "a list was emptied without calling the free function the game registered",
                    );
                } else if let Some((items, _)) = self.vectors.get_mut(&this) {
                    items.clear();
                }
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    pub(super) fn collection_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Collection.method(slot) else {
            return Ok(None);
        };
        if aee::is_placeholder(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let a1 = self.cpu.read_reg(Reg::R1);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.collections.remove(&this);
                    self.collection_params
                        .retain(|(obj, _), _| *obj != this);
                }
                remaining
            }
            "Reset" => {
                if let Some((_, cursor)) = self.collections.get_mut(&this) {
                    *cursor = 0;
                }
                SUCCESS
            }
            // The end is true when the cursor has passed the last item — and a collection nobody
            // filled is at the end from the start.
            "AtEnd" => {
                let (items, cursor) = self
                    .collections
                    .get(&this)
                    .map(|(items, cursor)| (items.len(), *cursor))
                    .unwrap_or((0, 0));
                u32::from(cursor >= items)
            }
            // `slot10(this, id, pointer, size)`, seen at `0x7d0c4` with
            // `(0, &{0x01070798}, 4)` — the number passed is the applet's own ClassID.
            //
            // What it **means** cannot be told: the function that calls it creates the
            // collection, makes this call and releases the object right after, without reading
            // anything back. It could be "store this parameter" or "append this item"; both
            // readings have the same observable effect, which is none. Storing the bytes covers
            // both and does not invent behavior.
            "Definir" => {
                let (id, pointer, size) = (
                    a1,
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3) as usize,
                );
                if pointer == 0 || size == 0 || size > MAX_STRING {
                    return Ok(Some(EBADPARM));
                }
                let mut data = vec![0u8; size];
                self.cpu.read_mem(pointer, &mut data)?;
                self.collection_params.insert((this, id), data);
                SUCCESS
            }
            // The current item goes out through the output pointer, and the cursor advances.
            // With no item, `EFAILED`.
            "GetCurrent" => {
                let item = self.collections.get_mut(&this).and_then(|(items, cursor)| {
                    let item = items.get(*cursor).copied();
                    if item.is_some() {
                        *cursor += 1;
                    }
                    item
                });
                match item {
                    Some(item) => {
                        if a1 != 0 {
                            self.cpu.write_u32(a1, item)?;
                        }
                        SUCCESS
                    }
                    None => EFAILED,
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }
}
