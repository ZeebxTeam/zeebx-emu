//! The drawing of the controller that lights up when a button is pressed.
//!
//! There are **two** pieces: the art, a PNG that only has to look good, and the map, an invisible
//! SVG of the same size in which each shape has as its `id` the name of a Zeebo button (`up`,
//! `b1`, `start`...). Keeping the two apart is what lets the controller be redrawn without
//! touching code, and spares the art from having to be made with an `id` on every stroke.
//!
//! The map is rasterized **once**, each shape becoming a cropped silhouette. Both things that
//! matter come from it — lighting up the right button and knowing which one the cursor is over.
//! Hit-testing the click against the silhouette, and not a rectangular box, is what makes a
//! cross-shaped D-pad and a round button respond only where there is a button.
//!
//! A button the art does not draw simply has no shape in the map: it does not light up, and stays
//! configurable through the list.

use std::fmt;

use resvg::{tiny_skia, usvg};

/// The controller art, embedded in the binary so the screen works without any file.
pub const DEFAULT_ART: &[u8] = include_bytes!("../../assets/controller.png");
/// The map of the regions of the embedded art.
pub const DEFAULT_MAP: &str = include_str!("../../assets/controller-map.svg");

/// From how much opacity the silhouette counts as "there is a button here".
const HIT_ALPHA: u8 = 96;

/// Up to what brightness a background pixel counts as background, when cutting out the art's
/// frame.
const BACKDROP_LUMA: u8 = 40;

#[derive(Debug)]
pub enum ArtError {
    Png(png::DecodingError),
    Svg(usvg::Error),
    /// The art and the map do not have the same size, or one of them has no size at all.
    Size,
}

impl fmt::Display for ArtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Png(err) => write!(f, "unreadable controller art: {err}"),
            Self::Svg(err) => write!(f, "unreadable controller map: {err}"),
            Self::Size => write!(f, "the controller art and map do not match in size"),
        }
    }
}

impl std::error::Error for ArtError {}

/// The silhouette of a button, cropped to its own size.
///
/// The crop is not about saving disk space: each silhouette becomes a texture on the graphics
/// card, and keeping them the size of the whole controller would cost tens of megabytes to draw
/// buttons a few pixels wide.
pub struct Part {
    /// The name of the Zeebo button, the same as the `id` in the drawing.
    pub button: String,
    /// Where the crop sits in the drawing, in pixels.
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    /// Only the opacity, one byte per pixel of the crop. The color does not matter: the
    /// silhouette is painted with whatever color the screen picks at the time, which keeps it
    /// readable on any drawing.
    pub alpha: Vec<u8>,
}

impl Part {
    /// The piece's place in the drawing, from 0 to 1, for the screen to position it at any size.
    pub fn bounds(&self, art: &PadArt) -> [f32; 4] {
        [
            self.x as f32 / art.width as f32,
            self.y as f32 / art.height as f32,
            (self.x + self.width) as f32 / art.width as f32,
            (self.y + self.height) as f32 / art.height as f32,
        ]
    }

    /// The opacity at a point of the drawing, or zero outside the crop.
    fn alpha_at(&self, x: usize, y: usize) -> u8 {
        if x < self.x || y < self.y || x >= self.x + self.width || y >= self.y + self.height {
            return 0;
        }
        self.alpha[(y - self.y) * self.width + (x - self.x)]
    }
}

pub struct PadArt {
    pub width: usize,
    pub height: usize,
    /// The art, non-premultiplied RGBA — what the interface expects to receive.
    pub base: Vec<u8>,
    parts: Vec<Part>,
}

impl PadArt {
    /// Decodes the art and rasterizes the map on top of it, at the same size.
    pub fn parse(art: &[u8], map: &str) -> Result<Self, ArtError> {
        let (mut base, width, height) = decode_png(art)?;
        cut_backdrop(&mut base, width as usize, height as usize);

        let tree = usvg::Tree::from_str(map, &usvg::Options::default()).map_err(ArtError::Svg)?;
        let size = tree.size();
        if size.width() <= 0.0 || size.height() <= 0.0 {
            return Err(ArtError::Size);
        }
        // The map is stretched to the art instead of requiring the same numbers: what has to
        // match is the aspect ratio, and a map drawn at half scale still works.
        let transform = tiny_skia::Transform::from_scale(
            width as f32 / size.width(),
            height as f32 / size.height(),
        );

        let mut parts = Vec::new();
        for button in crate::input::bindings::CONFIGURABLE {
            let Some(node) = tree.node_by_id(button) else {
                continue;
            };
            // `render_node` moves the node to the target's origin; giving the offset back here
            // keeps it where it was drawn, which is what aligns the silhouette with the
            // background.
            let Some(bbox) = node.abs_layer_bounding_box() else {
                continue;
            };
            let mut mask = tiny_skia::Pixmap::new(width, height).ok_or(ArtError::Size)?;
            let placed = transform.pre_translate(bbox.x(), bbox.y());
            if resvg::render_node(node, placed, &mut mask.as_mut()).is_none() {
                continue;
            }
            let full: Vec<u8> = mask.pixels().iter().map(|pixel| pixel.alpha()).collect();
            let Some(part) = crop(button, &full, width as usize, height as usize) else {
                continue;
            };
            parts.push(part);
        }

        Ok(Self {
            width: width as usize,
            height: height as usize,
            base,
            parts,
        })
    }

