//! The console framebuffer and the `IDisplay` drawing operations.
//!
//! The Zeebo screen is 640×480 VGA in RGB565 — the same framebuffer format that reverse
//! engineering captured from the console's TV encoder (`docs/vendor/tripleoxygen/research/tvenc/`).
//! We store the pixels in that format so that a `BitBlt` of a native bitmap is a straight copy.

/// A color in the format BREW uses in `RGBVAL`: `MAKE_RGB(r,g,b) = (r<<8) | (g<<16) | (b<<24)`,
/// per the SDK documentation — that is, the least significant byte is not color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
    };

    /// Converts an `RGBVAL` from the guest.
    pub fn from_rgbval(value: u32) -> Self {
        Self {
            r: (value >> 8) as u8,
            g: (value >> 16) as u8,
            b: (value >> 24) as u8,
        }
    }

    /// Packs into RGB565, the framebuffer format.
    pub fn to_rgb565(self) -> u16 {
        ((self.r as u16 & 0xf8) << 8) | ((self.g as u16 & 0xfc) << 3) | (self.b as u16 >> 3)
    }

    pub fn from_rgb565(value: u16) -> Self {
        // Replicates the high bits into the low ones so pure white stays 255, not 248.
        let r = ((value >> 11) & 0x1f) as u8;
        let g = ((value >> 5) & 0x3f) as u8;
        let b = (value & 0x1f) as u8;
        Self {
            r: (r << 3) | (r >> 2),
            g: (g << 2) | (g >> 4),
            b: (b << 3) | (b >> 2),
        }
    }
}

/// A BREW rectangle (`AEERect`): four `int16` — x, y, width, height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub width: i16,
    pub height: i16,
}

impl Rect {
    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        let read = |i: usize| i16::from_le_bytes([bytes[i], bytes[i + 1]]);
        Self {
            x: read(0),
            y: read(2),
            width: read(4),
            height: read(6),
        }
    }
}

#[derive(Debug)]
pub struct Framebuffer {
    width: u32,
    height: u32,
    pixels: Vec<u16>,
    /// How many pixels have actually been written — used to know whether there is anything to
    /// show.
    touched: u64,
    /// A number no other surface created in this run has.
    generation: u64,
    /// The `[x0, y0, x1, y1)` rectangle that changed since the last
    /// [`Framebuffer::take_dirty_rect`].
    dirty: Option<[u32; 4]>,
}

