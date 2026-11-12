//! `IDISPLAY_CreateDIBitmap`: a DIB whose header and pixels belong to the game.
//!
//! Every other `IDIB` here is a surface of ours mirrored into guest memory as RGB565. This one is
//! the other way around. Phone-era BREW games create the DIB empty and then **fill in the public
//! fields by hand**, pointing `pBmp` and `pRGB` at memory of their own. Devil May Cry: Dante x
//! Virgil does it with every sprite: its BMP reader (`0x40000` in `dmc_mas.mod`) asks for a
//! `4 x 0 x 0` DIB, then writes `pBmp` at the last row of the file's pixel data, a negative
//! `nPitch` because a BMP is stored bottom-up, `pRGB` at a copy of the file's `RGBQUAD`s, and `cx`
//! and `cy` from its header. Nothing of that goes through an API, so the only moment we can know
//! what the DIB holds is the moment it is drawn: the header is read again on every blit.

use super::*;

/// The public fields of an `IDIB` that say where the pixels are and how to read them.
///
/// Offsets from `inc/AEEIDIB.h`: `pvt` 0, `pPaletteMap` 4, `pBmp` 8, `pRGB` 12, `ncTransparent`
/// 16, `cx` 20, `cy` 22, `nPitch` 24, `cntRGB` 26, `nDepth` 28, `nColorScheme` 29.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DibHeader {
    pub bmp: u32,
    pub rgb: u32,
    pub cx: u16,
    pub cy: u16,
    pub pitch: i16,
    pub cnt_rgb: u16,
    pub depth: u8,
}

/// The size of the struct, public fields and the six reserved bytes.
const IDIB_SIZE: usize = 36;

impl DibHeader {
    fn from_bytes(bytes: &[u8; IDIB_SIZE]) -> Self {
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        Self {
            bmp: u32_at(8),
            rgb: u32_at(12),
            cx: u16_at(20),
            cy: u16_at(22),
            pitch: u16_at(24) as i16,
            cnt_rgb: u16_at(26),
            depth: bytes[28],
        }
    }

    /// Bytes that hold one row of pixels, without the padding the pitch may add.
    fn row_bytes(&self) -> usize {
        (self.cx as usize * self.depth as usize).div_ceil(8)
    }

    fn indexed(&self) -> bool {
        matches!(self.depth, 1 | 2 | 4 | 8)
    }
}

/// Bytes per row of a DIB we allocate: BMP's rule, rows aligned to four bytes.
fn aligned_pitch(width: u16, depth: u8) -> u32 {
    (width as u32 * depth as u32).div_ceil(32) * 4
}

/// Turns the game's pixels into an RGB565 surface.
///
/// `rows` holds `cy` rows of `header.row_bytes()` each, **top to bottom** — the sign of the pitch
/// was already resolved when reading. `palette` holds the `pRGB` words: a palette copied from a
/// BMP is `RGBQUAD`s (blue, green, red, zero), which read as a little-endian word is `0x00RRGGBB`.
/// Indexed pixels are packed from the most significant bit, as in a BMP.
///
/// `None` for a depth we do not know how to read yet.
pub(super) fn decode_dib(header: &DibHeader, rows: &[u8], palette: &[u32]) -> Option<Framebuffer> {
    let (width, height) = (header.cx as i32, header.cy as i32);
    let stride = header.row_bytes();
    let mut fb = Framebuffer::new(width as u32, height as u32);
    for y in 0..height {
        let row = &rows[y as usize * stride..][..stride];
        for x in 0..width {
            let native = match header.depth {
                1 | 2 | 4 | 8 => {
                    let depth = header.depth as usize;
                    let bit = x as usize * depth;
                    let shift = 8 - depth - bit % 8;
                    let index = (row[bit / 8] >> shift) as usize & ((1 << depth) - 1);
                    let rgb = palette.get(index).copied().unwrap_or(0);
                    Rgb {
                        r: (rgb >> 16) as u8,
                        g: (rgb >> 8) as u8,
                        b: rgb as u8,
                    }
                    .to_rgb565()
                }
                16 => u16::from_le_bytes([row[x as usize * 2], row[x as usize * 2 + 1]]),
                _ => return None,
            };
            fb.set_pixel_native(x, y, native);
        }
    }
    Some(fb)
}

