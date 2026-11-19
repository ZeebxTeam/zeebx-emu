//! The 960×544 frame, in `A8B8G8R8`, and the list drawn on top of it.
//!
//! The pixel word is `0xAABBGGRR`: the firmware calls the format `A8B8G8R8`, and on a
//! little-endian ARM red sits in the low byte. It is the layout `sceDisplaySetFrameBuf` scans.

use crate::font::FONT;

/// Each font dot becomes a 2×2 square. At 960×544, raw 8×8 is too small at the distance the
/// device is held.
const SCALE: usize = 2;
const LINE_HEIGHT: usize = 8 * SCALE;

pub const BLACK: u32 = 0xFF00_0000;
pub const BACKGROUND: u32 = 0xFF0C_121A;
pub const TEXT: u32 = 0xFFEE_EE_EE;
pub const ACCENT: u32 = 0xFF20_70E0;
pub const ERROR: u32 = 0xFF40_60FF;
pub const BAR: u32 = 0xFF10_2840;

pub struct Frame<'a> {
    pub pixels: &'a mut [u32],
    pub width: usize,
    pub step: usize,
    pub height: usize,
}

pub fn clear(q: &mut Frame<'_>, color: u32) {
    q.pixels.fill(color);
}

pub fn bar(q: &mut Frame<'_>, x: usize, y: usize, w: usize, h: usize, color: u32) {
    let y1 = y.saturating_add(h).min(q.height);
    let x1 = x.saturating_add(w).min(q.width);
    for yy in y..y1 {
        let line = yy * q.step;
        for xx in x..x1 {
            q.pixels[line + xx] = color;
        }
    }
}

/// The accent is dropped. The font is ASCII, and a list in Portuguese without "ã" stays readable;
/// a letter that turns into a hole does not.
pub fn strip_accent(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
        'Á' | 'À' | 'Â' | 'Ã' | 'Ä' => 'A',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'É' | 'È' | 'Ê' | 'Ë' => 'E',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'Í' | 'Ì' | 'Î' | 'Ï' => 'I',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'O',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'Ú' | 'Ù' | 'Û' | 'Ü' => 'U',
        'ç' => 'c',
        'Ç' => 'C',
        'ñ' => 'n',
        'Ñ' => 'N',
        other => other,
    }
}

pub fn text(q: &mut Frame<'_>, mut x: usize, y: usize, phrase: &str, color: u32) {
    for c in phrase.chars() {
        let c = strip_accent(c);
        let index = c as usize;
        if index >= FONT.len() || x + 8 * SCALE > q.width || y + LINE_HEIGHT > q.height {
            x += 8 * SCALE;
            continue;
        }
        let glyph = &FONT[index];
        for line in 0..8 {
            let bits = glyph[line];
            for column in 0..8 {
                // Bit 0 is the leftmost pixel. That is font8x8's convention.
                if bits & (1 << column) == 0 {
                    continue;
                }
                let px = x + column * SCALE;
                let py = y + line * SCALE;
                for dy in 0..SCALE {
                    let dest = (py + dy) * q.step + px;
                    for dx in 0..SCALE {
                        q.pixels[dest + dx] = color;
                    }
                }
            }
        }
        x += 8 * SCALE;
    }
}

pub fn wrap(text: &str, columns: usize) -> Vec<String> {
    if columns == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                current = word.to_string();
            } else if current.chars().count() + 1 + word.chars().count() <= columns {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(std::mem::take(&mut current));
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines
}

/// The whole game rectangle inside the screen, without cropping. In integers, so the neighboring
/// pixel does not depend on rounding.
pub fn fit(
    dst_w: u32,
    dst_h: u32,
    src_w: u32,
    src_h: u32,
) -> (u32, u32, u32, u32) {
    if dst_w == 0 || dst_h == 0 || src_w == 0 || src_h == 0 {
        return (0, 0, 0, 0);
    }
    let dw = u64::from(dst_w);
    let dh = u64::from(dst_h);
    let sw = u64::from(src_w);
    let sh = u64::from(src_h);
    let (w, h) = if dw * sh <= dh * sw {
        (dst_w, (sh * dw / sw) as u32)
    } else {
        ((sw * dh / sh) as u32, dst_h)
    };
    ((dst_w - w) / 2, (dst_h - h) / 2, w, h)
}

