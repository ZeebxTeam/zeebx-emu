//! Stage slides for the games the Z-Wheel's package has no slide for.
//!
//! The 3D wheel of the main menu shows one *slide* per featured game: a folder with a model, its
//! animation and its textures, listed in `asset_cache`. The package carries slides for twelve
//! games. Every other game in the ROM folder was simply not on the wheel.
//!
//! One of the package's slides has no game in it: `50088`, the store's notice — a framed panel
//! that shows three pictures in turn (`banner1.qxt` to `banner3.qxt`), with a tile for the top
//! reel (`slidebanner.qxt`). A game without a slide gets a copy of that one, with the panel
//! showing its box art and its name, and the tile showing the box art.
//!
//! **This is ours, not the console's.** A Zeebo only ever showed the slides the store shipped.

use super::*;
use crate::video::icon::Image;

/// The package's slide the generic ones are copied from, and the files taken as they are.
const TEMPLATE: &str = "assets/stage_slides/50088";
const TEMPLATE_FILES: [&str; 3] = ["slidemodel.qxm", "slideanim.qxa", "graybordertxt2.qxt"];
/// The pictures the template's panel shows in turn. All three get the same one.
const PANEL_FILES: [&str; 3] = ["banner1.qxt", "banner2.qxt", "banner3.qxt"];
/// The top reel's tile.
const TILE_FILE: &str = "slidebanner.qxt";
/// What a slide of ours was built from, next to its files: when it changes, the slide is redone.
const STAMP_FILE: &str = "zeebx-slide.txt";
/// Bumped when the pictures' layout changes, so the slides already written are redone.
const LAYOUT: u32 = 1;

/// The panel's picture. The panel takes about 230×115 pixels of the screen and the package's
/// pictures are 512×256; half of that keeps a slide's three pictures at 131 kB each instead of
/// half a megabyte, with dozens of slides to load.
const PANEL: (usize, usize) = (256, 128);
/// The tile's texture, the size of the package's. The reel shows it about twice as wide as tall.
const TILE: usize = 64;

/// A game that needs a slide.
pub(super) struct SlideGame {
    pub game_id: i64,
    pub title: String,
    /// The game's folder in the package (`./assets/games/<id>/`), empty for a game the package
    /// does not know.
    pub boxart_path: String,
    /// The ROM on disk, when the catalog has it: where a game the package does not know gets
    /// its picture from.
    pub rom_path: Option<std::path::PathBuf>,
}

