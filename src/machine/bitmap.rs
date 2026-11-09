//! IBitmap and the DIB: the surfaces, the blit and the sync with the game's memory.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Draws an image on a game surface by calling its `BltIn`.
    ///
    /// It is the same choreography as BREW: we build an `IBitmap` of our own with the decoded image
    /// and pass it as the source. The game's `BltIn` then asks our bitmap for
    /// `QueryInterface(AEECLSID_DIB)` and reads the pixels through the public fields, which is
    /// exactly what `IDIB` exists to offer. Checked in the disassembly: Bejeweled Twist's `BltIn`
    /// does that `QueryInterface` with `0x01001045` and then reads `cx`, `cy` and `nColorScheme`.
    pub(super) fn blit_into_foreign(
        &mut self,
        blit: PendingBlit,
        budget: u64,
    ) -> Result<(), CpuError> {
        let Some(info) = self.images.get(&blit.image).cloned() else {
            return Ok(());
        };
        let (frame_width, offset) = match (blit.frame, info.frame_width) {
            (Some(n), width) if width > 0 => (width as u32, n * width as u32),
            _ => (info.width, 0),
        };
        let src_x = blit.src_x.max(0) as u32;
        let src_y = blit.src_y.max(0) as u32;
        let blit_width = blit.width.min(frame_width.saturating_sub(src_x));
        let blit_height = blit.height.min(info.height.saturating_sub(src_y));
        if blit_width == 0 || blit_height == 0 {
            return Ok(());
        }

        // The source is a surface of ours, created only for this call.
        let source = self.new_object(Interface::Bitmap)?;
        if source == 0 {
            return Ok(());
        }
        let mut surface = Framebuffer::new(blit_width, blit_height);
        for row in 0..blit_height {
            for column in 0..blit_width {
                let index =
                    ((row + src_y) * info.width + column + src_x + offset) as usize;
                let opaque = info.opaque.get(index).copied().unwrap_or(true);
                let pixel = match (opaque, info.pixels.get(index)) {
                    (true, Some(&pixel)) => pixel,
                    // With no alpha channel in the destination, transparent becomes a reserved
                    // color — that is how BREW handles it, and `BltIn` honors the `IDIB`'s
                    // `ncTransparent`.
                    _ => TRANSPARENT_KEY,
                };
                surface.set_pixel_native(column as i32, row as i32, pixel);
            }
        }
        self.bitmaps.insert(source, surface);
        self.transparency.insert(source, TRANSPARENT_KEY);
        self.publish_source_888(source, &info, blit_width, blit_height, src_x + offset, src_y)?;

        let vtable = self.cpu.read_u32(blit.target)?;
        let blt_in = self.cpu.read_u32(vtable + BITMAP_BLT_IN_SLOT * 4)?;
        // BltIn(po, xDst, yDst, dx, dy, pSrc, xSrc, ySrc, rop)
        let outcome = self.call_guest_with_stack(
            blt_in,
            [blit.target, blit.x as u32, blit.y as u32, blit_width],
            &[blit_height, source, 0, 0, blit.rop],
            budget,
        )?;
        if !matches!(outcome, Outcome::Returned { code: 0 }) {
            self.assumptions
                .insert("a game surface's BltIn refused the draw");
        }

        self.objects.release(source);
        self.bitmaps.remove(&source);
        // **The buffer goes back to the region through the normal path.** Removing only the
        // `dib_buffers` entry here left the recorded capacity behind: the next bitmap born at this
        // address saw the record, concluded its buffer already fit and published an `IDIB` with a
        // null `pBmp`. Bejeweled Twist died of this — its `BltIn` builds the row table from
        // `pBmp`, and without it the source object had no pixels and the game crashed on the first
        // draw.
        self.release_dib(source);
        self.cpu.unwatch_dirty(source);
        self.transparency.remove(&source);
        Ok(())
    }

    /// Composites a temporary surface of ours onto a game surface through its `BltIn`.
    pub(super) fn blit_surface_into_foreign(
        &mut self,
        blit: PendingSurfaceBlit,
        budget: u64,
    ) -> Result<(), CpuError> {
        self.publish_framebuffer_888(blit.source)?;

        let vtable = self.cpu.read_u32(blit.target)?;
        let blt_in = self.cpu.read_u32(vtable + BITMAP_BLT_IN_SLOT * 4)?;
        let outcome = self.call_guest_with_stack(
            blt_in,
            [blit.target, blit.x as u32, blit.y as u32, blit.width],
            &[blit.height, blit.source, 0, 0, blit.rop],
            budget,
        )?;
        if !matches!(outcome, Outcome::Returned { code: 0 }) {
            self.assumptions
                .insert("a game surface's BltIn refused a 2D primitive");
        }

        self.objects.release(blit.source);
        self.bitmaps.remove(&blit.source);
        self.release_dib(blit.source);
        self.cpu.unwatch_dirty(blit.source);
        self.transparency.remove(&blit.source);
        Ok(())
    }

    /// Publishes the `IDIB` of a foreign `BltIn`'s source in 888: 24 bits per pixel, or 32 with
    /// alpha in the fourth byte when the image has transparency.
    ///
    /// **It cannot be our 565.** Bejeweled Twist's `BltIn` (0x139d8) only accepts
    /// `nColorScheme` 0, which it reads as an 8-bit palette, or 24; any other value gives up
    /// without creating the source surface, and its first blit jumped to address zero
    /// (0x32c1c) or read 0x24. It is the format in which BREW's decoder delivers the image.
    ///
    /// The order is B, G, R, A — BREW's `0x00RRGGBB` in little-endian. Checked in its conversion
    /// to 565 (0x23798): byte 2 goes to red and byte 0 to blue. Expanding our 565 loses nothing:
    /// the game converts back to the screen's format.
    fn publish_source_888(
        &mut self,
        source: u32,
        info: &DecodedImage,
        width: u32,
        height: u32,
        offset_x: u32,
        offset_y: u32,
    ) -> Result<(), CpuError> {
        let total = (info.width * info.height) as usize;
        let has_alpha = !info.alpha.is_empty() || info.opaque.iter().take(total).any(|&o| !o);
        let channels: usize = if has_alpha { 4 } else { 3 };
        let mut bytes = Vec::with_capacity(width as usize * height as usize * channels);
        for row in 0..height {
            for column in 0..width {
                let index = ((row + offset_y) * info.width + column + offset_x) as usize;
                let color = Rgb::from_rgb565(info.pixels.get(index).copied().unwrap_or(0));
                bytes.extend_from_slice(&[color.b, color.g, color.r]);
                if has_alpha {
                    let opaque = info.opaque.get(index).copied().unwrap_or(true);
                    let alpha = match info.alpha.get(index) {
                        Some(&alpha) => alpha,
                        None if opaque => 0xff,
                        None => 0,
                    };
                    bytes.push(alpha);
                }
            }
        }
        let Some((buffer, capacity)) = self.reserve_surface(bytes.len() as u32) else {
            return Ok(());
        };
        self.cpu.write_mem(buffer, &bytes)?;
        // Registered as a decoder's `IDIB`: the `QueryInterface`'s `expose_dib` does not
        // rewrite it in 565, and the cleanup's `release_dib` returns the buffer.
        self.decoder_dib.insert(source, (buffer, capacity));
        let step = width as usize * channels;
        self.cpu.write_u32(source + 4, 0)?; // pPaletteMap
        self.cpu.write_u32(source + 8, buffer)?; // pBmp
        self.cpu.write_u32(source + 12, 0)?; // pRGB
        self.cpu
            .write_u32(source + 16, to_rgbval(Rgb::from_rgb565(TRANSPARENT_KEY)))?; // ncTransparent
        self.cpu.write_mem(source + 20, &(width as u16).to_le_bytes())?;
        self.cpu.write_mem(source + 22, &(height as u16).to_le_bytes())?;
        self.cpu.write_mem(source + 24, &(step as i16).to_le_bytes())?;
        self.cpu.write_mem(source + 26, &0u16.to_le_bytes())?; // cntRGB
        self.cpu
            .write_mem(source + 28, &[(channels * 8) as u8, IDIB_COLORSCHEME_888])?;
        self.cpu.write_mem(source + 30, &[0u8; 6])?;
        Ok(())
    }

    /// Publishes a temporary framebuffer as an 888 `IDIB` for the game's `BltIn`.
    fn publish_framebuffer_888(&mut self, source: u32) -> Result<(), CpuError> {
        let Some(fb) = self.bitmaps.get(&source) else {
            return Ok(());
        };
        let (width, height) = (fb.width(), fb.height());
        let mut bytes = Vec::with_capacity(width as usize * height as usize * 3);
        for row in 0..height {
            for column in 0..width {
                let color = Rgb::from_rgb565(fb.get_pixel(column as i32, row as i32));
                bytes.extend_from_slice(&[color.b, color.g, color.r]);
            }
        }
        let Some((buffer, capacity)) = self.reserve_surface(bytes.len() as u32) else {
            return Ok(());
        };
        self.cpu.write_mem(buffer, &bytes)?;
        self.decoder_dib.insert(source, (buffer, capacity));
        let step = width as usize * 3;
        self.cpu.write_u32(source + 4, 0)?; // pPaletteMap
        self.cpu.write_u32(source + 8, buffer)?; // pBmp
        self.cpu.write_u32(source + 12, 0)?; // pRGB
        self.cpu
            .write_u32(source + 16, to_rgbval(Rgb::from_rgb565(TRANSPARENT_KEY)))?; // ncTransparent
        self.cpu.write_mem(source + 20, &(width as u16).to_le_bytes())?;
        self.cpu.write_mem(source + 22, &(height as u16).to_le_bytes())?;
        self.cpu.write_mem(source + 24, &(step as i16).to_le_bytes())?;
        self.cpu.write_mem(source + 26, &0u16.to_le_bytes())?; // cntRGB
        self.cpu
            .write_mem(source + 28, &[24, IDIB_COLORSCHEME_888])?;
        self.cpu.write_mem(source + 30, &[0u8; 6])?;
        Ok(())
    }

    /// Finds out where the pixels of an `IBitmap` implemented by the game itself are.
    ///
    /// It is the same path BREW uses: `QueryInterface(AEECLSID_DIB)` on the object, and the `IDIB`
    /// that comes back carries `pBmp`, `cx`, `cy` and `nPitch` as public fields. With that, the
    /// game's surface joins the same sync mechanism as ours — we draw on the host and the result
    /// is copied to its memory.
    pub(super) fn probe_foreign_surface(
        &mut self,
        target: u32,
        budget: u64,
    ) -> Result<(), CpuError> {
        let Ok(vtable) = self.cpu.read_u32(target) else {
            return Ok(());
        };
        let Ok(query) = self.cpu.read_u32(vtable + BITMAP_QUERY_INTERFACE_SLOT * 4) else {
            return Ok(());
        };
        // The output pointer must live in guest memory.
        let out = self.alloc_internal(4)?;
        if out == 0 {
            return Ok(());
        }
        // `IDIB` has two IIDs: the current one and the one BREW 2.0 used. A bitmap written for
        // the old platform only recognizes the second.
        let mut dib = 0;
        let mut outcome = Outcome::Returned { code: EFAILED };
        for iid in [AEECLSID_DIB, AEEIID_DIB_20] {
            self.cpu.write_u32(out, 0)?;
            outcome = self.call_guest(query, [target, iid, out, 0], budget)?;
            dib = self.cpu.read_u32(out).unwrap_or(0);
            if matches!(outcome, Outcome::Returned { code: 0 }) && dib != 0 {
                break;
            }
        }
        self.free(out);

        let failed = !matches!(outcome, Outcome::Returned { code: 0 });
        if failed || dib == 0 {
            // Bejeweled Twist is like this: its surface's `QueryInterface` is literally
            // `mov r0, #0x14; bx lr` — it always returns `ECLASSNOTSUPPORT`. On that surface the
            // only drawing method really implemented is `BltIn`.
            self.assumptions
                .insert("a game surface does not expose IDIB; drawing on it goes through BltIn when possible");
            return Ok(());
        }

        let buffer = self.cpu.read_u32(dib + 8)?;
        let mut fields = [0u8; 10];
        self.cpu.read_mem(dib + 20, &mut fields)?;
        let cx = u16::from_le_bytes([fields[0], fields[1]]) as u32;
        let cy = u16::from_le_bytes([fields[2], fields[3]]) as u32;
        let pitch = i16::from_le_bytes([fields[4], fields[5]]) as i32;
        let depth = fields[8];

        // We only know how to handle the console's screen format: RGB565, contiguous rows going
        // down. Anything else is better refused than drawn crooked.
        if cx == 0
            || cy == 0
            || buffer == 0
            || depth != COLOR_DEPTH as u8
            || pitch != ((cx * 2).div_ceil(4) * 4) as i32
        {
            self.assumptions
                .insert("a game surface uses a format we cannot draw yet");
            return Ok(());
        }

        self.bitmaps.insert(target, Framebuffer::new(cx, cy));
        self.dib_buffers.insert(target, buffer);
        // Here the buffer belongs to the game itself, and the pixels come from it.
        self.inherited_dibs.remove(&target);
        self.cpu.watch_dirty(target, buffer, pitch as u32 * cy)?;
        self.sync_from_guest(target)?;
        Ok(())
    }

    /// The screen bitmap, created the first time someone asks for it.
    pub(super) fn device_bitmap(&mut self) -> Result<u32, CpuError> {
        if self.device_bitmap != 0 {
            return Ok(self.device_bitmap);
        }
        let addr = self.new_object(Interface::Bitmap)?;
        if addr != 0 {
            let screen = std::mem::replace(&mut self.screen, Framebuffer::new(1, 1));
            self.bitmaps.insert(addr, screen);
            self.device_bitmap = addr;
            self.display_target = addr;
            // **A reference of ours, which is never released.** Whoever asks for the screen bitmap
            // releases what they got, as convention says; but its owner is the display, not whoever
            // asked. Without this count the address went back to the free list and the next
            // `CreateCompatibleBitmap` wrote its surface over the screen: the Z-Wheel entered the
            // Stage, created a 214×34 surface, and that was what showed up in the window instead of
            // the 640×480.
            self.objects.add_ref(addr);
        }
        Ok(addr)
    }

    /// The `0x0101e443` canvas: a bitmap seen as a place where an `IDisplay` draws.
    ///
    /// The Z-Wheel asks the bitmap it draws a widget subtree on for this canvas (`0x4166c`),
    /// calls slot 7 with `&out` and uses what comes out as an `IDisplay` — slots 14, 15 and 19,
    /// which are `SetDestination`, `GetDestination` and `GetClipRect`. With the display in hand it
    /// reads the destination, creates a compatible bitmap the size of the widget and draws on it.
    /// When this was refused, `0x41d68` got null and dereferenced it: that was the access to
    /// `0x00000000` at `0x41dbc` after confirming on "Jogar".
    pub(super) fn canvas_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Canvas.method(slot) else {
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
                    self.canvases.remove(&this);
                }
                remaining
            }
            "GetDisplay" => {
                let Some(&bitmap) = self.canvases.get(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let out = self.cpu.read_reg(Reg::R1);
                let display = self.new_object(Interface::Display)?;
                if display == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                // The canvas's display draws on its bitmap. Our displays share a single
                // destination, so handing one out means pointing that destination at the bitmap.
                self.display_target = bitmap;
                self.assumptions.insert(
                    "the 0x0101e443 canvas handed out an IDisplay pointed at its bitmap — reading from usage",
                );
                if out != 0 {
                    self.cpu.write_u32(out, display)?;
                }
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// `ITransform` methods.
    pub(super) fn transform_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Transform.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.transforms.remove(&this);
                }
                remaining
            }
            // int TransformBltSimple(ITransform *p, int x, int y, IBitmap *pSrc, int xSrc,
            //                        int ySrc, unsigned dx, unsigned dy, uint16 unTransform,
            //                        uint8 nComposite)
            //
            // The same rules as `TransformBltComplex`, with the matrix built from the flags:
            // rotation in quarter turns in bits 0-1, X mirror in bit 2 and the scale from bit 3 up.
            // Action Hero 3D passes `unTransform = 8` with `x = 160`, `y = 120` and a 320x240
            // canvas — the frame doubled and centered on the 640x480 screen, so `8` is the 2x
            // scale.
            "TransformBltSimple" => {
                let Some(&dest) = self.transforms.get(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let (x, y) = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let origin = self.cpu.read_reg(Reg::R3);
                let source_x = self.stack_arg(0)? as i32;
                let source_y = self.stack_arg(1)? as i32;
                let width = self.stack_arg(2)? as i32;
                let height = self.stack_arg(3)? as i32;
                let flags = self.stack_arg(4)? & 0xffff;
                let scale = match (flags >> 3) & 0x7 {
                    0 => 1.0,
                    1 => 2.0,
                    _ => {
                        self.assumptions.insert(
                            "ITransform::TransformBltSimple with a scale other than 1x and 2x — drawn at 1x",
                        );
                        1.0
                    }
                };
                let mirror = if flags & 0x4 != 0 { -1.0 } else { 1.0 };
                // Clockwise rotation, with screen y growing downward.
                let (cos, sin) = match flags & 0x3 {
                    0 => (1.0, 0.0),
                    1 => (0.0, 1.0),
                    2 => (-1.0, 0.0),
                    _ => (0.0, -1.0),
                };
                // R · mirror · scale
                let m = [
                    cos * mirror * scale,
                    -sin * scale,
                    sin * mirror * scale,
                    cos * scale,
                ];
                self.apply_transform(TransformBlt {
                    dest,
                    origin,
                    x,
                    y,
                    source_x,
                    source_y,
                    width,
                    height,
                    m,
                })
            }
            // int TransformBltComplex(ITransform *p, int x, int y, IBitmap *pSrc, int xSrc,
            //                         int ySrc, unsigned dx, unsigned dy,
            //                         const AEETransformMatrix *pMatrix, uint8 nComposite)
            //
            // The matrix is `{ int16 A, B, C, D }` in 8.8 fixed point, applied **around the
            // center** of the source rectangle, and `(x, y)` is where its corner would land without
            // transformation. The reading comes from usage: Zenonia passes `x = 160`, `y = 120`, a
            // 320x240 canvas and scale 1.9; the center of the result lands at (320, 240), in the
            // middle of the 640x480 screen.
            "TransformBltComplex" => {
                let Some(&dest) = self.transforms.get(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let (x, y) = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let origin = self.cpu.read_reg(Reg::R3);
                let source_x = self.stack_arg(0)? as i32;
                let source_y = self.stack_arg(1)? as i32;
                let width = self.stack_arg(2)? as i32;
                let height = self.stack_arg(3)? as i32;
                let matrix = self.stack_arg(4)?;
                let mut fields = [0u8; 8];
                self.cpu.read_mem(matrix, &mut fields)?;
                let field = |i: usize| i16::from_le_bytes([fields[i], fields[i + 1]]) as f32 / 256.0;
                let m = [field(0), field(2), field(4), field(6)];
                self.apply_transform(TransformBlt {
                    dest,
                    origin,
                    x,
                    y,
                    source_x,
                    source_y,
                    width,
                    height,
                    m,
                })
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Draws `origin` onto `dest` through the matrix, sampling the nearest pixel.
    ///
    /// It walks the destination, not the source: with a scale larger than one, walking the source
    /// would leave holes between the pixels. Each destination pixel goes back through the inverse
    /// matrix to the source.
    fn apply_transform(&mut self, t: TransformBlt) -> u32 {
        let [a, b, c, d] = t.m;
        let det = a * d - b * c;
        if det.abs() < f32::EPSILON || t.width <= 0 || t.height <= 0 {
            return EBADPARM;
        }
        let inverse = [d / det, -b / det, -c / det, a / det];
        let Some(source) = self.bitmaps.get(&t.origin) else {
            return EBADPARM;
        };
        let (half_w, half_h) = (t.width as f32 / 2.0, t.height as f32 / 2.0);
        // Copying the source rectangle first allows reading and writing when both are the same
        // surface map — and when source and destination are the same bitmap.
        let mut pixels = Vec::with_capacity((t.width * t.height) as usize);
        for line in 0..t.height {
            for column in 0..t.width {
                pixels.push(source.get_pixel(t.source_x + column, t.source_y + line));
            }
        }
        let Some(target) = self.bitmaps.get_mut(&t.dest) else {
            return EBADPARM;
        };
        let (center_x, center_y) = (t.x as f32 + half_w, t.y as f32 + half_h);
        // The result's box: the four corners of the source, transformed.
        let corners = [(-half_w, -half_h), (half_w, -half_h), (-half_w, half_h), (half_w, half_h)];
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (u, v) in corners {
            let (px, py) = (a * u + b * v + center_x, c * u + d * v + center_y);
            x0 = x0.min(px);
            y0 = y0.min(py);
            x1 = x1.max(px);
            y1 = y1.max(py);
        }
        let limit_x = target.width() as i32;
        let limit_y = target.height() as i32;
        for py in (y0.floor() as i32).max(0)..(y1.ceil() as i32).min(limit_y) {
            for px in (x0.floor() as i32).max(0)..(x1.ceil() as i32).min(limit_x) {
                let (u, v) = (px as f32 + 0.5 - center_x, py as f32 + 0.5 - center_y);
                let sx = (inverse[0] * u + inverse[1] * v + half_w).floor() as i32;
                let sy = (inverse[2] * u + inverse[3] * v + half_h).floor() as i32;
                if sx < 0 || sy < 0 || sx >= t.width || sy >= t.height {
                    continue;
                }
                target.set_pixel_native(px, py, pixels[(sy * t.width + sx) as usize]);
            }
        }
        SUCCESS
    }

    /// `IBitmap` methods, dispatched by slot name.
    pub(super) fn bitmap_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        // Blitting to the screen is drawing on top of it: the OpenGL frame must be in it
        // beforehand. See [`Machine::materialize_gl_frame`].
        self.materialize_gl_frame();
        let Some(name) = Interface::Bitmap.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            // `IBitmap` has `QueryInterface`, so AddRef and Release need to be handled here: the
            // dispatch's generic arm only reaches interfaces with no handling of their own.
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                // **The screen bitmap does not die.** Its owner is the display, not whoever asked;
                // an extra `Release` returned the address to the free list with the surface still
                // alive, and the next object was born on top of the screen. The guard holds even
                // with `GetDestination` already counting: it is the screen's address that must not
                // be recycled, and any new path that hands it out without counting would reopen
                // this.
                if this == self.device_bitmap && self.objects.count(this) <= 1 {
                    if self.serial.is_some() {
                        self.log_serial(format!(
                            "<extra release on the screen bitmap {this:#x}; holding on>"
                        ));
                    }
                    return Ok(Some(1));
                }
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.release_dib(this);
                }
                remaining
            }
            // int QueryInterface(IBitmap *, AEECLSID, void **) — the game uses this to ask for an
            // `IDIB`, which gives direct access to the pixels. We do not offer that interface yet,
            // and `ECLASSNOTSUPPORT` is the correct answer for that: BREW expects the app to have
            // an alternative path when the platform does not expose the DIB.
            "QueryInterface" => {
                let requested = self.cpu.read_reg(Reg::R1);
                let out = self.cpu.read_reg(Reg::R2);
                let answer = match requested {
                    AEEIID_IBITMAP => Some(this),
                    // An `IDIB` *is* an `IBitmap` — the struct starts with the `IBitmap` vtable and
                    // only adds public fields. So the object itself will do, as long as the
                    // fields are filled in.
                    AEECLSID_DIB | AEEIID_DIB_20 => {
                        self.expose_dib(this)?;
                        Some(this)
                    }
                    AEEIID_CANVAS => {
                        let canvas = self.new_object(Interface::Canvas)?;
                        if canvas == 0 {
                            return Ok(Some(ENOMEMORY));
                        }
                        self.canvases.insert(canvas, this);
                        if out != 0 {
                            self.cpu.write_u32(out, canvas)?;
                        }
                        return Ok(Some(SUCCESS));
                    }
                    // `ITransform` is another object, which draws **on this** bitmap.
                    AEEIID_TRANSFORM => {
                        let transform = self.new_object(Interface::Transform)?;
                        if transform == 0 {
                            return Ok(Some(ENOMEMORY));
                        }
                        self.transforms.insert(transform, this);
                        if out != 0 {
                            self.cpu.write_u32(out, transform)?;
                        }
                        return Ok(Some(SUCCESS));
                    }
                    _ => None,
                };
                match answer {
                    Some(pointer) => {
                        if out != 0 {
                            self.cpu.write_u32(out, pointer)?;
                        }
                        self.objects.add_ref(this);
                        SUCCESS
                    }
                    None => {
                        if out != 0 {
                            self.cpu.write_u32(out, 0)?;
                        }
                        self.unknown_classes.insert(requested);
                        ECLASSNOTSUPPORT
                    }
                }
            }
            // In our case NativeColor is the framebuffer's own RGB565.
            "RGBToNative" => Rgb::from_rgbval(self.cpu.read_reg(Reg::R1)).to_rgb565() as u32,
            "NativeToRGB" => {
                let native = self.cpu.read_reg(Reg::R1) as u16;
                to_rgbval(Rgb::from_rgb565(native))
            }
            // int DrawPixel(IBitmap *po, unsigned x, unsigned y, NativeColor c, AEERasterOp rop)
            "DrawPixel" => {
                let (x, y) = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let color = self.cpu.read_reg(Reg::R3) as u16;
                if let Some(fb) = self.bitmaps.get_mut(&this) {
                    fb.set_pixel_native(x, y, color);
                }
                // The game's buffer is the source of truth when it exists, so the pixel goes to
                // both places — and only the pixel, not the whole surface.
                if let Some(at) = self.dib_pixel(this, x, y) {
                    self.cpu.write_mem(at, &color.to_le_bytes())?;
                }
                SUCCESS
            }
            // int SetPixels(IBitmap *po, unsigned cnt, AEEPoint *pPoint, NativeColor color,
            //               AEERasterOp rop)
            //
            // `DrawPixel` in bulk: `cnt` points `{ int16 x; int16 y }`, all in the same color.
            // Zenonia uses this mid-game, and without it it stopped — on both engines, at the same
            // point, after almost 1,400 iterations.
            "SetPixels" => {
                let count = self.cpu.read_reg(Reg::R1);
                let points = self.cpu.read_reg(Reg::R2);
                let color = self.cpu.read_reg(Reg::R3) as u16;
                if points == 0 {
                    return Ok(Some(EBADPARM));
                }
                // An absurd count coming from the guest must not turn into a loop of billions.
                for i in 0..count.min(1 << 20) {
                    let mut point = [0u8; 4];
                    self.cpu.read_mem(points + i * 4, &mut point)?;
                    let x = i16::from_le_bytes([point[0], point[1]]) as i32;
                    let y = i16::from_le_bytes([point[2], point[3]]) as i32;
                    if let Some(fb) = self.bitmaps.get_mut(&this) {
                        fb.set_pixel_native(x, y, color);
                    }
                    if let Some(at) = self.dib_pixel(this, x, y) {
                        self.cpu.write_mem(at, &color.to_le_bytes())?;
                    }
                }
                SUCCESS
            }
            "GetPixel" => {
                let (x, y) = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                // From the game's buffer, when there is one: it may have written there directly.
                let value = match self.dib_pixel(this, x, y) {
                    Some(at) => {
                        let mut bytes = [0u8; 2];
                        self.cpu.read_mem(at, &mut bytes)?;
                        u16::from_le_bytes(bytes)
                    }
                    None => self
                        .bitmaps
                        .get(&this)
                        .map(|fb| fb.get_pixel(x, y))
                        .unwrap_or(0),
                };
                let out = self.cpu.read_reg(Reg::R3);
                if out != 0 {
                    self.cpu.write_u32(out, value as u32)?;
                }
                SUCCESS
            }
            // int DrawHScanline(IBitmap *po, unsigned y, unsigned xMin, unsigned xMax, ...)
            "DrawHScanline" => {
                let y = self.cpu.read_reg(Reg::R1) as i32;
                let x_min = self.cpu.read_reg(Reg::R2) as i32;
                let x_max = self.cpu.read_reg(Reg::R3) as i32;
                let color = self.stack_arg(0)? as u16;
                if let Some(fb) = self.bitmaps.get_mut(&this) {
                    for x in x_min..=x_max {
                        fb.set_pixel_native(x, y, color);
                    }
                }
                SUCCESS
            }
            // int FillRect(IBitmap *po, const AEERect *prc, NativeColor color, AEERasterOp rop)
            // int FillRect(IBitmap *po, const AEERect *prc, NativeColor color, AEERasterOp rop)
            //
            // `IBITMAP_FillRect.htm`: only `AEE_RO_COPY` and `AEE_RO_XOR` are valid; any other
            // operation is `EUNSUPPORTED` and **does not draw**. While we ignored `rop`, Bejeweled
            // Twist painted the whole screen black once per frame with an `AEE_RO_TRANSPARENT` the
            // console would have refused.
            "FillRect" => {
                let rop = self.cpu.read_reg(Reg::R3);
                let rect = self.read_rect(self.cpu.read_reg(Reg::R1))?;
                let color = self.cpu.read_reg(Reg::R2) as u16;
                // With `AEE_RO_TRANSPARENT`, filling with the transparent color itself writes
                // nothing. Ignoring `rop` was costly: Bejeweled Twist calls exactly
                // like that, with color zero, and the whole screen was erased once per frame.
                let transparent = self.transparency.get(&this).copied().unwrap_or(0);
                if rop == AEE_RO_TRANSPARENT && color == transparent {
                    return Ok(Some(SUCCESS));
                }
                if let (Some(rect), Some(fb)) = (rect, self.bitmaps.get_mut(&this)) {
                    // `AEE_RO_COPY` is the normal case; any other operation that gets here still
                    // paints, because refusing to draw is worse than painting too much.
                    if rop == AEE_RO_XOR {
                        fb.xor_rect_native(rect, color);
                    } else {
                        fb.fill_rect_native(rect, color);
                    }
                }
                SUCCESS
            }
            // int BltIn(IBitmap *po, int xDst, int yDst, int dx, int dy,
            //           IBitmap *pSrc, int xSrc, int ySrc, AEERasterOp rop)
            "BltIn" => {
                let dst = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let size = (self.cpu.read_reg(Reg::R3) as i32, self.stack_arg(0)? as i32);
                let src = self.stack_arg(1)?;
                let origin = (self.stack_arg(2)? as i32, self.stack_arg(3)? as i32);
                let rop = self.stack_arg(4)?;
                self.blit(this, dst, size, src, origin, rop);
                SUCCESS
            }
            // BltOut swaps the roles: the source is `po` and the destination comes in the argument.
            "BltOut" => {
                let dst_pos = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let size = (self.cpu.read_reg(Reg::R3) as i32, self.stack_arg(0)? as i32);
                let dst = self.stack_arg(1)?;
                let origin = (self.stack_arg(2)? as i32, self.stack_arg(3)? as i32);
                let rop = self.stack_arg(4)?;
                self.blit(dst, dst_pos, size, this, origin, rop);
                SUCCESS
            }
            // int GetInfo(IBitmap *po, AEEBitmapInfo *pinfo, int nSize)
            "GetInfo" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    let (cx, cy) = self
                        .bitmaps
                        .get(&this)
                        .map(|fb| (fb.width(), fb.height()))
                        .unwrap_or((0, 0));
                    self.cpu.write_u32(out, cx)?;
                    self.cpu.write_u32(out + 4, cy)?;
                    self.cpu.write_u32(out + 8, COLOR_DEPTH as u32)?;
                }
                SUCCESS
            }
            // int CreateCompatibleBitmap(IBitmap *po, IBitmap **ppIBitmap, uint16 w, uint16 h)
            "CreateCompatibleBitmap" => {
                let out = self.cpu.read_reg(Reg::R1);
                let width = self.cpu.read_reg(Reg::R2) & 0xffff;
                let height = self.cpu.read_reg(Reg::R3) & 0xffff;
                let addr = self.new_object(Interface::Bitmap)?;
                if addr == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                // **It is born with the transparent color, not black.** Toy Raid builds each
                // flipped sprite in a pair of compatible bitmaps: a transparent `BltIn` from the
                // atlas to the first, a mirrored copy through the `IDIB` to the second, and a
                // transparent `BitBlt` of the second onto the screen. The leftover background of
                // the first goes along, and it must be the key: born black, every right-facing
                // helicopter, plane and explosion came out inside a black rectangle.
                let mut create = Framebuffer::new(width, height);
                create.fill_rect_native(
                    Rect { x: 0, y: 0, width: width as i16, height: height as i16 },
                    TRANSPARENT_KEY,
                );
                self.bitmaps.insert(addr, create);
                // **The compatible bitmap is born an `IDIB`, with the public fields filled in.**
                // Zenonia creates its 320x240 canvas this way and reads `cx`, `cy` and `pBmp`
                // straight from the struct, without any `QueryInterface`. With the fields zeroed
                // the canvas had zero size: the game drew pixel by pixel through `SetPixels` — 197
                // thousand calls in thirty seconds — and presented an empty rectangle, with the
                // screen black.
                self.expose_dib(addr)?;
                if out != 0 {
                    self.cpu.write_u32(out, addr)?;
                }
                SUCCESS
            }
            "SetTransparencyColor" => {
                self.transparency
                    .insert(this, self.cpu.read_reg(Reg::R1) as u16);
                SUCCESS
            }
            "GetTransparencyColor" => {
                let value = self.transparency.get(&this).copied().unwrap_or(TRANSPARENT_KEY);
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    self.cpu.write_u32(out, value as u32)?;
                }
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Prepares a bitmap to be used as an `IDIB`: reserves the pixel buffer in guest memory,
    /// copies the current contents there and fills in the struct's public fields.
    ///
    /// Layout, from `inc/AEEIDIB.h`: `pvt`, `pPaletteMap`, `pBmp`, `pRGB`, `ncTransparent`,
    /// `cx`, `cy`, `nPitch`, `cntRGB`, `nDepth`, `nColorScheme` and six reserved bytes.
    pub(super) fn expose_dib(&mut self, bitmap: u32) -> Result<(), CpuError> {
        // The decoder's `IDIB` is already published, in its own format.
        if self.decoder_dib.contains_key(&bitmap) {
            return Ok(());
        }
        let Some(fb) = self.bitmaps.get(&bitmap) else {
            return Ok(());
        };
        let cy = fb.height();
        let needed = fb.dib_pitch() as u32 * cy;
        // **The header is rewritten every time, not only the first.** An object's address is
        // reused when the previous one is freed, and the new bitmap has another size: with the
        // check by address, a new image's `IDIB` kept announcing the size of the previous image.
        //
        // That is what broke Tekken 2's text. It decodes nine images in sequence, freeing each
        // one before the next — all of them were born at the same address. The `IDIB` said
        // 200x112 for all of them, so the game created a 200x112 page for a 360x280 sheet of
        // letters, copied only its corner and then asked for each glyph by coordinates in the
        // whole sheet. What fell outside became a block: the whole menu came out with the words
        // as orange rectangles.
        //
        // The buffer is reused when it fits. Reserving another one on each exposure also gets the
        // size right, but the surface region **does not recycle**: a game that decodes hundreds
        // of images would exhaust it.
        let fits = self
            .dib_capacity
            .get(&bitmap)
            .is_some_and(|&had| had >= needed);
        if !fits {
            // The buffer that no longer fits goes back to the region before asking for another.
            // The buffer is ours only with a recorded capacity; without it, it belongs to the game.
            if let (Some(&old), Some(_)) =
                (self.dib_buffers.get(&bitmap), self.dib_capacity.get(&bitmap))
            {
                self.release_surface(old);
            }
            let Some((buffer, capacity)) = self.reserve_surface(needed) else {
                self.dib_buffers.remove(&bitmap);
                self.dib_capacity.remove(&bitmap);
                return Ok(());
            };
            self.dib_buffers.insert(bitmap, buffer);
            self.dib_capacity.insert(bitmap, capacity);
            self.published_dib.remove(&bitmap);
            self.cpu.watch_dirty(bitmap, buffer, needed)?;
            // The first exposure needs to publish the current pixels. Later exposures only
            // update the header: rewriting the whole surface on every QueryInterface erased
            // changes made by the guest and cost tens of ms in games that query the bitmap
            // every frame.
            self.sync_to_guest(bitmap)?;
        } else if self.inherited_dibs.contains(&bitmap) {
            // The buffer fits, but it belongs to the object that died here: for this bitmap, this
            // **is** the first exposure.
            self.sync_to_guest(bitmap)?;
        }
        self.write_dib_header(bitmap)
    }

    /// Writes the public fields of a bitmap's `IDIB`: size, pitch, depth and the pointer to the
    /// pixels — the latter only when they already exist.
    ///
    /// A BREW software `IBitmap` **is** an `IDIB`: the struct starts with the `IBitmap` vtable
    /// and goes on with public fields, and the game reads those fields directly, without asking for
    /// anything. Peggle is the case: it decodes the PNG, asks for the size through the fields and
    /// prints `-size 0/0` in its log when it does not find it. Then it builds each sprite as a
    /// square with zero sides — 76,618 of the 77,208 triangles in a frame came out degenerate, and
    /// the screen stayed black with the game drawing the whole time.
    ///
    /// Outside the decoder, the pixels are still allocated only in `QueryInterface`, which is when
    /// the game declares it will touch them: the surface region does not recycle, and every
    /// published surface joins the loop that syncs the pixels on each call that touches them.
    /// Writing the header without the pixels would be worse than writing nothing — the game starts
    /// trusting `pBmp` and dereferences zero.
    pub(super) fn write_dib_header(&mut self, bitmap: u32) -> Result<(), CpuError> {
        let Some(fb) = self.bitmaps.get(&bitmap) else {
            return Ok(());
        };
        let (cx, cy) = (fb.width(), fb.height());
        let pitch = fb.dib_pitch() as u32;
        let buffer = self.dib_buffers.get(&bitmap).copied().unwrap_or(0);
        let transparent = self.transparency.get(&bitmap).copied().unwrap_or(TRANSPARENT_KEY) as u32;
        self.cpu.write_u32(bitmap + 4, 0)?; // pPaletteMap
        self.cpu.write_u32(bitmap + 8, buffer)?; // pBmp
        self.cpu.write_u32(bitmap + 12, 0)?; // pRGB: RGB565 has no palette
        self.cpu.write_u32(bitmap + 16, transparent)?;
        self.cpu
            .write_mem(bitmap + 20, &(cx as u16).to_le_bytes())?;
        self.cpu
            .write_mem(bitmap + 22, &(cy as u16).to_le_bytes())?;
        self.cpu
            .write_mem(bitmap + 24, &(pitch as i16).to_le_bytes())?;
        self.cpu.write_mem(bitmap + 26, &0u16.to_le_bytes())?; // cntRGB
        // `nDepth` in bits and `nColorScheme` with the code from `AEEIDIB.h`. Without the right
        // scheme the game does not know how to interpret the pixels and gives up.
        self.cpu
            .write_mem(bitmap + 28, &[COLOR_DEPTH as u8, IDIB_COLORSCHEME_565])?;
        self.cpu.write_mem(bitmap + 30, &[0u8; 6])?;
        Ok(())
    }

    /// Returns to the region the buffer of a bitmap that died.
    ///
    /// Only the buffers that **we** reserved go back — the ones with a recorded capacity. That of
    /// a game surface (its own `IDIB`) is its own memory.
    pub(super) fn release_dib(&mut self, bitmap: u32) {
        if let Some((buffer, _)) = self.decoder_dib.remove(&bitmap) {
            self.release_surface(buffer);
        }
        // Without a recorded capacity the buffer is not ours — it is the game's own surface.
        if self.dib_capacity.remove(&bitmap).is_none() {
            return;
        }
        if let Some(buffer) = self.dib_buffers.remove(&bitmap) {
            self.release_surface(buffer);
        }
        self.inherited_dibs.remove(&bitmap);
        self.published_dib.remove(&bitmap);
        self.cpu.unwatch_dirty(bitmap);
    }

    /// Returns a buffer to the surface region.
    ///
    /// **The surface region did not recycle, and the Z-Wheel exhausted it.** The tab bar creates
    /// one bitmap per frame, alternating between 440 and 441 wide, and each page of the list
    /// decodes new covers; measured, the 8 MB ran out after 27 s of browsing. From then on the
    /// message box's canvas had no pixels, the box did not open, and confirming a game crashed the
    /// Z-Wheel with a jump to address zero instead of launching it.
    ///
    /// Recycling only whole blocks was not enough either: without merging neighbors or splitting
    /// blocks, small holes did not serve large requests and the region ran out just the same, only
    /// more slowly. That is why it uses the same [`Heap`] as the game.
    pub(super) fn release_surface(&mut self, address: u32) {
        self.surfaces.free(address);
    }

    /// Reserves a buffer in the surface region. Returns address and capacity.
    pub(super) fn reserve_surface(&mut self, bytes: u32) -> Option<(u32, u32)> {
        let address = self.surfaces.alloc(bytes)?;
        Some((address, self.surfaces.size_of(address).unwrap_or(bytes)))
    }

    /// Copies the pixels from the host to the buffer the game sees.
    pub(super) fn sync_to_guest(&mut self, bitmap: u32) -> Result<(), CpuError> {
        let Some(&buffer) = self.dib_buffers.get(&bitmap) else {
            return Ok(());
        };
        let inherited = self.inherited_dibs.contains(&bitmap);
        let published = self.published_dib.get(&bitmap).copied();
        let Some(fb) = self.bitmaps.get_mut(&bitmap) else {
            return Ok(());
        };
        let generation = fb.generation();
        let dirty_rect = fb.take_dirty_rect();
        // **Only what changed goes to the game.** This surface has already been published whole,
        // and the buffer only fell behind in the rectangle we have drawn since. A sprite changes
        // a few thousand pixels; rewriting its 600 KB on every `IIMAGE_Draw` was 92% of
        // Pac-Mania's API time, and it draws 160 thousand sprites in five virtual seconds.
        //
        // The written span goes from the first pixel of the box to the last, including what lies
        // between the rows outside it: there both sides are already equal, because the import runs
        // before every draw, and one contiguous write is cheaper than one per row.
        if !inherited && published == Some(generation) {
            let Some([x0, y0, x1, y1]) = dirty_rect else {
                return Ok(());
            };
            let width = fb.width() as usize;
            let step = fb.dib_pitch();
            if step == width * 2 {
                let start = y0 as usize * width + x0 as usize;
                let end = (y1 as usize - 1) * width + x1 as usize;
                let bytes = fb.rgb565_range(start, end);
                self.cpu.write_mem(buffer + start as u32 * 2, &bytes)?;
            } else {
                // With padding at the end of the row, the contiguous span does not match the
                // buffer: go row by row.
                for line in y0 as usize..y1 as usize {
                    let start = line * width + x0 as usize;
                    let bytes = fb.rgb565_range(start, line * width + x1 as usize);
                    let dest = line * step + x0 as usize * 2;
                    self.cpu.write_mem(buffer + dest as u32, &bytes)?;
                }
            }
        } else {
            let bytes = fb.to_dib_bytes();
            self.cpu.write_mem(buffer, &bytes)?;
        }
        self.inherited_dibs.remove(&bitmap);
        self.published_dib.insert(bitmap, generation);
        // The guest's surface just became **identical** to ours — and it was the write above that
        // set the flag. Clearing it here is what allows skipping the next import: without
        // this every exit would dirty everything again and the watch would save nothing.
        self.cpu.take_dirty(bitmap);
        Ok(())
    }

    /// Brings back what the game wrote directly into the buffer.
    /// The address of a pixel inside the buffer the game sees, when it exists.
    ///
    /// It is what lets `DrawPixel` and `GetPixel` touch two bytes instead of shipping the
    /// whole surface from one side to the other.
    pub(super) fn dib_pixel(&self, bitmap: u32, x: i32, y: i32) -> Option<u32> {
        let &buffer = self.dib_buffers.get(&bitmap)?;
        let fb = self.bitmaps.get(&bitmap)?;
        let (width, height) = (fb.width() as i32, fb.height() as i32);
        if x < 0 || y < 0 || x >= width || y >= height {
            return None;
        }
        Some(buffer + (y as usize * fb.dib_pitch()) as u32 + x as u32 * 2)
    }

    pub(super) fn sync_from_guest(&mut self, bitmap: u32) -> Result<(), CpuError> {
        let Some(&buffer) = self.dib_buffers.get(&bitmap) else {
            return Ok(());
        };
        let Some(fb) = self.bitmaps.get(&bitmap) else {
            return Ok(());
        };
        let size = fb.dib_pitch() * fb.height() as usize;
        // Bytes of the previous object: the host is ahead, and there is nothing from the game to
        // bring.
        if self.inherited_dibs.contains(&bitmap) {
            return Ok(());
        }
        // **It only reads when the guest wrote to this surface.** It is the same saving the
        // pbuffer's color buffer already had, now per surface: the Z-Wheel copied 1.57 GB in
        // thirteen virtual seconds only to find out that almost nothing had changed.
        if !self.cpu.take_dirty(bitmap) {
            return Ok(());
        }
        let mut bytes = vec![0u8; size];
        self.cpu.read_mem(buffer, &mut bytes)?;
        if let Some(fb) = self.bitmaps.get_mut(&bitmap) {
            fb.load_dib_bytes(&bytes);
            // We just copied the whole buffer over our copy: both sides are equal, and nothing we
            // drew before still needs to go to the game.
            fb.take_dirty_rect();
            self.published_dib.insert(bitmap, fb.generation());
        }
        Ok(())
    }

    /// Syncs every surface the game may have changed directly.
    ///
    /// Called only from the interfaces that touch pixels: a game makes tens of thousands of
    /// calls to other APIs, and copying 600 KB on each one would be unworkable.
    pub(super) fn sync_surfaces_in(&mut self) -> Result<(), CpuError> {
        for bitmap in self.dib_buffers.keys().copied().collect::<Vec<_>>() {
            self.sync_from_guest(bitmap)?;
        }
        Ok(())
    }

    pub(super) fn sync_surfaces_out(&mut self) -> Result<(), CpuError> {
        for bitmap in self.dib_buffers.keys().copied().collect::<Vec<_>>() {
            self.sync_to_guest(bitmap)?;
        }
        Ok(())
    }

    /// Copies from one surface to another, honoring the transparent color when the raster op
    /// asks for it. It needs to take the destination out of the map first so as not to hold two
    /// mutable references.
    pub(super) fn blit(
        &mut self,
        dst: u32,
        dst_pos: (i32, i32),
        size: (i32, i32),
        src: u32,
        src_pos: (i32, i32),
        rop: u32,
    ) {
        if dst == src {
            return;
        }
        let Some(mut target) = self.bitmaps.remove(&dst) else {
            return;
        };
        if let Some(source) = self.bitmaps.get(&src) {
            let transparent = if rop == AEE_RO_TRANSPARENT {
                // With no color requested, BREW's applies: `RGB_MASK_COLOR`, magenta. Action Hero
                // 3D draws each letter with `AEE_RO_TRANSPARENT` from a sheet with a magenta
                // background without ever calling `SetTransparencyColor`; without the default, the
                // whole sheet showed up on screen with every letter.
                Some(self.transparency.get(&src).copied().unwrap_or(TRANSPARENT_KEY))
            } else {
                None
            };
            target.blit(
                dst_pos.0,
                dst_pos.1,
                size.0,
                size.1,
                source,
                src_pos.0,
                src_pos.1,
                transparent,
            );
        }
        self.bitmaps.insert(dst, target);
    }

    /// Reads an `AEERect` from guest memory. `None` when the pointer is null.
    pub(super) fn clip_rect(&self, rect: Rect) -> Option<Rect> {
        clip_rect(self.clip, rect)
    }

    pub(super) fn clip_blit(
        &self,
        dst: (i32, i32),
        size: (i32, i32),
        src: (i32, i32),
    ) -> Option<Blit> {
        clip_blit(self.clip, dst, size, src)
    }

    pub(super) fn read_rect(&self, addr: u32) -> Result<Option<Rect>, CpuError> {
        if addr == 0 {
            return Ok(None);
        }
        let mut bytes = [0u8; 8];
        self.cpu.read_mem(addr, &mut bytes)?;
        Ok(Some(Rect::from_bytes(bytes)))
    }

    /// Every live surface as a BMP, for inspection: address, size and bytes.
    ///
    /// It exists because "the game draws and the screen stays black" has two possible causes, and
    /// only the contents of the surfaces tell them apart: either the game drew on something that
    /// does not go to the screen, or it did not draw. Finding that out through temporary
    /// instrumentation cost two investigations.
    pub fn surfaces(&self) -> Vec<(u32, u32, u32, Vec<u8>)> {
        self.bitmaps
            .iter()
            .map(|(&addr, fb)| (addr, fb.width(), fb.height(), fb.to_bmp()))
            .collect()
    }
}

/// The arguments of a `TransformBltComplex`, already read from the guest.
struct TransformBlt {
    dest: u32,
    origin: u32,
    x: i32,
    y: i32,
    source_x: i32,
    source_y: i32,
    width: i32,
    height: i32,
    /// `A, B, C, D`, already out of fixed point.
    m: [f32; 4],
}