pub fn rgb565_to_screen(pixel: u16) -> u32 {
    let r5 = (pixel >> 11) & 0x1f;
    let g6 = (pixel >> 5) & 0x3f;
    let b5 = pixel & 0x1f;
    // Repeat the high bits in the low ones: the console's white is 31, and without this it
    // becomes 248.
    // `u32`: the shift by 16 does not fit in a `u16`, and the framebuffer's type is the word.
    let r = u32::from((r5 << 3) | (r5 >> 2));
    let g = u32::from((g6 << 2) | (g6 >> 4));
    let b = u32::from((b5 << 3) | (b5 >> 2));
    0xFF00_0000 | (b << 16) | (g << 8) | r
}

pub fn present_rgb565(q: &mut Frame<'_>, src: &[u16], src_w: u32, src_h: u32) {
    let (x0, y0, w, h) = fit(q.width as u32, q.height as u32, src_w, src_h);
    if w == 0 || h == 0 {
        return;
    }
    let sw = src_w as usize;
    let sh = src_h as usize;
    if src.len() < sw * sh {
        return;
    }
    let w = w as usize;
    let h = h as usize;
    for y in 0..h {
        let sy = y * sh / h;
        let line = (y0 as usize + y) * q.step + x0 as usize;
        for x in 0..w {
            let sx = x * sw / w;
            q.pixels[line + x] = rgb565_to_screen(src[sy * sw + sx]);
        }
    }
}

/// Copies the console frame pixel for pixel, centered. The border around it stays as it is.
///
/// The Vita is 960×544 and the Zeebo 640×480. The only integer that fits is 1. Stretching to
/// 725×544 with nearest neighbor repeats one column and not the next: the image shimmers and
/// looks blurry even when the 3D is at native resolution. If the source does not fit, it falls
/// back to fitting — the console frame does fit.
pub fn copy_rgb565(q: &mut Frame<'_>, src: &[u16], src_w: u32, src_h: u32) {
    if src_w == 0 || src_h == 0 {
        return;
    }
    let sw = src_w as usize;
    let sh = src_h as usize;
    if src.len() < sw * sh || sw > q.width || sh > q.height {
        present_rgb565(q, src, src_w, src_h);
        return;
    }
    let x0 = (q.width - sw) / 2;
    let y0 = (q.height - sh) / 2;
    for y in 0..sh {
        let dest = (y0 + y) * q.step + x0;
        let origin = y * sw;
        for x in 0..sw {
            q.pixels[dest + x] = rgb565_to_screen(src[origin + x]);
        }
    }
}

fn columns(q: &Frame<'_>) -> usize {
    q.width.saturating_sub(32) / (8 * SCALE)
}

pub fn draw_menu(q: &mut Frame<'_>, titles: &[&str], cursor: usize, error: &str) {
    clear(q, BACKGROUND);
    text(q, 16, 16, "Zeebx", ACCENT);
    text(q, 16, 16 + LINE_HEIGHT, "ux0:data/zeebx/roms", TEXT);
    let mut y = 16 + LINE_HEIGHT * 3;
    if !error.is_empty() {
        for line in wrap(error, columns(q)).into_iter().take(4) {
            text(q, 16, y, &line, ERROR);
            y += LINE_HEIGHT;
        }
        y += LINE_HEIGHT / 2;
    }
    let footer = LINE_HEIGHT * 2 + 8;
    let limit = q.height.saturating_sub(footer);
    if titles.is_empty() && error.is_empty() {
        text(q, 16, y, "No games in this folder.", TEXT);
    } else {
        let visible_items = limit.saturating_sub(y) / LINE_HEIGHT;
        let start = scroll_window(cursor, titles.len(), visible_items);
        for (i, title) in titles
            .iter()
            .enumerate()
            .skip(start)
            .take(visible_items.max(1))
        {
            if y + LINE_HEIGHT > limit {
                break;
            }
            if i == cursor {
                bar(q, 8, y, q.width.saturating_sub(16), LINE_HEIGHT, BAR);
            }
            let mark = if i == cursor { "> " } else { "  " };
            text(q, 16, y, &format!("{mark}{title}"), TEXT);
            y += LINE_HEIGHT;
        }
    }
    let footer_y = q.height.saturating_sub(footer);
    text(
        q,
        16,
        footer_y,
        "X starts    O quits    Triangle refreshes",
        TEXT,
    );
    text(q, 16, footer_y + LINE_HEIGHT, "Select+Start exits the game", TEXT);
}

