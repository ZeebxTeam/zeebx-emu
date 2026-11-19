//! GIF reading — the format of the Z-Wheel's opening animation.
//!
//! The game asks for `opening_low.gif` through `ISHELL_LoadResObject`, and cannot get past it
//! without the file: when the load fails, `AnimationVideo_Form.c` prints `Missing startup
//! animated GIF` and returns an error — and its cleanup path **enters infinite recursion**,
//! because it calls the object's own `+0x20` field, which points to the cleanup function. On the
//! console that load never fails, so the loop does not exist; here it showed up as a stack
//! overflow at `0x1177c`.
//!
//! Decoding is our own, like the rest of the imaging in this emulator. GIF is simple enough for
//! that to be reasonable: a palette, an LZW and a handful of blocks.
//!
//! **Frames become a horizontal strip.** That is what the rest of the emulator already
//! understands: an image with a `frame_width` smaller than its width is a sequence, and
//! `IIMAGE_DrawFrame` picks the column. That way the animation needs no path of its own.

/// A decoded GIF: the strip of frames and the size of each one.
pub struct Gif {
    /// Width of **one** frame.
    pub width: u32,
    pub height: u32,
    /// The frames, each with `width * height` RGBA pixels.
    pub frames: Vec<Vec<[u8; 4]>>,
}

/// What a frame does to the screen before the next one, from the graphic control block.
#[derive(Clone, Copy, PartialEq)]
enum Disposal {
    /// Leave it as is — which is also what "unspecified" means in practice.
    Keep,
    /// Restore the background, which for us is transparent.
    Background,
    /// Restore what was there before this frame.
    Previous,
}

/// A byte reader that never overruns: when the file ends, decoding ends.
///
/// GIF is a network format, and truncated files happen. Without this, every read would need its
/// own `get()?`, and the error would show up as a panic instead of "that didn't work".
struct ByteReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> ByteReader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(b)
    }

    fn word(&mut self) -> Option<u16> {
        Some(u16::from(self.byte()?) | u16::from(self.byte()?) << 8)
    }

    fn chunk(&mut self, count: usize) -> Option<&'a [u8]> {
        let output = self.bytes.get(self.at..self.at + count)?;
        self.at += count;
        Some(output)
    }

    /// Joins the sub-blocks up to the zero-sized one, which is how GIF ends every list.
    fn sub_blocks(&mut self) -> Option<Vec<u8>> {
        let mut output = Vec::new();
        loop {
            let n = self.byte()? as usize;
            if n == 0 {
                return Some(output);
            }
            output.extend_from_slice(self.chunk(n)?);
        }
    }

    /// Skips the sub-blocks without keeping them.
    fn skip_sub_blocks(&mut self) -> Option<()> {
        self.sub_blocks().map(|_| ())
    }
}