    /// The embedded art, with its map.
    pub fn builtin() -> Result<Self, ArtError> {
        Self::parse(DEFAULT_ART, DEFAULT_MAP)
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// The button under the point `(u, v)`, both from 0 to 1 over the drawing.
    ///
    /// Where two pieces overlap the more opaque one wins — the top one, in practice.
    pub fn hit(&self, u: f32, v: f32) -> Option<&str> {
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return None;
        }
        let x = (u * self.width as f32) as usize;
        let y = (v * self.height as f32) as usize;
        self.parts
            .iter()
            .filter_map(|part| {
                let alpha = part.alpha_at(x, y);
                (alpha >= HIT_ALPHA).then_some((alpha, part.button.as_str()))
            })
            .max_by_key(|(alpha, _)| *alpha)
            .map(|(_, button)| button)
    }
}

/// Decodes the art, returning non-premultiplied RGBA and the size.
fn decode_png(data: &[u8]) -> Result<(Vec<u8>, u32, u32), ArtError> {
    let decoder = png::Decoder::new(std::io::Cursor::new(data));
    let mut reader = decoder.read_info().map_err(ArtError::Png)?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or(ArtError::Size)?];
    let info = reader.next_frame(&mut buffer).map_err(ArtError::Png)?;
    let channels = info.color_type.samples();
    if info.bit_depth != png::BitDepth::Eight || !(3..=4).contains(&channels) {
        return Err(ArtError::Size);
    }
    let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
    for pixel in buffer[..info.buffer_size()].chunks_exact(channels) {
        rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2]]);
        rgba.push(match channels {
            4 => pixel[3],
            _ => 255,
        });
    }
    Ok((rgba, info.width, info.height))
}

/// Makes the dark background surrounding the art transparent.
///
/// Only what touches the border goes away: the black stroke inside the drawing is as dark as the
/// background, and erasing it by color would leave the controller with no outline at all.
fn cut_backdrop(rgba: &mut [u8], width: usize, height: usize) {
    let dark = |index: usize| -> bool {
        let [r, g, b] = [rgba[index * 4], rgba[index * 4 + 1], rgba[index * 4 + 2]];
        (r as u32 * 30 + g as u32 * 59 + b as u32 * 11) / 100 <= BACKDROP_LUMA as u32
    };
    let mut seen = vec![false; width * height];
    let mut queue: Vec<usize> = Vec::new();
    let mut borders: Vec<usize> = (0..width)
        .flat_map(|x| [x, (height - 1) * width + x])
        .chain((0..height).flat_map(|y| [y * width, y * width + width - 1]))
        .collect();
    borders.retain(|&index| !std::mem::replace(&mut seen[index], true) && dark(index));
    queue.extend(borders);

    // The sweep only marks; erasing is left for the end, because the darkness test reads the same
    // colors the erasing would change.
    let mut background = Vec::new();
    while let Some(index) = queue.pop() {
        background.push(index);
        let (x, y) = (index % width, index / width);
        let mut visit = |next: usize, queue: &mut Vec<usize>| {
            if !seen[next] && dark(next) {
                seen[next] = true;
                queue.push(next);
            }
            seen[next] = true;
        };
        if x > 0 {
            visit(index - 1, &mut queue);
        }
        if x + 1 < width {
            visit(index + 1, &mut queue);
        }
        if y > 0 {
            visit(index - width, &mut queue);
        }
        if y + 1 < height {
            visit(index + width, &mut queue);
        }
    }
    for index in background {
        rgba[index * 4 + 3] = 0;
    }
}

