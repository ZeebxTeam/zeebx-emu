//! The text games ask to draw.
//!
//! Until now `IDISPLAY_DrawText` received the right sentence and had nothing to draw it with: the
//! text went into a list that only showed up in the report. That is enough to know a game
//! *wants* to write, and not enough for any screen with a menu.
//!
//! The console's font came from the firmware, which we do not have. But several modules bring
//! their own: the Z-Wheel bundles a 191 KB `tectoy.ttf`, and it is the font the store was
//! designed with. So the rule is: **use the font that came with the game**. When there is none,
//! there is still no text — and the report says so, instead of drawing with a font that is not
//! the game's.

use ab_glyph::{Font as _, FontVec, PxScale, ScaleFont as _};

/// A font loaded from one of the game's own `.ttf` files.
pub struct Font {
    face: FontVec,
    /// Where it came from, for the report.
    pub source: String,
}

/// An already-rasterized glyph: where to put it and with what coverage.
pub struct Glyph {
    /// Offset from the drawing point, with the `bearing` already applied.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Per-pixel coverage, 0 to 255, in reading order.
    pub coverage: Vec<u8>,
}

impl Font {
    /// Reads a `.ttf`. `None` when the file is not a font we know how to open.
    pub fn load(bytes: Vec<u8>, source: String) -> Option<Self> {
        FontVec::try_from_vec(bytes)
            .ok()
            .map(|face| Self { face, source })
    }

    /// Height of the upper part of the line, at the requested size. It is what `GetFontMetrics`
    /// calls `ascent`.
    pub fn ascent(&self, size: f32) -> u32 {
        self.face.as_scaled(PxScale::from(size)).ascent().ceil() as u32
    }

    /// Height of the lower part, as a positive value — BREW's `descent` is positive, and the
    /// font's is negative.
    pub fn descent(&self, size: f32) -> u32 {
        (-self.face.as_scaled(PxScale::from(size)).descent())
            .ceil()
            .max(0.0) as u32
    }

    /// How much horizontal space the text takes, including the spacing between letters.
    pub fn width(&self, text: &str, size: f32) -> u32 {
        let scale = self.face.as_scaled(PxScale::from(size));
        let mut width = 0.0;
        let mut previous = None;
        for c in text.chars() {
            let id = self.face.glyph_id(c);
            if let Some(before) = previous {
                width += scale.kern(before, id);
            }
            width += scale.h_advance(id);
            previous = Some(id);
        }
        width.ceil().max(0.0) as u32
    }

    /// Rasterizes `text` and returns one glyph per character that has a drawing.
    ///
    /// The origin is the text's **top-left corner**, which is how BREW positions it — the
    /// baseline comes in here, by adding the `ascent`, so the caller does not need to know about
    /// it.
    pub fn layout(&self, text: &str, size: f32) -> Vec<Glyph> {
        let scale = self.face.as_scaled(PxScale::from(size));
        let base = scale.ascent();
        let mut output = Vec::new();
        let mut pen_x = 0.0;
        let mut previous = None;
        for c in text.chars() {
            let id = self.face.glyph_id(c);
            if let Some(before) = previous {
                pen_x += scale.kern(before, id);
            }
            let glyph =
                id.with_scale_and_position(PxScale::from(size), ab_glyph::point(pen_x, base));
            pen_x += scale.h_advance(id);
            previous = Some(id);
            // A space has no outline: advance the pen and draw nothing.
            let Some(drawing) = self.face.outline_glyph(glyph) else {
                continue;
            };
            let container = drawing.px_bounds();
            let (width, height) = (container.width() as u32, container.height() as u32);
            if width == 0 || height == 0 {
                continue;
            }
            let mut coverage = vec![0u8; (width * height) as usize];
            drawing.draw(|x, y, v| {
                if x < width && y < height {
                    coverage[(y * width + x) as usize] =
                        (v * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            });
            output.push(Glyph {
                x: container.min.x as i32,
                y: container.min.y as i32,
                width,
                height,
                coverage,
            });
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The font the Z-Wheel bundles. The test only runs when it is in the cache — it is the
    /// user's material, not the repository's.
    fn tectoy() -> Option<Font> {
        let cache = dirs_cache()?;
        let ttf = std::fs::read(cache).ok()?;
        Font::load(ttf, "tectoy.ttf".into())
    }

    fn dirs_cache() -> Option<std::path::PathBuf> {
        let base = std::env::var_os("HOME")?;
        let root = std::path::Path::new(&base).join(".config/zeebx/cache");
        std::fs::read_dir(root)
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("Z-Wheel"))
            })
            .map(|p| p.join("mod/274755/tectoy.ttf"))
            .filter(|p| p.exists())
    }

    #[test]
    fn what_is_not_a_font_is_rejected() {
        assert!(Font::load(b"nao sou uma fonte".to_vec(), "x".into()).is_none());
        assert!(Font::load(Vec::new(), "x".into()).is_none());
    }

    #[test]
    fn the_bundled_font_measures_and_draws() {
        let Some(font) = tectoy() else {
            // Without the game in the cache there is nothing to test: the test exists for whoever
            // has the material.
            return;
        };
        // A 16-pixel line has a plausible line height, neither zero nor absurd.
        let (ascent, descent) = (font.ascent(16.0), font.descent(16.0));
        assert!(
            (8..40).contains(&ascent),
            "ascent out of reasonable range: {ascent}"
        );
        assert!(descent < 20, "descent out of reasonable range: {descent}");

        // Longer text takes more space, and whitespace counts.
        assert!(font.width("Jogar", 16.0) > font.width("J", 16.0));
        assert!(font.width("a a", 16.0) > font.width("aa", 16.0));

        // "Jogar" has five letters and all of them draw something.
        let glyphs = font.layout("Jogar", 16.0);
        assert_eq!(glyphs.len(), 5);
        assert!(glyphs.iter().all(|g| g.coverage.iter().any(|&v| v > 0)));

        // The space does not become a glyph, but it pushes what comes after it.
        let with_space = font.layout("a b", 16.0);
        assert_eq!(with_space.len(), 2);
        assert!(with_space[1].x > with_space[0].x);
    }
}
