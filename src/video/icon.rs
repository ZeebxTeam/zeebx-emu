//! The images that come inside the games.
//!
//! A `.mif` holds the title's icon ready for display, and each console stored its own in
//! whatever format it wanted: the newer games use PNG, the first batch uses paletted BMP, and the
//! Z-Wheel uses JPEG. Since all three show up in real ROMs, all three are read here.
//!
//! BMP is decoded by hand because what the `.mif` files carry is the simplest subset of the
//! format — no compression, a 16- or 256-color palette — and a whole dependency for that would
//! cost more than the few dozen lines below.

use std::fmt;

/// Area ceiling for an image coming from a game. Real icons are no larger than 65×42; the limit
/// exists so a corrupt file does not ask for gigabytes by announcing an absurd size.
const MAX_PIXELS: usize = 4096 * 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    /// Non-premultiplied RGBA, one row at a time, top to bottom.
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn pixels(&self) -> usize {
        self.width * self.height
    }

    /// Shrinks the image until no side exceeds `max_side`, by averaging blocks.
    ///
    /// The average is taken with the color **premultiplied** by alpha and undone at the end.
    /// Without that, a transparent pixel enters the sum with the color it stores — usually
    /// black — and the edge of a logo on a transparent background gets a dark halo.
    ///
    /// The factor is an integer: shrinking by 5 means summing 5×5 blocks, with no interpolation
    /// and no fractional resampling. For an icon that is enough, and it avoids a whole resizer.
    pub fn downscaled(&self, max_side: usize) -> Image {
        let longest = self.width.max(self.height);
        if longest <= max_side || max_side == 0 {
            return self.clone();
        }
        let factor = longest.div_ceil(max_side);
        let (width, height) = (self.width.div_ceil(factor), self.height.div_ceil(factor));
        let mut rgba = Vec::with_capacity(width * height * 4);
        for block_y in 0..height {
            for block_x in 0..width {
                let (mut sum, mut alpha, mut count) = ([0u32; 3], 0u32, 0u32);
                for y in block_y * factor..((block_y + 1) * factor).min(self.height) {
                    for x in block_x * factor..((block_x + 1) * factor).min(self.width) {
                        let at = (y * self.width + x) * 4;
                        let a = self.rgba[at + 3] as u32;
                        for (channel, total) in sum.iter_mut().enumerate() {
                            *total += self.rgba[at + channel] as u32 * a;
                        }
                        alpha += a;
                        count += 1;
                    }
                }
                let count = count.max(1);
                for channel in sum {
                    rgba.push(match alpha {
                        0 => 0,
                        _ => (channel / alpha) as u8,
                    });
                }
                rgba.push((alpha / count) as u8);
            }
        }
        Image {
            width,
            height,
            rgba,
        }
    }
}

#[derive(Debug)]
pub enum ImageError {
    /// The format is none of the ones we know how to read.
    Unknown,
    /// The file says one thing and delivers another.
    Malformed(&'static str),
    Png(png::DecodingError),
    Jpeg(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => write!(f, "unknown image format"),
            Self::Malformed(what) => write!(f, "inconsistent image: {what}"),
            Self::Png(err) => write!(f, "unreadable png: {err}"),
            Self::Jpeg(err) => write!(f, "unreadable jpeg: {err}"),
        }
    }
}

impl std::error::Error for ImageError {}

/// Decodes by what the file is, not by what it claims to be.
///
/// The signature is more reliable than the type announced in the `.mif`: one of them calls JPEG
/// `image/jpg`, and following the label to the letter would leave the icon out over one letter.
pub fn decode(data: &[u8]) -> Result<Image, ImageError> {
    match data {
        [0x89, b'P', b'N', b'G', ..] => decode_png(data),
        [b'B', b'M', ..] => decode_bmp(data),
        [0xff, 0xd8, 0xff, ..] => decode_jpeg(data),
        _ => Err(ImageError::Unknown),
    }
}

