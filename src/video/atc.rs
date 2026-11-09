//! Decoding of ATITC compressed textures, from `GL_AMD_compressed_ATC_texture`.
//!
//! It is the format the Zeebo's Adreno 130 accepts — the graphics core came from the ATI Imageon,
//! and the compression came along. Boomerang Sports Dodgeball loads **all** its textures this way,
//! without a single call to `glTexImage2D`.
//!
//! Like DXT1, the blocks are 4×4 texels: two colors and two index bits per texel. The difference
//! is in the highest bit of the first color, which chooses between interpolating the four palette
//! colors or reserving the first one for black.

/// Block side, in texels.
const BLOCK: usize = 4;

/// Expands a `bits`-bit value to the full range of a byte.
fn expand(value: u16, bits: u32) -> u8 {
    let max = (1u16 << bits) - 1;
    ((value as u32 * 255 + max as u32 / 2) / max as u32) as u8
}

fn rgb555(value: u16) -> [u8; 3] {
    [
        expand((value >> 10) & 0x1f, 5),
        expand((value >> 5) & 0x1f, 5),
        expand(value & 0x1f, 5),
    ]
}

fn rgb565(value: u16) -> [u8; 3] {
    [
        expand(value >> 11, 5),
        expand((value >> 5) & 0x3f, 6),
        expand(value & 0x1f, 5),
    ]
}

/// The four-color palette of an ATC block.
fn palette(c0: u16, c1: u16) -> [[u8; 3]; 4] {
    let (a, b) = (rgb555(c0), rgb565(c1));
    // Bit 15 of the first color chooses the mode. With it set, entry 0 is black, and that is how
    // the format represents hard cutouts without spending an alpha channel.
    if c0 & 0x8000 == 0 {
        let mix = |x: u8, y: u8, wx: u32, wy: u32| ((x as u32 * wx + y as u32 * wy) / 8) as u8;
        [
            a,
            std::array::from_fn(|i| mix(a[i], b[i], 5, 3)),
            std::array::from_fn(|i| mix(a[i], b[i], 3, 5)),
            b,
        ]
    } else {
        [
            [0, 0, 0],
            std::array::from_fn(|i| a[i].saturating_sub(b[i] / 4)),
            a,
            b,
        ]
    }
}

/// Reads the 8-byte color block at position `at` and writes the 16 texels to `out`, which is the
/// whole image in 8-bit RGBA.
fn decode_color_block(
    data: &[u8],
    at: usize,
    out: &mut [[u8; 4]],
    width: usize,
    height: usize,
    bx: usize,
    by: usize,
) {
    let c0 = u16::from_le_bytes([data[at], data[at + 1]]);
    let c1 = u16::from_le_bytes([data[at + 2], data[at + 3]]);
    let indices = u32::from_le_bytes([data[at + 4], data[at + 5], data[at + 6], data[at + 7]]);
    let colors = palette(c0, c1);
    for row in 0..BLOCK {
        for column in 0..BLOCK {
            let (x, y) = (bx * BLOCK + column, by * BLOCK + row);
            // The texture may not be a multiple of four; the block is still complete in the file,
            // and whatever goes past the edge is discarded.
            if x >= width || y >= height {
                continue;
            }
            let index = (indices >> ((row * BLOCK + column) * 2)) & 0b11;
            let rgb = colors[index as usize];
            out[y * width + x] = [rgb[0], rgb[1], rgb[2], 255];
        }
    }
}

/// Decodes an ATC image to 8-bit RGBA.
///
/// `explicit_alpha` tells apart the two formats the console uses: without it the block is 8 bytes
/// and color only; with it the block is 16, and the first eight carry a four-bit alpha per texel,
/// with no compression at all.
pub fn decode(data: &[u8], width: usize, height: usize, explicit_alpha: bool) -> Vec<[u8; 4]> {
    let mut out = vec![[0u8, 0, 0, 255]; width * height];
    let stride = if explicit_alpha { 16 } else { 8 };
    let (columns, rows) = (width.div_ceil(BLOCK), height.div_ceil(BLOCK));
    for by in 0..rows {
        for bx in 0..columns {
            let at = (by * columns + bx) * stride;
            if at + stride > data.len() {
                return out;
            }
            let color_at = if explicit_alpha { at + 8 } else { at };
            decode_color_block(data, color_at, &mut out, width, height, bx, by);
            if !explicit_alpha {
                continue;
            }
            for row in 0..BLOCK {
                let line = u16::from_le_bytes([data[at + row * 2], data[at + row * 2 + 1]]);
                for column in 0..BLOCK {
                    let (x, y) = (bx * BLOCK + column, by * BLOCK + row);
                    if x >= width || y >= height {
                        continue;
                    }
                    out[y * width + x][3] = expand((line >> (column * 4)) & 0xf, 4);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an ATC color block with the given two colors and indices.
    fn block(c0: u16, c1: u16, indices: u32) -> Vec<u8> {
        let mut bytes = c0.to_le_bytes().to_vec();
        bytes.extend_from_slice(&c1.to_le_bytes());
        bytes.extend_from_slice(&indices.to_le_bytes());
        bytes
    }

    #[test]
    fn the_interpolated_mode_uses_all_four_palette_colors() {
        // White and black at the ends, with one texel of each index in the first row.
        let data = block(0x7fff, 0x0000, 0b11_10_01_00);
        let pixels = decode(&data, 4, 4, false);
        assert_eq!(pixels[0], [255, 255, 255, 255]);
        assert_eq!(pixels[3], [0, 0, 0, 255]);
        // The middle ones fall between the two, and index 1 is the lighter of the two.
        assert!(pixels[1][0] > pixels[2][0]);
        assert!(pixels[2][0] > pixels[3][0]);
    }

    #[test]
    fn the_mode_bit_reserves_index_zero_for_black() {
        // Same pair of colors, now with bit 15 set: index 0 becomes black and 2 becomes the full
        // color. Confusing the two modes leaves the texture dark or washed out.
        let data = block(0xffff, 0x0000, 0b00_00_10_00);
        let pixels = decode(&data, 4, 4, false);
        assert_eq!(pixels[0], [0, 0, 0, 255]);
        assert_eq!(pixels[1], [255, 255, 255, 255]);
    }

    #[test]
    fn explicit_alpha_comes_in_four_bits_per_texel() {
        let mut data: Vec<u8> = Vec::new();
        // First row: alpha 0, 5, 10, 15.
        data.extend_from_slice(&0xfa50u16.to_le_bytes());
        data.extend_from_slice(&[0u8; 6]);
        data.extend_from_slice(&block(0x7fff, 0x7fff, 0));
        let pixels = decode(&data, 4, 4, true);
        assert_eq!(pixels[0][3], 0);
        assert_eq!(pixels[3][3], 255);
        assert!(pixels[1][3] < pixels[2][3]);
    }

    #[test]
    fn textures_not_a_multiple_of_four_do_not_overflow() {
        // 6×3 takes two blocks in width and one in height; whatever goes past the edge is
        // discarded.
        let mut data = block(0x7fff, 0x7fff, 0);
        data.extend_from_slice(&block(0x7fff, 0x7fff, 0));
        let pixels = decode(&data, 6, 3, false);
        assert_eq!(pixels.len(), 18);
        assert!(pixels.iter().all(|p| *p == [255, 255, 255, 255]));
    }

    #[test]
    fn truncated_data_returns_what_could_be_read() {
        let pixels = decode(&[0u8; 4], 4, 4, false);
        assert_eq!(pixels.len(), 16);
    }
}
