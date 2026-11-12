//! IDisplay and IGraphics: 2D drawing and text on the screen.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `IDisplay` methods. We dispatch by the slot's **name**, not by its number: the name table
    /// comes from the SDK headers, so a method out of order would become a compile error here
    /// instead of a wrong drawing there.
    pub(super) fn display_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        // **The OpenGL frame comes before the 2D.** A HUD drawn over a screen that has not
        // received the scene yet would erase the scene — see [`Machine::materialize_gl_frame`].
        self.materialize_gl_frame();
        let Some(name) = Interface::Display.method(slot) else {
            return Ok(None);
        };
        let result = match name {
            // `IDisplay` has no count of its own: the object is the screen, and it lives as long as
            // the emulator does. We count references only so the game sees the number it expects.
            "AddRef" => self.objects.add_ref(self.cpu.read_reg(Reg::R0)),
            "Release" => self.objects.release(self.cpu.read_reg(Reg::R0)),
            // RGBVAL IDISPLAY_SetColor(IDisplay *p, AEEClrItem clr, RGBVAL rgb)
            "SetColor" => {
                let item = self.cpu.read_reg(Reg::R1) as usize;
                let previous = self.colors.get(item).copied().unwrap_or(Rgb::BLACK);
                if let Some(slot) = self.colors.get_mut(item) {
                    *slot = Rgb::from_rgbval(self.cpu.read_reg(Reg::R2));
                }
                to_rgbval(previous)
            }
            // void IDISPLAY_DrawRect(IDisplay *p, const AEERect *pr, RGBVAL cf, RGBVAL cfill,
            //                        uint32 flags)
            "DrawRect" => {
                let target = self.target()?;
                // **A null pointer is the whole surface.** That is how the SDK's `ClearScreen` is
                // written, and it is the only way to clear the screen that BREW offers.
                let request = match self.read_rect(self.cpu.read_reg(Reg::R1))? {
                    Some(rect) => Some(rect),
                    None => self.bitmaps.get(&target).map(|fb| Rect {
                        x: 0,
                        y: 0,
                        width: fb.width().min(i16::MAX as u32) as i16,
                        height: fb.height().min(i16::MAX as u32) as i16,
                    }),
                };
                let rect = request.and_then(|rect| self.clip_rect(rect));
                // `RGB_NONE` means "the current color" — the background one for the fill and the
                // line one for the frame, both from `IDISPLAY_SetColor`.
                let color = |value: u32, item: usize, colors: &[Rgb]| match value {
                    RGB_NONE => colors[item],
                    value => Rgb::from_rgbval(value),
                };
                let border = color(self.cpu.read_reg(Reg::R2), CLR_USER_LINE, &self.colors);
                let fill = color(
                    self.cpu.read_reg(Reg::R3),
                    CLR_USER_BACKGROUND,
                    &self.colors,
                );
                // **What decides what gets drawn is `flags`, not the color.** Quake asks for a
                // frame alone (`IDF_RECT_FRAME`) passing black as the fill: honoring the color and
                // ignoring the flag painted a black rectangle it did not ask for.
                //
                // No flag at all keeps the old behavior — drawing both. No game in the collection
                // calls it like that, and when in doubt it is better to keep drawing than to erase
                // a screen because of a reading that could not be checked.
                let flags = match self.stack_arg(0)? {
                    0 => IDF_RECT_FRAME | IDF_RECT_FILL,
                    flags => flags,
                };
                if let (Some(rect), Some(fb)) = (rect, self.bitmaps.get_mut(&target)) {
                    if flags & IDF_RECT_FILL != 0 {
                        fb.fill_rect(rect, fill);
                    }
                    if flags & IDF_RECT_FRAME != 0 {
                        fb.draw_frame(rect, border);
                    }
                }
                SUCCESS
            }
            // int IDISPLAY_GetDeviceBitmap(IDisplay *p, IBitmap **ppIBitmap).
            //
            // Returns an **error code** and hands over the surface through the output pointer,
            // unlike `GetDestination` just below, which returns the `IBitmap *` directly. I had
            // implemented both the same way, and Bejeweled Twist — which tests
            // `if (result != 0) failed` — gave up on its initialization because of it.
            "GetDeviceBitmap" => {
                let out = self.cpu.read_reg(Reg::R1);
                let bitmap = self.device_bitmap()?;
                if out != 0 {
                    self.cpu.write_u32(out, bitmap)?;
                }
                if bitmap == 0 {
                    ENOMEMORY
                } else {
                    // Returns a reference: the game calls `Release` when it is done, and does so
                    // once per frame. Without the `AddRef` the count reached zero and the screen
                    // surface was destroyed in the middle of execution.
                    self.objects.add_ref(bitmap);
                    SUCCESS
                }
            }
            // int SetDestination(IDisplay *, IBitmap *pbmDest)
            //
            // The destination is not always a surface of ours: Bejeweled Twist implements its
            // own `IBitmap`, with the vtable embedded in the object (BREW's `DECLARE_VTBL`).
            // Drawing on it requires asking it where the pixels are — which requires entering
            // the guest, and so it is left for the call boundary.
            "SetDestination" => {
                let target = self.cpu.read_reg(Reg::R1);

                if target != 0 && !self.bitmaps.contains_key(&target) && self.probed.insert(target)
                {
                    self.pending_probes.push(target);
                }
                // **The display holds the destination.** The Z-Wheel's transition (`0x74e20`)
                // creates an off-screen bitmap, points the display at it and releases its own
                // reference right after. Without ours, the address went back to the free list, the
                // next bitmap was born in its place, and the new screen was drawn on an object that
                // was no longer the destination — the transition stayed stuck on the old menu
                // forever.
                let previous_target = self.display_target;
                if target != previous_target {
                    if target != 0 && self.bitmaps.contains_key(&target) {
                        self.objects.add_ref(target);
                    }
                    if previous_target != 0
                        && previous_target != self.device_bitmap
                        && self.bitmaps.contains_key(&previous_target)
                        && self.objects.release(previous_target) == 0
                    {
                        self.release_dib(previous_target);
                    }
                }
                self.display_target = target;
                SUCCESS
            }
            // Returns the `IBitmap *` directly — and **with a count**, because the receiver
            // releases it.
            //
            // Without the `AddRef`, each `GetDestination` the game released took away a reference
            // nobody had added. Measured on the Z-Wheel: the screen bitmap got down to zero, the
            // address went back to the free list **with the surface still alive**, and the
            // next object — the roller widget — was born on top of the screen.
            "GetDestination" => {
                let current_target = self.target()?;
                if current_target != 0 {
                    self.objects.add_ref(current_target);
                }
                current_target
            }
            // void IDISPLAY_BitBlt(IDisplay *p, int xd, int yd, int w, int h,
            //                      const void *pbmSource, int xs, int ys, AEERasterOp rop)
            "BitBlt" => {
                let dst_pos = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let size = (self.cpu.read_reg(Reg::R3) as i32, self.stack_arg(0)? as i32);
                let src = self.stack_arg(1)?;
                let origin = (self.stack_arg(2)? as i32, self.stack_arg(3)? as i32);
                let rop = self.stack_arg(4)?;
                let target = self.target()?;
                if let Some((dst_pos, size, origin)) = self.clip_blit(dst_pos, size, origin) {
                    self.blit(target, dst_pos, size, src, origin, rop);
                }
                SUCCESS
            }
            // void IDISPLAY_DrawText(IDisplay *, AEEFont, const AECHAR *pcText, int nChars,
            //                        int x, int y, const AEERect *prcBackground, uint32 dwFlags)
            // int IDISPLAY_DrawText(IDisplay *p, AEEFont nFont, const AECHAR *pcText, int nChars,
            //                       int x, int y, const AEERect *prcBackground, uint32 dwFlags)
            //
            // The rectangle and the flags decide the position: with alignment requested on an axis,
            // that axis's `x` or `y` does not count, and the text does not leave the rectangle. The
            // Z-Wheel's tab bar draws each label at `(0, 0)` asking for center and middle in a
            // 214-pixel rectangle; ignoring that pushed everything to the left and threw the
            // previous label out of the bar.
            "DrawText" => {
                let mut units = self.read_aechar_units(self.cpu.read_reg(Reg::R2))?;
                let chars = self.cpu.read_reg(Reg::R3) as i32;
                if chars >= 0 {
                    units.truncate(chars as usize);
                }
                let text = String::from_utf16_lossy(&units);
                let (x, y) = (self.stack_arg(0)? as i32, self.stack_arg(1)? as i32);
                let background = self.read_rect(self.stack_arg(2)?)?;
                let flags = self.stack_arg(3)?;
                if let (Some(rect), true) = (background, flags & IDF_RECT_FILL != 0) {
                    let color = self.colors.get(CLR_USER_BACKGROUND).copied().unwrap_or(Rgb::WHITE);
                    let target = self.target()?;
                    if let (Some(rect), Some(fb)) = (self.clip_rect(rect), self.bitmaps.get_mut(&target)) {
                        fb.fill_rect(rect, color);
                    }
                }
                let measured = self.font.as_ref().map(|font| {
                    (
                        font.width(&text, FONT_SIZE) as i32,
                        (font.ascent(FONT_SIZE) + font.descent(FONT_SIZE)) as i32,
                    )
                });
                let (x, y) = match (background, measured) {
                    (Some(rect), Some(measured)) => text_position((x, y), rect, flags, measured),
                    _ => (x, y),
                };
                // The text stays inside the rectangle: it is the clipping BREW applies, and it is
                // what cuts the neighboring label at the edge of the bar instead of letting it
                // leak.
                let previous_clip = self.clip;
                let visible = match background {
                    Some(rect) => match self.clip_rect(rect) {
                        Some(inside) => {
                            self.clip = Some(inside);
                            true
                        }
                        None => false,
                    },
                    None => true,
                };
                let drew = match visible {
                    true => self.draw_text(&text, x, y),
                    false => Ok(true),
                };
                self.clip = previous_clip;
                match drew? {
                    true => {}
                    // Without a font, the text still goes only to the report: that is what lets us
                    // know the game *wants* to write even when there is nothing to write with.
                    false => {
                        // The comparison only runs while there is room; once full, the list costs
                        // one size test per call.
                        if self.pending_text.len() < MAX_TEXT && !self.pending_text.contains(&text)
                        {
                            self.pending_text.push(text);
                        }
                    }
                }
                SUCCESS
            }
            // No visible effect for us: the framebuffer is always up to date already.
            // void IDISPLAY_GetClipRect(IDisplay *p, AEERect *prc)
            //
            // Clipping is ignored when drawing, so the current rectangle is the whole screen —
            // which is what BREW returns when nobody tightened the clip. Pac-Mania reads this
            // to save it and restore it later.
            "GetClipRect" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    let rect = self.clip.unwrap_or_else(|| {
                        let screen = self.screen();
                        Rect {
                            x: 0,
                            y: 0,
                            width: screen.width() as i16,
                            height: screen.height() as i16,
                        }
                    });
                    let mut bytes = [0u8; 8];
                    bytes[0..2].copy_from_slice(&rect.x.to_le_bytes());
                    bytes[2..4].copy_from_slice(&rect.y.to_le_bytes());
                    bytes[4..6].copy_from_slice(&rect.width.to_le_bytes());
                    bytes[6..8].copy_from_slice(&rect.height.to_le_bytes());
                    self.cpu.write_mem(out, &bytes)?;
                }
                SUCCESS
            }
            // int IDISPLAY_GetFontMetrics(IDisplay *p, AEEFont font, int *pnAscent,
            //                              int *pnDescent)
            //
            // Returns the font's height. We do not draw text yet, but games measure before
            // positioning: without numbers here, Pac-Mania does not even get to build the
            // screen. The values are those of a small screen font, consistent with each other.
            // int IDISPLAY_MeasureTextEx(IDisplay *, AEEFont, const AECHAR *pcText,
            //                             int nChars, int nMaxWidth, int *pnFits)
            //
            // Returns the width in pixels and, in `pnFits`, how many characters fit in `nMaxWidth`.
            // Since we do not draw text yet, the width comes from a fixed advance per character,
            // consistent with the height `GetFontMetrics` reports. It is an approximate measure on
            // purpose: it serves for the game to center and wrap lines, and a plausible number lets
            // it carry on — without any, Pac-Mania stops at the language selection.
            "MeasureTextEx" => {
                let text = self.read_aechar_units(self.cpu.read_reg(Reg::R2))?;
                let chars = self.cpu.read_reg(Reg::R3) as i32;
                let count = match chars < 0 {
                    true => text.len(),
                    false => (chars as usize).min(text.len()),
                };
                let max_width = self.stack_arg(0)? as i32;
                // With a font, the width is measured character by character until it exceeds the
                // limit; without one the fixed width remains, which is an honest guess but a guess.
                let (fits, text_width) = match &self.font {
                    Some(font) => {
                        let mut fit_count = 0;
                        let mut text_width = 0;
                        for end in 1..=count {
                            let chunk: String = text[..end]
                                .iter()
                                .filter_map(|u| char::from_u32(u32::from(*u)))
                                .collect();
                            let candidate = font.width(&chunk, FONT_SIZE) as i32;
                            if max_width >= 0 && candidate > max_width {
                                break;
                            }
                            (fit_count, text_width) = (end, candidate);
                        }
                        (fit_count, text_width as u32)
                    }
                    None => {
                        let fit_count = match max_width < 0 {
                            true => count,
                            false => count.min((max_width / FONT_ADVANCE).max(0) as usize),
                        };
                        (fit_count, (fit_count as i32 * FONT_ADVANCE) as u32)
                    }
                };
                let out = self.stack_arg(1)?;
                if out != 0 {
                    self.cpu.write_u32(out, fits as u32)?;
                }
                text_width
            }
            "GetFontMetrics" => {
                // With a real font, the measure is the font's: a menu that centers by the line
                // height comes out crooked if the height is a guess.
                let (ascent, descent) = match &self.font {
                    Some(font) => (font.ascent(FONT_SIZE), font.descent(FONT_SIZE)),
                    None => (FONT_ASCENT, FONT_DESCENT),
                };
                self.write_at(self.cpu.read_reg(Reg::R2), ascent)?;
                self.write_at(self.cpu.read_reg(Reg::R3), descent)?;
                ascent + descent
            }
            // int IDISPLAY_SetClipRect(IDisplay *p, const AEERect *prc)
            //
            // A null pointer goes back to the full clip, which is what BREW defines. Ignoring this
            // call was costly: Pac-Mania draws the whole font sheet and relies on the clip so
            // that only the letter shows — without it, the whole sheet went to the screen.
            "SetClipRect" => {
                self.clip = self.read_rect(self.cpu.read_reg(Reg::R1))?;
                SUCCESS
            }
            // int IDISPLAY_Clone(IDisplay *po, IDisplay **ppNew)
            //
            // A copy of the screen object, so the app can draw in another state without touching
            // the current one. There is only one screen, so what comes out of here is the same
            // object with one more reference — which is what BREW does when the device has one
            // screen.
            "Clone" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out == 0 {
                    return Ok(Some(EBADPARM));
                }
                // A new object, not the same one with one more reference: the app releases the copy
                // when it is done with it, and returning the original would make that release take
                // down the screen it still uses. The drawing state belongs to the `Machine`, not to
                // the object, so two objects point at the same screen without getting in each
                // other's way.
                let clone = self.new_object(Interface::Display)?;
                if clone == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.cpu.write_u32(out, clone)?;
                SUCCESS
            }
            "Update" | "UpdateEx" => {
                self.store_update_frame()?;
                SUCCESS
            }
            // int IDISPLAY_CreateDIBitmap(IDisplay *p, IDIB **ppIDIB, uint8 colorDepth,
            //                             uint16 w, uint16 h)
            "CreateDIBitmap" => {
                let out = self.cpu.read_reg(Reg::R1);
                let depth = self.cpu.read_reg(Reg::R2) as u8;
                let width = self.cpu.read_reg(Reg::R3) as u16;
                let height = self.stack_arg(0)? as u16;
                self.create_dib_bitmap(out, depth, width, height)?
            }
            "SetFont" | "SetAnnunciators" | "Backlight" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Surface where `IDisplay` draws.
    pub(super) fn target(&mut self) -> Result<u32, CpuError> {
        if self.display_target == 0 {
            return self.device_bitmap();
        }
        Ok(self.display_target)
    }

    /// `IGraphics` — BREW's 2D API, drawn on the same surface as `IDisplay`.
    ///
    /// The state (stroke color, fill color, whether it fills or not, translation) lives on
    /// the host; the primitives become operations on the framebuffer.
    pub(super) fn graphics_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        // **`IGraphics` has its own clip, and `IDisplay` has its own.** The two used to share
        // `clip`: Zuma's Revenge puts the screen's clip (640 wide) on `IGraphics` and then draws
        // through `IImage` the 726x360 background onto a surface of its own. The leaked clip cut
        // the blit at 640, the game's engine sized the surface by it, and the copy to the
        // texture, which walks 726 pixels per row, scrambled the background into stripes.
        let display_clip = std::mem::replace(&mut self.clip, self.graphics_clip);
        let result = self.graphics_call_in_clip(slot);
        self.graphics_clip = std::mem::replace(&mut self.clip, display_clip);
        result
    }

    fn graphics_call_in_clip(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        // **The OpenGL frame comes before the 2D.** A HUD drawn over a screen that has not
        // received the scene yet would erase the scene — see [`Machine::materialize_gl_frame`].
        self.materialize_gl_frame();
        let Some(name) = Interface::Graphics.method(slot) else {
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
            // The colors arrive as three or four loose `uint8`s, not as an RGBVAL.
            "SetBackground" => {
                let previous = self.graphics.background;
                self.graphics.background = Rgb {
                    r: a1 as u8,
                    g: a2 as u8,
                    b: a3 as u8,
                };
                to_rgbval(previous)
            }
            "SetColor" => {
                let previous = self.graphics.stroke;
                self.graphics.stroke = Rgb {
                    r: a1 as u8,
                    g: a2 as u8,
                    b: a3 as u8,
                };
                to_rgbval(previous)
            }
            "SetFillColor" => {
                let previous = self.graphics.fill;
                self.graphics.fill = Rgb {
                    r: a1 as u8,
                    g: a2 as u8,
                    b: a3 as u8,
                };
                to_rgbval(previous)
            }
            "SetFillMode" => {
                let previous = self.graphics.fill_mode;
                self.graphics.fill_mode = a1 != 0;
                u32::from(previous)
            }
            "GetFillMode" => u32::from(self.graphics.fill_mode),
            "SetPointSize" => {
                let previous = self.graphics.point_size;
                self.graphics.point_size = a1 as u8;
                previous as u32
            }
            "GetPointSize" => self.graphics.point_size as u32,
            "GetColorDepth" => COLOR_DEPTH as u32,
            "Translate" => {
                self.graphics.origin = (a1 as i16 as i32, a2 as i16 as i32);
                SUCCESS
            }
            // int DrawPoint(IGraphics *, AEEPoint *) — AEEPoint is { int16 x, y }.
            "DrawPoint" => {
                let (x, y) = self.read_point(a1)?;
                let (x, y) = self.translated(x, y);
                let color = self.graphics.stroke;
                self.with_target_or_foreign(x, y, 1, 1, AEE_RO_TRANSPARENT, |fb, ox, oy| {
                    fb.set_pixel(x + ox, y + oy, color)
                })?;
                SUCCESS
            }
            // int DrawLine(IGraphics *, AEELine *) — AEELine is { int16 sx, sy, ex, ey }.
            "DrawLine" => {
                let mut bytes = [0u8; 8];
                self.cpu.read_mem(a1, &mut bytes)?;
                let read = |i: usize| i16::from_le_bytes([bytes[i], bytes[i + 1]]) as i32;
                let (sx, sy) = self.translated(read(0), read(2));
                let (ex, ey) = self.translated(read(4), read(6));
                let color = self.graphics.stroke;
                let x = sx.min(ex);
                let y = sy.min(ey);
                let box_width = (sx.max(ex) - x + 1) as u32;
                let box_height = (sy.max(ey) - y + 1) as u32;
                self.with_target_or_foreign(x, y, box_width, box_height, AEE_RO_TRANSPARENT, |fb, ox, oy| {
                    fb.draw_line(sx + ox, sy + oy, ex + ox, ey + oy, color)
                })?;
                SUCCESS
            }
            "DrawRect" | "ClearRect" => {
                let Some(rect) = self.read_rect(a1)? else {
                    return Ok(Some(EBADPARM));
                };
                let rect = self.translated_rect(rect);
                let (stroke, fill, filled) = (
                    self.graphics.stroke,
                    self.graphics.fill,
                    self.graphics.fill_mode,
                );
                // `ClearRect` paints with the background color; `DrawRect` honors the fill mode
                // and always draws the border.
                if name == "ClearRect" {
                    let background = self.graphics.background;
                    self.with_target_or_foreign(
                        rect.x as i32,
                        rect.y as i32,
                        rect.width.max(0) as u32,
                        rect.height.max(0) as u32,
                        AEE_RO_COPY,
                        |fb, ox, oy| {
                            fb.fill_rect(
                                Rect {
                                    x: rect.x + ox as i16,
                                    y: rect.y + oy as i16,
                                    ..rect
                                },
                                background,
                            )
                        },
                    )?;
                } else {
                    self.with_target_or_foreign(
                        rect.x as i32,
                        rect.y as i32,
                        rect.width.max(0) as u32,
                        rect.height.max(0) as u32,
                        AEE_RO_TRANSPARENT,
                        |fb, ox, oy| {
                            let rect = Rect {
                                x: rect.x + ox as i16,
                                y: rect.y + oy as i16,
                                ..rect
                            };
                            if filled {
                                fb.fill_rect(rect, fill);
                            }
                            fb.draw_frame(rect, stroke);
                        },
                    )?;
                }
                SUCCESS
            }
            // AEECircle is { int16 cx, cy, r }.
            "DrawCircle" => {
                let mut bytes = [0u8; 6];
                self.cpu.read_mem(a1, &mut bytes)?;
                let read = |i: usize| i16::from_le_bytes([bytes[i], bytes[i + 1]]) as i32;
                let (cx, cy) = self.translated(read(0), read(2));
                let radius = read(4);
                let (stroke, fill, filled) = (
                    self.graphics.stroke,
                    self.graphics.fill,
                    self.graphics.fill_mode,
                );
                let size = (radius.max(0) * 2 + 1) as u32;
                self.with_target_or_foreign(
                    cx - radius,
                    cy - radius,
                    size,
                    size,
                    AEE_RO_TRANSPARENT,
                    |fb, ox, oy| {
                        if filled {
                            fb.fill_circle(cx + ox, cy + oy, radius, fill);
                        }
                        fb.draw_circle(cx + ox, cy + oy, radius, stroke);
                    },
                )?;
                SUCCESS
            }
            // AEETriangle is { int16 x0, y0, x1, y1, x2, y2 }.
            "DrawTriangle" => {
                let mut bytes = [0u8; 12];
                self.cpu.read_mem(a1, &mut bytes)?;
                let read = |i: usize| i16::from_le_bytes([bytes[i], bytes[i + 1]]) as i32;
                let points: Vec<(i32, i32)> = (0..3)
                    .map(|i| self.translated(read(i * 4), read(i * 4 + 2)))
                    .collect();
                self.draw_shape(&points, true)?;
                SUCCESS
            }
            // AEEPolygon and AEEPolyline are { int16 len; AEEPoint *points }.
            "DrawPolygon" | "DrawPolyline" => {
                let count = self.cpu.read_u32(a1)? as u16 as usize;
                let array = self.cpu.read_u32(a1 + 4)?;
                let mut points = Vec::with_capacity(count);
                for i in 0..count.min(MAX_POLYGON_POINTS) {
                    let (x, y) = self.read_point(array + i as u32 * 4)?;
                    points.push(self.translated(x, y));
                }
                self.draw_shape(&points, name == "DrawPolygon")?;
                SUCCESS
            }
            "ClearViewport" => {
                let background = self.graphics.background;
                let rect = Rect {
                    x: 0,
                    y: 0,
                    width: SCREEN_WIDTH as i16,
                    height: SCREEN_HEIGHT as i16,
                };
                self.with_target(|fb| fb.fill_rect(rect, background))?;
                SUCCESS
            }
            "SetDestination" => {
                self.display_target = a1;
                SUCCESS
            }
            // With a count, for the same reason as `IDisplay` — see its `GetDestination`.
            "GetDestination" => {
                let target = self.target()?;
                if target != 0 {
                    self.objects.add_ref(target);
                }
                target
            }
            "SetClip" => {
                // Bejeweled Twist passes a 16-byte structure: a state field followed by an
                // `AEERect`. Reading from the start turns that state into a coordinate and corrupts
                // the flow already at load time.
                self.clip = self.read_rect(a1 + 4)?;
                SUCCESS
            }
            // No effect for us: we already draw directly on the surface.
            "Update" | "EnableDoubleBuffer" | "SetPaintMode" | "GetClip" | "SetViewport"
            | "SetAlgorithmHint" | "SetStrokeStyle" | "Pan" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Draws a closed polygon (or an open polyline) with the current state.
    pub(super) fn draw_shape(
        &mut self,
        points: &[(i32, i32)],
        closed: bool,
    ) -> Result<(), CpuError> {
        if points.is_empty() {
            return Ok(());
        }
        let (stroke, fill, filled) = (
            self.graphics.stroke,
            self.graphics.fill,
            self.graphics.fill_mode && closed,
        );
        let points = points.to_vec();
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for &(x, y) in &points {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        self.with_target_or_foreign(
            x0,
            y0,
            (x1 - x0 + 1) as u32,
            (y1 - y0 + 1) as u32,
            AEE_RO_TRANSPARENT,
            move |fb, ox, oy| {
            let points: Vec<_> = points.iter().map(|&(x, y)| (x + ox, y + oy)).collect();
            if filled {
                fb.fill_polygon(&points, fill);
            }
            let last = if closed {
                points.len()
            } else {
                points.len() - 1
            };
            for i in 0..last {
                let (x0, y0) = points[i];
                let (x1, y1) = points[(i + 1) % points.len()];
                fb.draw_line(x0, y0, x1, y1, stroke);
            }
        })
    }

    /// Runs a drawing operation on the current surface.
    pub(super) fn with_target(
        &mut self,
        draw: impl FnOnce(&mut Framebuffer),
    ) -> Result<(), CpuError> {
        let target = self.target()?;
        if let Some(fb) = self.bitmaps.get_mut(&target) {
            draw(fb);
        }
        Ok(())
    }

    /// Draws directly when the target is ours; when it is a game surface, queues a temporary
    /// surface for the foreign `BltIn` to composite at the safe boundary.
    pub(super) fn with_target_or_foreign(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        rop: u32,
        draw: impl FnOnce(&mut Framebuffer, i32, i32),
    ) -> Result<(), CpuError> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let target = self.target()?;
        if let Some(fb) = self.bitmaps.get_mut(&target) {
            draw(fb, 0, 0);
            return Ok(());
        }

        let (mut x, mut y, mut width, mut height) = (x, y, width, height);
        if let Some(clip) = self.clip {
            let x0 = x.max(clip.x as i32);
            let y0 = y.max(clip.y as i32);
            let x1 = (x + width as i32).min(clip.x as i32 + clip.width as i32);
            let y1 = (y + height as i32).min(clip.y as i32 + clip.height as i32);
            if x0 >= x1 || y0 >= y1 {
                return Ok(());
            }
            x = x0;
            y = y0;
            width = (x1 - x0) as u32;
            height = (y1 - y0) as u32;
        }

        let source = self.new_object(Interface::Bitmap)?;
        if source == 0 {
            return Ok(());
        }
        let mut fb = Framebuffer::new(width, height);
        fb.fill_rect_native(
            Rect {
                x: 0,
                y: 0,
                width: width.min(i16::MAX as u32) as i16,
                height: height.min(i16::MAX as u32) as i16,
            },
            TRANSPARENT_KEY,
        );
        draw(&mut fb, -x, -y);
        self.bitmaps.insert(source, fb);
        self.transparency.insert(source, TRANSPARENT_KEY);
        self.pending_surface_blits.push(PendingSurfaceBlit {
            source,
            target,
            x,
            y,
            width,
            height,
            rop,
        });
        Ok(())
    }

    /// Reads an `AEEPoint` — two `int16`s.
    pub(super) fn read_point(&self, addr: u32) -> Result<(i32, i32), CpuError> {
        let mut bytes = [0u8; 4];
        self.cpu.read_mem(addr, &mut bytes)?;
        Ok((
            i16::from_le_bytes([bytes[0], bytes[1]]) as i32,
            i16::from_le_bytes([bytes[2], bytes[3]]) as i32,
        ))
    }

    /// Applies `IGraphics`'s current translation.
    pub(super) fn translated(&self, x: i32, y: i32) -> (i32, i32) {
        (x + self.graphics.origin.0, y + self.graphics.origin.1)
    }

    pub(super) fn translated_rect(&self, rect: Rect) -> Rect {
        Rect {
            x: rect.x + self.graphics.origin.0 as i16,
            y: rect.y + self.graphics.origin.1 as i16,
            ..rect
        }
    }

    /// Draws `text` on the current surface. `false` when there is no font to draw it with.
    ///
    /// The color is `CLR_USER_TEXT`'s, which is what `IDISPLAY_SetColor` sets, and the clip
    /// applies here as in any other drawing.
    pub(super) fn draw_text(&mut self, text: &str, x: i32, y: i32) -> Result<bool, CpuError> {
        // The record is of what **was** drawn, not of what was missing: a game that shows several
        // warning screens over the run needs the report to say which one came when.
        if text.chars().any(|c| !c.is_whitespace()) {
            if self.drawn_texts.len() >= MAX_DRAWN_TEXTS {
                self.drawn_texts.pop_front();
            }
            self.drawn_texts
                .push_back((self.clock_ms(), x, y, text.to_string()));
        }
        let Some(font) = self.font.as_ref() else {
            return Ok(false);
        };
        let glyphs = font.layout(text, FONT_SIZE);
        if glyphs.is_empty() {
            return Ok(true);
        }
        let color = self
            .colors
            .get(CLR_USER_TEXT)
            .copied()
            .unwrap_or(Rgb::BLACK);
        self.write(text, x, y, color)
    }

    /// The drawn text, in order, with the virtual instant and the position.
    pub fn drawn_text(&self) -> impl Iterator<Item = &(u32, i32, i32, String)> {
        self.drawn_texts.iter()
    }

    /// Writes with the loaded font, in a given color. It is the core of [`Machine::draw_text`],
    /// separate because the widget carries its color in a property and does not use the palette's.
    pub(super) fn write(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        color: Rgb,
    ) -> Result<bool, CpuError> {
        let Some(font) = self.font.as_ref() else {
            return Ok(false);
        };
        let glyphs = font.layout(text, FONT_SIZE);
        if glyphs.is_empty() {
            return Ok(true);
        }
        let native = color.to_rgb565();
        let active_clip = self.clip;
        let target = self.target()?;
        let Some(surface) = self.bitmaps.get_mut(&target) else {
            return Ok(true);
        };
        for glyph in glyphs {
            for line in 0..glyph.height {
                for column in 0..glyph.width {
                    // Half-tones do not exist on a surface without an alpha channel: either the
                    // letter covers the pixel or it does not. Half is the cutoff that makes the
                    // edge look like the console's, which does not blend either.
                    if glyph.coverage[(line * glyph.width + column) as usize] < 128 {
                        continue;
                    }
                    let (px, py) = (x + glyph.x + column as i32, y + glyph.y + line as i32);
                    if let Some(clip) = active_clip {
                        let inside = px >= clip.x as i32
                            && py >= clip.y as i32
                            && px < clip.x as i32 + clip.width as i32
                            && py < clip.y as i32 + clip.height as i32;
                        if !inside {
                            continue;
                        }
                    }
                    surface.set_pixel_native(px, py, native);
                }
            }
        }
        Ok(true)
    }

    /// The screen size the applet of this class is told: the console's 640×480, or the phone's
    /// for a phone game (see [`crate::input::phone::phone_screen`]).
    pub(super) fn reported_screen(&self, class: u32) -> (u16, u16) {
        crate::input::phone::phone_screen(class).unwrap_or((SCREEN_WIDTH, SCREEN_HEIGHT))
    }

    /// The part of the screen the running game uses, when it is not the whole screen.
    ///
    /// A phone game is told its phone's screen and draws from the corner of the 640×480 one; this
    /// is that area, for the frontends to show on its own. `None` is the whole screen — every
    /// Zeebo game.
    pub fn visible_area(&self) -> Option<Rect> {
        crate::input::phone::phone_screen(self.applet_class).map(|(width, height)| Rect {
            x: 0,
            y: 0,
            width: width as i16,
            height: height as i16,
        })
    }

    /// The screen, as it is now.
    pub fn screen(&self) -> &Framebuffer {
        self.bitmaps
            .get(&self.device_bitmap)
            .unwrap_or(&self.screen)
    }

    /// Puts an RGB565 image the size of the screen on it, before the applet draws.
    ///
    /// It is the firmware's role between one applet and another: the device's screen is not
    /// cleared when one applet leaves and another enters, and what was on it stays until someone
    /// draws over it. Returns `false`, without touching anything, when the image is not the size
    /// of the screen.
    pub fn paint_screen_rgb565(&mut self, bytes: &[u8]) -> bool {
        let screen = match self.bitmaps.get_mut(&self.device_bitmap) {
            Some(screen) => screen,
            None => &mut self.screen,
        };
        if bytes.len() != screen.width() as usize * screen.height() as usize * 2 {
            return false;
        }
        screen.load_rgb565_bytes(bytes);
        true
    }

    /// Texts the game asked to draw but that we do not know how to rasterize yet.
    pub fn pending_text(&self) -> &[String] {
        &self.pending_text
    }
}

/// `IDISPLAY_DrawText` alignments, from `AEEDisp.h`. The values were checked against the
/// Z-Wheel's usage (`0x30434`): a one-line label asks for `0x8020 | 0x200` — transparent,
/// center, middle — and a two-line one swaps `0x200` for `0x400` on the first and `0x100` on
/// the second, which are bottom and top. They agree with `IDF_RECT_FRAME`/`FILL` being `1` and `2`.
const IDF_ALIGN_LEFT: u32 = 0x10;
const IDF_ALIGN_CENTER: u32 = 0x20;
const IDF_ALIGN_RIGHT: u32 = 0x40;
const IDF_ALIGN_TOP: u32 = 0x100;
const IDF_ALIGN_MIDDLE: u32 = 0x200;
const IDF_ALIGN_BOTTOM: u32 = 0x400;

/// Where the text starts, given the rectangle and the alignments. An axis without alignment
/// keeps the coordinate the game passed.
fn text_position(
    (x, y): (i32, i32),
    rect: Rect,
    flags: u32,
    (width, height): (i32, i32),
) -> (i32, i32) {
    let (rx, ry, rw, rh) = (
        i32::from(rect.x),
        i32::from(rect.y),
        i32::from(rect.width),
        i32::from(rect.height),
    );
    let x = match flags {
        f if f & IDF_ALIGN_CENTER != 0 => rx + (rw - width) / 2,
        f if f & IDF_ALIGN_RIGHT != 0 => rx + rw - width,
        f if f & IDF_ALIGN_LEFT != 0 => rx,
        _ => x,
    };
    let y = match flags {
        f if f & IDF_ALIGN_MIDDLE != 0 => ry + (rh - height) / 2,
        f if f & IDF_ALIGN_BOTTOM != 0 => ry + rh - height,
        f if f & IDF_ALIGN_TOP != 0 => ry,
        _ => y,
    };
    (x, y)
}

#[cfg(test)]
mod text_tests {
    use super::*;

    #[test]
    fn the_tab_bar_label_sits_at_the_center_of_the_rect() {
        let rect = Rect { x: 0, y: 0, width: 214, height: 37 };
        assert_eq!(text_position((0, 0), rect, 0x8220, (50, 17)), (82, 10));
    }

    #[test]
    fn without_alignment_the_given_coordinate_applies() {
        let rect = Rect { x: 0, y: 0, width: 640, height: 480 };
        assert_eq!(text_position((48, 272), rect, 0x8000, (90, 17)), (48, 272));
    }
}