pub fn decode_png(data: &[u8]) -> Result<Image, ImageError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    // The games' icons are almost all paletted; without this expansion we would get the color
    // index, not the color.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(ImageError::Png)?;
    let size = reader
        .output_buffer_size()
        .ok_or(ImageError::Malformed("impossible size"))?;
    let mut buffer = vec![0; size];
    let info = reader.next_frame(&mut buffer).map_err(ImageError::Png)?;
    let channels = info.color_type.samples();
    let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
    for pixel in buffer[..info.buffer_size()].chunks_exact(channels) {
        let (rgb, alpha) = match channels {
            1 => ([pixel[0]; 3], 255),
            2 => ([pixel[0]; 3], pixel[1]),
            3 => ([pixel[0], pixel[1], pixel[2]], 255),
            _ => ([pixel[0], pixel[1], pixel[2]], pixel[3]),
        };
        rgba.extend_from_slice(&rgb);
        rgba.push(alpha);
    }
    Ok(Image {
        width: info.width as usize,
        height: info.height as usize,
        rgba,
    })
}

fn decode_jpeg(data: &[u8]) -> Result<Image, ImageError> {
    let mut decoder = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(data));
    decoder
        .decode_headers()
        .map_err(|err| ImageError::Jpeg(err.to_string()))?;
    let info = decoder
        .info()
        .ok_or_else(|| ImageError::Jpeg("no header".into()))?;
    let pixels = decoder
        .decode()
        .map_err(|err| ImageError::Jpeg(err.to_string()))?;
    let (width, height) = (info.width as usize, info.height as usize);
    let channels = match width * height {
        0 => return Err(ImageError::Malformed("empty jpeg")),
        area => pixels.len() / area,
    };
    let mut rgba = Vec::with_capacity(width * height * 4);
    for pixel in pixels.chunks_exact(channels.max(1)) {
        match channels {
            1 => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]),
            _ => rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
        }
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// One channel of a packed pixel, stretched to eight bits.
fn channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let value = (pixel & mask) >> mask.trailing_zeros();
    let maximum = mask >> mask.trailing_zeros();
    (value * 255 / maximum) as u8
}

/// Unpacks the rows of a `BI_RLE8` or `BI_RLE4` BMP, returning one palette index per pixel, in
/// the order the file stores the rows.
///
/// The scheme is Windows': a `(count, value)` pair repeats the value; a zero count opens an
/// escape — `0` end of line, `1` end of image, `2` a `(dx, dy)` jump, and from `3` up a literal
/// run, always ending on a word boundary. In `RLE4` each value carries two pixels, the high and
/// the low one, alternating.
///
/// Whatever the image does not cover stays at index zero, as Windows does.
fn unpack_rle(
    data: &[u8],
    start: usize,
    width: usize,
    height: usize,
    depth: usize,
) -> Result<Vec<u8>, ImageError> {
    let mut indices = vec![0u8; width * height];
    let (mut x, mut y) = (0usize, 0usize);
    let mut at = start;
    let write = |x: usize, y: usize, value: u8, indices: &mut Vec<u8>| {
        if x < width && y < height {
            indices[y * width + x] = value;
        }
    };
    while at + 1 < data.len() {
        let (count, value) = (data[at] as usize, data[at + 1]);
        at += 2;
        if count > 0 {
            for i in 0..count {
                let pixel = match depth {
                    4 if i % 2 == 1 => value & 0x0f,
                    4 => value >> 4,
                    _ => value,
                };
                write(x + i, y, pixel, &mut indices);
            }
            x += count;
            continue;
        }
        match value {
            // End of line.
            0 => {
                x = 0;
                y += 1;
            }
            // End of image.
            1 => break,
            // Relative jump, in pixels.
            2 => {
                if at + 1 >= data.len() {
                    break;
                }
                x += data[at] as usize;
                y += data[at + 1] as usize;
                at += 2;
            }
            // Literal run of `value` pixels.
            n => {
                let n = n as usize;
                let bytes = match depth {
                    4 => n.div_ceil(2),
                    _ => n,
                };
                if at + bytes > data.len() {
                    break;
                }
                for i in 0..n {
                    let raw = match depth {
                        4 => data[at + i / 2],
                        _ => data[at + i],
                    };
                    let pixel = match depth {
                        4 if i % 2 == 1 => raw & 0x0f,
                        4 => raw >> 4,
                        _ => raw,
                    };
                    write(x + i, y, pixel, &mut indices);
                }
                x += n;
                // The run is padded to a two-byte boundary.
                at += bytes + bytes % 2;
            }
        }
        if y >= height {
            break;
        }
    }
    Ok(indices)
}

