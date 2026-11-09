//! IImage and IImageDecoder: decoding what the game brings and drawing it on screen.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `IImageDecoder` and the `IForceFeed` that feeds it.
    ///
    /// The game creates the decoder, asks it for the input interface, writes the file in chunks,
    /// closes with an empty write and fetches the bitmap. It is the path Heavy Weapon, Tork and
    /// Kral and Peggle use for their images.
    pub(super) fn decoder_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        // An `IForceFeed` always works on the decoder that created it.
        let decoder = match iface {
            Interface::ForceFeed => self.feeds.get(&this).copied().unwrap_or(0),
            _ => this,
        };
        let result = match (iface, name) {
            (_, "AddRef") => self.objects.add_ref(this),
            (_, "Release") => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.feeds.remove(&this);
                    self.decoders.remove(&this);
                }
                remaining
            }
            (_, "QueryInterface") => {
                if a2 == 0 {
                    return Ok(Some(EBADPARM));
                }
                match a1 {
                    AEEIID_FORCEFEED => {
                        let feed = self.new_object(Interface::ForceFeed)?;
                        if feed == 0 {
                            return Ok(Some(ENOMEMORY));
                        }
                        self.feeds.insert(feed, decoder);
                        self.cpu.write_u32(a2, feed)?;
                        SUCCESS
                    }
                    _ => {
                        self.unknown_classes.insert(a1);
                        self.cpu.write_u32(a2, 0)?;
                        ECLASSNOTSUPPORT
                    }
                }
            }
            // int Write(IForceFeed *, void *pBuf, int cb)
            //
            // An empty write is the end of the file — that is how the SDK sample closes the
            // delivery. Nothing to do here: decoding happens in `GetBitmap`, and doing it early
            // would only waste work if the game gave up halfway.
            (Interface::ForceFeed, "Write") => {
                // The size comes from the game: checked **before** allocating and **before**
                // accumulating, or an absurd request aborts the process instead of becoming an API
                // error. The cap on what was already delivered is checked in the same step: before,
                // the excess was detected after the memory had already been spent.
                let count = guest_size(a2 as usize)?;
                if a1 != 0 && count > 0 {
                    let already_fed = self.decoders.get(&decoder).map_or(0, |state| state.fed.len());
                    if already_fed + count > MAX_DECODED_INPUT {
                        return Err(CpuError(format!(
                            "image delivery would exceed {MAX_DECODED_INPUT} bytes: {already_fed} + {count}"
                        )));
                    }
                    let mut bytes = vec![0u8; count];
                    self.cpu.read_mem(a1, &mut bytes)?;
                    let state = self.decoders.entry(decoder).or_default();
                    if state.fed.len() + count <= MAX_DECODED_INPUT {
                        state.fed.extend_from_slice(&bytes);
                    }
                }
                SUCCESS
            }
            (Interface::ForceFeed, "Reset") => {
                self.decoders.remove(&decoder);
                SUCCESS
            }
            // int GetBitmap(IImageDecoder *, IBitmap **ppiBitmap)
            (Interface::ImageDecoder, "GetBitmap") => {
                let bitmap = self.decoded_bitmap(decoder)?;
                if a1 != 0 {
                    self.cpu.write_u32(a1, bitmap)?;
                }
                match bitmap {
                    0 => EFAILED,
                    _ => SUCCESS,
                }
            }
            // int GetRop(IImageDecoder *) — how to draw the returned bitmap.
            (Interface::ImageDecoder, "GetRop") => {
                self.decoded_bitmap(decoder)?;
                match self.decoders.get(&decoder).is_some_and(|d| d.transparent) {
                    true => AEE_RO_TRANSPARENT,
                    false => AEE_RO_COPY,
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// A decoder's bitmap, decoding it the first time it is requested.
    pub(super) fn decoded_bitmap(&mut self, decoder: u32) -> Result<u32, CpuError> {
        if let Some(state) = self.decoders.get(&decoder) {
            if let Some(bitmap) = state.bitmap {
                return Ok(bitmap);
            }
        }
        let Some(fed) = self.decoders.get(&decoder).map(|d| d.fed.clone()) else {
            return Ok(0);
        };
        let Some(image) = decode_fed_image(&fed) else {
            // The assumption names what the decoder did **not** recognize, and no longer "it is
            // not a PNG": it now tries at least three formats by signature.
            self.assumptions.insert(concat!(
                "a decoder received data that is not PNG, BMP or JPEG"
            ));
            return Ok(0);
        };
        let addr = self.bitmap_from_decoded(&image)?;
        if addr == 0 {
            return Ok(0);
        }
        self.publish_png_dib(addr, &fed)?;
        let transparent = self.transparency.contains_key(&addr);
        if let Some(state) = self.decoders.get_mut(&decoder) {
            state.bitmap = Some(addr);
            state.transparent = transparent;
        }
        Ok(addr)
    }

    /// Publishes the `IDIB` of a decoder bitmap in the PNG's format: 24 bits per pixel in RGB, or
    /// 32 in RGBA when the image has alpha, contiguous rows with no padding.
    ///
    /// That is what the BREW decoder delivers, and Alien Breaker Deluxe proves it by use: it reads
    /// `nDepth`, treats 8 bits as a palette and, otherwise, copies `nDepth / 8` bytes per pixel
    /// straight into a `GL_RGB` (3) or `GL_RGBA` texture. With our RGB565 it copied two bytes per
    /// pixel thinking they were four, and the logos came out as white squares.
    ///
    /// The RGBA's `nColorScheme` is also 888, not `IDIB_COLORSCHEME_NONE`: in `AEEIDIB.h` zero
    /// means **palette**. Bejeweled Twist's `BltIn` (0x139d8) decides by it — zero goes to the
    /// 8-bit indexed conversion, 24 goes to the truecolor one, which looks at `nDepth` and only
    /// with 32 creates the alpha surface. Published with zero, the RGBA was read as palette
    /// indices, the source surface came out with no format and the blit hit a null pointer.
    ///
    /// Our RGB565 copy stays in the surface map for blits; this buffer does not take part in
    /// synchronization.
    pub(super) fn publish_png_dib(&mut self, bitmap: u32, png: &[u8]) -> Result<(), CpuError> {
        let Some((width, height, channels, mut bytes)) = decode_dib_bytes(png) else {
            return Ok(());
        };
        // **Blue in the first byte.** BREW's 888 is `0x00RRGGBB` in little-endian, and a game that
        // uploads the DIB to GL swaps the channels on its own. Peggle does that: with R,G,B here,
        // its swap made the logo blue and the sky pink instead of orange and purple.
        for pixel in bytes.chunks_exact_mut(channels) {
            pixel.swap(0, 2);
        }
        // A buffer published earlier by another path is no longer usable: the format is
        // different.
        self.release_dib(bitmap);
        let Some((buffer, capacity)) = self.reserve_surface(bytes.len() as u32) else {
            return Ok(());
        };
        self.cpu.write_mem(buffer, &bytes)?;
        self.decoder_dib.insert(bitmap, (buffer, capacity));
        let step = width as usize * channels;
        let depth = if channels == 4 { 32u8 } else { 24u8 };
        let color_scheme = IDIB_COLORSCHEME_888;
        self.cpu.write_u32(bitmap + 4, 0)?; // pPaletteMap
        self.cpu.write_u32(bitmap + 8, buffer)?; // pBmp
        self.cpu.write_u32(bitmap + 12, 0)?; // pRGB
        self.cpu
            .write_u32(bitmap + 16, to_rgbval(Rgb::from_rgb565(TRANSPARENT_KEY)))?; // ncTransparent
        self.cpu.write_mem(bitmap + 20, &(width as u16).to_le_bytes())?;
        self.cpu.write_mem(bitmap + 22, &(height as u16).to_le_bytes())?;
        self.cpu.write_mem(bitmap + 24, &(step as i16).to_le_bytes())?;
        self.cpu.write_mem(bitmap + 26, &0u16.to_le_bytes())?; // cntRGB
        self.cpu.write_mem(bitmap + 28, &[depth, color_scheme])?;
        self.cpu.write_mem(bitmap + 30, &[0u8; 6])?;
        Ok(())
    }

    /// An `IBitmap` with the already decoded image inside.
    ///
    /// It comes out with the public `IDIB` fields filled in, because a BREW software `IBitmap`
    /// is an `IDIB` and the game reads those fields without asking for the interface.
    pub(super) fn bitmap_from_decoded(&mut self, image: &DecodedImage) -> Result<u32, CpuError> {
        let addr = self.new_object(Interface::Bitmap)?;
        if addr == 0 {
            return Ok(0);
        }
        let mut surface = Framebuffer::new(image.width, image.height);
        let mut transparent = false;
        for row in 0..image.height {
            for column in 0..image.width {
                let index = (row * image.width + column) as usize;
                let opaque = image.opaque.get(index).copied().unwrap_or(true);
                transparent |= !opaque;
                // With no alpha channel in the destination, transparent becomes the reserved
                // color — that is how BREW solves it, and it is what `GetRop` then announces to
                // the game.
                let pixel = match (opaque, image.pixels.get(index)) {
                    (true, Some(&pixel)) => pixel,
                    _ => TRANSPARENT_KEY,
                };
                surface.set_pixel_native(column as i32, row as i32, pixel);
            }
        }
        self.bitmaps.insert(addr, surface);
        if transparent {
            self.transparency.insert(addr, TRANSPARENT_KEY);
        }
        self.expose_dib(addr)?;
        Ok(addr)
    }

    /// Decodes the image in `buffer` and returns an `IBitmap` holding it.
    ///
    /// The returned pointer goes straight to `IDISPLAY_BitBlt`, which takes an `IBitmap *` — and
    /// that is why the "native format" here is one of our bitmaps, not a loose block of pixels:
    /// that way drawing follows the same path as everything else.
    ///
    /// The block size does not come as a parameter: what says how much to read is the image's
    /// own header, and for now only BMP — which is what the games pass — declares its own.
    pub(super) fn setup_native_image(
        &mut self,
        buffer: u32,
        info: u32,
        realloc: u32,
    ) -> Result<u32, CpuError> {
        if realloc != 0 {
            // The image comes out in an allocation of ours, and that is what this flag reports.
            self.cpu.write_mem(realloc, &[1])?;
        }
        let Some(len) = self.encoded_image_len(buffer)? else {
            return Ok(0);
        };
        let mut bytes = vec![0u8; len];
        self.cpu.read_mem(buffer, &mut bytes)?;
        let Ok(image) = crate::video::icon::decode(&bytes) else {
            self.assumptions
                .insert("a native image came in a format we cannot read");
            return Ok(0);
        };

        let addr = self.new_object(Interface::Bitmap)?;
        if addr == 0 {
            return Ok(0);
        }
        let (width, height) = (image.width as u32, image.height as u32);
        let mut fb = Framebuffer::new(width, height);
        for y in 0..image.height {
            for x in 0..image.width {
                let at = (y * image.width + x) * 4;
                let color = Rgb {
                    r: image.rgba[at],
                    g: image.rgba[at + 1],
                    b: image.rgba[at + 2],
                };
                fb.set_pixel_native(x as i32, y as i32, color.to_rgb565());
            }
        }
        self.bitmaps.insert(addr, fb);

        if info != 0 {
            let (cx, cy) = (width as u16, height as u16);
            self.cpu.write_mem(info, &cx.to_le_bytes())?;
            self.cpu.write_mem(info + 2, &cy.to_le_bytes())?;
            // `nColors` is zero for anything with more than 65535 colors, and `bAnimated` is
            // false.
            self.cpu.write_mem(info + 4, &[0u8; 4])?;
            self.cpu.write_mem(info + 8, &cx.to_le_bytes())?;
        }
        Ok(addr)
    }

    /// How much to read from an image block, according to its header.
    pub(super) fn encoded_image_len(&self, buffer: u32) -> Result<Option<usize>, CpuError> {
        if buffer == 0 {
            return Ok(None);
        }
        let mut header = [0u8; 6];
        self.cpu.read_mem(buffer, &mut header)?;
        // BMP declares the file size in the word following the signature.
        if &header[0..2] != b"BM" {
            return Ok(None);
        }
        let size = u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as usize;
        Ok((size > 0 && size <= MAX_NATIVE_IMAGE).then_some(size))
    }

    /// `IImage` over the PNG decoder (`AEECLSID_PNG` = `0x01004004`), from
    /// `inc/AEEIImage.h`.
    ///
    /// The game feeds the image with an `IMemAStream` (`SetStream`), reads the dimensions with
    /// `GetInfo` and draws with `Draw`. The decoding itself is standard PNG — there is nothing
    /// proprietary here, contrary to what the high-entropy bytes of `resources.dat` suggested at
    /// first sight.
    pub(super) fn image_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Image.method(slot) else {
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
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.images.remove(&this);
                    self.image_bitmaps.remove(&this);
                    self.image_clips.remove(&this);
                    // The address goes back to belonging to another object, which must not
                    // inherit the callback.
                    self.image_notify.remove(&this);
                    self.image_notify_queue.retain(|&queued_image| queued_image != this);
                    if let Some(info) = self.image_info.remove(&this) {
                        self.free(info);
                    }
                }
                remaining
            }
            // void SetStream(IImage *, IAStream *ps) — consumes the whole stream at once.
            "SetStream" => {
                self.decode_image(this, a1)?;
                SUCCESS
            }
            // void GetInfo(IImage *, AEEImageInfo *pi): cx, cy, nColors (uint16), bAnimated
            // (boolean) and cxFrame (uint16).
            "GetInfo" => {
                let (cx, cy, frame) = match self.images.get(&this) {
                    Some(image) => (image.width as u16, image.height as u16, image.frame_width),
                    None => (0, 0, 0),
                };
                if a1 != 0 {
                    self.cpu.write_mem(a1, &cx.to_le_bytes())?;
                    self.cpu.write_mem(a1 + 2, &cy.to_le_bytes())?;
                    // `nColors` is zero for images with more than 65535 colors, which is the case.
                    self.cpu.write_mem(a1 + 4, &0u16.to_le_bytes())?;
                    self.cpu.write_mem(a1 + 6, &[0u8, 0])?;
                    self.cpu.write_mem(a1 + 8, &frame.to_le_bytes())?;
                }
                SUCCESS
            }
            // void SetParm(IImage *, int nParm, int p1, int p2)
            "SetParm" => {
                self.image_set_parm(this, a1, a2, a3)?;
                SUCCESS
            }
            "Draw" => {
                self.draw_image(this, a1 as i32, a2 as i32, None)?;
                SUCCESS
            }
            // void DrawFrame(IImage *, int nFrame, int x, int y)
            "DrawFrame" => {
                self.draw_image(this, a2 as i32, a3 as i32, Some(a1))?;
                SUCCESS
            }
            // Without animation, `Start` is a `Draw` and `Stop` has nothing to stop.
            "Start" => {
                self.draw_image(this, a1 as i32, a2 as i32, None)?;
                SUCCESS
            }
            "Stop" | "HandleEvent" => SUCCESS,
            // void Notify(IImage *, PFNIMAGEINFO pfn, void *pUser)
            //
            "Notify" => {
                if a1 != 0 {
                    self.image_notify.insert(
                        this,
                        Callback {
                            function: a1,
                            context: a2,
                        },
                    );
                    // An image that came from `LoadResObject` is already ready when the game
                    // registers the callback: the notification is immediate, it does not wait
                    // for any stream.
                    if self.images.contains_key(&this) {
                        self.notify_image(this)?;
                    }
                } else {
                    // Without a function, `Notify` cancels: the notification still in the queue
                    // does not go out.
                    self.image_notify.remove(&this);
                    self.image_notify_queue.retain(|&queued_image| queued_image != this);
                }
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Reads the whole stream and decodes the image, whatever its format.
    pub(super) fn decode_image(&mut self, image: u32, stream: u32) -> Result<(), CpuError> {
        let Some(source) = self.streams.get(&stream).copied() else {
            return Ok(());
        };
        let bytes = self.read_bytes(source.buffer, source.size)?;
        match decode_image_bytes(&bytes) {
            Some(decoded) => {
                self.images.insert(image, std::rc::Rc::new(decoded));
            }
            None => {
                self.assumptions
                    .insert("a game image was rejected by the decoder");
            }
        }
        self.notify_image(image)
    }

    /// Queues the image's `PFNIMAGEINFO(pUser, IImage *, AEEImageInfo *, int nErr)`.
    ///
    /// The image is already ready when the stream arrives — there is no background decoding
    /// here —, but the game waits for the notification to keep loading.
    ///
    /// **The notification goes out on the next turn of the event loop, not on return from
    /// `SetStream`** — as in BREW, which delivers through the loop after the game's handler
    /// returns control. Bejeweled Twist relies on that order: it registers `Notify`, calls
    /// `SetStream` and, still in the same handler, asks for `GetInfo` and stores the size in the
    /// object that mirrors the image (0x68010). Only in the notification does it draw the image
    /// on the surface and mirror it (0x7c530), with the stored width. Delivered on return from
    /// `SetStream`, the notification arrived before `GetInfo`: the 81 mirrors done up to the menu
    /// ran with zero width, and every half-piece of the interface — the right end of the
    /// buttons, the right half of the menu rings — showed up unmirrored, same as the left one.
    ///
    /// **The queue holds only the image; the notification is built at delivery.** Until then the
    /// game may release the image or swap the callback, and in BREW releasing the image cancels
    /// the notification. Keeping the notification prebuilt handed the handler a dead object and
    /// the `AEEImageInfo` that `Release` had already returned to the heap: in Bejeweled Twist,
    /// pressing "Play" took the game to a garbage pointer, and it stopped reading 0x006f0092 at
    /// 0x1c780.
    pub(super) fn notify_image(&mut self, image: u32) -> Result<(), CpuError> {
        if self.image_notify.contains_key(&image) && !self.image_notify_queue.contains(&image) {
            self.image_notify_queue.push(image);
        }
        Ok(())
    }

    /// Delivers the notification for a queued image, if anyone is still waiting for it.
    pub(super) fn deliver_image_notify(
        &mut self,
        image: u32,
        budget: u64,
    ) -> Result<Option<Outcome>, CpuError> {
        let Some(&callback) = self.image_notify.get(&image) else {
            return Ok(None);
        };
        let decoded = self.images.get(&image).cloned();
        // `AEEImageInfo` is 10 bytes; we allocate 12 to keep alignment. One block per image,
        // reused on every notification and returned on `Release`: a new block per notification
        // never went back to the heap.
        let info = match self.image_info.get(&image) {
            Some(&info) => info,
            None => {
                let info = self.alloc_internal(12)?;
                if info != 0 {
                    self.image_info.insert(image, info);
                }
                info
            }
        };
        if info != 0 {
            self.cpu.write_mem(info, &[0u8; 12])?;
            if let Some(image) = &decoded {
                self.cpu
                    .write_mem(info, &(image.width as u16).to_le_bytes())?;
                self.cpu
                    .write_mem(info + 2, &(image.height as u16).to_le_bytes())?;
                self.cpu
                    .write_mem(info + 8, &image.frame_width.to_le_bytes())?;
            }
        }
        let error = if decoded.is_some() { SUCCESS } else { EFAILED };
        let outcome = self.call_guest(
            callback.function,
            [callback.context, image, info, error],
            budget,
        )?;
        Ok(Some(outcome))
    }

    /// `IPARM_*` from `inc/AEEIImage.h`. We only respond to the ones that change drawing.
    pub(super) fn image_set_parm(
        &mut self,
        image: u32,
        parm: u32,
        p1: u32,
        p2: u32,
    ) -> Result<(), CpuError> {
        match parm {
            IPARM_SIZE => {
                self.image_clips.entry(image).or_default().size =
                    Some((p1 as i32, p2 as i32));
            }
            IPARM_OFFSET => {
                let crop = self.image_clips.entry(image).or_default();
                crop.x = p1 as i32;
                crop.y = p2 as i32;
            }
            IPARM_ROP => {
                self.image_clips.entry(image).or_default().transparent =
                    p1 == AEE_RO_TRANSPARENT;
            }
            IPARM_CXFRAME => {
                if let Some(info) = self.images.get_mut(&image) {
                    std::rc::Rc::make_mut(info).frame_width = p1 as u16;
                }
            }
            IPARM_NFRAMES => {
                if let Some(info) = self.images.get_mut(&image) {
                    let frames = (p1 as u16).max(1);
                    let width = info.width as u16;
                    std::rc::Rc::make_mut(info).frame_width = width / frames;
                }
            }
            // p1 = pointer to receive the `IBitmap *`, p2 = pointer to the return code.
            IPARM_GETBITMAP => {
                let bitmap = self.bitmap_from_image(image)?;
                if p1 != 0 {
                    self.cpu.write_u32(p1, bitmap)?;
                }
                if p2 != 0 {
                    self.cpu
                        .write_u32(p2, if bitmap == 0 { EFAILED } else { SUCCESS })?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Materializes the decoded image as an `IBitmap` surface.
    /// Unlike [`Machine::bitmap_from_decoded`], this path does **not** publish the `IDIB`.
    ///
    /// Publishing is expensive where it is not needed: every published surface enters the loop
    /// that synchronizes the pixels on every call that touches them, and Pac-Mania — which draws
    /// pixel by pixel through the API — went from "runs" to "far too slow" when the two paths
    /// were unified. Whoever wants the DIB asks for it through `QueryInterface`, and then it is
    /// published.
    pub(super) fn bitmap_from_image(&mut self, image: u32) -> Result<u32, CpuError> {
        // The same image is requested again every frame — Pac-Mania does 34 thousand
        // `IPARM_GETBITMAP` in four virtual seconds. Materializing a new surface on every
        // request took seven seconds and left thirty-four thousand objects alive.
        if let Some(&bitmap) = self.image_bitmaps.get(&image) {
            return Ok(bitmap);
        }
        let Some(info) = self.images.get(&image).cloned() else {
            return Ok(0);
        };
        let addr = self.new_object(Interface::Bitmap)?;
        if addr == 0 {
            return Ok(0);
        }
        let mut surface = Framebuffer::new(info.width, info.height);
        for (index, pixel) in info.pixels.iter().enumerate() {
            let (x, y) = (index as u32 % info.width, index as u32 / info.width);
            surface.set_pixel_native(x as i32, y as i32, *pixel);
        }
        self.bitmaps.insert(addr, surface);
        self.image_bitmaps.insert(image, addr);
        Ok(addr)
    }

    /// Draws the image on the current surface.
    ///
    /// When the destination is one of the game's own surfaces, the drawing cannot be done here:
    /// it becomes a call to that surface's `BltIn`, and entering the guest in the middle of
    /// dispatch is not safe. It goes to the boundary queue, like the callbacks.
    pub(super) fn draw_image(
        &mut self,
        image: u32,
        x: i32,
        y: i32,
        frame: Option<u32>,
    ) -> Result<(), CpuError> {
        if !self.images.contains_key(&image) {
            return Ok(());
        }
        let target = self.target()?;
        let crop = self.image_clips.get(&image).copied().unwrap_or_default();
        if !self.bitmaps.contains_key(&target) {
            let Some(info) = self.images.get(&image) else {
                return Ok(());
            };
            let frame_width = match (frame, info.frame_width) {
                (Some(_), width) if width > 0 => width as i32,
                _ => info.width as i32,
            };
            let (src_x, src_y) = (crop.x.max(0), crop.y.max(0));
            let (crop_width, crop_height) = crop.size.unwrap_or((i32::MAX, i32::MAX));
            let mut first_column = 0;
            let mut last_column = crop_width.min(frame_width - src_x);
            let mut first_row = 0;
            let mut last_row = crop_height.min(info.height as i32 - src_y);
            if let Some(clip) = self.clip {
                first_column = first_column.max(clip.x as i32 - x);
                last_column = last_column.min(clip.x as i32 + clip.width as i32 - x);
                first_row = first_row.max(clip.y as i32 - y);
                last_row = last_row.min(clip.y as i32 + clip.height as i32 - y);
            }
            if first_column >= last_column || first_row >= last_row {
                return Ok(());
            }
            self.pending_blits.push(PendingBlit {
                image,
                target,
                x: x + first_column,
                y: y + first_row,
                src_x: src_x + first_column,
                src_y: src_y + first_row,
                width: (last_column - first_column) as u32,
                height: (last_row - first_row) as u32,
                rop: match crop.transparent {
                    true => AEE_RO_TRANSPARENT,
                    false => AEE_RO_COPY,
                },
                frame,
            });
            return Ok(());
        }
        let clip = self.clip;
        // **The image is not copied to be read.** Reading the image map and writing to the
        // surface map are different fields of `self`, and splitting them here is what lets the
        // borrow through without a copy.
        //
        // With the `.cloned()` that used to be on this line, every `IIMAGE_Draw` duplicated the
        // whole image — pixels and opacity mask — just to read a rectangle of it. In Pac-Mania
        // that is 21,923 calls in five virtual seconds, at 0.6 ms each: 90% of all the game's API
        // time was in that copy, and not in the loop that clipping had already reduced.
        let Self {
            images, bitmaps, ..
        } = self;
        let Some(info) = images.get(&image) else {
            return Ok(());
        };
        let Some(surface) = bitmaps.get_mut(&target) else {
            return Ok(());
        };
        let (frame_width, offset) = match (frame, info.frame_width) {
            (Some(n), width) if width > 0 => (width as u32, n * width as u32),
            _ => (info.width, 0),
        };
        // Clipping is not a finishing touch here: it is what decides the size of the work.
        // Pac-Mania draws the **whole font sheet** and relies on the clip so that only the letter
        // shows. Walking the whole image and checking pixel by pixel meant 3.9 billion pixels read
        // in four virtual seconds to put a few hundred thousand on screen — and it still put on
        // screen what the game had told it to hide.
        //
        // The piece requested by `IPARM_OFFSET` and `IPARM_SIZE` comes before everything:
        // `(column, row)` is still the position **on screen** from `(x, y)`, and the pixel read is
        // offset by the piece's corner.
        let (clip_x, clip_y) = (crop.x.max(0), crop.y.max(0));
        let (crop_width, crop_height) = crop.size.unwrap_or((i32::MAX, i32::MAX));
        let (mut first_column, mut last_column) =
            (0, crop_width.min(frame_width as i32 - clip_x));
        let (mut first_row, mut last_row) = (0, crop_height.min(info.height as i32 - clip_y));
        if let Some(clip) = clip {
            first_column = first_column.max(clip.x as i32 - x);
            last_column = last_column.min(clip.x as i32 + clip.width as i32 - x);
            first_row = first_row.max(clip.y as i32 - y);
            last_row = last_row.min(clip.y as i32 + clip.height as i32 - y);
        }
        // The same goes for the surface edges: whatever falls outside never needed to be read.
        first_column = first_column.max(-x).max(0);
        last_column = last_column.min(surface.width() as i32 - x);
        first_row = first_row.max(-y).max(0);
        last_row = last_row.min(surface.height() as i32 - y);

        for row in first_row..last_row {
            for column in first_column..last_column {
                let source = ((row + clip_y) as u32 * info.width
                    + (column + clip_x) as u32
                    + offset) as usize;
                let Some(&pixel) = info.pixels.get(source) else {
                    continue;
                };
                if crop.transparent && pixel == TRANSPARENT_KEY {
                    continue;
                }
                match info.alpha.get(source).copied() {
                    Some(0) => {}
                    Some(u8::MAX) => surface.set_pixel_native(x + column, y + row, pixel),
                    Some(alpha) => {
                        let background = surface.get_pixel(x + column, y + row);
                        let color = blend_rgb565(background, pixel, alpha);
                        surface.set_pixel_native(x + column, y + row, color);
                    }
                    None if info.opaque.get(source).copied().unwrap_or(true) => {
                        surface.set_pixel_native(x + column, y + row, pixel)
                    }
                    None => {}
                }
            }
        }
        Ok(())
    }
}