/// Where each new surface's [`Framebuffer::generation`] comes from.
static NEXT_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height) as usize],
            touched: 0,
            generation: NEXT_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            dirty: None,
        }
    }

    /// A number no other surface created in this run has.
    ///
    /// It tells this surface apart from another one that took its place at the same address: the
    /// dirty box only describes what changed **in this one**.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns and clears the `[x0, y0, x1, y1)` rectangle written since the last time.
    pub fn take_dirty_rect(&mut self) -> Option<[u32; 4]> {
        self.dirty.take()
    }

    /// The dirty box **without** clearing it.
    ///
    /// It exists for save states: recording what changed must not consume the notice, otherwise
    /// saving a state would erase the only information that tells the frontend which part of the
    /// screen changed — and the next frame would come out stale because of a `save state` nobody
    /// asked to see.
    pub fn dirty_rect(&self) -> Option<[u32; 4]> {
        self.dirty
    }

    /// The raw pixels, in reading order.
    pub fn pixels(&self) -> &[u16] {
        &self.pixels
    }

    /// Restores the pixels and the surface's history.
    ///
    /// This is the way back from a save state: the pixels come from the file, and `touched` and
    /// `generation` come back with them because they are how the frontend knows what changed.
    /// Without the `generation`, this surface would be mistaken for another one that had taken
    /// the same address.
    pub fn restore_state(
        &mut self,
        touched: u64,
        generation: u64,
        dirty: Option<[u32; 4]>,
        pixels: Vec<u16>,
    ) {
        debug_assert_eq!(pixels.len(), (self.width * self.height) as usize);
        self.pixels = pixels;
        self.touched = touched;
        self.generation = generation;
        self.dirty = dirty;
    }

    /// The pixels from `start` to `end` (pixel indices, end exclusive), as RGB565 bytes.
    pub fn rgb565_range(&self, start: usize, end: usize) -> Vec<u8> {
        self.pixels[start..end]
            .iter()
            .flat_map(|p| p.to_le_bytes())
            .collect()
    }

    /// Extends the dirty box to cover `[x0, x1) × [y0, y1)`.
    fn mark_watches(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) {
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.dirty = Some(match self.dirty {
            None => [x0, y0, x1, y1],
            Some([a, b, c, d]) => [a.min(x0), b.min(y0), c.max(x1), d.max(y1)],
        });
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Whether any drawing operation has touched the screen.
    /// How many writes the surface has received. Used to know whether it changed since a given
    /// moment.
    pub fn writes(&self) -> u64 {
        self.touched
    }

    pub fn is_dirty(&self) -> bool {
        self.touched > 0
    }

    pub fn set_pixel(&mut self, x: i32, y: i32, color: Rgb) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.pixels[(y as u32 * self.width + x as u32) as usize] = color.to_rgb565();
        self.touched += 1;
        self.mark_watches(x as u32, y as u32, x as u32 + 1, y as u32 + 1);
    }

    /// Fills a rectangle, clipping whatever falls off the screen.
    pub fn fill_rect(&mut self, rect: Rect, color: Rgb) {
        let value = color.to_rgb565();
        let x0 = rect.x.max(0) as u32;
        let y0 = rect.y.max(0) as u32;
        let x1 = ((rect.x as i32 + rect.width as i32).max(0) as u32).min(self.width);
        let y1 = ((rect.y as i32 + rect.height as i32).max(0) as u32).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                self.pixels[(y * self.width + x) as usize] = value;
                self.touched += 1;
            }
        }
        self.mark_watches(x0, y0, x1, y1);
    }

    /// Draws a rectangle's outline, one pixel thick.
    pub fn draw_frame(&mut self, rect: Rect, color: Rgb) {
        let (x, y) = (rect.x as i32, rect.y as i32);
        let (w, h) = (rect.width as i32, rect.height as i32);
        if w <= 0 || h <= 0 {
            return;
        }
        for i in 0..w {
            self.set_pixel(x + i, y, color);
            self.set_pixel(x + i, y + h - 1, color);
        }
        for i in 0..h {
            self.set_pixel(x, y + i, color);
            self.set_pixel(x + w - 1, y + i, color);
        }
    }

    /// Reads a pixel in the native format (RGB565). Off screen it returns zero.
    pub fn get_pixel(&self, x: i32, y: i32) -> u16 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return 0;
        }
        self.pixels[(y as u32 * self.width + x as u32) as usize]
    }

    /// Writes a pixel already in the native format.
    pub fn set_pixel_native(&mut self, x: i32, y: i32, value: u16) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.pixels[(y as u32 * self.width + x as u32) as usize] = value;
        self.touched += 1;
        self.mark_watches(x as u32, y as u32, x as u32 + 1, y as u32 + 1);
    }

    /// Fills a rectangle with a color already in the native format.
    pub fn fill_rect_native(&mut self, rect: Rect, value: u16) {
        let x1 = (rect.x as i32 + rect.width as i32)
            .max(0)
            .min(self.width as i32);
        let y1 = (rect.y as i32 + rect.height as i32)
            .max(0)
            .min(self.height as i32);
        let (x0, y0) = (rect.x.max(0) as i32, rect.y.max(0) as i32);
        for y in y0..y1 {
            for x in x0..x1 {
                self.pixels[(y as u32 * self.width + x as u32) as usize] = value;
                self.touched += 1;
            }
        }
        if x0 < x1 && y0 < y1 {
            self.mark_watches(x0 as u32, y0 as u32, x1 as u32, y1 as u32);
        }
    }

    /// Copies a region of `src` into this surface.
    ///
    /// `transparent` marks the color to skip — that is how BREW does sprites with
    /// `AEE_RO_TRANSPARENT`. Passing `None` copies everything.
    ///
    /// The signature is wide on purpose: it mirrors BREW's `BltIn`, and regrouping the
    /// parameters into structs would only move the code away from the API it implements.
    #[allow(clippy::too_many_arguments)]
    pub fn blit(
        &mut self,
        dst_x: i32,
        dst_y: i32,
        width: i32,
        height: i32,
        src: &Framebuffer,
        src_x: i32,
        src_y: i32,
        transparent: Option<u16>,
    ) {
        for row in 0..height {
            for col in 0..width {
                let (sx, sy) = (src_x + col, src_y + row);
                if sx < 0 || sy < 0 || sx >= src.width as i32 || sy >= src.height as i32 {
                    continue;
                }
                let value = src.get_pixel(sx, sy);
                if Some(value) == transparent {
                    continue;
                }
                self.set_pixel_native(dst_x + col, dst_y + row, value);
            }
        }
    }

    /// Draws a line with Bresenham's algorithm — integers only, like the hardware of the time.
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgb) {
        let value = color.to_rgb565();
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut error) = (x0, y0, dx + dy);
        loop {
            self.set_pixel_native(x, y, value);
            if x == x1 && y == y1 {
                break;
            }
            let doubled = error * 2;
            if doubled >= dy {
                error += dy;
                x += sx;
            }
            if doubled <= dx {
                error += dx;
                y += sy;
            }
        }
    }

    /// A circle outline with the midpoint algorithm, mirroring the eight octants.
    pub fn draw_circle(&mut self, cx: i32, cy: i32, radius: i32, color: Rgb) {
        if radius < 0 {
            return;
        }
        let value = color.to_rgb565();
        let (mut x, mut y) = (radius, 0);
        let mut error = 1 - radius;
        while x >= y {
            for (px, py) in [
                (cx + x, cy + y),
                (cx + y, cy + x),
                (cx - y, cy + x),
                (cx - x, cy + y),
                (cx - x, cy - y),
                (cx - y, cy - x),
                (cx + y, cy - x),
                (cx + x, cy - y),
            ] {
                self.set_pixel_native(px, py, value);
            }
            y += 1;
            if error < 0 {
                error += 2 * y + 1;
            } else {
                x -= 1;
                error += 2 * (y - x) + 1;
            }
        }
    }

    /// Fills a circle by scanning row by row.
    pub fn fill_circle(&mut self, cx: i32, cy: i32, radius: i32, color: Rgb) {
        let value = color.to_rgb565();
        for dy in -radius..=radius {
            let half = ((radius * radius - dy * dy) as f64).sqrt() as i32;
            for dx in -half..=half {
                self.set_pixel_native(cx + dx, cy + dy, value);
            }
        }
    }

    /// Fills a polygon by the parity rule of the intersections with each row.
    pub fn fill_polygon(&mut self, points: &[(i32, i32)], color: Rgb) {
        if points.len() < 3 {
            return;
        }
        let value = color.to_rgb565();
        let top = points.iter().map(|p| p.1).min().unwrap_or(0).max(0);
        let bottom = points
            .iter()
            .map(|p| p.1)
            .max()
            .unwrap_or(0)
            .min(self.height as i32 - 1);
        for y in top..=bottom {
            let mut crossings: Vec<i32> = Vec::new();
            for i in 0..points.len() {
                let (x0, y0) = points[i];
                let (x1, y1) = points[(i + 1) % points.len()];
                if (y0 <= y && y1 > y) || (y1 <= y && y0 > y) {
                    let t = (y - y0) as f64 / (y1 - y0) as f64;
                    crossings.push(x0 + (t * (x1 - x0) as f64) as i32);
                }
            }
            crossings.sort_unstable();
            for pair in crossings.chunks(2) {
                if let [start, end] = pair {
                    for x in *start..=*end {
                        self.set_pixel_native(x, y, value);
                    }
                }
            }
        }
    }

    /// The pixels as RGB565 bytes, in the order `IDIB` exposes them to the game.
    /// Writes the frame as RGB565 into the given buffer, reusing its allocation.
    ///
    /// [`Framebuffer::to_rgb565_bytes`] allocates a new `Vec` every frame; in a frontend that
    /// delivers 60 frames per second that is garbage every 16 ms, and the collector shows up as
    /// stutter.
    pub fn write_rgb565_into(&self, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(self.pixels.len() * 2);
        for pixel in &self.pixels {
            out.extend_from_slice(&pixel.to_le_bytes());
        }
    }

    /// A cheap signature of the screen contents, to recognize a repeated frame.
    ///
    /// It is not a cryptographic hash: it is enough to say "nothing changed" without paying for
    /// an expensive pass over 600 KB per frame.
    pub fn signature(&self) -> u64 {
        let mut value: u64 = 0xcbf2_9ce4_8422_2325;
        for pixel in &self.pixels {
            value ^= u64::from(*pixel);
            value = value.wrapping_mul(0x100_0000_01b3);
        }
        value
    }

    pub fn to_rgb565_bytes(&self) -> Vec<u8> {
        self.pixels.iter().flat_map(|p| p.to_le_bytes()).collect()
    }

    /// Paints an RGB565 image onto this surface.
    ///
    /// Same size and a packed pitch replace the pixels. Anything else is scaled with
    /// nearest neighbor. Returns whether the buffer was large enough to read.
    pub fn paint_rgb565(&mut self, src: &[u8], width: u32, height: u32, pitch: u32) -> bool {
        if width == 0 || height == 0 || self.width == 0 || self.height == 0 {
            return false;
        }
        let row = width as usize * 2;
        if (pitch as usize) < row || src.len() < (pitch as usize) * (height as usize) {
            return false;
        }
        let (dw, dh) = (self.width, self.height);
        if width == dw && height == dh && pitch as usize == row {
            self.load_rgb565_bytes(src);
        } else {
            for y in 0..dh {
                let sy = (y as u64 * height as u64 / dh as u64) as usize;
                let base = sy * pitch as usize;
                for x in 0..dw {
                    let sx = (x as u64 * width as u64 / dw as u64) as usize;
                    let at = base + sx * 2;
                    self.pixels[(y * dw + x) as usize] =
                        u16::from_le_bytes([src[at], src[at + 1]]);
                }
            }
            self.touched = self.touched.saturating_add(1);
        }
        self.mark_watches(0, 0, dw, dh);
        true
    }

    /// Reloads the pixels from the buffer the game wrote.
    pub fn load_rgb565_bytes(&mut self, bytes: &[u8]) {
        for (pixel, chunk) in self.pixels.iter_mut().zip(bytes.chunks_exact(2)) {
            let value = u16::from_le_bytes([chunk[0], chunk[1]]);
            if *pixel != value {
                *pixel = value;
                self.touched += 1;
            }
        }
    }

    /// Reloads a surface already converted to RGB565 words.
    pub fn load_rgb565_words(&mut self, words: &[u16]) {
        for (pixel, &value) in self.pixels.iter_mut().zip(words) {
            if *pixel != value {
                *pixel = value;
                self.touched += 1;
            }
        }
    }

    /// Bytes per row in the `IDIB` buffer: the RGB565 width aligned to 4 bytes.
    ///
    /// For an even width it is the same `width * 2`; for an odd one there is one padding pixel at
    /// the end of each row. Action Hero 3D walks the rows of an odd-width image with a stride of
    /// `cx + 1` — without the padding, the 149-pixel logo and the 179-pixel portraits came out
    /// with the rows sliding and pieces cut off.
    pub fn dib_pitch(&self) -> usize {
        (self.width as usize * 2).div_ceil(4) * 4
    }

    /// Like [`Framebuffer::to_rgb565_bytes`], with the rows at the `IDIB` stride.
    pub fn to_dib_bytes(&self) -> Vec<u8> {
        let step = self.dib_pitch();
        let width = self.width as usize;
        if step == width * 2 {
            return self.to_rgb565_bytes();
        }
        let mut bytes = vec![0u8; step * self.height as usize];
        for (line, pixels) in self.pixels.chunks_exact(width).enumerate() {
            for (column, pixel) in pixels.iter().enumerate() {
                let at = line * step + column * 2;
                bytes[at..at + 2].copy_from_slice(&pixel.to_le_bytes());
            }
        }
        bytes
    }

    /// Writes the frame as RGB565 **straight into someone else's buffer**, with its row pitch.
    ///
    /// It exists because of `RETRO_ENVIRONMENT_GET_CURRENT_SOFTWARE_FRAMEBUFFER`: the frontend
    /// lends the core a buffer to draw into, and the pitch may not be `width * 2`. Without this,
    /// the frame would go through a vector of ours and then through a copy — which is exactly the
    /// copy the loan exists to avoid.
    ///
    /// `out` comes sized `pitch * height`; whatever is left at the end of each row (the pitch
    /// being larger than the width) stays as it was, which is what the frontend expects.
    pub fn write_rgb565_with_pitch(&self, out: &mut [u8], pitch: usize) {
        let width = self.width as usize;
        let (row_bytes, step) = (width * 2, pitch.max(width * 2));
        for (y, line) in self.pixels.chunks_exact(width).enumerate() {
            let start = y * step;
            let Some(dest) = out.get_mut(start..start + row_bytes) else {
                return;
            };
            for (pair, pixel) in dest.chunks_exact_mut(2).zip(line) {
                pair.copy_from_slice(&pixel.to_le_bytes());
            }
        }
    }

    /// Like [`Framebuffer::load_rgb565_bytes`], with the rows at the `IDIB` stride.
    pub fn load_dib_bytes(&mut self, bytes: &[u8]) {
        let step = self.dib_pitch();
        let width = self.width as usize;
        if step == width * 2 {
            return self.load_rgb565_bytes(bytes);
        }
        for (pixels, origin) in self.pixels.chunks_exact_mut(width).zip(bytes.chunks(step)) {
            for (pixel, chunk) in pixels.iter_mut().zip(origin.chunks_exact(2)) {
                let value = u16::from_le_bytes([chunk[0], chunk[1]]);
                if *pixel != value {
                    *pixel = value;
                    self.touched += 1;
                }
            }
        }
    }

    /// The pixels as `0x00RRGGBB`, which is the format the host's windows expect.
    pub fn to_argb(&self) -> Vec<u32> {
        self.pixels
            .iter()
            .map(|&p| {
                // Repeating the highest bits into the missing ones spreads the value over the
                // whole range: that is what makes 0b11111 become 255 and not 248.
                let r = ((p >> 11) & 0x1f) as u32;
                let g = ((p >> 5) & 0x3f) as u32;
                let b = (p & 0x1f) as u32;
                (((r << 3) | (r >> 2)) << 16) | (((g << 2) | (g >> 4)) << 8) | ((b << 3) | (b >> 2))
            })
            .collect()
    }

    /// Inverts the bits of a rectangle — BREW's `AEE_RO_XOR`.
    pub fn xor_rect_native(&mut self, rect: Rect, value: u16) {
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                let (x, y) = (x as i32, y as i32);
                let current = self.get_pixel(x, y);
                self.set_pixel_native(x, y, current ^ value);
            }
        }
    }

    /// Serializes as a 24-bit BMP — a format any viewer opens and that can be written with no
    /// dependency at all.
    pub fn to_bmp(&self) -> Vec<u8> {
        // Each BMP row is aligned to 4 bytes, and the rows go bottom to top.
        let row_padding = (4 - (self.width as usize * 3) % 4) % 4;
        let row_size = self.width as usize * 3 + row_padding;
        let pixel_data = row_size * self.height as usize;
        const HEADER: usize = 54;

        let mut out = Vec::with_capacity(HEADER + pixel_data);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&((HEADER + pixel_data) as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // reserved
        out.extend_from_slice(&(HEADER as u32).to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes()); // size of BITMAPINFOHEADER
        out.extend_from_slice(&(self.width as i32).to_le_bytes());
        out.extend_from_slice(&(self.height as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&0u32.to_le_bytes()); // no compression
        out.extend_from_slice(&(pixel_data as u32).to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes()); // ~72 dpi
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // colors in the palette
        out.extend_from_slice(&0u32.to_le_bytes()); // important colors

        for y in (0..self.height).rev() {
            for x in 0..self.width {
                let color = Rgb::from_rgb565(self.pixels[(y * self.width + x) as usize]);
                out.extend_from_slice(&[color.b, color.g, color.r]);
            }
            out.extend(std::iter::repeat_n(0u8, row_padding));
        }
        out
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn paint_rgb565_replaces_a_packed_frame_and_scales_a_smaller_one() {
        let mut fb = Framebuffer::new(2, 2);
        let packed = [0x34u8, 0x12, 0x78, 0x56, 0xbc, 0x9a, 0xf0, 0xde];
        assert!(fb.paint_rgb565(&packed, 2, 2, 4));
        assert_eq!(fb.get_pixel(0, 0), 0x1234);
        assert_eq!(fb.get_pixel(1, 1), 0xdef0);

        let mut scaled = Framebuffer::new(2, 2);
        let one = [0x00u8, 0xf8];
        assert!(scaled.paint_rgb565(&one, 1, 1, 2));
        assert_eq!(scaled.get_pixel(0, 0), 0xf800);
        assert_eq!(scaled.get_pixel(1, 1), 0xf800);
        assert!(!scaled.paint_rgb565(&one, 2, 2, 4));
    }

    #[test]
    fn odd_width_gets_one_padding_pixel_per_row_in_the_dib() {
        let mut fb = Framebuffer::new(3, 2);
        fb.set_pixel_native(0, 1, 0x1234);
        fb.set_pixel_native(2, 1, 0xabcd);
        assert_eq!(fb.dib_pitch(), 8);
        let bytes = fb.to_dib_bytes();
        assert_eq!(bytes.len(), 16);
        // The second row starts at byte 8, not 6.
        assert_eq!(&bytes[8..10], &0x1234u16.to_le_bytes());
        assert_eq!(&bytes[12..14], &0xabcdu16.to_le_bytes());

        let mut tick = Framebuffer::new(3, 2);
        tick.load_dib_bytes(&bytes);
        assert_eq!(tick.get_pixel(0, 1), 0x1234);
        assert_eq!(tick.get_pixel(2, 1), 0xabcd);
        // For an even width nothing changes.
        assert_eq!(Framebuffer::new(4, 1).dib_pitch(), 8);
    }

    #[test]
    fn conversion_for_the_window_spreads_the_bits_to_the_end_of_the_range() {
        let mut fb = Framebuffer::new(2, 1);
        fb.load_rgb565_bytes(&[0xff, 0xff, 0x00, 0x00]);
        // White in RGB565 has to become real white, not 0xf8fcf8.
        assert_eq!(fb.to_argb(), vec![0x00ff_ffff, 0x0000_0000]);
    }
    use super::*;

    #[test]
    fn converts_rgbval_from_guest() {
        // MAKE_RGB(0x12, 0x34, 0x56) = 0x56341200
        let color = Rgb::from_rgbval(0x5634_1200);
        assert_eq!(
            color,
            Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56
            }
        );
    }

    #[test]
    fn rgb565_preserves_white_and_black() {
        assert_eq!(Rgb::BLACK.to_rgb565(), 0x0000);
        assert_eq!(Rgb::WHITE.to_rgb565(), 0xffff);
        assert_eq!(Rgb::from_rgb565(0xffff), Rgb::WHITE);
        assert_eq!(Rgb::from_rgb565(0x0000), Rgb::BLACK);
    }

    #[test]
    fn fill_rect_clips_at_the_edge() {
        let mut fb = Framebuffer::new(4, 4);
        fb.fill_rect(
            Rect {
                x: 2,
                y: 2,
                width: 10,
                height: 10,
            },
            Rgb::WHITE,
        );
        assert_eq!(
            fb.pixels,
            vec![
                0, 0, 0, 0, //
                0, 0, 0, 0, //
                0, 0, 0xffff, 0xffff, //
                0, 0, 0xffff, 0xffff,
            ]
        );
    }

    #[test]
    fn offscreen_rect_does_not_overflow() {
        let mut fb = Framebuffer::new(4, 4);
        fb.fill_rect(
            Rect {
                x: -20,
                y: -20,
                width: 5,
                height: 5,
            },
            Rgb::WHITE,
        );
        fb.fill_rect(
            Rect {
                x: 100,
                y: 100,
                width: 5,
                height: 5,
            },
            Rgb::WHITE,
        );
        assert!(!fb.is_dirty());
    }

    #[test]
    fn reads_aeerect_from_guest() {
        let rect = Rect::from_bytes([10, 0, 20, 0, 30, 0, 40, 0]);
        assert_eq!(
            rect,
            Rect {
                x: 10,
                y: 20,
                width: 30,
                height: 40
            }
        );
    }

    #[test]
    fn horizontal_line_paints_every_pixel() {
        let mut fb = Framebuffer::new(8, 8);
        fb.draw_line(1, 3, 6, 3, Rgb::WHITE);
        for x in 1..=6 {
            assert_eq!(fb.get_pixel(x, 3), 0xffff, "missing pixel at x={x}");
        }
        assert_eq!(fb.get_pixel(0, 3), 0);
        assert_eq!(fb.get_pixel(7, 3), 0);
    }

    #[test]
    fn diagonal_line_joins_both_ends() {
        let mut fb = Framebuffer::new(8, 8);
        fb.draw_line(0, 0, 7, 7, Rgb::WHITE);
        assert_eq!(fb.get_pixel(0, 0), 0xffff);
        assert_eq!(fb.get_pixel(4, 4), 0xffff);
        assert_eq!(fb.get_pixel(7, 7), 0xffff);
    }

    #[test]
    fn offscreen_line_does_not_overflow() {
        let mut fb = Framebuffer::new(4, 4);
        fb.draw_line(-100, -100, 100, 100, Rgb::WHITE);
        assert_eq!(fb.get_pixel(0, 0), 0xffff);
    }

    #[test]
    fn circle_touches_all_four_extremes() {
        let mut fb = Framebuffer::new(16, 16);
        fb.draw_circle(8, 8, 4, Rgb::WHITE);
        assert_eq!(fb.get_pixel(12, 8), 0xffff);
        assert_eq!(fb.get_pixel(4, 8), 0xffff);
        assert_eq!(fb.get_pixel(8, 12), 0xffff);
        assert_eq!(fb.get_pixel(8, 4), 0xffff);
        assert_eq!(fb.get_pixel(8, 8), 0, "the middle stays empty: it is only the outline");
    }

    #[test]
    fn filled_circle_paints_the_center() {
        let mut fb = Framebuffer::new(16, 16);
        fb.fill_circle(8, 8, 4, Rgb::WHITE);
        assert_eq!(fb.get_pixel(8, 8), 0xffff);
        assert_eq!(fb.get_pixel(8, 15), 0, "outside the radius stays empty");
    }

    #[test]
    fn filled_polygon_covers_the_interior() {
        let mut fb = Framebuffer::new(10, 10);
        // A square from (2,2) to (7,7).
        fb.fill_polygon(&[(2, 2), (7, 2), (7, 7), (2, 7)], Rgb::WHITE);
        assert_eq!(fb.get_pixel(4, 4), 0xffff);
        assert_eq!(fb.get_pixel(2, 2), 0xffff);
        assert_eq!(fb.get_pixel(1, 1), 0, "outside the polygon stays empty");
    }

    #[test]
    fn blit_copies_region_and_respects_transparency() {
        let mut src = Framebuffer::new(2, 2);
        src.set_pixel_native(0, 0, 0x1234);
        src.set_pixel_native(1, 0, 0xf81f); // color marked as transparent
        src.set_pixel_native(0, 1, 0x4321);

        let mut dst = Framebuffer::new(2, 2);
        dst.blit(0, 0, 2, 2, &src, 0, 0, Some(0xf81f));
        assert_eq!(dst.get_pixel(0, 0), 0x1234);
        assert_eq!(
            dst.get_pixel(1, 0),
            0,
            "a transparent pixel must not be copied"
        );
        assert_eq!(dst.get_pixel(0, 1), 0x4321);
    }

    #[test]
    fn blit_clips_at_the_edges_of_both_surfaces() {
        let mut src = Framebuffer::new(2, 2);
        src.fill_rect_native(
            Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            0xffff,
        );
        let mut dst = Framebuffer::new(2, 2);
        // Source and destination out of bounds: it must not overflow or write anything.
        dst.blit(-5, -5, 2, 2, &src, 10, 10, None);
        assert!(!dst.is_dirty());
    }

    #[test]
    fn bmp_has_correct_header_and_size() {
        let mut fb = Framebuffer::new(2, 2);
        fb.set_pixel(0, 0, Rgb::WHITE);
        let bmp = fb.to_bmp();
        assert_eq!(&bmp[0..2], b"BM");
        // 2 pixels per row = 6 bytes, rounded up to 8; two rows = 16.
        assert_eq!(bmp.len(), 54 + 16);
        assert_eq!(
            u32::from_le_bytes([bmp[2], bmp[3], bmp[4], bmp[5]]),
            bmp.len() as u32
        );
    }
}