/// Decodes the BMP that the `.mif` files and the games store.
///
/// Covers 1, 4, 8, 16, 24 and 32 bits per pixel, and the two paletted RLE compressions — Disney
/// All Star Cards stores most of its images in `BI_RLE8` and `BI_RLE4`. The `BI_BITFIELDS` masks
/// are not compression, and they are supported: that is how a 16-bit BMP says it is 5-6-5.
fn decode_bmp(data: &[u8]) -> Result<Image, ImageError> {
    let u16_at = |at: usize| -> u16 { u16::from_le_bytes([data[at], data[at + 1]]) };
    let u32_at = |at: usize| -> u32 {
        u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
    };
    if data.len() < 54 {
        return Err(ImageError::Malformed("truncated header"));
    }
    let data_offset = u32_at(10) as usize;
    let header_size = u32_at(14) as usize;
    let width = u32_at(18) as i32 as isize;
    let raw_height = u32_at(22) as i32 as isize;
    let depth = u16_at(28) as usize;
    let compression = u32_at(30);
    // A negative height means the rows come top to bottom, the opposite of the usual.
    let (height, top_down) = (raw_height.unsigned_abs(), raw_height < 0);
    let width = match width > 0 {
        true => width as usize,
        false => return Err(ImageError::Malformed("invalid width")),
    };
    // With no declared masks, 16 bits is 5-5-5: the Z-Wheel covers come that way, and their grays
    // only come out neutral when read like that.
    let masks = match (compression, depth) {
        (0, _) => [0x7c00, 0x03e0, 0x001f],
        (1, 8) | (2, 4) => [0, 0, 0],
        (3, 16 | 32) if data.len() >= 66 => [u32_at(54), u32_at(58), u32_at(62)],
        _ => return Err(ImageError::Malformed("compressed bmp")),
    };
    let rle = matches!((compression, depth), (1, 8) | (2, 4));
    if compression == 3 && depth == 32 {
        return Err(ImageError::Malformed("compressed bmp"));
    }
    if width * height > MAX_PIXELS {
        return Err(ImageError::Malformed("image too large"));
    }

    // The palette sits between the header and the pixels. When the color count is zero, it
    // takes up everything that fits there — which is how the `.mif` files store it.
    let palette_start = 14 + header_size;
    let mut palette = Vec::new();
    if depth <= 8 {
        let declared = u32_at(46) as usize;
        let available = data_offset.saturating_sub(palette_start) / 4;
        let count = match declared {
            0 => available,
            n => n.min(available),
        };
        for i in 0..count {
            let at = palette_start + i * 4;
            if at + 3 >= data.len() {
                break;
            }
            // BMP stores blue, green and red in this order.
            palette.push([data[at + 2], data[at + 1], data[at]]);
        }
    }

    // Each row is padded to a multiple of four bytes.
    let stride = (width * depth).div_ceil(8).div_ceil(4) * 4;
    let decompressed = match rle {
        true => Some(unpack_rle(data, data_offset, width, height, depth)?),
        false => None,
    };
    if decompressed.is_none() && data_offset + stride * height > data.len() {
        return Err(ImageError::Malformed("truncated pixels"));
    }
    let color = |index: usize| -> [u8; 3] {
        palette.get(index).copied().unwrap_or([255, 0, 255]) // An index outside the palette stands out instead of vanishing.
    };

    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let row = match top_down {
            true => y,
            false => height - 1 - y,
        };
        if let Some(indices) = &decompressed {
            for x in 0..width {
                let rgb = color(indices[row * width + x] as usize);
                rgba.extend_from_slice(&rgb);
                rgba.push(255);
            }
            continue;
        }
        let line = &data[data_offset + row * stride..][..stride];
        for x in 0..width {
            let (rgb, alpha) = match depth {
                1 => (color((line[x / 8] >> (7 - x % 8)) as usize & 1), 255),
                4 => {
                    let byte = line[x / 2];
                    let index = match x % 2 {
                        0 => byte >> 4,
                        _ => byte & 0x0f,
                    };
                    (color(index as usize), 255)
                }
                8 => (color(line[x] as usize), 255),
                16 => {
                    let pixel = u32::from(u16::from_le_bytes([line[x * 2], line[x * 2 + 1]]));
                    (masks.map(|mask| channel(pixel, mask)), 255)
                }
                24 => ([line[x * 3 + 2], line[x * 3 + 1], line[x * 3]], 255),
                32 => (
                    [line[x * 4 + 2], line[x * 4 + 1], line[x * 4]],
                    // A 32-bit BMP whose channel is always zero is opaque, not invisible.
                    line[x * 4 + 3],
                ),
                _ => return Err(ImageError::Malformed("unsupported depth")),
            };
            rgba.extend_from_slice(&rgb);
            rgba.push(alpha);
        }
    }
    // The zeroed-channel rule can only be applied after seeing the whole image.
    if depth == 32 && rgba.iter().skip(3).step_by(4).all(|&a| a == 0) {
        rgba.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a 2x2, 4-bit paletted BMP with the header the `.mif` files use.
    fn bmp4() -> Vec<u8> {
        let palette: [[u8; 4]; 2] = [[0, 0, 255, 0], [0, 255, 0, 0]]; // BGRA: red, green.
        let data_offset = 54 + palette.len() * 4;
        let mut bmp = vec![0u8; data_offset];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[10..14].copy_from_slice(&(data_offset as u32).to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&2u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&2u32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&4u16.to_le_bytes());
        bmp[46..50].copy_from_slice(&(palette.len() as u32).to_le_bytes());
        for (i, entry) in palette.iter().enumerate() {
            bmp[54 + i * 4..54 + i * 4 + 4].copy_from_slice(entry);
        }
        // Bottom to top: the bottom row first (green, red).
        bmp.extend_from_slice(&[0x10, 0, 0, 0]);
        bmp.extend_from_slice(&[0x01, 0, 0, 0]);
        bmp
    }

    /// Builds a 4x2 `BI_RLE8` BMP with the palette and the given data.
    fn bmp_rle8(data: &[u8]) -> Vec<u8> {
        let palette: [[u8; 4]; 3] = [[0, 0, 0, 0], [0, 0, 255, 0], [0, 255, 0, 0]]; // black, red, green.
        let data_offset = 54 + palette.len() * 4;
        let mut bmp = vec![0u8; data_offset];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[10..14].copy_from_slice(&(data_offset as u32).to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&4u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&2u32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&8u16.to_le_bytes());
        bmp[30..34].copy_from_slice(&1u32.to_le_bytes()); // BI_RLE8
        bmp[46..50].copy_from_slice(&(palette.len() as u32).to_le_bytes());
        for (i, entry) in palette.iter().enumerate() {
            bmp[54 + i * 4..54 + i * 4 + 4].copy_from_slice(entry);
        }
        bmp.extend_from_slice(data);
        bmp
    }

    #[test]
    fn rle8_compressed_bmp_repeats_and_copies() {
        // Bottom row: four reds. Top row: two literal greens and the rest empty, which stays at
        // index zero — that is what Windows does with whatever the image does not cover.
        let image = decode(&bmp_rle8(&[
            4, 1, // index 1 four times
            0, 0, // end of line
            0, 3, 2, 2, 2, 0, // three literals of index 2, padded to the word
            0, 1, // end of image
        ]))
        .unwrap();
        assert_eq!((image.width, image.height), (4, 2));
        // The top is the last stored row: green, green, green, empty.
        assert_eq!(&image.rgba[0..4], &[0, 255, 0, 255]);
        assert_eq!(&image.rgba[8..12], &[0, 255, 0, 255]);
        assert_eq!(&image.rgba[12..16], &[0, 0, 0, 255], "what was not covered stays at index 0");
        // The bottom is the first stored row: four reds.
        assert_eq!(&image.rgba[16..20], &[255, 0, 0, 255]);
        assert_eq!(&image.rgba[28..32], &[255, 0, 0, 255]);
    }

    #[test]
    fn paletted_bmp_becomes_colors_in_the_right_order() {
        // BMP stores the rows bottom to top and the colors as BGR: getting either one wrong gives
        // a plausible, wrong image that nobody notices by looking.
        let image = decode(&bmp4()).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(&image.rgba[0..4], &[255, 0, 0, 255], "top left");
        assert_eq!(&image.rgba[4..8], &[0, 255, 0, 255], "top right");
        assert_eq!(&image.rgba[8..12], &[0, 255, 0, 255], "bottom left");
        assert_eq!(&image.rgba[12..16], &[255, 0, 0, 255], "bottom right");
    }

    #[test]
    fn downscaling_respects_the_longer_side_and_leaves_what_fits() {
        let large = Image {
            width: 100,
            height: 50,
            rgba: vec![255; 100 * 50 * 4],
        };
        // Factor 10: 100 -> 10, and the height follows.
        let small = large.downscaled(10);
        assert_eq!((small.width, small.height), (10, 5));
        // What already fits comes back unchanged.
        assert_eq!(small.downscaled(64), small);
    }

    #[test]
    fn transparency_does_not_darken_the_edge_when_downscaling() {
        // A 2x2 block with one opaque white pixel and three transparent black ones. A naive
        // average would give dark gray; with premultiplied color, the color left is that of the
        // only pixel that really exists — and only the alpha drops.
        let image = Image {
            width: 2,
            height: 2,
            rgba: vec![
                255, 255, 255, 255, // opaque white
                0, 0, 0, 0, //
                0, 0, 0, 0, //
                0, 0, 0, 0,
            ],
        };
        let one = image.downscaled(1);
        assert_eq!((one.width, one.height), (1, 1));
        assert_eq!(
            &one.rgba[0..3],
            &[255, 255, 255],
            "the color is the visible pixel's"
        );
        assert_eq!(one.rgba[3], 63, "and the alpha is the average of the four");
    }

    #[test]
    fn the_format_comes_from_the_signature_not_the_label() {
        assert!(matches!(decode(b"nada disso"), Err(ImageError::Unknown)));
        assert!(matches!(decode(&[]), Err(ImageError::Unknown)));
    }

    /// The Z-Wheel covers are 16-bit BMPs with no mask: 5-5-5.
    #[test]
    fn sixteen_bit_bmp_is_five_five_five() {
        let mut bmp = vec![0u8; 54];
        bmp[..2].copy_from_slice(b"BM");
        bmp[10] = 54;
        bmp[14] = 40;
        bmp[18] = 2;
        bmp[22] = 1;
        bmp[26] = 1;
        bmp[28] = 16;
        // Pure white and pure red, with the row padded to four bytes.
        bmp.extend_from_slice(&[0xff, 0x7f, 0x00, 0x7c]);
        let image = decode_bmp(&bmp).unwrap();
        assert_eq!(&image.rgba[..4], &[255, 255, 255, 255]);
        assert_eq!(&image.rgba[4..], &[255, 0, 0, 255]);
    }

    #[test]
    fn compressed_bmp_is_rejected_by_name() {
        // Refusing with the reason avoids a hunt for an icon that vanishes without explanation.
        let mut bmp = bmp4();
        bmp[30..34].copy_from_slice(&1u32.to_le_bytes());
        let err = decode(&bmp).unwrap_err();
        assert_eq!(err.to_string(), "inconsistent image: compressed bmp");
    }

    #[test]
    fn truncated_bmp_crashes_nothing() {
        let bmp = bmp4();
        assert!(decode(&bmp[..bmp.len() - 4]).is_err());
        assert!(decode(&bmp[..20]).is_err());
    }

    #[test]
    fn paletted_png_is_expanded_to_colors() {
        // `png` hands over the palette index if we do not ask for the expansion, and the games'
        // icons are almost all paletted.
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut out), 2, 1);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(vec![255, 0, 0, 0, 0, 255]);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[0, 1])
                .unwrap();
        }
        let image = decode(&out).unwrap();
        assert_eq!(&image.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&image.rgba[4..8], &[0, 0, 255, 255]);
    }
}