impl<C: CpuBackend> Machine<C> {
    /// `int IDISPLAY_CreateDIBitmap(IDisplay *p, IDIB **ppIDIB, uint8 colorDepth, uint16 w,
    /// uint16 h)`.
    ///
    /// The header is born describing what was asked for. With a size, the pixels get a buffer in
    /// the surface region — never in the game's heap, whose addresses RetroAchievements reads.
    /// Without one, `pBmp` stays null until the game points it somewhere.
    pub(super) fn create_dib_bitmap(
        &mut self,
        out: u32,
        depth: u8,
        width: u16,
        height: u16,
    ) -> Result<u32, CpuError> {
        let dib = self.new_object(Interface::Bitmap)?;
        if dib == 0 {
            return Ok(ENOMEMORY);
        }
        let pitch = aligned_pitch(width, depth);
        let buffer = match pitch * height as u32 {
            0 => 0,
            bytes => match self.reserve_surface(bytes) {
                Some((buffer, _)) => buffer,
                None => {
                    self.objects.release(dib);
                    return Ok(ENOMEMORY);
                }
            },
        };
        let mut header = [0u8; IDIB_SIZE];
        header[8..12].copy_from_slice(&buffer.to_le_bytes()); // pBmp
        header[20..22].copy_from_slice(&width.to_le_bytes()); // cx
        header[22..24].copy_from_slice(&height.to_le_bytes()); // cy
        header[24..26].copy_from_slice(&(pitch as i16).to_le_bytes()); // nPitch
        header[28] = depth; // nDepth
        // `nColorScheme`: none for a palette; for the direct depths the codes in `AEEIDIB.h`
        // are the depth itself (16 is 565, 24 is 888).
        header[29] = if depth <= 8 { 0 } else { depth };
        // The vtable pointer at offset 0 was written by `new_object`.
        self.cpu.write_mem(dib + 4, &header[4..])?;
        self.guest_dibs.insert(dib, buffer);
        self.bitmaps
            .insert(dib, Framebuffer::new(width as u32, height as u32));
        if out != 0 {
            self.cpu.write_u32(out, dib)?;
        }
        Ok(SUCCESS)
    }

    /// Rebuilds our copy of a game-described DIB from what is in its header now.
    ///
    /// A header we cannot read — an unknown depth, an indexed DIB without a palette, rows outside
    /// memory — leaves the previous copy alone: drawing nothing is better than drawing garbage.
    pub(super) fn refresh_guest_dib(&mut self, dib: u32) -> Result<(), CpuError> {
        let mut bytes = [0u8; IDIB_SIZE];
        self.cpu.read_mem(dib, &mut bytes)?;
        let header = DibHeader::from_bytes(&bytes);
        if header.bmp == 0 || (header.indexed() && header.rgb == 0) {
            return Ok(());
        }
        let stride = header.row_bytes();
        let mut rows = vec![0u8; guest_size(stride * header.cy as usize)?];
        for y in 0..header.cy as usize {
            let address = header.bmp as i64 + y as i64 * header.pitch as i64;
            self.cpu
                .read_mem(address as u32, &mut rows[y * stride..][..stride])?;
        }
        let mut palette = Vec::new();
        if header.indexed() {
            let count = match header.cnt_rgb {
                0 => 1usize << header.depth,
                count => (count as usize).min(256),
            };
            let mut raw = vec![0u8; count * 4];
            self.cpu.read_mem(header.rgb, &mut raw)?;
            palette = raw
                .chunks_exact(4)
                .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
                .collect();
        }
        if let Some(fb) = decode_dib(&header, &rows, &palette) {
            self.bitmaps.insert(dib, fb);
        }
        Ok(())
    }

    /// The DIB died: its buffer, if we allocated one, goes back to the region.
    pub(super) fn release_guest_dib(&mut self, dib: u32) {
        if let Some(buffer) = self.guest_dibs.remove(&dib) {
            if buffer != 0 {
                self.release_surface(buffer);
            }
        }
    }
}
