//! Paletted textures from `OES_compressed_paletted_texture`.
//!
//! They are part of the OpenGL ES 1.1 core and come in through the same `glCompressedTexImage2D`
//! as the Adreno textures. The block starts with the palette — 16 colors in the `PALETTE4`
//! formats and 256 in the `PALETTE8` ones — followed by the indices, one per texel, packed two per
//! byte in the `PALETTE4` formats.
//!
//! The `level` of these formats is **non-positive**: zero means only the base level, and a
//! negative value says how many mipmap levels follow it. Since we only use the base level, what
//! matters is always the first run of indices after the palette.

use crate::video::gles;

/// How each palette entry is encoded, and how many entries it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    /// Bits per index: 4 or 8.
    index_bits: u32,
    /// Bytes per palette entry.
    entry_bytes: usize,
    kind: Entry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    Rgb8,
    Rgba8,
    Rgb565,
    Rgba4,
    Rgb5A1,
}

impl Format {
    /// The format for an OpenGL `internalformat`, or `None` if it is not paletted.
    pub fn from_gl(format: u32) -> Option<Self> {
        let (index_bits, kind) = match format {
            gles::GL_PALETTE4_RGB8_OES => (4, Entry::Rgb8),
            gles::GL_PALETTE4_RGBA8_OES => (4, Entry::Rgba8),
            gles::GL_PALETTE4_R5_G6_B5_OES => (4, Entry::Rgb565),
            gles::GL_PALETTE4_RGBA4_OES => (4, Entry::Rgba4),
            gles::GL_PALETTE4_RGB5_A1_OES => (4, Entry::Rgb5A1),
            gles::GL_PALETTE8_RGB8_OES => (8, Entry::Rgb8),
            gles::GL_PALETTE8_RGBA8_OES => (8, Entry::Rgba8),
            gles::GL_PALETTE8_R5_G6_B5_OES => (8, Entry::Rgb565),
            gles::GL_PALETTE8_RGBA4_OES => (8, Entry::Rgba4),
            gles::GL_PALETTE8_RGB5_A1_OES => (8, Entry::Rgb5A1),
            _ => return None,
        };
        let entry_bytes = match kind {
            Entry::Rgb8 => 3,
            Entry::Rgba8 => 4,
            Entry::Rgb565 | Entry::Rgba4 | Entry::Rgb5A1 => 2,
        };
        Some(Self {
            index_bits,
            entry_bytes,
            kind,
        })
    }

    fn entries(self) -> usize {
        1 << self.index_bits
    }
}

/// Expands a `bits`-bit value to the full range of a byte.
fn expand(value: u16, bits: u32) -> u8 {
    let max = (1u16 << bits) - 1;
    ((value as u32 * 255 + max as u32 / 2) / max as u32) as u8
}

fn decode_entry(kind: Entry, bytes: &[u8]) -> [u8; 4] {
    match kind {
        Entry::Rgb8 => [bytes[0], bytes[1], bytes[2], 255],
        Entry::Rgba8 => [bytes[0], bytes[1], bytes[2], bytes[3]],
        // The 16-bit formats are `GLushort`, and they are in the device's byte order —
        // little-endian on ARM. Read the other way round, the Double Dragon logo comes out with
        // rainbow jaggies instead of gold; that is how the order was settled.
        Entry::Rgb565 => {
            let v = u16::from_le_bytes([bytes[0], bytes[1]]);
            [
                expand(v >> 11, 5),
                expand((v >> 5) & 0x3f, 6),
                expand(v & 0x1f, 5),
                255,
            ]
        }
        Entry::Rgba4 => {
            let v = u16::from_le_bytes([bytes[0], bytes[1]]);
            [
                expand((v >> 12) & 0xf, 4),
                expand((v >> 8) & 0xf, 4),
                expand((v >> 4) & 0xf, 4),
                expand(v & 0xf, 4),
            ]
        }
        Entry::Rgb5A1 => {
            let v = u16::from_le_bytes([bytes[0], bytes[1]]);
            [
                expand((v >> 11) & 0x1f, 5),
                expand((v >> 6) & 0x1f, 5),
                expand((v >> 1) & 0x1f, 5),
                match v & 1 {
                    0 => 0,
                    _ => 255,
                },
            ]
        }
    }
}