impl<C: CpuBackend> Machine<C> {
    /// Writes the slide `dslid` for `game`, unless one built from the same things is already
    /// there. Returns whether the folder holds a usable slide.
    pub(super) fn write_generic_slide(&mut self, dslid: i64, game: &SlideGame) -> bool {
        let folder = format!("assets/stage_slides/{dslid}");
        let host = |machine: &Self, name: &str, intent| {
            machine
                .vfs
                .open_target(&format!("{folder}/{name}"), intent)
                .map(|target| target.path)
        };
        let stamp = format!("{LAYOUT}\n{}\n{}\n{}\n", game.game_id, game.title, game.boxart_path);
        let Some(stamp_path) = host(self, STAMP_FILE, crate::brew::vfs::OpenIntent::Create) else {
            return false;
        };
        if std::fs::read_to_string(&stamp_path).is_ok_and(|current| current == stamp) {
            return true;
        }
        let Some(parent) = stamp_path.parent() else {
            return false;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
        for name in TEMPLATE_FILES {
            let source = self
                .vfs
                .open_target(&format!("{TEMPLATE}/{name}"), crate::brew::vfs::OpenIntent::Read)
                .map(|target| target.path);
            let copied = source.is_some_and(|source| {
                std::fs::copy(source, parent.join(name)).is_ok()
            });
            if !copied {
                return false;
            }
        }
        let art = self.slide_art(game);
        let panel = self.panel_picture(art.as_ref(), &game.title);
        let panel = qxt_rgb(&panel, PANEL.0, PANEL.1, true);
        let tile = self.tile_picture(art.as_ref(), &game.title);
        let tile = qxt_rgb(&tile, TILE, TILE, false);
        let written = PANEL_FILES
            .iter()
            .all(|name| std::fs::write(parent.join(name), &panel).is_ok())
            && std::fs::write(parent.join(TILE_FILE), &tile).is_ok()
            // Last: a slide interrupted halfway has no stamp and is redone.
            && std::fs::write(&stamp_path, stamp).is_ok();
        written
    }

    /// The picture that stands for the game: its box art from the package or, for a game the
    /// package does not know, the cover next to the ROM or the largest icon of its `.mif`.
    fn slide_art(&self, game: &SlideGame) -> Option<Image> {
        let folder = game.boxart_path.trim_start_matches("./").trim_end_matches('/');
        let packaged = (!folder.is_empty())
            .then(|| {
                ["boxart.bmp", "boxartlg.jpg"].iter().find_map(|name| {
                    let target = self.vfs.open_target(
                        &format!("{folder}/{name}"),
                        crate::brew::vfs::OpenIntent::Read,
                    )?;
                    crate::video::icon::decode(&std::fs::read(target.path).ok()?).ok()
                })
            })
            .flatten();
        packaged.or_else(|| crate::library::art_of(game.rom_path.as_deref()?))
    }

    /// The panel: the art on the left, as tall as the panel allows, and the name beside it.
    fn panel_picture(&self, art: Option<&Image>, title: &str) -> Vec<u8> {
        const MARGIN: usize = 8;
        const TEXT_SIZE: f32 = 22.0;
        let (width, height) = PANEL;
        let mut picture = backdrop(width, height);
        let mut text_left = MARGIN;
        if let Some(art) = art.filter(|art| art.width != 0 && art.height != 0) {
            let tall = height - 2 * MARGIN;
            let wide = (tall * art.width / art.height).clamp(1, width / 2);
            let tall = (wide * art.height / art.width).clamp(1, tall);
            let top = (height - tall) / 2;
            // A thin light frame, as the grid's box art has.
            fill(&mut picture, width, (MARGIN - 1, top - 1, wide + 2, tall + 2), [0xe8, 0xe8, 0xe8]);
            paste(&mut picture, width, art, (MARGIN, top, wide, tall));
            text_left = MARGIN + wide + 2 * MARGIN;
        }
        let Some(font) = self.font.as_ref() else {
            return picture;
        };
        let room = width.saturating_sub(text_left + MARGIN) as u32;
        let line_height = (font.ascent(TEXT_SIZE) + font.descent(TEXT_SIZE)) as usize;
        let lines = wrap(title, room, |text| font.width(text, TEXT_SIZE));
        let lines = &lines[..lines.len().min((height - 2 * MARGIN) / line_height.max(1))];
        let top = (height.saturating_sub(lines.len() * line_height)) / 2;
        for (index, line) in lines.iter().enumerate() {
            // Centered in what is left of the panel.
            let line_width = font.width(line, TEXT_SIZE).min(room) as usize;
            let left = text_left + (room as usize - line_width) / 2;
            draw_text(
                &mut picture,
                (width, height),
                font,
                line,
                TEXT_SIZE,
                (left as i32, (top + index * line_height) as i32),
                [0xff, 0xff, 0xff],
            );
        }
        picture
    }

    /// The reel's tile: the art, fitted and centered. The reel draws the square texture about
    /// twice as wide as tall, so the art goes in at half its width to come out right.
    fn tile_picture(&self, art: Option<&Image>, title: &str) -> Vec<u8> {
        const MARGIN: usize = 3;
        let mut picture = vec![0xf4u8; TILE * TILE * 3];
        match art.filter(|art| art.width != 0 && art.height != 0) {
            Some(art) => {
                let tall = TILE - 2 * MARGIN;
                // On screen the tile is 2:1; in the texture, half of the on-screen width.
                let wide = (tall * art.width / art.height / 2).clamp(1, TILE - 2 * MARGIN);
                paste(&mut picture, TILE, art, ((TILE - wide) / 2, MARGIN, wide, tall));
            }
            None => {
                // No picture at all: the name's first word, which the reel stretches sideways.
                const TEXT_SIZE: f32 = 18.0;
                if let Some(font) = self.font.as_ref() {
                    let word = title.split_whitespace().next().unwrap_or("");
                    let left = (TILE as i32 - font.width(word, TEXT_SIZE) as i32).max(0) / 2;
                    let top = (TILE as i32
                        - (font.ascent(TEXT_SIZE) + font.descent(TEXT_SIZE)) as i32)
                        / 2;
                    draw_text(
                        &mut picture,
                        (TILE, TILE),
                        font,
                        word,
                        TEXT_SIZE,
                        (left, top),
                        [0x30, 0x30, 0x30],
                    );
                }
            }
        }
        picture
    }
}

/// The panel's background: the dark blue of the Z-Wheel's own panels, lighter towards the top.
fn backdrop(width: usize, height: usize) -> Vec<u8> {
    let mut picture = Vec::with_capacity(width * height * 3);
    for row in 0..height {
        let shade = |top: usize, bottom: usize| {
            ((top * (height - row) + bottom * row) / height) as u8
        };
        let color = [shade(0x1c, 0x05), shade(0x3a, 0x10), shade(0x66, 0x24)];
        for _ in 0..width {
            picture.extend_from_slice(&color);
        }
    }
    picture
}

/// Fills `(left, top, width, height)` of an RGB picture `stride` pixels wide.
fn fill(picture: &mut [u8], stride: usize, area: (usize, usize, usize, usize), color: [u8; 3]) {
    let (left, top, width, height) = area;
    for row in top..top + height {
        for column in left..left + width {
            let at = (row * stride + column) * 3;
            if let Some(pixel) = picture.get_mut(at..at + 3) {
                pixel.copy_from_slice(&color);
            }
        }
    }
}

/// Draws `art` scaled into `(left, top, width, height)` of an RGB picture `stride` pixels wide,
/// each destination pixel the average of the source pixels it covers. A transparent pixel lets
/// the picture underneath through.
fn paste(picture: &mut [u8], stride: usize, art: &Image, area: (usize, usize, usize, usize)) {
    let (left, top, width, height) = area;
    for row in 0..height {
        let (from_y, to_y) = (row * art.height / height, ((row + 1) * art.height).div_ceil(height));
        for column in 0..width {
            let (from_x, to_x) =
                (column * art.width / width, ((column + 1) * art.width).div_ceil(width));
            let (mut sum, mut count) = ([0u32; 4], 0u32);
            for y in from_y..to_y.max(from_y + 1).min(art.height) {
                for x in from_x..to_x.max(from_x + 1).min(art.width) {
                    let at = (y * art.width + x) * 4;
                    let Some(texel) = art.rgba.get(at..at + 4) else {
                        continue;
                    };
                    let alpha = u32::from(texel[3]);
                    for channel in 0..3 {
                        sum[channel] += u32::from(texel[channel]) * alpha;
                    }
                    sum[3] += alpha;
                    count += 1;
                }
            }
            let at = ((top + row) * stride + left + column) * 3;
            let (Some(pixel), true) = (picture.get_mut(at..at + 3), count != 0 && sum[3] != 0)
            else {
                continue;
            };
            let coverage = sum[3] / count;
            for channel in 0..3 {
                let color = sum[channel] / sum[3];
                let under = u32::from(pixel[channel]);
                pixel[channel] = ((color * coverage + under * (255 - coverage)) / 255) as u8;
            }
        }
    }
}

/// Writes `text` on an RGB picture with its top-left corner at `at`, blending the letters' edges.
fn draw_text(
    picture: &mut [u8],
    size: (usize, usize),
    font: &crate::video::font::Font,
    text: &str,
    text_size: f32,
    at: (i32, i32),
    color: [u8; 3],
) {
    for glyph in font.layout(text, text_size) {
        for line in 0..glyph.height {
            for column in 0..glyph.width {
                let coverage = u32::from(glyph.coverage[(line * glyph.width + column) as usize]);
                let (x, y) = (at.0 + glyph.x + column as i32, at.1 + glyph.y + line as i32);
                if coverage == 0 || x < 0 || y < 0 || x >= size.0 as i32 || y >= size.1 as i32 {
                    continue;
                }
                let offset = (y as usize * size.0 + x as usize) * 3;
                for channel in 0..3 {
                    let under = u32::from(picture[offset + channel]);
                    let over = u32::from(color[channel]);
                    picture[offset + channel] =
                        ((over * coverage + under * (255 - coverage)) / 255) as u8;
                }
            }
        }
    }
}

/// Breaks `text` into lines no wider than `width`, at the spaces.
fn wrap(text: &str, width: u32, measure: impl Fn(&str) -> u32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = match line.is_empty() {
            true => word.to_string(),
            false => format!("{line} {word}"),
        };
        if !line.is_empty() && measure(&candidate) > width {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// A QXEngine texture in format 1: `RGB888`, rows top to bottom, optionally followed by its mip
/// chain down to 1×1.
///
/// Read from the package's own: `res_easel64.qxt` is 64×64 and 16423 bytes — a 40-byte header
/// and `12288 + 3072 + 768 + … + 3` texels. The header is the signature `QX\0\0QXT\0`, the word
/// `0x01240028` every texture of the package carries, width, height, format, two zero words, the
/// flags (`0x10000` with a mip chain, zero without) and a last zero word.
fn qxt_rgb(rgb: &[u8], width: usize, height: usize, mips: bool) -> Vec<u8> {
    let mut data = Vec::with_capacity(40 + rgb.len() * 4 / 3);
    data.extend_from_slice(b"QX\0\0QXT\0");
    for word in [
        0x0124_0028u32,
        width as u32,
        height as u32,
        1,
        0,
        0,
        if mips { 0x1_0000 } else { 0 },
        0,
    ] {
        data.extend_from_slice(&word.to_le_bytes());
    }
    data.extend_from_slice(rgb);
    let (mut level, mut width, mut height) = (rgb.to_vec(), width, height);
    while mips && (width > 1 || height > 1) {
        let (half_width, half_height) = ((width / 2).max(1), (height / 2).max(1));
        let mut smaller = Vec::with_capacity(half_width * half_height * 3);
        for row in 0..half_height {
            for column in 0..half_width {
                for channel in 0..3 {
                    // The four texels above, or the two a 1-pixel-wide level has.
                    let sample = |x: usize, y: usize| {
                        u32::from(level[(y.min(height - 1) * width + x.min(width - 1)) * 3 + channel])
                    };
                    let (x, y) = (column * 2, row * 2);
                    let sum = sample(x, y) + sample(x + 1, y) + sample(x, y + 1) + sample(x + 1, y + 1);
                    smaller.push((sum / 4) as u8);
                }
            }
        }
        data.extend_from_slice(&smaller);
        (level, width, height) = (smaller, half_width, half_height);
    }
    data
}

#[cfg(test)]
mod tests {
    use super::{qxt_rgb, wrap};

    #[test]
    fn a_texture_has_the_size_of_the_packages_own() {
        // `res_easel64.qxt`: 64×64 with mips, 16423 bytes. `wheel_t.qxt`: 32×32, 4135.
        assert_eq!(qxt_rgb(&vec![0; 64 * 64 * 3], 64, 64, true).len(), 16423);
        assert_eq!(qxt_rgb(&vec![0; 32 * 32 * 3], 32, 32, true).len(), 4135);
        let plain = qxt_rgb(&vec![7; 64 * 64 * 3], 64, 64, false);
        assert_eq!(plain.len(), 40 + 64 * 64 * 3);
        assert_eq!(
            plain[..40],
            [
                b'Q', b'X', 0, 0, b'Q', b'X', b'T', 0, 0x28, 0, 0x24, 1, 64, 0, 0, 0, 64, 0, 0, 0,
                1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );
    }

    #[test]
    fn a_name_breaks_at_the_spaces() {
        let lines = wrap("Alice no Pais das Maravilhas", 10, |text| text.len() as u32);
        assert_eq!(lines, ["Alice no", "Pais das", "Maravilhas"]);
    }
}