fn scroll_window(cursor: usize, total: usize, visible_items: usize) -> usize {
    crate::list::window(cursor, total, visible_items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_white_and_red_fill_the_channel() {
        assert_eq!(rgb565_to_screen(0), BLACK);
        assert_eq!(rgb565_to_screen(0xffff), 0xFFFF_FFFF);
        assert_eq!(
            rgb565_to_screen(0xf800),
            0xFF00_00FF,
            "red in the low byte"
        );
    }

    #[test]
    fn the_zeebo_screen_fits_whole_with_side_bars() {
        assert_eq!(fit(960, 544, 640, 480), (117, 0, 725, 544));
        assert_eq!(fit(0, 10, 10, 10), (0, 0, 0, 0));
    }

    #[test]
    fn one_red_pixel_fills_the_target() {
        let mut pixels = vec![0u32; 4];
        let mut q = Frame {
            pixels: &mut pixels,
            width: 2,
            step: 2,
            height: 2,
        };
        present_rgb565(&mut q, &[0xf800], 1, 1);
        assert!(q.pixels.iter().all(|p| *p == 0xFF00_00FF));
    }

    #[test]
    fn the_console_pixel_lands_centered_without_stretching() {
        let mut pixels = vec![BACKGROUND; 8 * 6];
        let mut q = Frame {
            pixels: &mut pixels,
            width: 8,
            step: 8,
            height: 6,
        };
        copy_rgb565(&mut q, &[0xf800, 0, 0, 0], 2, 2);
        assert_eq!(
            q.pixels[2 * 8 + 3],
            0xFF00_00FF,
            "corner of the block, in the center"
        );
        assert_eq!(
            q.pixels[2 * 8 + 4],
            BLACK,
            "the pixel next to it, still from the source"
        );
        assert_eq!(q.pixels[0], BACKGROUND, "the border is not stretched");
        assert_eq!(q.pixels[2 * 8 + 5], BACKGROUND);
    }

    #[test]
    fn the_exclamation_lights_the_middle_and_the_accent_drops() {
        assert_eq!(strip_accent('ã'), 'a');
        assert_eq!(strip_accent('Ç'), 'C');
        let mut pixels = vec![BACKGROUND; 32 * 16];
        let mut q = Frame {
            pixels: &mut pixels,
            width: 32,
            step: 32,
            height: 16,
        };
        text(&mut q, 0, 0, "!", TEXT);
        assert_eq!(q.pixels[6], TEXT);
        assert_eq!(q.pixels[0], BACKGROUND);
    }

    #[test]
    fn wrapping_respects_the_column() {
        let lines = wrap("um dois tres", 7);
        assert_eq!(lines, vec!["um dois".to_string(), "tres".to_string()]);
    }

    #[test]
    fn the_menu_paints_the_title() {
        let mut pixels = vec![0u32; 960 * 64];
        let mut q = Frame {
            pixels: &mut pixels,
            width: 960,
            step: 960,
            height: 64,
        };
        draw_menu(&mut q, &["Quake"], 0, "");
        assert!(q.pixels.contains(&ACCENT));
    }
}