/// Decodes the base level of a paletted texture to 8-bit RGBA.
///
/// Returns `None` when the data does not even cover the palette. Indices past the end of the
/// block become transparent, because a truncated texture has to come out incomplete, not bring
/// down the emulator.
pub fn decode(data: &[u8], width: usize, height: usize, format: Format) -> Option<Vec<[u8; 4]>> {
    let palette_bytes = format.entries() * format.entry_bytes;
    if data.len() < palette_bytes {
        return None;
    }
    let palette: Vec<[u8; 4]> = data[..palette_bytes]
        .chunks_exact(format.entry_bytes)
        .map(|entry| decode_entry(format.kind, entry))
        .collect();

    let indices = &data[palette_bytes..];
    let mut out = vec![[0u8, 0, 0, 0]; width * height];
    for (texel, slot) in out.iter_mut().enumerate() {
        let index = match format.index_bits {
            4 => {
                // Two texels per byte, the even-indexed one in the high nibble.
                let Some(&byte) = indices.get(texel / 2) else {
                    continue;
                };
                match texel % 2 {
                    0 => byte >> 4,
                    _ => byte & 0xf,
                }
            }
            _ => match indices.get(texel) {
                Some(&byte) => byte,
                None => continue,
            },
        };
        if let Some(&color) = palette.get(index as usize) {
            *slot = color;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette8_rgb5_a1_reads_the_palette_and_indices() {
        let format = Format::from_gl(gles::GL_PALETTE8_RGB5_A1_OES).unwrap();
        // A palette of 256 two-byte entries: entry 0 is opaque black, entry 1 is opaque red.
        let mut data = vec![0u8; 256 * 2];
        data[0..2].copy_from_slice(&0x0001u16.to_le_bytes());
        data[2..4].copy_from_slice(&0xf801u16.to_le_bytes());
        // Four texels: 0, 1, 1, 0.
        data.extend_from_slice(&[0, 1, 1, 0]);

        let pixels = decode(&data, 2, 2, format).unwrap();
        assert_eq!(pixels[0], [0, 0, 0, 255]);
        assert_eq!(pixels[1], [255, 0, 0, 255]);
        assert_eq!(pixels[3], [0, 0, 0, 255]);
    }

    #[test]
    fn palette4_packs_two_texels_per_byte() {
        let format = Format::from_gl(gles::GL_PALETTE4_RGB5_A1_OES).unwrap();
        let mut data = vec![0u8; 16 * 2];
        data[0..2].copy_from_slice(&0x0001u16.to_le_bytes()); // black
        data[2..4].copy_from_slice(&0xf801u16.to_le_bytes()); // red
        // One byte, two texels: the high one is 0, the low one is 1.
        data.push(0x01);

        let pixels = decode(&data, 2, 1, format).unwrap();
        assert_eq!(pixels[0], [0, 0, 0, 255]);
        assert_eq!(pixels[1], [255, 0, 0, 255]);
    }

    #[test]
    fn the_rgb5_a1_alpha_bit_is_all_or_nothing() {
        let format = Format::from_gl(gles::GL_PALETTE8_RGB5_A1_OES).unwrap();
        let mut data = vec![0u8; 256 * 2];
        // Same color, one with the alpha bit set and one without.
        data[0..2].copy_from_slice(&0xf800u16.to_le_bytes());
        data[2..4].copy_from_slice(&0xf801u16.to_le_bytes());
        data.extend_from_slice(&[0, 1]);
        let pixels = decode(&data, 2, 1, format).unwrap();
        assert_eq!(pixels[0][3], 0);
        assert_eq!(pixels[1][3], 255);
    }

    #[test]
    fn data_not_covering_the_palette_is_rejected() {
        let format = Format::from_gl(gles::GL_PALETTE8_RGB8_OES).unwrap();
        assert_eq!(decode(&[0u8; 10], 4, 4, format), None);
    }

    #[test]
    fn missing_indices_come_out_transparent() {
        // The palette is complete, but the indices end early: the rest stays transparent instead
        // of bringing down the emulator.
        let format = Format::from_gl(gles::GL_PALETTE8_RGB8_OES).unwrap();
        let mut data = vec![0u8; 256 * 3];
        data[0..3].copy_from_slice(&[10, 20, 30]);
        data.push(0);
        let pixels = decode(&data, 2, 1, format).unwrap();
        assert_eq!(pixels[0], [10, 20, 30, 255]);
        assert_eq!(pixels[1], [0, 0, 0, 0]);
    }

    #[test]
    fn a_non_paletted_format_is_not_recognized() {
        assert_eq!(Format::from_gl(gles::GL_ATC_RGB_AMD), None);
    }
}