/// Crops the silhouette to the smallest rectangle that contains it. `None` if it is empty — which
/// happens with an `id` attached to a piece with no outline.
fn crop(button: &str, alpha: &[u8], width: usize, height: usize) -> Option<Part> {
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0usize, 0usize);
    for y in 0..height {
        for x in 0..width {
            if alpha[y * width + x] > 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let mut cropped = Vec::with_capacity(w * h);
    for y in y0..y1 {
        cropped.extend_from_slice(&alpha[y * width + x0..y * width + x1]);
    }
    Some(Part {
        button: button.to_string(),
        x: x0,
        y: y0,
        width: w,
        height: h,
        alpha: cropped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal art: 20x10 black with a white square in the middle.
    fn art() -> Vec<u8> {
        let mut rgba = vec![0u8; 20 * 10 * 4];
        for y in 3..7 {
            for x in 6..14 {
                let i = (y * 20 + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut out), 20, 10);
        encoder.set_color(png::ColorType::Rgba);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&rgba)
            .unwrap();
        out
    }

    #[test]
    fn the_builtin_art_has_the_map_of_the_buttons_it_draws() {
        let art = PadArt::builtin().expect("the embedded art must open");
        // The D-pad, the action buttons, the analog sticks and HOME — the middle button — are
        // drawn; the lower triggers do not appear in this art, and so they have no shape.
        for button in [
            "up", "down", "left", "right", "b1", "b2", "b3", "b4", "lthumb", "rthumb", "back",
            "zl", "zr",
        ] {
            assert!(
                art.parts().iter().any(|part| part.button == button),
                "the map has no piece {button}"
            );
        }
        assert_eq!(art.width, 1616);
        assert_eq!(art.height, 973);
    }

    #[test]
    fn the_click_lands_on_the_drawn_button() {
        let art = PadArt::builtin().unwrap();
        let at = |x: f32, y: f32| art.hit(x / 1616.0, y / 973.0);
        // The center of each arm of the D-pad, and then the four action buttons.
        assert_eq!(at(373.0, 370.0), Some("up"));
        assert_eq!(at(373.0, 620.0), Some("down"));
        assert_eq!(at(230.0, 498.0), Some("left"));
        assert_eq!(at(520.0, 498.0), Some("right"));
        // The diamond, in the device's numbering: 1 at the bottom, 2 on the left, 3 at the top,
        // 4 on the right.
        assert_eq!(at(1246.0, 367.0), Some("b3"));
        assert_eq!(at(1373.0, 465.0), Some("b4"));
        assert_eq!(at(1246.0, 565.0), Some("b1"));
        assert_eq!(at(1119.0, 465.0), Some("b2"));
        assert_eq!(at(619.0, 715.0), Some("lthumb"));
        assert_eq!(at(995.0, 715.0), Some("rthumb"));
        // The middle button is HOME; the console has no Start.
        assert_eq!(at(807.0, 427.0), Some("back"));
    }

    #[test]
    fn outside_the_touch_controls_there_is_no_button() {
        let art = PadArt::builtin().unwrap();
        // The middle of the D-pad is no direction at all, and the plain body is not a button.
        assert_eq!(art.hit(373.0 / 1616.0, 498.0 / 973.0), None);
        assert_eq!(art.hit(0.5, 0.95), None);
        // Outside the drawing neither.
        assert_eq!(art.hit(-0.1, 0.5), None);
        assert_eq!(art.hit(0.5, 1.5), None);
    }

    #[test]
    fn a_shape_with_an_unknown_id_is_ignored() {
        let map = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 10">
            <rect id="nao_e_botao" x="0" y="0" width="20" height="10" fill="red"/>
            <rect id="back" x="8" y="4" width="4" height="2" fill="blue"/>
        </svg>"#;
        let art = PadArt::parse(&art(), map).unwrap();
        assert_eq!(art.parts().len(), 1);
        assert_eq!(art.parts()[0].button, "back");
        assert_eq!(art.hit(0.05, 0.05), None);
        assert_eq!(art.hit(0.5, 0.5), Some("back"));
    }

    #[test]
    fn the_dark_border_of_the_art_becomes_transparent() {
        // The background surrounding the drawing goes away so the controller does not become a
        // black rectangle on screen — but only what touches the border, otherwise the black
        // stroke inside would go too.
        let map = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 10"/>"#;
        let art = PadArt::parse(&art(), map).unwrap();
        let alpha = |x: usize, y: usize| art.base[(y * 20 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(19, 9), 0);
        assert_eq!(alpha(8, 4), 255);
    }

    #[test]
    fn the_touch_control_is_cropped_to_its_size() {
        // The crop has to fall on the map's shape, not on the whole sheet: it is what gives the
        // place where the silhouette is painted.
        let art = PadArt::builtin().unwrap();
        let up = art.parts().iter().find(|part| part.button == "up").unwrap();
        let [u0, v0, u1, v1] = up.bounds(&art);
        // `up` is the rectangle 299..447 x 322..432 of the map.
        let close = |got: f32, want: f32| (got - want).abs() < 0.01;
        assert!(close(u0, 299.0 / 1616.0), "u0 = {u0}");
        assert!(close(v0, 322.0 / 973.0), "v0 = {v0}");
        assert!(close(u1, 447.0 / 1616.0), "u1 = {u1}");
        assert!(close(v1, 432.0 / 973.0), "v1 = {v1}");
        assert_eq!(up.alpha.len(), up.width * up.height);
    }

    #[test]
    fn invalid_art_or_map_breaks_nothing() {
        assert!(PadArt::parse(b"isto nao e um png", DEFAULT_MAP).is_err());
        assert!(PadArt::parse(&art(), "isto não é um svg").is_err());
    }
}