/// Reads a color table of `count` entries, three bytes each.
fn palette(reader: &mut ByteReader, count: usize) -> Option<Vec<[u8; 3]>> {
    let raw = reader.chunk(count * 3)?;
    Some(raw.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
}

/// Decodes the GIF, or returns `None` if it is not one.
pub fn decode(bytes: &[u8]) -> Option<Gif> {
    let mut reader = ByteReader { bytes, at: 0 };
    let signature = reader.chunk(6)?;
    if signature != b"GIF87a" && signature != b"GIF89a" {
        return None;
    }
    let width = reader.word()? as usize;
    let height = reader.word()? as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let fields = reader.byte()?;
    let _background = reader.byte()?;
    let _aspect_ratio = reader.byte()?;
    let global_palette = match fields & 0x80 != 0 {
        true => palette(&mut reader, 2 << (fields & 7))?,
        false => Vec::new(),
    };

    // The screen starts fully transparent. A GIF whose first frame does not cover everything
    // counts on that — and zero alpha is what the rest of the emulator takes as "do not draw this
    // pixel".
    let mut screen = vec![[0u8; 4]; width * height];
    let mut frames: Vec<Vec<[u8; 4]>> = Vec::new();
    let (mut transparent, mut disposal) = (None, Disposal::Keep);

    loop {
        match reader.byte()? {
            // End of file.
            0x3b => break,
            // Extension. Only the graphic control one matters to us; the others are comment,
            // text and application, and all of them end in sub-blocks.
            0x21 => {
                let kind = reader.byte()?;
                if kind == 0xf9 {
                    let size = reader.byte()? as usize;
                    let block = reader.chunk(size)?;
                    let fields = *block.first()?;
                    transparent = (fields & 1 != 0).then(|| *block.get(3).unwrap_or(&0));
                    disposal = match (fields >> 2) & 7 {
                        2 => Disposal::Background,
                        3 => Disposal::Previous,
                        _ => Disposal::Keep,
                    };
                    reader.skip_sub_blocks()?;
                } else {
                    reader.skip_sub_blocks()?;
                }
            }
            // A frame.
            0x2c => {
                let left = reader.word()? as usize;
                let top = reader.word()? as usize;
                let frame_width = reader.word()? as usize;
                let frame_height = reader.word()? as usize;
                let fields = reader.byte()?;
                let colors = match fields & 0x80 != 0 {
                    true => palette(&mut reader, 2 << (fields & 7))?,
                    false => global_palette.clone(),
                };
                let interlaced = fields & 0x40 != 0;
                let indices = decompress(&mut reader, frame_width * frame_height)?;

                let saved = (disposal == Disposal::Previous).then(|| screen.clone());
                paint(
                    &mut screen,
                    width,
                    height,
                    (left, top, frame_width, frame_height),
                    &indices,
                    &colors,
                    transparent,
                    interlaced,
                );
                frames.push(screen.clone());
                match disposal {
                    Disposal::Keep => {}
                    Disposal::Background => clear(
                        &mut screen,
                        width,
                        height,
                        (left, top, frame_width, frame_height),
                    ),
                    Disposal::Previous => {
                        if let Some(saved) = saved {
                            screen = saved;
                        }
                    }
                }
                // The graphic control applies to **one** frame only.
                transparent = None;
                disposal = Disposal::Keep;
            }
            // A byte that opens no block: a corrupt file, and stopping here is better than going
            // on interpreting garbage. What was already read still counts.
            _ => break,
        }
    }

    (!frames.is_empty()).then(|| Gif {
        width: width as u32,
        height: height as u32,
        frames,
    })
}

/// Writes the decoded indices to the screen, honoring clipping, transparency and interlacing.
#[allow(clippy::too_many_arguments)]
fn paint(
    screen: &mut [[u8; 4]],
    width: usize,
    height: usize,
    (left, top, frame_width, frame_height): (usize, usize, usize, usize),
    indices: &[u8],
    colors: &[[u8; 3]],
    transparent: Option<u8>,
    interlaced: bool,
) {
    for i in 0..frame_height.min(indices.len() / frame_width.max(1)) {
        // In an interlaced GIF the rows come in four passes, and the i-th row **read** is not
        // the i-th row of the image. Ignoring that does not break decoding: it just scrambles the
        // image into bands, which is a subtle defect that is hard to recognize later.
        let line = match interlaced {
            true => interlaced_row(i, frame_height),
            false => i,
        };
        for column in 0..frame_width {
            let index = indices[i * frame_width + column];
            if Some(index) == transparent {
                continue;
            }
            let (x, y) = (left + column, top + line);
            if x >= width || y >= height {
                continue;
            }
            let color = colors.get(index as usize).copied().unwrap_or([0, 0, 0]);
            screen[y * width + x] = [color[0], color[1], color[2], 0xff];
        }
    }
}

/// Which image row the `n`-th row read is, in an interlaced GIF.
///
/// There are four passes: rows 0, 8, 16…; then 4, 12, 20…; then 2, 6, 10…; then the odd
/// ones.
fn interlaced_row(n: usize, height: usize) -> usize {
    let eighths = height.div_ceil(8);
    let quarters = height.saturating_sub(4).div_ceil(8);
    let seconds = height.saturating_sub(2).div_ceil(4);
    if n < eighths {
        return n * 8;
    }
    if n < eighths + quarters {
        return (n - eighths) * 8 + 4;
    }
    if n < eighths + quarters + seconds {
        return (n - eighths - quarters) * 4 + 2;
    }
    (n - eighths - quarters - seconds) * 2 + 1
}

/// Erases the frame's rectangle, leaving it transparent.
fn clear(
    screen: &mut [[u8; 4]],
    width: usize,
    height: usize,
    (left, top, frame_width, frame_height): (usize, usize, usize, usize),
) {
    for y in top..(top + frame_height).min(height) {
        for x in left..(left + frame_width).min(width) {
            screen[y * width + x] = [0; 4];
        }
    }
}

/// GIF's LZW, which is neither TIFF's nor `compress`'s.
///
/// The dictionary starts with one entry per color, plus the clear code and the end code. Each
/// new code is "the previous one plus the first byte of the current one", and the code size grows
/// on its own when the dictionary fills up — up to twelve bits, where it stops and waits for a
/// clear.
fn decompress(reader: &mut ByteReader, count: usize) -> Option<Vec<u8>> {
    let initial_bits = reader.byte()? as u32;
    if !(1..=11).contains(&initial_bits) {
        return None;
    }
    let data = reader.sub_blocks()?;

    let clear_code = 1u32 << initial_bits;
    let end = clear_code + 1;
    // Each entry is `(previous, byte)`; the first ones are the colors, with no previous.
    let initial: Vec<(u32, u8)> = (0..clear_code).map(|c| (u32::MAX, c as u8)).collect();
    let mut table = initial.clone();
    let mut bits = initial_bits + 1;
    let mut previous: Option<u32> = None;

    let mut output = Vec::with_capacity(count);
    let (mut accumulator, mut bit_count, mut at) = (0u32, 0u32, 0usize);
    let mut sequence = Vec::new();

    while output.len() < count {
        while bit_count < bits {
            let Some(&b) = data.get(at) else {
                // Truncated stream: hand over what we got, because half an image is still an
                // image, and the caller has no other way to tell the difference.
                return Some(output);
            };
            accumulator |= u32::from(b) << bit_count;
            bit_count += 8;
            at += 1;
        }
        let code = accumulator & ((1 << bits) - 1);
        accumulator >>= bits;
        bit_count -= bits;

        if code == clear_code {
            table.clone_from(&initial);
            table.push((u32::MAX, 0));
            table.push((u32::MAX, 0));
            bits = initial_bits + 1;
            previous = None;
            continue;
        }
        if code == end {
            break;
        }

        // The case where the code is not in the table yet is legitimate, and it is what makes
        // LZW compress repeated sequences well: it means "the previous one plus the first byte of
        // the previous one". Treating it as an error ruins perfectly valid images.
        let new_first;
        sequence.clear();
        if (code as usize) < table.len() {
            unroll(&table, code, &mut sequence);
            new_first = *sequence.first()?;
        } else {
            let previous = previous?;
            unroll(&table, previous, &mut sequence);
            new_first = *sequence.first()?;
            sequence.push(new_first);
        }
        output.extend_from_slice(&sequence);

        if let Some(previous) = previous
            && table.len() < 4096
        {
            table.push((previous, new_first));
            if table.len() == (1 << bits) && bits < 12 {
                bits += 1;
            }
        }
        previous = Some(code);
    }
    Some(output)
}

/// Builds a code's byte sequence, following the chain of prefixes.
fn unroll(table: &[(u32, u8)], code: u32, output: &mut Vec<u8>) {
    let mut current = code;
    // The ceiling is the table size: a chain longer than that only exists if it has a cycle,
    // and then what stopping buys is not hanging the emulator on a corrupt file.
    for _ in 0..table.len() {
        let Some(&(previous, byte)) = table.get(current as usize) else {
            break;
        };
        output.push(byte);
        if previous == u32::MAX {
            break;
        }
        current = previous;
    }
    output.reverse();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-written 2×2 GIF with four colors and no real compression: each pixel goes out as a
    /// literal code, with a clear in front and an end behind.
    fn four_pixel_gif() -> Vec<u8> {
        let mut g = Vec::new();
        g.extend_from_slice(b"GIF89a");
        g.extend_from_slice(&2u16.to_le_bytes()); // width
        g.extend_from_slice(&2u16.to_le_bytes()); // height
        g.push(0x80 | 1); // four-color global palette
        g.push(0);
        g.push(0);
        g.extend_from_slice(&[0xff, 0, 0]); // 0 red
        g.extend_from_slice(&[0, 0xff, 0]); // 1 green
        g.extend_from_slice(&[0, 0, 0xff]); // 2 blue
        g.extend_from_slice(&[0xff, 0xff, 0xff]); // 3 white
        g.push(0x2c);
        g.extend_from_slice(&0u16.to_le_bytes());
        g.extend_from_slice(&0u16.to_le_bytes());
        g.extend_from_slice(&2u16.to_le_bytes());
        g.extend_from_slice(&2u16.to_le_bytes());
        g.push(0); // no local palette, no interlacing
        g.push(2); // two bits per initial code
        // Clear(4), the four literals, end(5) — **with the width growing in the middle**.
        //
        // The dictionary starts with six entries (four colors, clear and end) and gains one per
        // code read from the second one on. When it reaches eight, the width goes from three to
        // four bits — and that already applies to the **next** code. Writing everything with
        // three bits, as I did in the first version of this test, produces a file no decoder
        // reads: the test failed blaming the decoder, which was right.
        let codes: [(u32, u32); 6] = [(4, 3), (0, 3), (1, 3), (2, 3), (3, 4), (5, 4)];
        let (mut acc, mut n, mut stream) = (0u32, 0u32, Vec::new());
        for (c, width) in codes {
            acc |= c << n;
            n += width;
            while n >= 8 {
                stream.push((acc & 0xff) as u8);
                acc >>= 8;
                n -= 8;
            }
        }
        if n > 0 {
            stream.push(acc as u8);
        }
        g.push(stream.len() as u8);
        g.extend_from_slice(&stream);
        g.push(0);
        g.push(0x3b);
        g
    }

    #[test]
    fn reads_the_four_pixels_in_the_right_order() {
        let gif = decode(&four_pixel_gif()).expect("should decode");
        assert_eq!((gif.width, gif.height), (2, 2));
        assert_eq!(gif.frames.len(), 1);
        assert_eq!(
            gif.frames[0],
            vec![
                [0xff, 0, 0, 0xff],
                [0, 0xff, 0, 0xff],
                [0, 0, 0xff, 0xff],
                [0xff, 0xff, 0xff, 0xff],
            ]
        );
    }

    #[test]
    fn rejects_what_is_not_a_gif() {
        assert!(decode(b"PNG\x00qualquercoisa").is_none());
        assert!(decode(b"").is_none());
    }

    /// Truncating at any point has to return `None` or an image — never a panic. A corrupt
    /// resource in a game's package must not bring down the emulator.
    #[test]
    fn a_truncated_file_does_not_crash() {
        let integer = four_pixel_gif();
        for end in 0..integer.len() {
            let _ = decode(&integer[..end]);
        }
    }

    #[test]
    fn interlacing_goes_through_all_four_passes() {
        let read_count: Vec<usize> = (0..8).map(|n| interlaced_row(n, 8)).collect();
        assert_eq!(read_count, vec![0, 4, 2, 6, 1, 3, 5, 7]);
    }
}
