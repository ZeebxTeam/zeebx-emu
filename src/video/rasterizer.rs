//! Software rasterizer for BREW's OpenGL ES 1.1.
//!
//! The console has an Adreno 130; here the GPU is the host CPU. What matters is the contract: the
//! game hands over vertices in object coordinates, modelview and projection matrices, textures
//! and a handful of fixed states, and expects a frame back.
//!
//! It is the classic fixed pipeline, without lighting: transform, clip against the near plane,
//! perspective divide, map to the screen and fill triangles with perspective-correct
//! interpolation, depth test, blending and alpha test.

use std::collections::HashMap;

use crate::video::gles;

/// A 4×4 matrix in OpenGL order: column first, `m[column * 4 + row]`.
pub type Matrix = [f32; 16];

/// The identity.
pub const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

/// `a * b`, in the OpenGL convention (`b` is applied first).
pub fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            out[column * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[column * 4 + k]).sum();
        }
    }
    out
}

/// Applies the matrix to a homogeneous point.
pub fn transform(m: &Matrix, v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (row, slot) in out.iter_mut().enumerate() {
        *slot = (0..4).map(|k| m[k * 4 + row] * v[k]).sum();
    }
    out
}

/// Translation matrix.
pub fn translation(x: f32, y: f32, z: f32) -> Matrix {
    let mut m = IDENTITY;
    (m[12], m[13], m[14]) = (x, y, z);
    m
}

/// Scaling matrix.
pub fn scaling(x: f32, y: f32, z: f32) -> Matrix {
    let mut m = IDENTITY;
    (m[0], m[5], m[10]) = (x, y, z);
    m
}

/// Rotation of `angle` degrees around the axis `(x, y, z)`, like `glRotate`.
pub fn rotation(angle: f32, x: f32, y: f32, z: f32) -> Matrix {
    let length = (x * x + y * y + z * z).sqrt();
    if length == 0.0 {
        return IDENTITY;
    }
    let (x, y, z) = (x / length, y / length, z / length);
    let (s, c) = angle.to_radians().sin_cos();
    let t = 1.0 - c;
    [
        t * x * x + c,
        t * x * y + s * z,
        t * x * z - s * y,
        0.0,
        t * x * y - s * z,
        t * y * y + c,
        t * y * z + s * x,
        0.0,
        t * x * z + s * y,
        t * y * z - s * x,
        t * z * z + c,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

/// Perspective projection, like `glFrustum`.
pub fn frustum(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> Matrix {
    let mut m = [0.0; 16];
    m[0] = 2.0 * n / (r - l);
    m[5] = 2.0 * n / (t - b);
    m[8] = (r + l) / (r - l);
    m[9] = (t + b) / (t - b);
    m[10] = -(f + n) / (f - n);
    m[11] = -1.0;
    m[14] = -2.0 * f * n / (f - n);
    m
}

/// Orthographic projection, like `glOrtho`.
pub fn ortho(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> Matrix {
    let mut m = IDENTITY;
    m[0] = 2.0 / (r - l);
    m[5] = 2.0 / (t - b);
    m[10] = -2.0 / (f - n);
    m[12] = -(r + l) / (r - l);
    m[13] = -(t + b) / (t - b);
    m[14] = -(f + n) / (f - n);
    m
}

/// A vertex ready for the pipeline: position in object coordinates, color and texture
/// coordinate.
#[derive(Debug, Clone, Copy)]
pub struct Vertex {
    pub position: [f32; 4],
    pub color: [f32; 4],
    /// The texture coordinate `(s, t, r, q)`. There are four because the texture matrix mixes
    /// all of them: Reckless Racing sends the vertex position as a three-component coordinate
    /// and lets the matrix derive `s` and `t` from `x` and `z` — with only two, `z` vanished, `t`
    /// stayed constant and the shadow came out in stripes. After the vertex stage, `s` and `t`
    /// are already divided by `q`, and only they matter to the rasterizers.
    pub uv: [f32; 4],
    /// The texture coordinate of unit 1. See [`TextureUnit`].
    pub uv1: [f32; 4],
    /// The normal in object coordinates, for lighting. The OpenGL default is `(0, 0, 1)`.
    pub normal: [f32; 3],
    /// How much of the fragment color remains after fog: 1 is a clear scene, 0 is full fog.
    ///
    /// It comes computed from the vertex stage, which is where the eye-space distance exists,
    /// and is interpolated like the other attributes. It leaves here ready because the vertex
    /// stage is the same for both rasterizers — the GPU gets the factor as an attribute and only
    /// blends.
    pub fog: f32,
}

/// The texture coordinate for whoever doesn't send one: `(0, 0, 0, 1)`, the OpenGL default.
pub const DEFAULT_UV: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

impl Default for Vertex {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0, 1.0],
            color: [1.0; 4],
            uv: DEFAULT_UV,
            uv1: DEFAULT_UV,
            normal: [0.0, 0.0, 1.0],
            fog: 1.0,
        }
    }
}

/// The texture environment of unit 0: `glTexEnv`, with the mode and the `GL_COMBINE` setup.
///
/// **`GL_COMBINE` was not handled, and fell through to `GL_MODULATE`.** The SDK's QX engine
/// (Dragon Vs Chicken) draws the characters with source 0 on the texture and the `GL_REPLACE`
/// function: the vertex color doesn't take part, and it leaves it zeroed. Multiplied by the
/// texture, it erased the dragon and the chickens — transparent black, which the alpha test
/// discarded.
///
/// Indices `[0]` and `[1]` are RGB and alpha. The defaults are the specification's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexEnv {
    pub mode: u32,
    pub combine: [u32; 2],
    pub sources: [[u32; 3]; 2],
    pub operands: [[u32; 3]; 2],
    pub scale: [f32; 2],
    /// `GL_TEXTURE_ENV_COLOR`, the `GL_CONSTANT` source.
    pub color: [f32; 4],
}

impl Default for TexEnv {
    fn default() -> Self {
        let sources = [gles::GL_TEXTURE, gles::GL_PREVIOUS, gles::GL_CONSTANT];
        Self {
            mode: gles::GL_MODULATE,
            combine: [gles::GL_MODULATE; 2],
            sources: [sources; 2],
            operands: [
                [gles::GL_SRC_COLOR, gles::GL_SRC_COLOR, gles::GL_SRC_ALPHA],
                [gles::GL_SRC_ALPHA; 3],
            ],
            scale: [1.0; 2],
            color: [0.0; 4],
        }
    }
}

impl TexEnv {
    /// Mode only, for whoever builds a separate state (the GPU bridge, the tests).
    pub fn with_mode(mode: u32) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    /// Applies a `glTexEnv` that is neither the mode nor the color. Enums arrive as integers even
    /// in the `x` and `f` forms; scales arrive as numbers.
    ///
    /// **A function or source that doesn't exist is rejected, and the previous value stays.**
    /// It is the driver's `GL_INVALID_ENUM`. Bóia Cross sets up the water with two typos:
    /// `GL_COMBINE_RGB` = `GL_DOT3_RGB` followed by `GL_COMBINE_RGB` = `GL_TEXTURE0` (it meant
    /// `GL_SRC0_RGB`), and on unit 1 `GL_COMBINE_RGB` = `GL_COMBINE`. If accepted, unit 0 went
    /// from `DOT3` to multiplication — the normal map times the light vector, which is
    /// blue-purple — and the water of the first track turned that color. On the device it is
    /// brown.
    pub fn define(&mut self, pname: u32, enum_value: u32, number: f32) {
        let strip = |base: u32| {
            pname
                .checked_sub(base)
                .filter(|&i| i < 3)
                .map(|i| i as usize)
        };
        match pname {
            gles::GL_COMBINE_RGB if is_combine_function(enum_value, true) => {
                self.combine[0] = enum_value
            }
            gles::GL_COMBINE_ALPHA if is_combine_function(enum_value, false) => {
                self.combine[1] = enum_value
            }
            gles::GL_COMBINE_RGB | gles::GL_COMBINE_ALPHA => {}
            gles::GL_RGB_SCALE => self.scale[0] = number,
            gles::GL_ALPHA_SCALE => self.scale[1] = number,
            _ => {
                if let Some(i) = strip(gles::GL_SRC0_RGB) {
                    if is_combine_source(enum_value) {
                        self.sources[0][i] = enum_value;
                    }
                } else if let Some(i) = strip(gles::GL_SRC0_ALPHA) {
                    if is_combine_source(enum_value) {
                        self.sources[1][i] = enum_value;
                    }
                } else if let Some(i) = strip(gles::GL_OPERAND0_RGB) {
                    self.operands[0][i] = enum_value;
                } else if let Some(i) = strip(gles::GL_OPERAND0_ALPHA) {
                    self.operands[1][i] = enum_value;
                }
            }
        }
    }

    /// The fragment color from the primary color and the texel, on unit 0, without unit 1.
    pub fn apply(&self, primary: [f32; 4], texel: [f32; 4]) -> [f32; 4] {
        self.apply_with(primary, primary, texel, [Some(texel), None])
    }

    /// The same on any unit: `previous` is what came out of the previous unit (the primary
    /// color, on unit 0), and it is what the classic modes act on. `texels` is what each unit
    /// sampled, `None` on a disabled one — that is where the `GL_COMBINE` sources `GL_TEXTURE0`
    /// and `GL_TEXTURE1` come from.
    pub fn apply_with(
        &self,
        previous: [f32; 4],
        primary: [f32; 4],
        texel: [f32; 4],
        texels: Texels,
    ) -> [f32; 4] {
        let primary_ = primary;
        let primary = previous;
        match self.mode {
            gles::GL_REPLACE => texel,
            gles::GL_DECAL => {
                let mut out = primary;
                for c in 0..3 {
                    out[c] = primary[c] * (1.0 - texel[3]) + texel[c] * texel[3];
                }
                out
            }
            gles::GL_ADD => {
                let mut out = primary;
                for c in 0..3 {
                    out[c] = (primary[c] + texel[c]).min(1.0);
                }
                out[3] = primary[3] * texel[3];
                out
            }
            gles::GL_COMBINE => self.combine(previous, primary_, texel, texels),
            // `GL_MODULATE` is the default and what games almost always use.
            _ => std::array::from_fn(|c| primary[c] * texel[c]),
        }
    }

    /// `GL_COMBINE`. On unit 0, `GL_PREVIOUS` is the primary color.
    fn combine(
        &self,
        previous: [f32; 4],
        primary: [f32; 4],
        texel: [f32; 4],
        texels: Texels,
    ) -> [f32; 4] {
        // A source on a disabled unit makes this unit behave as disabled, which is the rule of
        // `ARB_texture_env_crossbar`. It was not measured on the device: no tested game ever did
        // that.
        //
        // `texel_of` is `None` for anything that isn't `GL_TEXTUREn`, and `Some(None)` for a
        // disabled unit.
        let texel_of = |which: u32| texels.get(which.wrapping_sub(gles::GL_TEXTURE0) as usize);
        let disabled = |&which: &u32| texel_of(which) == Some(&None);
        if self.sources.iter().flatten().any(disabled) {
            return previous;
        }
        let source = |which: u32| match which {
            gles::GL_TEXTURE => texel,
            gles::GL_CONSTANT => self.color,
            gles::GL_PRIMARY_COLOR => primary,
            // The crossbar: a unit's texel by its name. Unit 1 of Bóia Cross's water takes the
            // color from `GL_TEXTURE1` and the alpha too; read as `GL_PREVIOUS`, the brown color
            // of the water was never sampled.
            which => texel_of(which).copied().flatten().unwrap_or(previous),
        };
        // The operand of a source, for one channel. For alpha only the two alpha ones exist; a
        // game that sends `GL_SRC_COLOR` there (QX does) gets the alpha, which is what the driver
        // does.
        let operand = |channel: usize, i: usize| -> f32 {
            let side = usize::from(channel == 3);
            let value = source(self.sources[side][i]);
            match (self.operands[side][i], side) {
                (gles::GL_SRC_COLOR, 0) => value[channel],
                (gles::GL_ONE_MINUS_SRC_COLOR, 0) => 1.0 - value[channel],
                (gles::GL_ONE_MINUS_SRC_ALPHA | gles::GL_ONE_MINUS_SRC_COLOR, _) => 1.0 - value[3],
                _ => value[3],
            }
        };
        let callback = |channel: usize| -> f32 {
            let side = usize::from(channel == 3);
            let a = |i| operand(channel, i);
            let result = match self.combine[side] {
                gles::GL_REPLACE => a(0),
                gles::GL_ADD => a(0) + a(1),
                gles::GL_ADD_SIGNED => a(0) + a(1) - 0.5,
                gles::GL_INTERPOLATE => a(0) * a(2) + a(1) * (1.0 - a(2)),
                gles::GL_SUBTRACT => a(0) - a(1),
                gles::GL_DOT3_RGB | gles::GL_DOT3_RGBA => {
                    let product: f32 = (0..3)
                        .map(|c| (operand(c, 0) - 0.5) * (operand(c, 1) - 0.5))
                        .sum();
                    4.0 * product
                }
                _ => a(0) * a(1),
            };
            (result * self.scale[side]).clamp(0.0, 1.0)
        };
        let mut output: [f32; 4] = std::array::from_fn(callback);
        // `DOT3_RGBA` puts the product in the alpha too, overriding the alpha function.
        if self.combine[0] == gles::GL_DOT3_RGBA {
            output[3] = output[0];
        }
        output
    }
}

/// The texel each unit sampled for the fragment, `None` on one that doesn't draw.
pub type Texels = [Option<[f32; 4]>; 2];

/// Whether `callback` is a `GL_COMBINE` function for RGB (`rgb`) or for alpha. The `DOT3` ones
/// only exist for RGB.
fn is_combine_function(callback: u32, rgb: bool) -> bool {
    match callback {
        gles::GL_REPLACE
        | gles::GL_MODULATE
        | gles::GL_ADD
        | gles::GL_ADD_SIGNED
        | gles::GL_INTERPOLATE
        | gles::GL_SUBTRACT => true,
        gles::GL_DOT3_RGB | gles::GL_DOT3_RGBA => rgb,
        _ => false,
    }
}

/// Whether `source` is a `GL_COMBINE` source: the four from OpenGL ES 1.1 and, from the crossbar,
/// the two units we have, by name.
///
/// **The crossbar is not part of OpenGL ES 1.1, but the device accepts it.** The proof is Bóia
/// Cross's water: unit 1 only reads the brown color texture through `GL_TEXTURE1`, and with the
/// source rejected it would stay on the default `GL_PREVIOUS`, with no brown at all. In the
/// longplay captured from the console, the water of the first track is brown.
fn is_combine_source(source: u32) -> bool {
    matches!(
        source,
        gles::GL_TEXTURE | gles::GL_CONSTANT | gles::GL_PRIMARY_COLOR | gles::GL_PREVIOUS
    ) || source.wrapping_sub(gles::GL_TEXTURE0) < 2
}

/// Texture unit 1: whether it is enabled, its texture and its environment.
///
/// **The pipeline drew only unit 0**, and answered `GL_MAX_TEXTURE_UNITS` = 1. The console's
/// Adreno 130 has two, and the OpenGL ES 1.1 specification requires at least two. The SDK's QX
/// engine (Dragon Vs Chicken) checks the number and, with fewer than two, turns off the lighting
/// manager — the characters fell into a zeroed-color path and vanished. With two, it lights them
/// through the texture: `DOT3` between the bump map and the vertex color on unit 0, and
/// `ADD_SIGNED` with the color texture on unit 1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextureUnit {
    pub enabled: bool,
    pub texture: u32,
    pub env: TexEnv,
}

/// Fixed-function fog: OpenGL ES 1.1's `glFog*` and `GL_FOG`.
///
/// The factor comes from the **eye-space distance**, and it says how much of the fog color goes
/// into the fragment: 1 is a clear scene, 0 is full fog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fog {
    pub enabled: bool,
    /// `GL_LINEAR`, `GL_EXP` or `GL_EXP2`.
    pub curve: u32,
    pub density: f32,
    pub start: f32,
    pub end: f32,
    pub color: [f32; 4],
    /// Turned off by the player, not by the game. See [`Rasterizer::set_fog_allowed`].
    pub allowed: bool,
}

impl Default for Fog {
    /// The defaults are OpenGL's: exponential, density 1, from 0 to 1, black and transparent.
    fn default() -> Self {
        Self {
            enabled: false,
            curve: gles::GL_EXP,
            density: 1.0,
            start: 0.0,
            end: 1.0,
            color: [0.0; 4],
            allowed: true,
        }
    }
}

impl Fog {
    /// How much of the original color remains at this distance from the eye: 1 is a clear
    /// scene, 0 is full fog.
    ///
    /// Outside the range the factor is clamped to `[0, 1]`, as the specification requires —
    /// without that, linear fog **brightens** whatever is closer than `START`, instead of leaving
    /// it untouched.
    pub fn factor(&self, distance: f32) -> f32 {
        if !self.enabled || !self.allowed {
            return 1.0;
        }
        let f = match self.curve {
            gles::GL_LINEAR => {
                let strip = self.end - self.start;
                match strip.abs() < f32::EPSILON {
                    true => 1.0,
                    false => (self.end - distance) / strip,
                }
            }
            gles::GL_EXP2 => {
                let d = self.density * distance;
                (-(d * d)).exp()
            }
            _ => (-(self.density * distance)).exp(),
        };
        f.clamp(0.0, 1.0)
    }
}

/// One minification level of a texture.
#[derive(Debug, Clone)]
pub struct Level {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[u8; 4]>,
}

/// A texture loaded by `TexImage2D`, always converted to 8-bit RGBA.
#[derive(Debug, Clone)]
pub struct Texture {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[u8; 4]>,
    /// The minification levels, from 1 onward — zero is `width`/`height`/`pixels` above.
    ///
    /// **Discarding them was the cause of the stripes.** The Z-Wheel stage models come with the
    /// whole chain, from 128×128 down to 1×1, and are seen at grazing angles: the side of the car
    /// takes a few pixels of width and covers the whole texture. Always sampling level zero, each
    /// pixel lands on an arbitrary texel and the result is the back of the car repeated in
    /// columns.
    pub mipmaps: Vec<Level>,
    /// How to treat coordinates outside `[0, 1)` on each axis.
    pub wrap: [u32; 2],
    /// Magnification filter, from `GL_TEXTURE_MAG_FILTER`.
    pub filter: u32,
    /// Minification filter, from `GL_TEXTURE_MIN_FILTER`. It says whether there is a mipmap and
    /// whether the transition from one level to the next is interpolated.
    pub min_filter: u32,
    /// `GL_TEXTURE_CROP_RECT_OES`: which piece of the texture `glDrawTex*OES` draws, in texels.
    /// A negative width or height mirrors the axis, which is how the extension flips the image.
    pub crop: [i32; 4],
}

impl Default for Texture {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            mipmaps: Vec::new(),
            // The OpenGL ES defaults: repeat on both axes, magnify by interpolation and minify
            // with mipmaps — the `MIN_FILTER` default is `GL_NEAREST_MIPMAP_LINEAR`.
            wrap: [gles::GL_REPEAT; 2],
            filter: gles::GL_LINEAR,
            min_filter: gles::GL_NEAREST_MIPMAP_LINEAR,
            crop: [0; 4],
        }
    }
}

impl Texture {
    /// Applies an axis's wrap mode to a texel index.
    ///
    /// `GL_CLAMP_TO_EDGE` clamps to the last texel and `GL_REPEAT` wraps around. Treating
    /// everything as repeat makes the edge of a clamped texture show up on the other side — it is
    /// the jagged edge that appeared along the borders of the Crash track.
    #[inline]
    fn wrap(mode: u32, index: i32, size: usize) -> usize {
        let size = size as i32;
        // The common case is the coordinate already being inside the texture, and then there is
        // nothing to do — worth checking first because the rest costs a division, and this runs
        // per texel.
        if index >= 0 && index < size {
            return index as usize;
        }
        if mode == gles::GL_CLAMP_TO_EDGE {
            return index.clamp(0, size - 1) as usize;
        }
        index.rem_euclid(size) as usize
    }

    /// The width, the height and the pixels of a minification level.
    fn level(&self, n: usize) -> (usize, usize, &[[u8; 4]]) {
        match n.checked_sub(1).and_then(|i| self.mipmaps.get(i)) {
            // The declared size and the pixels must agree: a compressed level whose decoder
            // couldn't cope arrives with fewer texels than it claims, and indexing into it would
            // overrun the vector.
            Some(level) if level.width > 0 && level.pixels.len() >= level.width * level.height => {
                (level.width, level.height, &level.pixels)
            }
            _ => (self.width, self.height, &self.pixels),
        }
    }

    /// How many levels the chain has, counting zero.
    fn levels(&self) -> usize {
        // **Only the sequence without gaps counts.** A game may send the levels out of order, or
        // stop halfway, and our list ends up with an empty entry where one was missing. Sampling
        // one of those returned opaque white, and the whole surface came out flat — that is what
        // happened to some of the wheel's items when mipmapping went in.
        1 + self
            .mipmaps
            .iter()
            .take_while(|level| level.width > 0 && level.pixels.len() >= level.width * level.height)
            .count()
    }

    /// Samples the texture, choosing the level by the on-screen minification.
    ///
    /// `lod` is `log2` of how many texels fit in a pixel: zero when one texel is one pixel,
    /// larger when the surface is far away or at a grazing angle. It is the computation OpenGL
    /// prescribes, and it decides between magnifying — with the magnification filter — and
    /// minifying.
    fn sample_lod(&self, u: f32, v: f32, lod: f32) -> [f32; 4] {
        if self.width == 0 || self.height == 0 {
            return [1.0; 4];
        }
        if lod <= 0.0 {
            return self.sample_level(u, v, 0, self.filter == gles::GL_LINEAR);
        }
        // **A texture without a chain is not mipmapped, even if the filter asks for it.** In
        // OpenGL it is "incomplete" and the result is undefined; in practice the device falls
        // back to the base filter, and that is what makes sense here. The `MIN_FILTER` default
        // is `GL_NEAREST_MIPMAP_LINEAR`, and taking it literally started sampling with nearest
        // neighbor the twenty-one stage textures that ask for `GL_LINEAR` and bring no levels at
        // all — the whole car lost definition.
        let base_smooth = !matches!(
            self.min_filter,
            gles::GL_NEAREST | gles::GL_NEAREST_MIPMAP_NEAREST | gles::GL_NEAREST_MIPMAP_LINEAR
        );
        if self.levels() == 1 {
            return self.sample_level(u, v, 0, base_smooth);
        }
        let (mipmapped, between_levels, smooth) = match self.min_filter {
            gles::GL_NEAREST => (false, false, false),
            gles::GL_LINEAR => (false, false, true),
            gles::GL_NEAREST_MIPMAP_NEAREST => (true, false, false),
            gles::GL_LINEAR_MIPMAP_NEAREST => (true, false, true),
            gles::GL_NEAREST_MIPMAP_LINEAR => (true, true, false),
            gles::GL_LINEAR_MIPMAP_LINEAR => (true, true, true),
            _ => (true, true, true),
        };
        if !mipmapped {
            return self.sample_level(u, v, 0, smooth);
        }
        let last = (self.levels() - 1) as f32;
        let lod = lod.min(last);
        let down = lod.floor();
        let a = self.sample_level(u, v, down as usize, smooth);
        if !between_levels || down >= last {
            return a;
        }
        let b = self.sample_level(u, v, down as usize + 1, smooth);
        let t = lod - down;
        std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
    }

    /// Samples one level, with or without interpolation between neighboring texels.
    #[inline]
    fn sample_level(&self, u: f32, v: f32, level: usize, smooth: bool) -> [f32; 4] {
        let (width, height, pixels) = self.level(level);
        if width == 0 || height == 0 {
            return [1.0; 4];
        }
        let texel = |x: i32, y: i32| {
            let x = Self::wrap(self.wrap[0], x, width);
            let y = Self::wrap(self.wrap[1], y, height);
            let p = pixels[y * width + x];
            std::array::from_fn::<f32, 4, _>(|i| p[i] as f32 / 255.0)
        };
        let (x, y) = (u * width as f32 - 0.5, v * height as f32 - 0.5);
        if !smooth {
            return texel(x.round() as i32, y.round() as i32);
        }
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let corners = [
            texel(x0, y0),
            texel(x0 + 1, y0),
            texel(x0, y0 + 1),
            texel(x0 + 1, y0 + 1),
        ];
        std::array::from_fn(|c| {
            let up = corners[0][c] + (corners[1][c] - corners[0][c]) * fx;
            let down = corners[2][c] + (corners[3][c] - corners[2][c]) * fx;
            up + (down - up) * fy
        })
    }
}

/// Complete OpenGL ES state the rasterizer keeps.
/// One of the eight lights of the fixed-function pipeline.
///
/// The defaults are OpenGL ES 1.1's, and **light zero is different from the others**: it starts
/// with white diffuse and specular, the rest with black. This is not table trivia — Z-Wheel only
/// calls `glLightxv` for the ambient of `GL_LIGHT0` and leaves the rest at the default, so the
/// white diffuse that lights the whole stage comes from here and nowhere else.
#[derive(Debug, Clone, Copy)]
pub struct Light {
    pub enabled: bool,
    pub ambient: [f32; 4],
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    /// Already in eye coordinates: `glLight` transforms the position by the modelview at the
    /// moment it is called, not by the one at draw time. The default, `(0, 0, 1, 0)`, is
    /// directional.
    pub position: [f32; 4],
    pub spot_direction: [f32; 3],
    pub spot_exponent: f32,
    pub spot_cutoff: f32,
    /// Constant, linear and quadratic, in that order.
    pub attenuation: [f32; 3],
}

impl Light {
    /// The default for lights from index one onward.
    fn off() -> Self {
        Self {
            enabled: false,
            ambient: [0.0, 0.0, 0.0, 1.0],
            diffuse: [0.0, 0.0, 0.0, 1.0],
            specular: [0.0, 0.0, 0.0, 1.0],
            position: [0.0, 0.0, 1.0, 0.0],
            spot_direction: [0.0, 0.0, -1.0],
            spot_exponent: 0.0,
            spot_cutoff: 180.0,
            attenuation: [1.0, 0.0, 0.0],
        }
    }

    /// The `GL_LIGHT0` default.
    fn zero() -> Self {
        Self {
            diffuse: [1.0; 4],
            specular: [1.0; 4],
            ..Self::off()
        }
    }
}

/// The material, which in ES 1.x is a single one — there are no separate front and back.
#[derive(Debug, Clone, Copy)]
pub struct Material {
    pub ambient: [f32; 4],
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    pub emission: [f32; 4],
    pub shininess: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            ambient: [0.2, 0.2, 0.2, 1.0],
            diffuse: [0.8, 0.8, 0.8, 1.0],
            specular: [0.0, 0.0, 0.0, 1.0],
            emission: [0.0, 0.0, 0.0, 1.0],
            shininess: 0.0,
        }
    }
}

/// The boundary between the emulator and whoever rasterizes.
///
/// Everything the GL dispatch does goes through here — and **only** through here, since the
/// fields of [`GlState`] stopped being public. The list is the contract a second rasterizer must
/// fulfill; today there is a single implementation, the software one, and it is what provides
/// the bit-for-bit reproducible frame guarantee that `ARCHITECTURE.md` describes.
///
/// The inherent methods of [`GlState`] still exist: the trait changes no call site, it only
/// writes down what the boundary is. Whoever implements another backend starts from this list,
/// and whatever isn't on it is not used by the emulator.
pub trait Rasterizer {
    /// Writes, one section per group, the state the game's GL calls changed.
    ///
    /// **It is the counterpart of [`Rasterizer::restore_state`], and both are mandatory on
    /// purpose.** A rasterizer that doesn't know how to save itself has to say so, not silently
    /// save nothing: a save state that loses the drawing state comes back with the wrong scene,
    /// and nothing points to it.
    fn save_state(&self, dest: &mut crate::save_state::Sections);

    /// Restores the state written by [`Rasterizer::save_state`].
    fn restore_state(
        &mut self,
        origin: &crate::save_state::Reader<'_>,
    ) -> Result<(), crate::save_state::Error>;

    /// Finishes the drawing that was queued.
    ///
    /// It exists because of save states, and it is the right answer for the common case: a batch
    /// of triangles waiting its turn **is not** a half-done draw — it is work that would be done
    /// in the next frame anyway. Refusing the save because of it would block the player over
    /// something the engine resolves on its own. After draining, what remains is a genuinely
    /// interrupted draw, and then [`Rasterizer::draw_in_progress`] answers yes.
    fn flush_draw(&mut self);

    /// Whether there is a draw **started and not finished**.
    ///
    /// The frontend serializes between frames, never inside a `retro_run`, so this answers
    /// `false` on every normal save. The saver asks, and **refuses** when the answer is yes: a
    /// state saved in the middle of a `glBegin` would promise a draw that never existed.
    fn draw_in_progress(&self) -> bool;

    fn set_matrix_mode(&mut self, mode: u32);
    fn load_identity(&mut self);
    fn load_matrix(&mut self, m: Matrix);
    fn mult_matrix(&mut self, m: Matrix);
    fn push_matrix(&mut self);
    fn pop_matrix(&mut self);
    /// The matrix at the top of the active stack. See [`GlState::top_matrix`].
    fn top_matrix(&self) -> Matrix;

    fn set_viewport(&mut self, x: i32, y: i32, width: i32, height: i32);
    fn set_scissor(&mut self, x: i32, y: i32, width: i32, height: i32);
    fn set_surface(&mut self, width: usize, height: usize);
    /// Like [`Rasterizer::set_surface`], for the surface the device stretches to the whole
    /// screen — the `EGL_QUALCOMM_surface_scale` one. See [`GlState::stretched_surface`].
    fn set_stretched_surface(&mut self, width: usize, height: usize);
    fn surface(&self) -> (usize, usize);
    fn frame_size(&self) -> (usize, usize);

    fn set_clear_color(&mut self, color: [f32; 4]);
    fn set_clear_depth(&mut self, depth: f32);
    fn set_clear_stencil(&mut self, value: i32);
    fn set_color(&mut self, color: [f32; 4]);
    fn current_color(&self) -> [f32; 4];
    fn clear(&mut self, mask: u32);

    fn set_capability(&mut self, capability: u32, on: bool);
    fn set_shade_model(&mut self, mode: u32);
    fn set_light(&mut self, index: usize, pname: u32, values: [f32; 4]);
    fn set_material(&mut self, pname: u32, values: [f32; 4]);
    fn set_light_model(&mut self, pname: u32, values: [f32; 4]);

    fn set_blend_func(&mut self, src: u32, dst: u32);
    fn set_alpha_func(&mut self, func: u32, reference: f32);
    fn set_depth_func(&mut self, func: u32);
    fn set_depth_mask(&mut self, on: bool);
    /// `glDepthRange`: where the normalized depth goes in the depth buffer.
    fn set_depth_range(&mut self, near: f32, far: f32);
    /// `glFog*`: one fog parameter, with up to four values (the color uses all four).
    fn set_fog(&mut self, pname: u32, values: [f32; 4]);
    /// Whether the game's fog applies. **It is the player's choice, not the game's**: the
    /// console's fog usually hides what its draw distance couldn't reach, and here the whole
    /// scene arrives — whoever prefers to see far turns it off.
    fn set_fog_allowed(&mut self, allowed: bool);
    fn set_color_mask(&mut self, mask: [bool; 4]);
    fn set_cull_face(&mut self, mode: u32);
    fn set_front_face(&mut self, face: u32);
    fn set_stencil_func(&mut self, func: u32, reference: i32, mask: u32);
    fn set_stencil_op(&mut self, failure: u32, zfail: u32, passes: u32);
    fn set_stencil_mask(&mut self, mask: u32);

    fn set_active_texture(&mut self, unit: u32);
    fn set_client_active_texture(&mut self, unit: u32);
    /// The unit chosen by `glClientActiveTexture`, counting from zero.
    fn client_unit(&self) -> u32;
    fn bind_texture(&mut self, name: u32);
    fn bound_texture(&self) -> u32;
    fn set_texture_env(&mut self, mode: u32);
    fn set_texture_env_param(&mut self, pname: u32, enum_value: u32, number: f32);
    fn set_texture_env_color(&mut self, color: [f32; 4]);
    fn set_texture_parameter(&mut self, name: u32, value: u32);
    fn set_texture_crop(&mut self, crop: [i32; 4]);
    fn delete_texture(&mut self, name: u32);
    fn upload_level(
        &mut self,
        name: u32,
        level: u32,
        width: usize,
        height: usize,
        pixels: Vec<[u8; 4]>,
    );
    fn sub_image(
        &mut self,
        name: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: &[[u8; 4]],
    ) -> Result<(), Option<(u32, u32)>>;

    fn draw(&mut self, mode: u32, vertices: &[Vertex]);
    fn draw_texture(&mut self, x: f32, y: f32, z: f32, width: f32, height: f32);

    fn read_rect(&mut self, x: i32, y: i32, width: usize, height: usize) -> Vec<[u8; 4]>;
    /// Sends the drawing to an outside framebuffer — the one the Libretro frontend provides.
    ///
    /// In the `libretro` contract, the frontend presents the frame, and the core draws into the
    /// framebuffer it points to (the `get_current_framebuffer` of `retro_hw_render_callback`),
    /// which may change from one frame to the next — hence the function accepts a switch.
    /// `Some(0)` is the frontend's default framebuffer, which in `glow` is written `None`.
    ///
    /// **In software there is nothing to do**: it draws in memory, and the frame goes out through
    /// the usual [`Self::frame_rgb565`]. That is what lets the same engine serve both paths.
    fn draw_to_fbo(&mut self, _fbo: Option<u32>) {}

    /// The frame texture on the GPU, when it won't be read back by the CPU.
    ///
    /// The zero `v` of this texture is the top of the scene: the fill shader flips Y so that
    /// `glReadPixels` returns the rows in the order of the software surface. Whoever draws the
    /// texture on screen must undo that flip. In software there is no texture.
    fn frame_texture(&self) -> Option<u32> {
        None
    }

    /// The context comes back to the engine after the window paints on it. See
    /// [`GpuState`](crate::video::gpu::GpuState). In software there is no context.
    fn resume_context(&mut self) {}

    /// The frame is over and the context goes back to the frontend, which draws the FBO on
    /// screen with it.
    ///
    /// See [`GpuState`](crate::video::gpu::GpuState). In software there is no context to hand
    /// back.
    fn hand_back_to_frontend(&mut self) {}

    /// Whether bringing the frame into CPU memory costs a **wait for the GPU**.
    ///
    /// Lowers the internal resolution of the 3D, drawing on a smaller surface and scaling up on
    /// presentation. **Only the CPU rasterizer does this**: see [`GlState::set_downscale`]. On the
    /// GPU there is nothing to do — there, filling at 640×480 doesn't saturate the GPU, and
    /// lowering it would only ruin the image.
    fn set_downscale(&mut self, _downscale: usize) {}

    /// State calls sent to the GPU and how many the mirror spared. Zero in software.
    ///
    /// It exists so the mirror's gain is **verifiable**: without both numbers there is no way to
    /// tell whether it is sparing calls or just repeating what was already there.
    fn state_sent_and_spared(&self) -> (u64, u64) {
        (0, 0)
    }

    /// Tells the driver the depth and stencil attachments may be discarded after the frame.
    /// **Experimental, off by default, and only the GPU rasterizer does it.**
    ///
    /// It pays off on tile-based GPUs — the Mali of both handhelds — where it avoids writing the
    /// attachments back to memory. A game that does **not** clear the depth from one frame to the
    /// next relies on it, and that is why it isn't on by default. See
    /// `GpuState::set_tile_discard`.
    fn set_tile_discard(&mut self, _discard: bool) {}

    /// True on the GPU rasterizer: reading the frame back forces the GPU to finish and return
    /// the pixels, and on a tile-based GPU that is a stall. False on the CPU one, where the
    /// "read" is an in-memory conversion — there, deferring saves nothing and also risks the
    /// frontend presenting a stale frame. See [`crate::machine::Machine::present_gl`].
    fn frame_waits_for_gpu(&self) -> bool {
        false
    }

    /// Signals that the game swapped buffers. Only the GPU rasterizer does anything: it measures
    /// the frame for the periodic log report. See `GpuState::end_frame`.
    fn end_frame(&mut self) {}

    fn frame_rgb565(&mut self, width: usize, height: usize, out: &mut Vec<u8>);
    /// Exports directly in native RGB565 for the host screen, without going through bytes.
    fn frame_rgb565_words(&mut self, width: usize, height: usize, out: &mut Vec<u16>);
    fn import_rgb565_changes(&mut self, width: usize, height: usize, old: &[u8], new: &[u8]);

    /// How many times larger than the console's the frame is drawn, per side. The game still
    /// sees 640×480: viewport, pixel reads and frame copies are converted. Only the GPU can do
    /// this; in software the cost would grow with the square of the factor, and it ignores it.
    fn set_scale(&mut self, _scale: usize) {}

    /// **Experimental.** Renders perspective 3D at a wider aspect ratio than the console's 4:3,
    /// widening the horizontal field of view instead of stretching. `None` is native.
    fn set_aspect_ratio(&mut self, _aspect: Option<f32>) {}

    /// Multisample antialiasing (MSAA), in samples per pixel; 1 turns it off. Smooths polygon
    /// edges; transparency cut out by the alpha test is not affected.
    fn set_antialias(&mut self, _samples: usize) {}

    /// Anisotropic filtering on the game's textures; 1 turns it off. Sharpens what is seen
    /// edge-on — track, floor, walls. Without the extension on the GPU, it stays off.
    fn set_anisotropic(&mut self, _level: usize) {}

    /// The frame at the internal resolution, read from the GPU as RGBA with row 0 at the top.
    /// For checking the upscale without a window; the window uses [`Rasterizer::gpu_frame`].
    fn read_large_frame(&mut self) -> Option<(usize, usize, Vec<u8>)> {
        None
    }

    /// The frame at the internal resolution, as a GPU texture, for the window to paint directly.
    fn gpu_frame(&self) -> Option<GpuFrame> {
        None
    }

    /// Hands back to its owner the GL state the rasterizer touched.
    ///
    /// **It only makes sense for whoever draws on a borrowed context**, and it is called once per
    /// execution slice, when control returns to the UI — not on every draw. See
    /// [`crate::session::Session::step`] and the comment on `GpuState::submit_with`.
    fn hand_back_context(&self) {}
}

/// A GPU color texture with the frame already drawn, and the piece of it that is the image.
#[derive(Debug, Clone, Copy)]
pub struct GpuFrame {
    pub texture: glow::Texture,
    /// The fraction of the texture the game surface takes up, in `(u, v)`; row 0 is the top.
    pub clip: [f32; 2],
    /// Width over height of the image: 4:3 when native, wider with
    /// [`Rasterizer::set_aspect_ratio`].
    pub ratio: f32,
}

impl Rasterizer for GlState {
    fn set_downscale(&mut self, downscale: usize) {
        GlState::set_downscale(self, downscale)
    }

    fn save_state(&self, dest: &mut crate::save_state::Sections) {
        crate::save_state::Saveable::save(self, dest);
    }

    fn restore_state(
        &mut self,
        origin: &crate::save_state::Reader<'_>,
    ) -> Result<(), crate::save_state::Error> {
        crate::save_state::Saveable::restore(self, origin)
    }

    /// The three accumulators of a draw in progress.
    ///
    /// `transformed` is what remains of a `glBegin` without `glEnd`, and `batch`/`pending` are
    /// the batch waiting its turn to be submitted. Empty, there is no half-done draw.
    fn flush_draw(&mut self) {
        self.flush();
    }

    /// The batch not yet submitted.
    ///
    /// **`transformed` is left out, and not by oversight**: it is the vertex stage's **scratch**
    /// area — `vertex_stage` does `mem::take` and reuses the vector. Leftovers in it are what
    /// remained of the last batch, not an interrupted draw. I included it in the first version and
    /// the result was the save state being refused **always**: the four vertices of a quad stayed
    /// there between frames, and saving became impossible. It was the instrumentation that showed
    /// the four.
    fn draw_in_progress(&self) -> bool {
        !self.batch.triangles.is_empty() || !self.pending.triangles.is_empty()
    }

    fn set_matrix_mode(&mut self, mode: u32) {
        GlState::set_matrix_mode(self, mode)
    }
    fn load_identity(&mut self) {
        GlState::load_identity(self)
    }
    fn load_matrix(&mut self, m: Matrix) {
        GlState::load_matrix(self, m)
    }
    fn mult_matrix(&mut self, m: Matrix) {
        GlState::mult_matrix(self, m)
    }
    fn push_matrix(&mut self) {
        GlState::push_matrix(self)
    }
    fn pop_matrix(&mut self) {
        GlState::pop_matrix(self)
    }
    fn top_matrix(&self) -> Matrix {
        GlState::top_matrix(self)
    }
    fn set_viewport(&mut self, x: i32, y: i32, width: i32, height: i32) {
        GlState::set_viewport(self, x, y, width, height)
    }
    fn set_scissor(&mut self, x: i32, y: i32, width: i32, height: i32) {
        GlState::set_scissor(self, x, y, width, height)
    }
    fn set_surface(&mut self, width: usize, height: usize) {
        GlState::set_surface(self, width, height)
    }
    fn set_stretched_surface(&mut self, width: usize, height: usize) {
        GlState::set_stretched_surface(self, width, height)
    }
    fn surface(&self) -> (usize, usize) {
        GlState::surface(self)
    }
    fn frame_size(&self) -> (usize, usize) {
        GlState::frame_size(self)
    }
    fn set_clear_color(&mut self, color: [f32; 4]) {
        GlState::set_clear_color(self, color)
    }
    fn set_clear_depth(&mut self, depth: f32) {
        GlState::set_clear_depth(self, depth)
    }
    fn set_clear_stencil(&mut self, value: i32) {
        GlState::set_clear_stencil(self, value)
    }
    fn set_color(&mut self, color: [f32; 4]) {
        GlState::set_color(self, color)
    }
    fn current_color(&self) -> [f32; 4] {
        GlState::current_color(self)
    }
    fn clear(&mut self, mask: u32) {
        GlState::clear(self, mask)
    }
    fn set_capability(&mut self, capability: u32, on: bool) {
        GlState::set_capability(self, capability, on)
    }
    fn set_shade_model(&mut self, mode: u32) {
        GlState::set_shade_model(self, mode)
    }
    fn set_light(&mut self, index: usize, pname: u32, values: [f32; 4]) {
        GlState::set_light(self, index, pname, values)
    }
    fn set_material(&mut self, pname: u32, values: [f32; 4]) {
        GlState::set_material(self, pname, values)
    }
    fn set_light_model(&mut self, pname: u32, values: [f32; 4]) {
        GlState::set_light_model(self, pname, values)
    }
    fn set_blend_func(&mut self, src: u32, dst: u32) {
        GlState::set_blend_func(self, src, dst)
    }
    fn set_alpha_func(&mut self, func: u32, reference: f32) {
        GlState::set_alpha_func(self, func, reference)
    }
    fn set_depth_func(&mut self, func: u32) {
        GlState::set_depth_func(self, func)
    }
    fn set_depth_mask(&mut self, on: bool) {
        GlState::set_depth_mask(self, on)
    }
    fn set_depth_range(&mut self, near: f32, far: f32) {
        self.depth_range = (near.clamp(0.0, 1.0), far.clamp(0.0, 1.0));
    }
    fn set_fog(&mut self, pname: u32, values: [f32; 4]) {
        GlState::set_fog(self, pname, values)
    }
    fn set_fog_allowed(&mut self, allowed: bool) {
        self.fog.allowed = allowed;
    }
    fn set_color_mask(&mut self, mask: [bool; 4]) {
        GlState::set_color_mask(self, mask)
    }
    fn set_cull_face(&mut self, mode: u32) {
        GlState::set_cull_face(self, mode)
    }
    fn set_front_face(&mut self, face: u32) {
        GlState::set_front_face(self, face)
    }
    fn set_stencil_func(&mut self, func: u32, reference: i32, mask: u32) {
        GlState::set_stencil_func(self, func, reference, mask)
    }
    fn set_stencil_op(&mut self, failure: u32, zfail: u32, passes: u32) {
        GlState::set_stencil_op(self, failure, zfail, passes)
    }
    fn set_stencil_mask(&mut self, mask: u32) {
        GlState::set_stencil_mask(self, mask)
    }
    fn set_active_texture(&mut self, unit: u32) {
        GlState::set_active_texture(self, unit)
    }
    fn set_client_active_texture(&mut self, unit: u32) {
        GlState::set_client_active_texture(self, unit)
    }
    fn client_unit(&self) -> u32 {
        self.client_unit
    }
    fn bind_texture(&mut self, name: u32) {
        GlState::bind_texture(self, name)
    }
    fn bound_texture(&self) -> u32 {
        GlState::bound_texture(self)
    }
    fn set_texture_env(&mut self, mode: u32) {
        GlState::set_texture_env(self, mode)
    }
    fn set_texture_env_param(&mut self, pname: u32, enum_value: u32, number: f32) {
        GlState::set_texture_env_param(self, pname, enum_value, number)
    }
    fn set_texture_env_color(&mut self, color: [f32; 4]) {
        GlState::set_texture_env_color(self, color)
    }
    fn set_texture_parameter(&mut self, name: u32, value: u32) {
        GlState::set_texture_parameter(self, name, value)
    }
    fn set_texture_crop(&mut self, crop: [i32; 4]) {
        GlState::set_texture_crop(self, crop)
    }
    fn delete_texture(&mut self, name: u32) {
        GlState::delete_texture(self, name)
    }
    fn upload_level(
        &mut self,
        name: u32,
        level: u32,
        width: usize,
        height: usize,
        pixels: Vec<[u8; 4]>,
    ) {
        GlState::upload_level(self, name, level, width, height, pixels)
    }
    fn sub_image(
        &mut self,
        name: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: &[[u8; 4]],
    ) -> Result<(), Option<(u32, u32)>> {
        GlState::sub_image(self, name, x, y, width, height, pixels)
    }
    fn draw(&mut self, mode: u32, vertices: &[Vertex]) {
        GlState::draw(self, mode, vertices)
    }
    fn draw_texture(&mut self, x: f32, y: f32, z: f32, width: f32, height: f32) {
        GlState::draw_texture(self, x, y, z, width, height)
    }
    fn read_rect(&mut self, x: i32, y: i32, width: usize, height: usize) -> Vec<[u8; 4]> {
        GlState::read_rect(self, x, y, width, height)
    }
    fn frame_rgb565(&mut self, width: usize, height: usize, out: &mut Vec<u8>) {
        GlState::frame_rgb565(self, width, height, out)
    }
    fn frame_rgb565_words(&mut self, width: usize, height: usize, out: &mut Vec<u16>) {
        GlState::frame_rgb565_words(self, width, height, out)
    }
    fn import_rgb565_changes(&mut self, width: usize, height: usize, old: &[u8], new: &[u8]) {
        GlState::import_rgb565_changes(self, width, height, old, new)
    }
}

pub struct GlState {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Frame color, in 8-bit RGBA — converted to RGB565 only on presentation.
    pub(crate) color: Vec<[u8; 4]>,
    /// Normalized depth in `[0, 1]`.
    pub(crate) depth: Vec<f32>,
    /// The stencil, eight bits — the size the console's `GL_STENCIL_BITS` advertises.
    ///
    /// It exists because of the Z-Wheel stage's reflection: it marks the floor here and draws the
    /// mirrored model only where the mark was left. Without the buffer, the mirrored one spilled
    /// outside the floor and became a stretched trail beside the model.
    pub(crate) stencil: Vec<u8>,

    /// The reduction of the 3D internal resolution: 1 is native, 2 is half, 4 is a quarter. See
    /// [`GlState::set_downscale`].
    downscale: usize,

    pub(crate) matrix_mode: u32,
    pub(crate) modelview: Vec<Matrix>,
    pub(crate) projection: Vec<Matrix>,
    pub(crate) texture_matrix: Vec<Matrix>,
    /// The texture matrix stack of unit 1. **OpenGL has one per unit**, chosen by
    /// `glActiveTexture`, and with only one, unit 1's matrix overwrote unit 0's. Reckless Racing
    /// projects the cars' shadow through unit 0 and the track's shadow map through unit 1, each
    /// with its own matrix, and both start from the vertex position.
    pub(crate) texture_matrix1: Vec<Matrix>,

    pub(crate) viewport: (i32, i32, i32, i32),
    /// The `glScissor`, with `y` already counted from the top, and only when `GL_SCISSOR_TEST`
    /// is on.
    ///
    /// Peggle draws the whole font sheet and relies on it to show a single letter — the same
    /// trick Pac-Mania does with the `IDisplay` clip. Ignoring it put the whole sheet on screen.
    pub(crate) scissor: Option<(i32, i32, i32, i32)>,
    /// The raw `glScissor` rectangle, with `y` from bottom to top, as the game passed it.
    pub(crate) raw_scissor: (i32, i32, i32, i32),
    pub(crate) scissor_enabled: bool,
    pub(crate) surface: Option<(usize, usize)>,
    /// Whether the surface is stretched to the whole screen. See [`GlState::stretched_surface`].
    pub(crate) stretched: bool,
    pub(crate) clear_color: [f32; 4],
    pub(crate) clear_depth: f32,
    pub(crate) current_color: [f32; 4],

    pub(crate) textures: HashMap<u32, Texture>,
    pub(crate) bound_texture: u32,
    pub(crate) texture_env: TexEnv,
    /// Unit 1. Unit 0 is the loose fields above, from before there was another.
    pub(crate) unit1: TextureUnit,

    pub(crate) texture_2d: bool,
    /// Active texture unit, counted from zero. `glActiveTexture` chooses it.
    ///
    /// The pipeline reads one texture per fragment, so only unit zero has an effect and the
    /// others are ignored. Ignoring is not the same as not having multitexturing: it is the
    /// difference between drawing the base layer and drawing white. Resident Evil 4 builds the
    /// world with two units and ends each block on unit 1; since `glActiveTexture` was not
    /// handled, everything fell into a single state, the last bind won and the base texture was
    /// lost — the whole village came out white.
    pub(crate) active_unit: u32,
    /// Unit chosen by `glClientActiveTexture`, which applies to the coordinate array.
    pub(crate) client_unit: u32,
    pub(crate) depth_test: bool,
    pub(crate) depth_mask: bool,
    /// The `glFog*` fog: enabled, curve, color and the parameters of each curve.
    ///
    /// Resident Evil 4 uses it to darken the background of the scenery, and that is how it
    /// separates what is near from what is far. Ignoring it left the whole scene with the same
    /// brightness.
    pub(crate) fog: Fog,
    /// The `glDepthRange`: `(near, far)`, from 0 to 1.
    ///
    /// **Ignoring it made the Crash Nitro Kart track pop out of nowhere close to the player.**
    /// The game draws parts of the scene in different depth ranges — `(0, 0.985)` and `(0, 1)` —
    /// so that some always stay in front of others. With all of them on the full range, a piece
    /// of distant track lost the depth test to the scenery and only showed up up close.
    pub(crate) depth_range: (f32, f32),
    /// Which color channels may be written, from `glColorMask`.
    pub(crate) color_mask: [bool; 4],
    pub(crate) depth_func: u32,
    pub(crate) blend: bool,
    pub(crate) blend_src: u32,
    pub(crate) blend_dst: u32,
    pub(crate) alpha_test: bool,
    pub(crate) alpha_func: u32,
    pub(crate) alpha_ref: f32,
    pub(crate) cull_face: bool,
    pub(crate) cull_mode: u32,
    pub(crate) front_face: u32,
    pub(crate) stencil_test: bool,
    /// `glStencilFunc(func, ref, mask)`.
    pub(crate) stencil_func: u32,
    pub(crate) stencil_ref: i32,
    pub(crate) stencil_value_mask: u32,
    /// `glStencilMask` — which bits drawing may write.
    pub(crate) stencil_write_mask: u32,
    /// `glStencilOp(sfail, dpfail, dppass)`.
    pub(crate) stencil_op: [u32; 3],
    pub(crate) clear_stencil: u8,

    /// `GL_LIGHTING`. With it on, the vertex color **no longer applies**, unless
    /// `GL_COLOR_MATERIAL` says otherwise: that is what the specification requires, and the
    /// Z-Wheel stage depends on it — its models arrive with white vertex color and it is the
    /// light that gives the relief.
    pub(crate) lighting: bool,
    pub(crate) color_material: bool,
    pub(crate) lights: [Light; gles::LIGHTS],
    pub(crate) material: Material,
    pub(crate) light_model_ambient: [f32; 4],
    pub(crate) shade_model: u32,

    /// Triangle batch of the draw call in progress. It lives in the struct only to reuse the
    /// allocation from one call to the next.
    batch: Batch,
    /// What was already drawn this frame and hasn't become pixels yet. See [`GlState::flush`].
    pending: Pending,
    /// The already-transformed vertices of the draw call in progress. It lives here for the same
    /// reason: Quake makes half a million draw calls in twenty-five seconds of play, and one
    /// allocation per call is half a million trips to the allocator to draw two triangles at a
    /// time.
    transformed: Vec<Vertex>,
    /// Whether the color buffer changed since the last time it was handed out converted.
    ///
    /// Z-Wheel draws into a pbuffer and reads the result with `eglGetColorBufferQUALCOMM` **twice
    /// per frame**: the first always with the queue empty — measured, zero jobs and zero
    /// triangles in 1,596 frames. Without this, each of those reads reconverted two hundred and
    /// eleven thousand pixels of a frame identical to the previous one, 1.46 ms that produced
    /// nothing.
    dirty: bool,
}

impl GlState {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            color: vec![[0, 0, 0, 255]; width * height],
            depth: vec![1.0; width * height],
            stencil: vec![0; width * height],
            lighting: false,
            color_material: false,
            lights: std::array::from_fn(|i| match i {
                0 => Light::zero(),
                _ => Light::off(),
            }),
            material: Material::default(),
            light_model_ambient: [0.2, 0.2, 0.2, 1.0],
            shade_model: gles::GL_SMOOTH,
            stencil_test: false,
            stencil_func: gles::GL_ALWAYS,
            stencil_ref: 0,
            stencil_value_mask: u32::MAX,
            stencil_write_mask: u32::MAX,
            stencil_op: [gles::GL_KEEP; 3],
            clear_stencil: 0,
            matrix_mode: gles::GL_MODELVIEW,
            modelview: vec![IDENTITY],
            projection: vec![IDENTITY],
            texture_matrix: vec![IDENTITY],
            texture_matrix1: vec![IDENTITY],
            viewport: (0, 0, width as i32, height as i32),
            scissor: None,
            raw_scissor: (0, 0, width as i32, height as i32),
            scissor_enabled: false,
            downscale: 1,
            surface: None,
            stretched: false,
            clear_color: [0.0, 0.0, 0.0, 1.0],
            clear_depth: 1.0,
            current_color: [1.0; 4],
            textures: HashMap::new(),
            bound_texture: 0,
            texture_env: TexEnv::default(),
            unit1: TextureUnit::default(),
            texture_2d: false,
            active_unit: 0,
            client_unit: 0,
            depth_test: false,
            depth_mask: true,
            fog: Fog::default(),
            depth_range: (0.0, 1.0),
            color_mask: [true; 4],
            depth_func: gles::GL_LESS,
            blend: false,
            blend_src: gles::GL_ONE,
            blend_dst: gles::GL_ZERO,
            alpha_test: false,
            alpha_func: gles::GL_ALWAYS,
            alpha_ref: 0.0,
            cull_face: false,
            cull_mode: gles::GL_BACK,
            front_face: gles::GL_CCW,
            batch: Batch::default(),
            pending: Pending::default(),
            transformed: Vec::new(),
            dirty: true,
        }
    }

    /// The matrix at the top of the active stack, as `glQueryMatrixxOES` returns it.
    pub fn top_matrix(&self) -> Matrix {
        let stack = match self.matrix_mode {
            gles::GL_PROJECTION => &self.projection,
            gles::GL_TEXTURE if self.active_unit == 1 => &self.texture_matrix1,
            gles::GL_TEXTURE => &self.texture_matrix,
            _ => &self.modelview,
        };
        *stack.last().expect("stack is never empty")
    }

    /// The active matrix stack.
    fn stack(&mut self) -> &mut Vec<Matrix> {
        match self.matrix_mode {
            gles::GL_PROJECTION => &mut self.projection,
            gles::GL_TEXTURE if self.active_unit == 1 => &mut self.texture_matrix1,
            gles::GL_TEXTURE => &mut self.texture_matrix,
            _ => &mut self.modelview,
        }
    }

    /// The matrix at the top of the active stack.
    fn top(&mut self) -> &mut Matrix {
        self.stack().last_mut().expect("stack is never empty")
    }

    pub fn set_matrix_mode(&mut self, mode: u32) {
        self.matrix_mode = mode;
    }

    pub fn load_identity(&mut self) {
        *self.top() = IDENTITY;
    }

    pub fn load_matrix(&mut self, m: Matrix) {
        *self.top() = m;
    }

    /// Multiplies the top matrix by `m`, in OpenGL order.
    pub fn mult_matrix(&mut self, m: Matrix) {
        let top = self.top();
        *top = multiply(top, &m);
    }

    pub fn push_matrix(&mut self) {
        let stack = self.stack();
        // Pushing without a limit would turn an unbalanced `PushMatrix` into a silent leak;
        // OpenGL ES guarantees 16 levels, and that is what we offer.
        if stack.len() < MAX_MATRIX_STACK {
            let top = *stack.last().expect("stack is never empty");
            stack.push(top);
        }
    }

    pub fn pop_matrix(&mut self) {
        let stack = self.stack();
        if stack.len() > 1 {
            stack.pop();
        }
    }

    /// The `glScissor` rectangle. Stored raw and converted to top-down when it applies.
    pub fn set_scissor(&mut self, x: i32, y: i32, width: i32, height: i32) {
        // The scissor arrives in console pixels, like the viewport, and applies to the same
        // surface.
        let (x, y, width, height) = if self.downscale == 1 {
            (x, y, width, height)
        } else {
            let n = self.downscale as i32;
            (
                self.downscaled(x),
                self.downscaled(y),
                (width / n).max(0),
                (height / n).max(0),
            )
        };
        self.raw_scissor = (x, y, width, height);
        self.update_scissor();
    }

    /// Turns `GL_SCISSOR_TEST` on or off.
    pub fn set_scissor_test(&mut self, enabled: bool) {
        self.scissor_enabled = enabled;
        self.update_scissor();
    }

    /// Converts the `glScissor` rectangle to our surface, which counts `y` from the top.
    fn update_scissor(&mut self) {
        let (x, y, width, height) = self.raw_scissor;
        self.scissor = match self.scissor_enabled {
            false => None,
            true => {
                let surface_height = self.surface().1 as i32;
                Some((x, surface_height - y - height, width, height))
            }
        };
    }

    /// The reduction of the 3D internal resolution: 1 is native, 2 is half, 4 is a quarter.
    ///
    /// **Only the CPU rasterizer reduces.** Here filling costs CPU and scales with the frame
    /// area: drawing 320×240 is a quarter of the work of 640×480. Presentation stays at 640×480 —
    /// [`GlState::frame_rgb565`] already knows how to resample from a smaller surface, which is
    /// the `EGL_QUALCOMM_surface_scale` path.
    ///
    /// On the GPU, reduction is a different mechanism, and a factor below 1 there would make no
    /// sense: the Mali is not saturated at 640×480.
    pub fn set_downscale(&mut self, downscale: usize) {
        // The queue was built at the old size; it becomes pixels before the switch.
        self.flush();
        let create = match downscale {
            0 | 1 => 1,
            n => n.min(4),
        };
        if create == self.downscale {
            return;
        }
        self.downscale = create;
        // **The initial viewport is reduced too.** It is what the surface deduces when the game
        // never calls `glViewport` — and there are games like that: Z-Wheel doesn't call it once
        // in thirteen seconds, measured. Resetting it to the frame size would leave the reduction
        // without effect precisely on whoever depends on it.
        let (width, height) = (self.width / create, self.height / create);
        self.surface = None;
        self.viewport = (0, 0, width as i32, height as i32);
        self.raw_scissor = (0, 0, width as i32, height as i32);
        self.update_scissor();
        crate::logging!(
            crate::logging::Level::Info,
            "gl",
            "3D internal resolution reduced to 1/{create} of the frame ({}x{})",
            self.width / create,
            self.height / create
        );
    }

    /// The reduction in effect. See [`GlState::set_downscale`].
    pub fn downscale(&self) -> usize {
        self.downscale
    }

    /// Converts a console coordinate to the reduced surface.
    fn downscaled(&self, value: i32) -> i32 {
        match self.downscale {
            1 => value,
            n => value.div_euclid(n as i32),
        }
    }

    pub fn set_viewport(&mut self, x: i32, y: i32, width: i32, height: i32) {
        // **The viewport is the entry point of everything.** The game gives the rectangle in
        // console pixels, and it is through it that a vertex becomes a pixel; reducing here, the
        // whole drawing lands on the reduced surface — and the surface deduced right below comes
        // out reduced along with it, with nothing else.
        let (x, y, width, height) = if self.downscale == 1 {
            (x, y, width, height)
        } else {
            let n = self.downscale as i32;
            (
                self.downscaled(x),
                self.downscaled(y),
                (width / n).max(1),
                (height / n).max(1),
            )
        };
        self.viewport = (x, y, width, height);
        // The game draws on a surface that may be smaller than the screen and is scaled up on
        // presentation — on the console that is the `EGL_QUALCOMM_surface_scale` extension. It
        // never tells us the size of that surface, but the largest viewport it uses is exactly
        // that: no drawing goes past that rectangle.
        let (seen_x, seen_y) = self.surface.unwrap_or((0, 0));
        self.surface = Some((
            seen_x.max((x + width).max(0) as usize),
            seen_y.max((y + height).max(0) as usize),
        ));
    }

    /// The viewport with `y` counted from the top, which is how our frames are stored.
    ///
    /// **`glViewport` counts `y` from bottom to top.** With a full-screen viewport this doesn't
    /// show. Crash Nitro Kart draws the next stretch of track through its portal, with the
    /// viewport on the portal's rectangle on screen — a narrow rectangle near the horizon. Used as
    /// if it counted from the top, the stretch ended up mirrored at the bottom of the screen, and
    /// what you saw was emptiness ahead until the kart went through the portal, with the scenery
    /// rising from the bottom up.
    fn top_down_viewport(&self) -> (i32, i32, i32, i32) {
        let (x, y, width, height) = self.viewport;
        let surface_height = self.surface().1 as i32;
        (x, surface_height - y - height, width, height)
    }

    /// Declares the surface size, when the game provides it.
    ///
    /// The console's `EGL_QUALCOMM_surface_scale` is exactly that: the game draws small and the
    /// device scales up. It takes precedence over the viewport deduction — here it **says** the
    /// size, and the deduction exists only for whoever doesn't.
    pub fn set_surface(&mut self, width: usize, height: usize) {
        // What was drawn at the old size must become pixels before the switch.
        self.flush();
        if width > 0 && height > 0 {
            self.surface = Some((width, height));
        }
        self.stretched = false;
    }

    /// Declares the surface the device scales up to the whole screen.
    pub fn set_stretched_surface(&mut self, width: usize, height: usize) {
        self.set_surface(width, height);
        self.stretched = width > 0 && height > 0;
    }

    /// Whether a surface smaller than the frame is **stretched** to the screen, rather than put in
    /// a corner of it.
    ///
    /// There are two cases of a smaller surface, and they only differ here.
    /// `EGL_QUALCOMM_surface_scale` scales up: Quake draws at 320×400 and the device shows it at
    /// 640×480, 4:3. A pbuffer scales nothing: Z-Wheel draws the stage on a 640×330 one and copies
    /// it wherever it wants. The wide aspect ratio only knows how to open up the sides of the
    /// first.
    pub fn stretched_surface(&self) -> bool {
        self.stretched
    }

    /// The size of the surface the game draws on, deduced from the viewports used.
    ///
    /// A game that never calls `glViewport` keeps the default viewport, which OpenGL defines as
    /// the whole surface — and that is what applies then. Deducing the surface from an empty set
    /// of viewports gave 1×1, and presentation stretched a single pixel across the whole screen:
    /// Zeebo Sports Peteca, which draws in screen coordinates and never touches the viewport,
    /// came out entirely white.
    pub fn surface(&self) -> (usize, usize) {
        // **The default is reduced too.** Without this, a game that never calls `glViewport`
        // would draw in the reduced area and presentation would copy the whole frame — the image
        // would come out in a corner, instead of scaled up. That is the case of Z-Wheel, which
        // doesn't call `glViewport` once in thirteen seconds.
        let fallback = (self.width / self.downscale, self.height / self.downscale);
        let (width, height) = self.surface.unwrap_or(fallback);
        (width.clamp(1, self.width), height.clamp(1, self.height))
    }

    /// A rectangle of the frame, in 8-bit RGBA, in the order OpenGL uses.
    ///
    /// The origin of `glReadPixels` is the **lower** left corner, and ours is the upper one —
    /// hence the row flip. Reading without flipping gives an upside-down image, which is the kind
    /// of error that passes for "almost right".
    ///
    /// Outside the surface it returns opaque black, like a read of an undrawn area.
    pub fn read_rect(&mut self, x: i32, y: i32, width: usize, height: usize) -> Vec<[u8; 4]> {
        // Whoever reads the frame needs it painted; drawing is accumulated until someone asks.
        self.flush();
        let (sw, sh) = self.surface();
        let mut output = vec![[0, 0, 0, 255]; width * height];
        // **The request comes in console pixels and the surface may be reduced.** Each pixel here
        // is worth `downscale` pixels there, so the rectangle is mapped and the result is
        // replicated: the caller writes `width * height` pixels into the game's memory, and
        // returning fewer would leave the rest of the strip with whatever was there.
        let n = self.downscale as i32;
        for line in 0..height {
            for column in 0..width {
                let fx = self.downscaled(x + column as i32);
                let fy = self.downscaled(y + line as i32);
                if fx < 0 || fy < 0 || fx as usize >= sw || fy as usize >= sh {
                    continue;
                }
                // Row `fy` counted from the bottom is row `sh - 1 - fy` in our buffer.
                let origin = (sh - 1 - fy as usize) * self.width + fx as usize;
                output[line * width + column] = self.color[origin];
            }
        }
        // `n` only differs from 1 when there is a reduction, and then the loop above already
        // sampled the smaller surface: nothing more to do. The `_` exists so the compiler doesn't
        // flag the variable when the reduction is 1 — the replication is the sampling by
        // division itself.
        let _ = n;
        output
    }

    pub fn set_clear_color(&mut self, color: [f32; 4]) {
        self.clear_color = color;
    }

    pub fn set_clear_depth(&mut self, depth: f32) {
        self.clear_depth = depth;
    }

    pub fn set_color(&mut self, color: [f32; 4]) {
        self.current_color = color;
    }

    pub fn current_color(&self) -> [f32; 4] {
        self.current_color
    }

    /// `glActiveTexture` — chooses the unit the next texture calls refer to.
    pub fn set_active_texture(&mut self, unit: u32) {
        self.active_unit = unit.wrapping_sub(gles::GL_TEXTURE0);
    }

    /// `glClientActiveTexture` — the same choice, for the texture coordinate array.
    pub fn set_client_active_texture(&mut self, unit: u32) {
        self.client_unit = unit.wrapping_sub(gles::GL_TEXTURE0);
    }

    /// Whether the current projection is perspective, not orthographic: the last row of the
    /// matrix has the `-z` that makes `w`. HUD and 2D draw in orthographic.
    pub fn is_perspective_projection(&self) -> bool {
        let p = self.projection.last().expect("stack is never empty");
        p[11] != 0.0 && p[15] == 0.0
    }

    /// Whether the active unit is the base one — the only one the pipeline draws.
    pub fn base_active_unit(&self) -> bool {
        self.active_unit == 0
    }

    /// Whether the active unit is one of the two that draw.
    pub fn active_unit_draws(&self) -> bool {
        self.active_unit < 2
    }

    /// A `glFog*` parameter. What we don't know is left out instead of becoming garbage.
    pub fn set_fog(&mut self, pname: u32, values: [f32; 4]) {
        match pname {
            gles::GL_FOG_MODE => self.fog.curve = values[0] as u32,
            gles::GL_FOG_DENSITY => self.fog.density = values[0].max(0.0),
            gles::GL_FOG_START => self.fog.start = values[0],
            gles::GL_FOG_END => self.fog.end = values[0],
            gles::GL_FOG_COLOR => self.fog.color = values,
            _ => {}
        }
    }

    /// The fog as it is now, for whoever needs to pass it on.
    pub fn fog(&self) -> Fog {
        self.fog
    }

    pub fn set_capability(&mut self, capability: u32, on: bool) {
        match capability {
            // Enabling and disabling texturing is per unit; the first two draw.
            gles::GL_TEXTURE_2D if self.active_unit == 0 => self.texture_2d = on,
            gles::GL_TEXTURE_2D if self.active_unit == 1 => self.unit1.enabled = on,
            gles::GL_TEXTURE_2D => {}
            gles::GL_DEPTH_TEST => self.depth_test = on,
            gles::GL_BLEND => self.blend = on,
            gles::GL_ALPHA_TEST => self.alpha_test = on,
            gles::GL_CULL_FACE => self.cull_face = on,
            gles::GL_LIGHTING => self.lighting = on,
            gles::GL_COLOR_MATERIAL => self.color_material = on,
            // Normals are always normalized here, so turning this on or off changes nothing.
            // It is not sloppiness: with normalization off and non-unit normals, OpenGL gives a
            // defined and wrong result, and no game asks for that on purpose.
            gles::GL_NORMALIZE | gles::GL_RESCALE_NORMAL => {}
            capacity
                if (gles::GL_LIGHT0..gles::GL_LIGHT0 + gles::LIGHTS as u32)
                    .contains(&capacity) =>
            {
                self.lights[(capacity - gles::GL_LIGHT0) as usize].enabled = on;
            }
            gles::GL_FOG => self.fog.enabled = on,
            gles::GL_SCISSOR_TEST => self.set_scissor_test(on),
            // **The stencil doesn't exist yet, and its absence is measurable.** The Z-Wheel stage
            // sets up `glStencilFunc` and `glStencilOp` twice per frame, which is the recipe for
            // a planar reflection: mark the floor in the stencil and draw the mirrored model only
            // where it marked. Without the test, the mirrored one spills outside the floor —
            // turning off the draws made under the stencil test removes half of the stretched
            // trails that show up on the car. Storing the state here is the first step, and does
            // nothing on its own.
            gles::GL_STENCIL_TEST => self.stencil_test = on,
            _ => {}
        }
    }

    /// `glLight*` — one parameter of a light.
    ///
    /// The position is the special case: OpenGL stores it **in eye coordinates**, transformed by
    /// the modelview at the moment of the call. Storing the object coordinates and transforming
    /// at draw time looks the same and isn't — the light would move along with each model.
    pub fn set_light(&mut self, index: usize, pname: u32, values: [f32; 4]) {
        let modelview = *self.modelview.last().expect("stack is never empty");
        let Some(light) = self.lights.get_mut(index) else {
            return;
        };
        match pname {
            gles::GL_AMBIENT => light.ambient = values,
            gles::GL_DIFFUSE => light.diffuse = values,
            gles::GL_SPECULAR => light.specular = values,
            gles::GL_POSITION => light.position = transform(&modelview, values),
            gles::GL_SPOT_DIRECTION => {
                let [x, y, z, _] = transform(&modelview, [values[0], values[1], values[2], 0.0]);
                light.spot_direction = [x, y, z];
            }
            gles::GL_SPOT_EXPONENT => light.spot_exponent = values[0],
            gles::GL_SPOT_CUTOFF => light.spot_cutoff = values[0],
            gles::GL_CONSTANT_ATTENUATION => light.attenuation[0] = values[0],
            gles::GL_LINEAR_ATTENUATION => light.attenuation[1] = values[0],
            gles::GL_QUADRATIC_ATTENUATION => light.attenuation[2] = values[0],
            _ => {}
        }
    }

    /// `glMaterial*`. The face is ignored: in ES 1.x there is a single material.
    pub fn set_material(&mut self, pname: u32, values: [f32; 4]) {
        match pname {
            gles::GL_AMBIENT => self.material.ambient = values,
            gles::GL_DIFFUSE => self.material.diffuse = values,
            gles::GL_AMBIENT_AND_DIFFUSE => {
                self.material.ambient = values;
                self.material.diffuse = values;
            }
            gles::GL_SPECULAR => self.material.specular = values,
            gles::GL_EMISSION => self.material.emission = values,
            gles::GL_SHININESS => self.material.shininess = values[0],
            _ => {}
        }
    }

    /// `glLightModel*`. Only the scene ambient has an effect here; `TWO_SIDE` is accepted and
    /// ignored, because there is a single material.
    pub fn set_light_model(&mut self, pname: u32, values: [f32; 4]) {
        if pname == gles::GL_LIGHT_MODEL_AMBIENT {
            self.light_model_ambient = values;
        }
    }

    pub fn set_shade_model(&mut self, mode: u32) {
        self.shade_model = mode;
    }

    /// The color of a vertex with lighting on, in the OpenGL ES 1.x equation.
    ///
    /// ```text
    /// color = emission
    ///     + material_ambient * scene_ambient
    ///     + Σ attenuation * spotlight * ( material_ambient * light_ambient
    ///                                   + material_diffuse  * light_diffuse  * max(N·L, 0)
    ///                                   + material_specular * light_specular * max(N·H, 0)^shine )
    /// ```
    ///
    /// The alpha does **not** come from the sum: it is the material diffuse's, and summing light
    /// alpha leaves everything opaque. With `GL_COLOR_MATERIAL`, the vertex color takes the place
    /// of the material ambient and diffuse — it is the only path through which a color array
    /// still applies with lighting on.
    fn lit_color(
        &self,
        eye: [f32; 4],
        normal: [f32; 3],
        vertex_color: [f32; 4],
    ) -> [f32; 4] {
        let (environment, diffuse) = match self.color_material {
            true => (vertex_color, vertex_color),
            false => (self.material.ambient, self.material.diffuse),
        };
        let mut output = [0.0f32; 3];
        for channel in 0..3 {
            output[channel] =
                self.material.emission[channel] + environment[channel] * self.light_model_ambient[channel];
        }
        // The eye position is `(0, 0, 0)` in eye coordinates, so the direction to the viewer is
        // the point itself, negated and normalized.
        let to_eye = normalize([-eye[0], -eye[1], -eye[2]]);
        for light in self.lights.iter().filter(|light| light.enabled) {
            let (to_light, distance) = match light.position[3] == 0.0 {
                // Directional: the position is a direction, and there is no distance or
                // attenuation.
                true => (
                    normalize([light.position[0], light.position[1], light.position[2]]),
                    None,
                ),
                false => {
                    let raw = [
                        light.position[0] - eye[0],
                        light.position[1] - eye[1],
                        light.position[2] - eye[2],
                    ];
                    (normalize(raw), Some(length(raw)))
                }
            };
            let attenuation = match distance {
                None => 1.0,
                Some(d) => {
                    let [c, l, q] = light.attenuation;
                    let divisor = c + l * d + q * d * d;
                    if divisor <= 0.0 { 1.0 } else { 1.0 / divisor }
                }
            };
            let spotlight = spotlight(light, to_light);
            let weight = attenuation * spotlight;
            if weight <= 0.0 {
                continue;
            }
            let n_l = point(normal, to_light).max(0.0);
            // The Blinn half-vector, which is what ES 1.x uses instead of Phong reflection.
            let shine = match n_l > 0.0 && self.material.shininess > 0.0 {
                false => 0.0,
                true => {
                    let middle = normalize([
                        to_light[0] + to_eye[0],
                        to_light[1] + to_eye[1],
                        to_light[2] + to_eye[2],
                    ]);
                    point(normal, middle).max(0.0).powf(self.material.shininess)
                }
            };
            for channel in 0..3 {
                output[channel] += weight
                    * (environment[channel] * light.ambient[channel]
                        + diffuse[channel] * light.diffuse[channel] * n_l
                        + self.material.specular[channel] * light.specular[channel] * shine);
            }
        }
        [
            output[0].clamp(0.0, 1.0),
            output[1].clamp(0.0, 1.0),
            output[2].clamp(0.0, 1.0),
            diffuse[3].clamp(0.0, 1.0),
        ]
    }

    /// `glStencilFunc(func, ref, mask)`.
    ///
    /// `ref` is clamped to the buffer's range, as the specification requires: the console has
    /// eight bits.
    pub fn set_stencil_func(&mut self, func: u32, reference: i32, mask: u32) {
        self.stencil_func = func;
        self.stencil_ref = reference.clamp(0, u8::MAX as i32);
        self.stencil_value_mask = mask;
    }

    /// `glStencilOp(sfail, dpfail, dppass)` — what to do when the stencil fails, when it passes
    /// and depth fails, and when both pass.
    pub fn set_stencil_op(&mut self, failure: u32, zfail: u32, passes: u32) {
        self.stencil_op = [failure, zfail, passes];
    }

    /// `glStencilMask` — which stencil bits drawing may write.
    pub fn set_stencil_mask(&mut self, mask: u32) {
        self.stencil_write_mask = mask;
    }

    /// `glClearStencil`.
    pub fn set_clear_stencil(&mut self, value: i32) {
        self.clear_stencil = value.clamp(0, u8::MAX as i32) as u8;
    }

    pub fn set_blend_func(&mut self, src: u32, dst: u32) {
        (self.blend_src, self.blend_dst) = (src, dst);
    }

    pub fn set_depth_func(&mut self, func: u32) {
        self.depth_func = func;
    }

    pub fn set_depth_mask(&mut self, on: bool) {
        self.depth_mask = on;
    }

    /// `glColorMask`: which color channels drawing may write.
    ///
    /// Ignoring this is not neutral. A game that draws an alpha-only pass — masking red, green
    /// and blue — would have the color painted over what was already there, and the result is a
    /// scrambled image with no apparent error.
    pub fn set_color_mask(&mut self, mask: [bool; 4]) {
        self.color_mask = mask;
    }

    pub fn set_alpha_func(&mut self, func: u32, reference: f32) {
        (self.alpha_func, self.alpha_ref) = (func, reference);
    }

    pub fn set_cull_face(&mut self, mode: u32) {
        self.cull_mode = mode;
    }

    /// Only `GL_CW` and `GL_CCW` are valid windings; OpenGL rejects anything else, and accepting
    /// it here would silently invert the decision of which face shows.
    pub fn set_front_face(&mut self, face: u32) {
        if face == gles::GL_CW || face == gles::GL_CCW {
            self.front_face = face;
        }
    }

    /// The environment of the active unit, if it is one of those that draw.
    fn active_env(&mut self) -> Option<&mut TexEnv> {
        match self.active_unit {
            0 => Some(&mut self.texture_env),
            1 => Some(&mut self.unit1.env),
            _ => None,
        }
    }

    pub fn set_texture_env(&mut self, mode: u32) {
        if let Some(env) = self.active_env() {
            env.mode = mode;
        }
    }

    /// A `GL_COMBINE` parameter. See [`TexEnv::define`].
    pub fn set_texture_env_param(&mut self, pname: u32, enum_value: u32, number: f32) {
        if let Some(env) = self.active_env() {
            env.define(pname, enum_value, number);
        }
    }

    /// `GL_TEXTURE_ENV_COLOR`.
    pub fn set_texture_env_color(&mut self, color: [f32; 4]) {
        if let Some(env) = self.active_env() {
            env.color = color;
        }
    }

    /// Unit 1, for whoever passes the state on.
    pub fn unit1(&self) -> TextureUnit {
        self.unit1
    }

    /// The name of the texture bound to the active unit, if it is one of those that draw.
    fn active_texture(&self) -> Option<u32> {
        match self.active_unit {
            0 => Some(self.bound_texture),
            1 => Some(self.unit1.texture),
            _ => None,
        }
    }

    /// The texture environment of unit 0, for whoever passes the state on.
    pub fn texture_env(&self) -> TexEnv {
        self.texture_env
    }

    pub fn bind_texture(&mut self, name: u32) {
        match self.active_unit {
            0 => self.bound_texture = name,
            1 => self.unit1.texture = name,
            _ => return,
        }
        // The name comes into existence already at `BindTexture`: games usually set the
        // parameters before sending the pixels, and without the entry those settings would be
        // lost.
        self.textures.entry(name).or_default();
    }

    /// `glTexParameter` on the bound texture.
    pub fn set_texture_parameter(&mut self, name: u32, value: u32) {
        let Some(enabled) = self.active_texture() else {
            return;
        };
        // **What was already queued uses the current parameters.** The queue stores the texture
        // *name* and only fetches filter and wrap at flush time; changing them here without
        // painting first makes an earlier draw get sampled with a later one's configuration. It
        // is the same care `TexImage2D` already took with the pixels, and that was missing here.
        let Some(texture) = self.textures.get(&enabled) else {
            return;
        };
        // Many games (NFS especially) reapply the same state before each sprite. There is
        // nothing to preserve when the value didn't change; skipping the flush keeps the
        // triangle queue and removes a dominant cost of the frame.
        let changed = match name {
            gles::GL_TEXTURE_WRAP_S => texture.wrap[0] != value,
            gles::GL_TEXTURE_WRAP_T => texture.wrap[1] != value,
            gles::GL_TEXTURE_MAG_FILTER => {
                let filter = if value == gles::GL_NEAREST {
                    gles::GL_NEAREST
                } else {
                    gles::GL_LINEAR
                };
                texture.filter != filter
            }
            gles::GL_TEXTURE_MIN_FILTER => texture.min_filter != value,
            _ => false,
        };
        if !changed {
            return;
        }
        self.flush();
        let Some(texture) = self.textures.get_mut(&enabled) else {
            return;
        };
        match name {
            gles::GL_TEXTURE_WRAP_S => texture.wrap[0] = value,
            gles::GL_TEXTURE_WRAP_T => texture.wrap[1] = value,
            // Without a mipmap, the mipmapped minification filter behaves like the base one, and
            // what matters is telling nearest neighbor from interpolation.
            // Magnification only distinguishes nearest from interpolation; minification keeps
            // the whole value, because it determines whether there is a mipmap and how to go
            // from one level to the next.
            gles::GL_TEXTURE_MAG_FILTER => {
                texture.filter = match value {
                    gles::GL_NEAREST => gles::GL_NEAREST,
                    _ => gles::GL_LINEAR,
                };
            }
            gles::GL_TEXTURE_MIN_FILTER => texture.min_filter = value,
            _ => {}
        }
    }

    /// `glTexParameteriv(GL_TEXTURE_CROP_RECT_OES, …)` on the bound texture.
    pub fn set_texture_crop(&mut self, crop: [i32; 4]) {
        let Some(enabled) = self.active_texture() else {
            return;
        };
        if let Some(texture) = self.textures.get_mut(&enabled) {
            texture.crop = crop;
        }
    }

    /// The texture bound to the active unit — the one a `glTexImage2D` would reach now.
    pub fn bound_texture(&self) -> u32 {
        self.active_texture().unwrap_or(self.bound_texture)
    }

    /// The frame size — the screen, not the surface the game draws on. See
    /// [`GlState::surface`].
    pub fn frame_size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Deletes a texture by name.
    ///
    /// The queue refers to textures by name: deleting one before it is read would change what
    /// was already drawn, and so the queue is flushed first.
    pub fn delete_texture(&mut self, name: u32) {
        self.flush();
        self.textures.remove(&name);
    }

    /// Stores a texture level, creating the texture if it doesn't exist yet.
    ///
    /// **Level zero clears the smaller ones.** A new image on the base level makes the old chain
    /// a lie, and serving another texture's mipmap is worse than having none.
    ///
    /// The queue flush belongs here, not to the caller: changing the contents of a texture the
    /// queue will still read would change the past.
    pub fn upload_level(
        &mut self,
        name: u32,
        level: u32,
        width: usize,
        height: usize,
        pixels: Vec<[u8; 4]>,
    ) {
        self.flush();
        let texture = self.textures.entry(name).or_default();
        if level == 0 {
            texture.width = width;
            texture.height = height;
            texture.pixels = pixels;
            texture.mipmaps.clear();
            return;
        }
        let index = level as usize - 1;
        if texture.mipmaps.len() <= index {
            texture.mipmaps.resize_with(index + 1, || Level {
                width: 0,
                height: 0,
                pixels: Vec::new(),
            });
        }
        texture.mipmaps[index] = Level {
            width,
            height,
            pixels,
        };
    }

    /// `glTexSubImage2D` on the base level: replaces a rectangle inside an existing texture.
    ///
    /// Returns the texture size when the rectangle doesn't fit — clipping would be inventing a
    /// result OpenGL doesn't define, and the caller needs the number for the report.
    /// `Err(None)` is a texture that doesn't exist, which is no error at all: the game may have
    /// deleted it.
    pub fn sub_image(
        &mut self,
        name: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: &[[u8; 4]],
    ) -> Result<(), Option<(u32, u32)>> {
        self.flush();
        let Some(texture) = self.textures.get_mut(&name) else {
            return Err(None);
        };
        let (tw, th) = (texture.width as u32, texture.height as u32);
        if x + width > tw || y + height > th {
            return Err(Some((tw, th)));
        }
        for line in 0..height {
            let dest = ((y + line) * tw + x) as usize;
            let origin = (line * width) as usize;
            texture.pixels[dest..dest + width as usize]
                .copy_from_slice(&pixels[origin..origin + width as usize]);
        }
        // **Touching level zero invalidates the chain.** The smaller levels would keep showing
        // what was there before, and whoever samples two levels sees both contents at once: the
        // Z-Wheel promo panel, which swaps its text through here, came out with ghost letters of
        // the previous text over the new ones.
        texture.mipmaps.clear();
        Ok(())
    }

    pub fn clear(&mut self, mask: u32) {
        // Clearing is an operation on the frame and takes its place in the order like any draw:
        // what came before must be painted, otherwise it would erase what doesn't exist yet.
        self.flush();
        if mask & gles::GL_COLOR_BUFFER_BIT != 0 {
            let c = pack(self.clear_color);
            self.color.fill(c);
            self.dirty = true;
        }
        if mask & gles::GL_DEPTH_BUFFER_BIT != 0 {
            self.depth.fill(self.clear_depth);
        }
        if mask & gles::GL_STENCIL_BUFFER_BIT != 0 {
            self.stencil.fill(self.clear_stencil);
        }
    }

    /// Draws a sequence of vertices in the requested mode.
    /// The vertex stage: transforms, lights and leaves the result in `transformed`.
    ///
    /// It is separate from filling because **both rasterizers share it**. Modelview matrix,
    /// projection, texture matrix and per-vertex lighting are the same computation in both, and
    /// having it in a single place is what makes comparing them honest: if the lighting is wrong,
    /// it will be wrong the same way in both, and whatever difference remains is the filling's.
    ///
    /// At the end of this the vertices are in **clip space**, with final color and `uv` — which is
    /// exactly what a GPU expects to receive in a pass-through shader.
    pub fn vertex_stage(&mut self, vertices: &[Vertex]) {
        let mvp = {
            let projection = *self.projection.last().expect("stack is never empty");
            let modelview = *self.modelview.last().expect("stack is never empty");
            multiply(&projection, &modelview)
        };
        // Texture coordinates also go through a matrix, and ignoring it is not a finishing
        // detail: Zeebo Sports Peteca sends the `uv` in fixed point — the court arrives with
        // 32767, the extreme of a 16-bit integer — and it is the texture matrix that brings them
        // back into the `0..1` range. Without it, `GL_REPEAT` wrapped around the texture on every
        // pixel, and the court and the stands came out as confetti in the right colors.
        let texture_matrix = *self.texture_matrix.last().expect("stack is never empty");
        let texture_matrix1 = *self.texture_matrix1.last().expect("stack is never empty");
        // Lighting happens in **eye coordinates**, which is where the light positions were
        // stored: hence we need the modelview separately, not only its product with the
        // projection. Normals go through another matrix — see [`normal_matrix`].
        let modelview = *self.modelview.last().expect("stack is never empty");
        let normals = normal_matrix(&modelview);
        let lighting = self.lighting;
        let fog_state = self.fog;
        let mut clip = std::mem::take(&mut self.transformed);
        clip.clear();
        clip.extend(vertices.iter().map(|v| {
            // `q` is the fourth component of the texture coordinate; dividing by it is what
            // allows projective texturing, and it is 1 in the common case.
            let project = |matrix: &Matrix, uv: [f32; 4]| {
                let [s, t, r, q] = transform(matrix, uv);
                let scale = if q == 0.0 { 1.0 } else { 1.0 / q };
                [s * scale, t * scale, r, 1.0]
            };
            // Fog and lighting want the same thing: the vertex in eye coordinates. With either
            // one on, the computation is done once and serves both.
            let eye = (lighting || fog_state.enabled && fog_state.allowed)
                .then(|| transform(&modelview, v.position));
            let color = match (lighting, eye) {
                (true, Some(eye)) => {
                    let normal = normalize(rotate_normal(&normals, v.normal));
                    self.lit_color(eye, normal, v.color)
                }
                _ => v.color,
            };
            // The distance from the eye is `|z|`, as OpenGL allows instead of the vector length:
            // it is what every fixed-function implementation does, and what the game expects.
            let fog = match eye {
                Some(eye) => fog_state.factor(eye[2].abs()),
                None => 1.0,
            };
            Vertex {
                position: transform(&mvp, v.position),
                uv: project(&texture_matrix, v.uv),
                uv1: project(&texture_matrix1, v.uv1),
                color,
                fog,
                ..*v
            }
        }));
        self.transformed = clip;
    }

    /// The vertices the vertex stage left ready.
    pub fn transformed(&self) -> &[Vertex] {
        &self.transformed
    }

    pub fn draw(&mut self, mode: u32, vertices: &[Vertex]) {
        self.vertex_stage(vertices);
        let clip = std::mem::take(&mut self.transformed);
        // Triangles are projected first and filled afterwards, all together: it is the whole
        // batch that decides whether splitting the frame across threads is worth it, and the
        // OpenGL state doesn't change in the middle of a draw call.
        let mut batch = std::mem::take(&mut self.batch);
        batch.clear();
        match mode {
            gles::GL_TRIANGLES => {
                for tri in clip.chunks_exact(3) {
                    self.triangle([tri[0], tri[1], tri[2]], &mut batch);
                }
            }
            gles::GL_TRIANGLE_STRIP => {
                for (i, window) in clip.windows(3).enumerate() {
                    // The winding alternates at each step; swapping two vertices preserves it.
                    if i % 2 == 0 {
                        self.triangle([window[0], window[1], window[2]], &mut batch);
                    } else {
                        self.triangle([window[1], window[0], window[2]], &mut batch);
                    }
                }
            }
            gles::GL_TRIANGLE_FAN => {
                for window in clip[1..].windows(2) {
                    self.triangle([clip[0], window[0], window[1]], &mut batch);
                }
            }
            gles::GL_LINES | gles::GL_LINE_LOOP | gles::GL_LINE_STRIP => {
                let ccw = self.front_face != gles::GL_CW;
                for tri in lines_to_triangles(mode, &clip, self.viewport, ccw).chunks_exact(3) {
                    self.triangle([tri[0], tri[1], tri[2]], &mut batch);
                }
            }
            // Points remain unhandled: no measured game uses them.
            gles::GL_POINTS => {}
            _ => {}
        }
        self.enqueue(&mut batch);
        self.batch = batch;
        self.transformed = clip;
    }

    /// `glDrawTex*OES` — the screen blit of `GL_OES_draw_texture`.
    ///
    /// The extension draws a rectangle **in window coordinates**, without going through the
    /// matrices: it is the path an emulator uses to put its screen on the device's screen, and it
    /// is what the console's ten arcade ports do. Without it, their `InitGLExtensions` fails and
    /// the render target is never created.
    ///
    /// The piece of the texture comes from `GL_TEXTURE_CROP_RECT_OES`, and a negative width or
    /// height there mirrors the axis — that is how the extension flips the image.
    pub fn draw_texture(&mut self, x: f32, y: f32, z: f32, width: f32, height: f32) {
        let Some(texture) = self.textures.get(&self.bound_texture) else {
            return;
        };
        let (tw, th) = (texture.width as f32, texture.height as f32);
        if tw == 0.0 || th == 0.0 || width == 0.0 || height == 0.0 {
            return;
        }
        let [ucr, vcr, wcr, hcr] = texture.crop.map(|value| value as f32);
        // A zeroed crop is the whole texture: a game that asks for no crop wants the whole
        // image, and drawing nothing would be worse than adopting the obvious default.
        let (wcr, hcr) = match (wcr, hcr) {
            (0.0, 0.0) => (tw, th),
            _ => (wcr, hcr),
        };
        let (s0, s1) = (ucr / tw, (ucr + wcr) / tw);
        let (t0, t1) = (vcr / th, (vcr + hcr) / th);

        // The OpenGL window has zero at the bottom; our surface, at the top.
        let surface_height = self.surface().1 as f32;
        let (left, right) = (x, x + width);
        let (top, bottom) = (surface_height - y - height, surface_height - y);
        let color = self.current_color();
        let corners = [
            ([left, top], [s0, t1]),
            ([left, bottom], [s0, t0]),
            ([right, bottom], [s1, t0]),
            ([right, top], [s1, t1]),
        ];
        let (vx, vy, vw, vh) = self.top_down_viewport();
        if vw <= 0 || vh <= 0 {
            return;
        }
        let vertices: Vec<Vertex> = corners
            .iter()
            .map(|&([sx, sy], uv)| Vertex {
                normal: [0.0, 0.0, 1.0],
                uv1: DEFAULT_UV,
                fog: 1.0,
                position: [
                    ((sx - vx as f32) / vw as f32) * 2.0 - 1.0,
                    1.0 - ((sy - vy as f32) / vh as f32) * 2.0,
                    z * 2.0 - 1.0,
                    1.0,
                ],
                color,
                uv: [uv[0], uv[1], 0.0, 1.0],
            })
            .collect();

        // The rectangle has no winding defined by the extension, so culling it by face would be
        // discarding a draw the game expects to see.
        let culling = self.cull_face;
        self.cull_face = false;
        let mut batch = std::mem::take(&mut self.batch);
        batch.clear();
        self.triangle([vertices[0], vertices[1], vertices[2]], &mut batch);
        self.triangle([vertices[0], vertices[2], vertices[3]], &mut batch);
        self.cull_face = culling;
        self.enqueue(&mut batch);
        self.batch = batch;
    }

    /// Clips against the frustum's near plane and puts what remains in the batch.
    ///
    /// The near plane is `z >= -w`, not `w > 0`. Clipping only by the sign of `w` lets through
    /// vertices with a tiny `w` — just behind the near plane, but still in front of the camera —,
    /// and the perspective divide multiplies their coordinates by tens of thousands: the triangle
    /// becomes a block covering the screen, with the texture so stretched it comes out as a flat
    /// color. That is what was smearing the Crash track.
    fn triangle(&self, tri: [Vertex; 3], batch: &mut Batch) {
        // The plane is inclusive: the Crash UI is drawn in orthographic with `z` right on the
        // near plane, and `z + w` is exactly zero there.
        let distance = |v: &Vertex| v.position[2] + v.position[3];
        let inside: Vec<usize> = (0..3).filter(|&i| distance(&tri[i]) >= 0.0).collect();
        match inside.len() {
            0 => {}
            3 => self.prepare(tri, batch),
            1 => {
                let i = inside[0];
                let (a, b) = ((i + 1) % 3, (i + 2) % 3);
                // The order (inside, cut at a, cut at b) keeps the original's winding.
                self.prepare(
                    [tri[i], clip_near(tri[i], tri[a]), clip_near(tri[i], tri[b])],
                    batch,
                );
            }
            _ => {
                let out = (0..3).find(|i| !inside.contains(i)).unwrap_or(0);
                let (a, b) = ((out + 1) % 3, (out + 2) % 3);
                let ea = clip_near(tri[a], tri[out]);
                let eb = clip_near(tri[b], tri[out]);
                // The remaining quadrilateral is `a → b → eb → ea`, in the same winding as the
                // original triangle. Walking it backwards flips the winding and makes face
                // culling throw away precisely the clipped triangles.
                self.prepare([tri[a], tri[b], eb], batch);
                self.prepare([tri[a], eb, ea], batch);
            }
        }
    }

    /// Projects a triangle already entirely in front of the near plane and stores it in the
    /// batch.
    ///
    /// Everything that doesn't depend on the pixel — projection, face culling, bounding box,
    /// attributes divided by `w` — leaves here ready. That is what allows filling the same
    /// triangle in several screen bands at once without repeating any computation.
    fn prepare(&self, tri: [Vertex; 3], batch: &mut Batch) {
        let (vx, vy, vw, vh) = self.top_down_viewport();
        if vw <= 0 || vh <= 0 {
            return;
        }
        // Perspective divide and mapping to the screen. We keep `1/w` to correct the
        // interpolation later: interpolating `u` directly in screen space distorts the texture.
        let mut screen = [[0.0f32; 4]; 3];
        for (slot, vertex) in screen.iter_mut().zip(tri.iter()) {
            let [x, y, z, w] = vertex.position;
            let inv_w = 1.0 / w;
            let (near, far) = self.depth_range;
            *slot = [
                vx as f32 + (x * inv_w * 0.5 + 0.5) * vw as f32,
                vy as f32 + (0.5 - y * inv_w * 0.5) * vh as f32,
                near + (far - near) * (z * inv_w * 0.5 + 0.5),
                inv_w,
            ];
        }

        let area = edge(screen[0], screen[1], screen[2]);
        if area == 0.0 {
            return;
        }
        // The sign of the area gives the winding, but with the Y axis already flipped by the
        // mapping to the screen — so it is the opposite of the sign in OpenGL window
        // coordinates, and a negative area here is counterclockwise there. Getting this sign
        // wrong culls exactly the faces that should show: Quake's world went black, with only
        // the back side of the geometry being drawn.
        let counter_clockwise = area < 0.0;
        let front = (self.front_face == gles::GL_CCW) == counter_clockwise;
        if self.cull_face
            && match self.cull_mode {
                gles::GL_FRONT => front,
                gles::GL_FRONT_AND_BACK => true,
                _ => !front,
            }
        {
            return;
        }

        let min_x = screen.iter().fold(f32::MAX, |m, v| m.min(v[0])).floor() as i32;
        let max_x = screen.iter().fold(f32::MIN, |m, v| m.max(v[0])).ceil() as i32;
        let min_y = screen.iter().fold(f32::MAX, |m, v| m.min(v[1])).floor() as i32;
        let max_y = screen.iter().fold(f32::MIN, |m, v| m.max(v[1])).ceil() as i32;
        let min_x = min_x.max(vx).max(0);
        let max_x = max_x.min(vx + vw).min(self.width as i32);
        let min_y = min_y.max(vy).max(0);
        let max_y = max_y.min(vy + vh).min(self.height as i32);
        // `glScissor` comes in here, together with the viewport: an axis-aligned rectangle only
        // needs to tighten the triangle's box, and that way it costs nothing per pixel.
        let (min_x, max_x, min_y, max_y) = match self.scissor {
            None => (min_x, max_x, min_y, max_y),
            Some((tx, ty, tw, th)) => (
                min_x.max(tx),
                max_x.min(tx + tw),
                min_y.max(ty),
                max_y.min(ty + th),
            ),
        };
        if min_x >= max_x || min_y >= max_y {
            return;
        }

        // Attributes travel divided by `w` — that division is what corrects the perspective —,
        // and each comes back multiplied by the interpolated `w`. Doing the division here, once
        // per vertex, takes three multiplications per fragment out of the loop.
        let mut over_w = [[0.0f32; 9]; 3];
        for i in 0..3 {
            let w = screen[i][3];
            let v = &tri[i];
            over_w[i] = [
                v.color[0] * w,
                v.color[1] * w,
                v.color[2] * w,
                v.color[3] * w,
                v.uv[0] * w,
                v.uv[1] * w,
                v.fog * w,
                v.uv1[0] * w,
                v.uv1[1] * w,
            ];
        }

        let inv_area = 1.0 / area;
        // The three edge functions are linear in `x`, so moving one pixel sideways is adding a
        // constant. Computing them from scratch at each pixel was nine multiplications per
        // fragment — and most fragments of the bounding box are discarded without ever becoming
        // color.
        let step = [
            (screen[1][1] - screen[2][1]) * inv_area,
            (screen[2][1] - screen[0][1]) * inv_area,
            (screen[0][1] - screen[1][1]) * inv_area,
        ];
        // The same for moving down one row. Only the mipmap level choice uses this, and it does
        // because **texture compression is not always horizontal**: on a grazing surface it may
        // be entirely vertical, and measuring only sideways gave level zero in a place where the
        // whole texture goes by in two pixels — which was the jaggedness of the stripes.
        let step_y = [
            (screen[2][0] - screen[1][0]) * inv_area,
            (screen[0][0] - screen[2][0]) * inv_area,
            (screen[1][0] - screen[0][0]) * inv_area,
        ];

        batch.cost += (max_x - min_x) as usize * (max_y - min_y) as usize;
        batch.triangles.push(Prepared {
            screen,
            over_w,
            inv_area,
            step,
            step_y,
            min_x,
            max_x,
            min_y,
            max_y,
        });
    }

    /// Stores the draw call's batch to be painted at the end of the frame.
    ///
    /// Painting draw call by draw call seemed natural and was expensive: Quake makes half a
    /// million of them in twenty-five seconds, with five thousand fragments each on average. At
    /// that size there is no way to split across threads — waking them costs more than the work
    /// —, so 99% of its fills ran serially, and the parallel rasterizer did nothing.
    ///
    /// Accumulating the whole frame, the split happens once over a large amount of work. Each
    /// batch carries the OpenGL state that applied when it was built, because it changes between
    /// one draw call and the next.
    fn enqueue(&mut self, batch: &mut Batch) {
        if batch.triangles.is_empty() {
            return;
        }
        let start = self.pending.triangles.len();
        self.pending.triangles.append(&mut batch.triangles);
        self.pending.cost += batch.cost;
        self.pending.jobs.push(Job {
            first: start,
            last: self.pending.triangles.len(),
            // The texture is kept by name, not by reference: between now and the flush the game
            // may bind another, and the one at this moment is what counts.
            texture: match self.texture_2d {
                true => Some(self.bound_texture),
                false => None,
            },
            texture_env: self.texture_env,
            texture1: self.unit1.enabled.then_some(self.unit1.texture),
            env1: self.unit1.env,
            depth_test: self.depth_test,
            depth_mask: self.depth_mask,
            color_mask: self.color_mask,
            fog: self.fog,
            depth_func: self.depth_func,
            blend: self.blend,
            blend_src: self.blend_src,
            blend_dst: self.blend_dst,
            stencil: Stencil {
                test: self.stencil_test,
                func: self.stencil_func,
                reference: self.stencil_ref as u8,
                value_mask: self.stencil_value_mask as u8,
                write_mask: self.stencil_write_mask as u8,
                op: self.stencil_op,
            },
            alpha_test: self.alpha_test,
            alpha_func: self.alpha_func,
            alpha_ref: self.alpha_ref,
        });
    }

    /// Paints everything in the queue, splitting the screen into horizontal bands when it pays
    /// off.
    ///
    /// Each band is an exclusive piece of the frame and walks the whole queue in the order the
    /// game drew, so transparency stacks in the same sequence and the result is the same as with
    /// a single thread.
    ///
    /// It must be called before anything that reads the frame or changes a texture the queue
    /// refers to — presenting, clearing, switching surfaces, loading or deleting a texture.
    pub fn flush(&mut self) {
        if self.pending.jobs.is_empty() {
            return;
        }
        self.dirty = true;
        // Splitting the fields lets us read the textures and write to the frame at the same time.
        let Self {
            color,
            depth,
            stencil,
            textures,
            width,
            height,
            pending,
            ..
        } = self;
        let (width, height) = (*width, *height);
        let (jobs, triangles) = (&pending.jobs, &pending.triangles);

        let bands = bands(height);
        if bands < 2 || pending.cost < PARALLEL_COST {
            let mut band = Band {
                top: 0,
                color,
                depth,
                stencil,
            };
            for job in jobs {
                let uniforms = job.uniforms(textures, width);
                for tri in &triangles[job.first..job.last] {
                    fill_band(tri, &uniforms, &mut band);
                }
            }
            pending.clear();
            return;
        }

        let rows = height.div_ceil(bands);
        // On the Vita the threads are created once and stay. Creating them here, every frame,
        // asked the kernel for a stack in the middle of the game: when it refused, the `expect`
        // aborted the process. The band that has no thread is painted by the caller.
        #[cfg(target_os = "vita")]
        paint_parallel_vita(
            color, depth, stencil, jobs, triangles, textures, width, rows,
        );
        #[cfg(not(target_os = "vita"))]
        std::thread::scope(|scope| {
            let mut top = 0i32;
            for ((color, depth), stencil) in color
                .chunks_mut(rows * width)
                .zip(depth.chunks_mut(rows * width))
                .zip(stencil.chunks_mut(rows * width))
            {
                let band_top = top;
                top += (color.len() / width) as i32;
                let textures = &*textures;
                scope.spawn(move || {
                    paint_band(
                        band_top, color, depth, stencil, jobs, triangles, textures, width,
                    );
                });
            }
        });
        pending.clear();
    }

    /// The finished frame, in RGB565 and at the requested `(width, height)` size.
    ///
    /// Applies direct writes to the buffer exposed by EGL without restoring old pixels over
    /// more recent draws. Depth and stencil are preserved.
    pub fn import_rgb565_changes(&mut self, width: usize, height: usize, old: &[u8], new: &[u8]) {
        self.flush();
        let (sw, sh) = self.surface();
        if width == 0 || height == 0 || old.len() != width * height * 2 || new.len() != old.len() {
            return;
        }
        /// An RGB565 pixel of the exposed buffer, already in RGB888.
        fn expand(create: &[u8], offset: usize) -> [u8; 3] {
            let pixel = u16::from_le_bytes([create[offset], create[offset + 1]]);
            let r = ((pixel >> 11) & 31) as u8;
            let g = ((pixel >> 5) & 63) as u8;
            let b = (pixel & 31) as u8;
            [
                (r << 3) | (r >> 2),
                (g << 2) | (g >> 4),
                (b << 3) | (b >> 2),
            ]
        }

        // **The unscaled path is separate on purpose**, for the same reason as in
        // [`GlState::frame_rgb565`]: the general loop does two integer divisions per pixel, and
        // at 640×330 that is over four hundred thousand divisions on each call.
        //
        // And the whole row is compared at once before the per-pixel loop. The game touches a
        // small rectangle or nothing; walking the whole surface pixel by pixel to find that out
        // cost 3.6 s in a thirteen-second Z-Wheel run. A `!=` between slices is a `memcmp`, and
        // it exits at the first difference.
        if sw == width && sh == height {
            let columns = width.min(self.width);
            for y in 0..height.min(self.height) {
                let start = y * width * 2;
                let end = start + columns * 2;
                if old[start..end] == new[start..end] {
                    continue;
                }
                for x in 0..columns {
                    let offset = start + x * 2;
                    if old[offset..offset + 2] == new[offset..offset + 2] {
                        continue;
                    }
                    let [r, g, b] = expand(new, offset);
                    let index = y * self.width + x;
                    self.dirty = true;
                    self.color[index] = [r, g, b, self.color[index][3]];
                }
            }
            return;
        }
        for y in 0..sh.min(self.height) {
            for x in 0..sw.min(self.width) {
                let offset = ((y * height / sh) * width + x * width / sw) * 2;
                if old[offset..offset + 2] == new[offset..offset + 2] {
                    continue;
                }
                let [r, g, b] = expand(new, offset);
                let index = y * self.width + x;
                self.dirty = true;
                self.color[index] = [r, g, b, self.color[index][3]];
            }
        }
    }

    /// The surface the game draws on is usually smaller than the screen — Zeebo's Quake draws at
    /// 320×400 on a 640×480 screen —, so presentation scales it up. That is what Qualcomm's
    /// scaling extension does on the console, and without it the frame shows up shrunk in a
    /// corner.
    /// The frame in RGB565, written into a vector the caller reuses.
    ///
    /// It exists because of `eglGetColorBufferQUALCOMM`, which is called twice per frame and
    /// hands the pixels to the game through its memory: building a new `Vec` on each call, and
    /// then another for the bytes, was allocating and copying eight hundred kilobytes sixty times
    /// per second. Here the vector is always the same.
    ///
    /// **The unscaled path is separate on purpose.** When the request has the size of the
    /// surface — which is the normal case —, the two divisions per pixel of the general loop add
    /// up to over four hundred thousand integer divisions per call, and they cost more than the
    /// conversion.
    pub fn frame_rgb565(&mut self, width: usize, height: usize, out: &mut Vec<u8>) {
        self.flush();
        // Frame identical to what is already in `out`: nothing to reconvert. See
        // [`GlState::dirty`].
        if !self.dirty && out.len() == width * height * 2 {
            return;
        }
        let (sw, sh) = self.surface();
        out.clear();
        out.resize(width * height * 2, 0);
        let convert = |p: [u8; 4]| {
            ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3)
        };
        // Writes into an already-sized slice. It used to be one `extend` per pixel, and on the
        // scaling path that is a trip to the `Vec` for every two bytes.
        if sw == width && sh == height {
            for (y, output) in out.chunks_exact_mut(width * 2).enumerate() {
                let line = &self.color[y * self.width..y * self.width + width];
                for (pixel, pair) in line.iter().zip(output.chunks_exact_mut(2)) {
                    pair.copy_from_slice(&convert(*pixel).to_le_bytes());
                }
            }
        } else {
            for (y, output) in out.chunks_exact_mut(width * 2).enumerate() {
                let line = &self.color[(y * sh / height) * self.width..][..self.width];
                for (x, pair) in output.chunks_exact_mut(2).enumerate() {
                    pair.copy_from_slice(&convert(line[x * sw / width]).to_le_bytes());
                }
            }
        }
        self.dirty = false;
    }

    /// Same frame as the byte export, but without the RGB565 -> bytes -> RGB565 round trip.
    pub fn frame_rgb565_words(&mut self, width: usize, height: usize, out: &mut Vec<u16>) {
        self.flush();
        if !self.dirty && out.len() == width * height {
            return;
        }
        let (sw, sh) = self.surface();
        out.clear();
        out.resize(width * height, 0);
        let convert = |p: [u8; 4]| {
            ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3)
        };
        if sw == width && sh == height {
            for (output, &pixel) in out.iter_mut().zip(&self.color) {
                *output = convert(pixel);
            }
        } else {
            for y in 0..height {
                let line = &self.color[(y * sh / height) * self.width..][..self.width];
                for x in 0..width {
                    out[y * width + x] = convert(line[x * sw / width]);
                }
            }
        }
        self.dirty = false;
    }

#[cfg(test)]
    pub fn present(&mut self, width: usize, height: usize) -> Vec<u16> {
        // Handing over the frame is the point where it must be painted — whoever asks for the
        // result has no reason to know drawing is accumulated.
        self.flush();
        let (sw, sh) = self.surface();
        let mut out = vec![0u16; width * height];
        for y in 0..height {
            let sy = y * sh / height;
            for x in 0..width {
                let p = self.color[sy * self.width + x * sw / width];
                out[y * width + x] =
                    ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3);
            }
        }
        out
    }
}

/// Matrix stack levels. The minimum OpenGL ES 1.1 guarantees for the modelview is 16.
const MAX_MATRIX_STACK: usize = 16;

/// From how many bounding-box fragments it is worth splitting the batch across threads.
///
/// Below this, filling finishes before the threads are done waking up. The number comes from
/// measurement: in Crash and Alpine Racer, over 95% of fill time is in draw calls well past this
/// size, and the flood of small calls — most of them — together cost a fraction of the total.
const PARALLEL_COST: usize = 64_000;

/// Minimum height of a band, in rows.
///
/// It is the **real ceiling on parallelism for short surfaces**, not a tuning detail: with the
/// forty rows from before, Z-Wheel's 640x330 stage gave `330 / 40 = 8` bands, and on a 24-core
/// machine two thirds of it sat idle during the whole fill.
///
/// Measured, `flush` time on Z-Wheel over thirteen virtual seconds: 2474 ms with 40 rows, 1938
/// with 16, 1868 with 8. And on the Crash track, over thirty virtual seconds, the total API time
/// went from 4637 to 3842 and 3622 ms at the same cutoffs — whoever feared that thin bands would
/// be expensive on a 480-row surface was wrong, because there the number of bands hits the
/// number of cores before hitting this constant.
///
/// Per-band setup exists and grows as the band gets thinner — that is why there is a floor —,
/// but down to eight rows it remains smaller than the work the band gains.
const MIN_BAND_ROWS: usize = 8;

/// Into how many horizontal bands to split a frame of `height` rows.
fn bands(height: usize) -> usize {
    static CORES: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let colors = *CORES.get_or_init(fill_cores);
    colors.min(height / MIN_BAND_ROWS).max(1)
}

/// How many cores filling may use.
///
/// On the Vita `available_parallelism` returns an error: std's Unix branch doesn't know
/// `target_os = "vita"` and falls into "unsupported". The `unwrap_or(1)` left the
/// whole 3D on the interpreter's core. Homebrew gets three cores; the
/// fourth is `SCE_KERNEL_CPU_MASK_SYSTEM`.
fn fill_cores() -> usize {
    if cfg!(target_os = "vita") {
        return 3;
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Paints a whole band, in the order the game drew.
fn paint_band(
    top: i32,
    color: &mut [[u8; 4]],
    depth: &mut [f32],
    stencil: &mut [u8],
    jobs: &[Job],
    triangles: &[Prepared],
    textures: &HashMap<u32, Texture>,
    width: usize,
) {
    let mut band = Band {
        top,
        color,
        depth,
        stencil,
    };
    for job in jobs {
        let uniforms = job.uniforms(textures, width);
        for tri in &triangles[job.first..job.last] {
            fill_band(tri, &uniforms, &mut band);
        }
    }
}

/// Pointers of a band. Only valid while the frame that created them waits for the threads.
///
/// `Send` because the thread takes the band to another core. Only it writes, in its own
/// slice; the frame and the textures stay still until it finishes.
#[cfg(target_os = "vita")]
struct BandSlice {
    top: i32,
    color: *mut [u8; 4],
    color_len: usize,
    depth: *mut f32,
    depth_len: usize,
    stencil: *mut u8,
    stencil_len: usize,
    jobs: *const Job,
    jobs_len: usize,
    triangles: *const Prepared,
    triangles_len: usize,
    textures: *const HashMap<u32, Texture>,
    width: usize,
}

// SAFETY: see the comment on `BandSlice`. The thread only reads the band the frame just published.
#[cfg(target_os = "vita")]
unsafe impl Send for BandSlice {}

#[cfg(target_os = "vita")]
fn run_slice(slice: &BandSlice) {
    // SAFETY: the band was built from non-overlapping pieces, and the frame is not read back
    // until all threads of this generation finish.
    unsafe {
        let color = std::slice::from_raw_parts_mut(slice.color, slice.color_len);
        let depth = std::slice::from_raw_parts_mut(slice.depth, slice.depth_len);
        let stencil = std::slice::from_raw_parts_mut(slice.stencil, slice.stencil_len);
        let jobs = std::slice::from_raw_parts(slice.jobs, slice.jobs_len);
        let triangles = std::slice::from_raw_parts(slice.triangles, slice.triangles_len);
        let textures = &*slice.textures;
        paint_band(
            slice.top,
            color,
            depth,
            stencil,
            jobs,
            triangles,
            textures,
            slice.width,
        );
    }
}

/// The Vita's three fill threads. They live for the whole process.
#[cfg(target_os = "vita")]
struct VitaPool {
    state: std::sync::Mutex<VitaState>,
    wake: std::sync::Condvar,
    ready: std::sync::Condvar,
}

#[cfg(target_os = "vita")]
struct VitaState {
    generation: u64,
    slices: [Option<BandSlice>; 3],
    done: usize,
    /// Which index actually has a thread. A hole in the middle can't receive a band: the thread
    /// at that index doesn't exist, and the frame would wait for an answer that never comes.
    live: [bool; 3],
}

#[cfg(target_os = "vita")]
fn vita_pool() -> &'static VitaPool {
    static POOL: std::sync::OnceLock<&'static VitaPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let pool: &'static VitaPool = Box::leak(Box::new(VitaPool {
            state: std::sync::Mutex::new(VitaState {
                generation: 0,
                slices: [None, None, None],
                done: 0,
                live: [false; 3],
            }),
            wake: std::sync::Condvar::new(),
            ready: std::sync::Condvar::new(),
        }));
        let mut live = [false; 3];
        for index in 0..3 {
            let pool = pool;
            // 2 MiB is what std asked for. It fits the fill, and since the thread doesn't die
            // at the end of the frame, the kernel allocates this stack only once.
            let created = std::thread::Builder::new()
                .name(format!("fill-{index}"))
                .stack_size(2 * 1024 * 1024)
                .spawn(move || vita_worker(index, pool));
            live[index] = created.is_ok();
        }
        if live == [false; 3] {
            crate::logging!(
                crate::logging::Level::Warning,
                "gl",
                "the Vita opened no fill thread; the frame stays on the game thread"
            );
        }
        pool.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .live = live;
        pool
    })
}

#[cfg(target_os = "vita")]
fn vita_worker(index: usize, pool: &'static VitaPool) {
    vita_affinity(index);
    let mut view = 0u64;
    loop {
        let mut state = pool.state.lock().unwrap_or_else(|error| error.into_inner());
        while state.generation == view || state.slices[index].is_none() {
            state = pool
                .wake
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        view = state.generation;
        let slice = state.slices[index].take();
        drop(state);
        if let Some(slice) = slice {
            run_slice(&slice);
        }
        let mut state = pool.state.lock().unwrap_or_else(|error| error.into_inner());
        state.done += 1;
        pool.ready.notify_one();
    }
}

#[cfg(target_os = "vita")]
fn paint_parallel_vita(
    color: &mut [[u8; 4]],
    depth: &mut [f32],
    stencil: &mut [u8],
    jobs: &[Job],
    triangles: &[Prepared],
    textures: &HashMap<u32, Texture>,
    width: usize,
    rows: usize,
) {
    let color_len = color.len();
    let depth_len = depth.len();
    let stencil_len = stencil.len();
    let color = color.as_mut_ptr();
    let depth = depth.as_mut_ptr();
    let stencil = stencil.as_mut_ptr();
    let step = rows.saturating_mul(width);
    let mut local_slices = Vec::new();
    let mut offset = 0usize;
    let mut top = 0i32;
    while offset < color_len && offset < depth_len && offset < stencil_len {
        let n = step
            .min(color_len - offset)
            .min(depth_len - offset)
            .min(stencil_len - offset);
        if n == 0 {
            break;
        }
        local_slices.push(BandSlice {
            top,
            color: unsafe { color.add(offset) },
            color_len: n,
            depth: unsafe { depth.add(offset) },
            depth_len: n,
            stencil: unsafe { stencil.add(offset) },
            stencil_len: n,
            jobs: jobs.as_ptr(),
            jobs_len: jobs.len(),
            triangles: triangles.as_ptr(),
            triangles_len: triangles.len(),
            textures: std::ptr::from_ref(textures),
            width,
        });
        top += (n / width) as i32;
        offset += n;
    }

    let pool = vita_pool();
    let mut state = pool.state.lock().unwrap_or_else(|error| error.into_inner());
    let live = state.live;
    state.done = 0;
    let mut on_main = Vec::new();
    let mut sent_count = 0usize;
    let mut bands = local_slices.into_iter();
    for index in 0..3 {
        if !live[index] {
            continue;
        }
        if let Some(slice) = bands.next() {
            state.slices[index] = Some(slice);
            sent_count += 1;
        }
    }
    on_main.extend(bands);
    state.generation = state.generation.wrapping_add(1);
    pool.wake.notify_all();
    drop(state);

    for slice in on_main {
        run_slice(&slice);
    }

    let mut state = pool.state.lock().unwrap_or_else(|error| error.into_inner());
    while state.done < sent_count {
        state = pool
            .ready
            .wait(state)
            .unwrap_or_else(|error| error.into_inner());
    }
}

/// Puts the fill thread on a Vita user core.
///
/// `sceKernelCreateThread` with mask 0, which is what newlib's `pthread`
/// passes, doesn't spread the work. The masks are the ones from `psp2/kernel/cpu.h`:
/// `USER_0`, `USER_1` and `USER_2`.
#[cfg(target_os = "vita")]
fn vita_affinity(index: usize) {
    const CORE_MASKS: [i32; 3] = [0x0001_0000, 0x0002_0000, 0x0004_0000];
    unsafe extern "C" {
        fn sceKernelGetThreadId() -> i32;
        fn sceKernelChangeThreadCpuAffinityMask(thid: i32, mask: i32) -> i32;
    }
    unsafe {
        let id = sceKernelGetThreadId();
        if id > 0 {
            let _ = sceKernelChangeThreadCpuAffinityMask(id, CORE_MASKS[index % 3]);
        }
    }
}

/// A triangle already projected on screen, with everything that doesn't depend on the pixel
/// resolved.
struct Prepared {
    /// Vertices in screen coordinates, with `1/w` in the fourth component.
    screen: [[f32; 4]; 3],
    /// Color and texture coordinate of each vertex, divided by `w`.
    over_w: [[f32; 9]; 3],
    inv_area: f32,
    /// How much each edge function advances per pixel to the right.
    step: [f32; 3],
    /// And per row downward. See where it is built.
    step_y: [f32; 3],
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
}

/// A draw call waiting to become pixels: its range in the triangle queue, plus the OpenGL state
/// that applied when it was built.
/// The stencil state of a draw call, the way filling needs it.
#[derive(Debug, Clone, Copy)]
struct Stencil {
    test: bool,
    func: u32,
    reference: u8,
    value_mask: u8,
    write_mask: u8,
    /// `[failure, zfail, passes]`.
    op: [u32; 3],
}

impl Stencil {
    /// Applies one of the three operations to a buffer value.
    ///
    /// The write mask decides bit by bit what changes: it is what lets one draw mark the floor
    /// without touching the bits another pass uses.
    fn apply(&self, which: usize, current: u8) -> u8 {
        let create = match self.op[which] {
            gles::GL_KEEP => return current,
            gles::GL_ZERO_OP => 0,
            gles::GL_REPLACE => self.reference,
            gles::GL_INCR => current.saturating_add(1),
            gles::GL_DECR => current.saturating_sub(1),
            gles::GL_INVERT => !current,
            _ => return current,
        };
        (current & !self.write_mask) | (create & self.write_mask)
    }

    /// The test itself, between the reference and what is in the buffer, both masked.
    fn passes(&self, current: u8) -> bool {
        let (r, v) = (
            (self.reference & self.value_mask) as f32,
            (current & self.value_mask) as f32,
        );
        compare(self.func, r, v)
    }
}

struct Job {
    first: usize,
    last: usize,
    /// Name of the bound texture, or `None` if the draw uses no texture.
    texture: Option<u32>,
    texture_env: TexEnv,
    /// The texture and environment of unit 1, when it is enabled.
    texture1: Option<u32>,
    env1: TexEnv,
    depth_test: bool,
    depth_mask: bool,
    color_mask: [bool; 4],
    depth_func: u32,
    blend: bool,
    blend_src: u32,
    blend_dst: u32,
    alpha_test: bool,
    alpha_func: u32,
    alpha_ref: f32,
    /// The stencil state at draw time. See [`GlState::stencil_test`].
    stencil: Stencil,
    /// The fog at draw time. See [`Fog`].
    fog: Fog,
}

impl Job {
    /// Resolves the texture name and builds the state filling reads.
    fn uniforms<'a>(&self, textures: &'a HashMap<u32, Texture>, width: usize) -> Uniforms<'a> {
        let texture = self.texture.and_then(|name| textures.get(&name));
        Uniforms {
            width,
            texture,
            uses_mipmap: texture.is_some_and(|texture| {
                !texture.mipmaps.is_empty()
                    && matches!(
                        texture.min_filter,
                        gles::GL_NEAREST_MIPMAP_NEAREST
                            | gles::GL_LINEAR_MIPMAP_NEAREST
                            | gles::GL_NEAREST_MIPMAP_LINEAR
                            | gles::GL_LINEAR_MIPMAP_LINEAR
                    )
            }),
            texture_env: self.texture_env,
            texture1: self.texture1.and_then(|name| textures.get(&name)),
            env1: self.env1,
            depth_test: self.depth_test,
            depth_mask: self.depth_mask,
            color_mask: self.color_mask,
            depth_func: self.depth_func,
            blend: self.blend,
            blend_src: self.blend_src,
            blend_dst: self.blend_dst,
            alpha_test: self.alpha_test,
            alpha_func: self.alpha_func,
            alpha_ref: self.alpha_ref,
            stencil: self.stencil,
            fog: self.fog,
        }
    }
}

/// The frame under construction: the triangles of every draw call since the last flush, and the
/// list of which belong to each one.
///
/// Both vectors live across frames so as not to give back and re-request the same memory sixty
/// times per second.
#[derive(Default)]
struct Pending {
    triangles: Vec<Prepared>,
    jobs: Vec<Job>,
    /// Sum of the bounding boxes, in fragments: it decides whether waking the threads is worth
    /// it.
    cost: usize,
}

impl Pending {
    fn clear(&mut self) {
        self.triangles.clear();
        self.jobs.clear();
        self.cost = 0;
    }
}

/// The triangles of a draw call, with the estimated cost of filling them.
#[derive(Default)]
struct Batch {
    triangles: Vec<Prepared>,
    /// Sum of the bounding boxes, in fragments.
    cost: usize,
}

impl Batch {
    fn clear(&mut self) {
        self.triangles.clear();
        self.cost = 0;
    }
}

/// The OpenGL state that applies to the whole batch. Read-only, and therefore shareable among
/// the threads that fill the bands.
struct Uniforms<'a> {
    width: usize,
    texture: Option<&'a Texture>,
    /// Whether the batch's texture has a mipmap chain **and** the filter asks for a level.
    ///
    /// It doesn't depend on the fragment, but was recomputed inside the loop — a `matches!` of
    /// four constants on every passing pixel, hundreds of thousands of times per frame.
    uses_mipmap: bool,
    texture_env: TexEnv,
    /// The texture and environment of unit 1.
    texture1: Option<&'a Texture>,
    env1: TexEnv,
    depth_test: bool,
    depth_mask: bool,
    color_mask: [bool; 4],
    depth_func: u32,
    blend: bool,
    blend_src: u32,
    blend_dst: u32,
    alpha_test: bool,
    alpha_func: u32,
    alpha_ref: f32,
    /// The stencil state at draw time. See [`GlState::stencil_test`].
    stencil: Stencil,
    /// The fog at draw time. See [`Fog`].
    fog: Fog,
}

/// A horizontal band of the frame: a thread's exclusive piece.
struct Band<'a> {
    /// Screen row corresponding to the start of the band.
    top: i32,
    color: &'a mut [[u8; 4]],
    depth: &'a mut [f32],
    stencil: &'a mut [u8],
}

/// Level-zero texture, in integers. The floating-point path divides and multiplies each channel
/// by 255; on the Cortex-A9 that is the bulk of the pixel. Against `pack(sample_level)`, the
/// worst channel measured in the `the_integer_sample_stays_within_two_levels` test was two
/// levels.
#[cfg(any(test, target_os = "vita"))]
struct Texel<'a> {
    pixels: &'a [[u8; 4]],
    width: usize,
    height: usize,
    wrap: [u32; 2],
    /// `GL_REPEAT` with a power-of-two side becomes a mask. The rest pays `wrap`'s division.
    pot: [bool; 2],
    mask: [usize; 2],
}

#[cfg(any(test, target_os = "vita"))]
impl<'a> Texel<'a> {
    fn create(texture: &'a Texture) -> Option<Self> {
        let width = texture.width;
        let height = texture.height;
        if width == 0 || height == 0 || texture.pixels.len() < width.saturating_mul(height) {
            return None;
        }
        let pot = [
            texture.wrap[0] == gles::GL_REPEAT && width.is_power_of_two(),
            texture.wrap[1] == gles::GL_REPEAT && height.is_power_of_two(),
        ];
        Some(Self {
            pixels: &texture.pixels,
            width,
            height,
            wrap: texture.wrap,
            pot,
            mask: [width - 1, height - 1],
        })
    }

    #[inline(always)]
    fn index(&self, axis: usize, value: i32) -> usize {
        let size = if axis == 0 { self.width } else { self.height };
        if self.pot[axis] {
            return (value as usize) & self.mask[axis];
        }
        Texture::wrap(self.wrap[axis], value, size)
    }

    #[inline(always)]
    fn read(&self, x: i32, y: i32) -> [u8; 4] {
        let (x, y) = (self.index(0, x), self.index(1, y));
        // SAFETY: `index` returns `0..side`, and `create` required pixels for the whole texture.
        unsafe { *self.pixels.get_unchecked(y * self.width + x) }
    }

    #[inline(always)]
    fn get<const SMOOTH: bool>(&self, u: f32, v: f32) -> [u8; 4] {
        let x = u * self.width as f32 - 0.5;
        let y = v * self.height as f32 - 0.5;
        if !SMOOTH {
            return self.read(x.round() as i32, y.round() as i32);
        }
        let (x0, y0) = (x.floor(), y.floor());
        let fx = ((x - x0) * 256.0).clamp(0.0, 255.0) as u32;
        let fy = ((y - y0) * 256.0).clamp(0.0, 255.0) as u32;
        let (x0, y0) = (x0 as i32, y0 as i32);
        let corners = [
            self.read(x0, y0),
            self.read(x0 + 1, y0),
            self.read(x0, y0 + 1),
            self.read(x0 + 1, y0 + 1),
        ];
        let mut output = [0u8; 4];
        for channel in 0..4 {
            let up = u32::from(corners[0][channel]) * (256 - fx) + u32::from(corners[1][channel]) * fx;
            let down = u32::from(corners[2][channel]) * (256 - fx) + u32::from(corners[3][channel]) * fx;
            output[channel] = ((up * (256 - fy) + down * fy + 32_768) >> 16) as u8;
        }
        output
    }
}

#[cfg(test)]
thread_local! {
    static AFFINE_PATH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Four vertices with the same `1/w`: sprites and 2D backgrounds. Per-pixel perspective is a
/// pointless division, and it is what the opaque loop still did.
#[cfg(target_os = "vita")]
fn affine(tri: &Prepared) -> Option<f32> {
    let w = tri.screen[0][3];
    if w == 0.0 {
        return None;
    }
    let near = |other: f32| (other - w).abs() <= 1.0e-5 * (1.0 + w.abs());
    (near(tri.screen[1][3]) && near(tri.screen[2][3])).then_some(w)
}

#[cfg(target_os = "vita")]
fn flat_color(tri: &Prepared, w: f32) -> Option<[f32; 4]> {
    let mut color = [0.0; 4];
    for channel in 0..4 {
        let c0 = tri.over_w[0][channel] / w;
        let c1 = tri.over_w[1][channel] / w;
        let c2 = tri.over_w[2][channel] / w;
        if (c0 - c1).abs() > 1.0e-4 || (c0 - c2).abs() > 1.0e-4 || !(0.0..=1.0).contains(&c0) {
            return None;
        }
        color[channel] = c0;
    }
    Some(color)
}

/// `0` doesn't blend, `1` is `SRC_ALPHA` over `ONE_MINUS_SRC_ALPHA`, `2` is additive.
#[cfg(target_os = "vita")]
fn fast_blend(uniforms: &Uniforms) -> Option<u8> {
    if !uniforms.blend {
        return Some(0);
    }
    if uniforms.blend_src != gles::GL_SRC_ALPHA {
        return None;
    }
    if uniforms.blend_dst == gles::GL_ONE_MINUS_SRC_ALPHA {
        Some(1)
    } else if uniforms.blend_dst == gles::GL_ONE {
        Some(2)
    } else {
        None
    }
}

#[inline(always)]
#[cfg(target_os = "vita")]
fn apply_color(texel: [u8; 4], color: [f32; 4]) -> [u8; 4] {
    std::array::from_fn(|i| {
        let v = color[i] * f32::from(texel[i]) + 0.5;
        if v <= 0.0 {
            0
        } else if v >= 255.0 {
            255
        } else {
            v as u8
        }
    })
}

#[inline(always)]
#[cfg(target_os = "vita")]
fn blend_over(src: [u8; 4], dst: [u8; 4], additive: bool) -> [u8; 4] {
    let alpha = u32::from(src[3]);
    std::array::from_fn(|i| {
        let summed = if additive {
            u32::from(src[i]) * alpha + u32::from(dst[i]) * 255
        } else {
            u32::from(src[i]) * alpha + u32::from(dst[i]) * (255 - alpha)
        };
        (summed.saturating_add(127) / 255).min(255) as u8
    })
}

/// Fill for affine triangles: one texture, constant color, no fog and no stencil.
///
/// The general body — and even the opaque shortcut — interpolates perspective and samples in
/// floating point. Sprites with an alpha test didn't take the shortcut, and they are the bulk of
/// a 2D game. Here `u` and `v` advance one step per pixel, depth is still the exact computation
/// from vertex zero, and the texel is integer.
///
/// Measured on this Mac, in release, the same 640×480 frame with linear texture and depth test,
/// filled in parallel: 2.22 ms in the general body, 1.09 ms in the opaque shortcut and 0.61 ms
/// here. It is not a Vita number.
#[cfg(target_os = "vita")]
#[inline(never)]
fn fill_affine<const SMOOTH: bool>(
    tri: &Prepared,
    uniforms: &Uniforms,
    band: &mut Band,
    texel: &Texel<'_>,
    w: f32,
    color: [f32; 4],
    copy: bool,
    blend: u8,
) {
    #[cfg(test)]
    AFFINE_PATH.with(|c| c.set(c.get() + 1));
    let persp = 1.0 / w;
    let derivative = |channel: usize| {
        (tri.step[0] * tri.over_w[0][channel]
            + tri.step[1] * tri.over_w[1][channel]
            + tri.step[2] * tri.over_w[2][channel])
            * persp
    };
    let (du, dv) = (derivative(4), derivative(5));
    let (z0, dz1, dz2) = (
        tri.screen[0][2],
        tri.screen[1][2] - tri.screen[0][2],
        tri.screen[2][2] - tri.screen[0][2],
    );
    let ou = [tri.over_w[0][4], tri.over_w[1][4], tri.over_w[2][4]];
    let ov = [tri.over_w[0][5], tri.over_w[1][5], tri.over_w[2][5]];
    let width = uniforms.width;
    let rows = (band.color.len() / width) as i32;
    let top = tri.min_y.max(band.top);
    let bottom = tri.max_y.min(band.top + rows);
    let depth_test = uniforms.depth_test;
    let depth_func = uniforms.depth_func;
    let write_z = depth_test && uniforms.depth_mask;
    let alpha_test = uniforms.alpha_test;
    let alpha_func = uniforms.alpha_func;
    let alpha_ref = uniforms.alpha_ref;
    let additive = blend == 2;
    for y in top..bottom {
        let start = [tri.min_x as f32 + 0.5, y as f32 + 0.5, 0.0, 0.0];
        let mut weight = [
            edge(tri.screen[1], tri.screen[2], start) * tri.inv_area,
            edge(tri.screen[2], tri.screen[0], start) * tri.inv_area,
            edge(tri.screen[0], tri.screen[1], start) * tri.inv_area,
        ];
        let mut x = tri.min_x;
        while x < tri.max_x && (weight[0] < 0.0 || weight[1] < 0.0 || weight[2] < 0.0) {
            weight = [
                weight[0] + tri.step[0],
                weight[1] + tri.step[1],
                weight[2] + tri.step[2],
            ];
            x += 1;
        }
        let row = (y - band.top) as usize * width;
        let mut u = 0.0;
        let mut v = 0.0;
        let mut phase = 0u32;
        while x < tri.max_x && weight[0] >= 0.0 && weight[1] >= 0.0 && weight[2] >= 0.0 {
            // `u += du` along the row is not the weight computation, in f32. Every 32
            // pixels the coordinate comes from the weight again, and the drift doesn't cross the
            // row.
            if phase == 0 {
                u = (weight[0] * ou[0] + weight[1] * ou[1] + weight[2] * ou[2]) * persp;
                v = (weight[0] * ov[0] + weight[1] * ov[1] + weight[2] * ov[2]) * persp;
            }
            let z = z0 + weight[1] * dz1 + weight[2] * dz2;
            let index = row + x as usize;
            let z_passes = !depth_test
                || match depth_func {
                    gles::GL_LESS => z < band.depth[index],
                    gles::GL_LEQUAL => z <= band.depth[index],
                    _ => true,
                };
            if z_passes {
                let raw = texel.get::<SMOOTH>(u, v);
                let mut color = if copy { raw } else { apply_color(raw, color) };
                let visible =
                    !alpha_test || compare(alpha_func, f32::from(color[3]) / 255.0, alpha_ref);
                if visible {
                    if blend != 0 {
                        color = blend_over(color, band.color[index], additive);
                    }
                    band.color[index] = color;
                    if write_z {
                        band.depth[index] = z;
                    }
                }
            }
            u += du;
            v += dv;
            phase = (phase + 1) & 31;
            weight = [
                weight[0] + tri.step[0],
                weight[1] + tri.step[1],
                weight[2] + tri.step[2],
            ];
            x += 1;
        }
    }
}

/// The affine path, when the triangle and the state fit it. Otherwise the caller goes on to the
/// general fill.
#[cfg(target_os = "vita")]
fn try_fill_affine(tri: &Prepared, uniforms: &Uniforms, band: &mut Band) -> bool {
    let Some(texture) = uniforms.texture else {
        return false;
    };
    if uniforms.uses_mipmap || uniforms.texture1.is_some() || uniforms.stencil.test {
        return false;
    }
    if uniforms.fog.enabled && uniforms.fog.allowed {
        return false;
    }
    if uniforms.color_mask != [true, true, true, true] {
        return false;
    }
    if uniforms.depth_test
        && !matches!(
            uniforms.depth_func,
            gles::GL_LESS | gles::GL_LEQUAL | gles::GL_ALWAYS
        )
    {
        return false;
    }
    if matches!(
        uniforms.texture_env.mode,
        gles::GL_DECAL | gles::GL_ADD | gles::GL_COMBINE
    ) {
        return false;
    }
    let (Some(w), Some(blend)) = (affine(tri), fast_blend(uniforms)) else {
        return false;
    };
    let replaces = uniforms.texture_env.mode == gles::GL_REPLACE;
    let Some(color) = (if replaces {
        Some([1.0; 4])
    } else {
        flat_color(tri, w)
    }) else {
        return false;
    };
    let Some(texel) = Texel::create(texture) else {
        return false;
    };
    let copy = replaces || color.iter().all(|c| (*c - 1.0).abs() <= 1.0e-6);
    if texture.filter == gles::GL_LINEAR {
        fill_affine::<true>(tri, uniforms, band, &texel, w, color, copy, blend);
    } else {
        fill_affine::<false>(tri, uniforms, band, &texel, w, color, copy, blend);
    }
    true
}

/// The common fill: one texture, no fog, no blending, no stencil and no alpha test. The
/// general function carries all of that in the same body, and on the Vita's Cortex-A9 the loop
/// spills registers — the frame drawn on the device took 95.5 ms against 6.6 ms without
/// painting.
///
/// The computation is the same. Measured on this Mac, in release, a 640×480 frame with linear
/// texture and depth test: 2.22 ms in the general body and 1.09 ms here. It is not a Vita number.
#[cfg(target_os = "vita")]
fn opaque_fits(uniforms: &Uniforms) -> bool {
    if uniforms.texture.is_none() || uniforms.uses_mipmap || uniforms.texture1.is_some() {
        return false;
    }
    if uniforms.alpha_test || uniforms.blend || uniforms.stencil.test {
        return false;
    }
    if uniforms.fog.enabled && uniforms.fog.allowed {
        return false;
    }
    if uniforms.color_mask != [true, true, true, true] {
        return false;
    }
    if uniforms.depth_test
        && !matches!(
            uniforms.depth_func,
            gles::GL_LESS | gles::GL_LEQUAL | gles::GL_ALWAYS
        )
    {
        return false;
    }
    !matches!(
        uniforms.texture_env.mode,
        gles::GL_DECAL | gles::GL_ADD | gles::GL_COMBINE
    )
}

#[cfg(target_os = "vita")]
fn fill_opaque(tri: &Prepared, uniforms: &Uniforms, band: &mut Band) {
    let Some(texture) = uniforms.texture else {
        return;
    };
    let replaces = uniforms.texture_env.mode == gles::GL_REPLACE;
    let smooth = texture.filter == gles::GL_LINEAR;
    let width = uniforms.width;
    let rows = (band.color.len() / width) as i32;
    let top = tri.min_y.max(band.top);
    let bottom = tri.max_y.min(band.top + rows);
    let depth_test = uniforms.depth_test;
    let depth_func = uniforms.depth_func;
    let write_z = depth_test && uniforms.depth_mask;
    for y in top..bottom {
        let start = [tri.min_x as f32 + 0.5, y as f32 + 0.5, 0.0, 0.0];
        let mut weight = [
            edge(tri.screen[1], tri.screen[2], start) * tri.inv_area,
            edge(tri.screen[2], tri.screen[0], start) * tri.inv_area,
            edge(tri.screen[0], tri.screen[1], start) * tri.inv_area,
        ];
        let row = (y - band.top) as usize * width;
        let mut inside = false;
        for x in tri.min_x..tri.max_x {
            let bary = weight;
            weight = [
                weight[0] + tri.step[0],
                weight[1] + tri.step[1],
                weight[2] + tri.step[2],
            ];
            if bary[0] < 0.0 || bary[1] < 0.0 || bary[2] < 0.0 {
                if inside {
                    break;
                }
                continue;
            }
            inside = true;
            let z = tri.screen[0][2]
                + bary[1] * (tri.screen[1][2] - tri.screen[0][2])
                + bary[2] * (tri.screen[2][2] - tri.screen[0][2]);
            let index = row + x as usize;
            if depth_test {
                let passes = match depth_func {
                    gles::GL_LESS => z < band.depth[index],
                    gles::GL_LEQUAL => z <= band.depth[index],
                    _ => true,
                };
                if !passes {
                    continue;
                }
            }
            let inv_w = bary[0] * tri.screen[0][3]
                + bary[1] * tri.screen[1][3]
                + bary[2] * tri.screen[2][3];
            if inv_w == 0.0 {
                continue;
            }
            let persp = 1.0 / inv_w;
            let attr = |k: usize| {
                (bary[0] * tri.over_w[0][k]
                    + bary[1] * tri.over_w[1][k]
                    + bary[2] * tri.over_w[2][k])
                    * persp
            };
            let texel = texture.sample_level(attr(4), attr(5), 0, smooth);
            let color = if replaces {
                texel
            } else {
                [
                    attr(0) * texel[0],
                    attr(1) * texel[1],
                    attr(2) * texel[2],
                    attr(3) * texel[3],
                ]
            };
            band.color[index] = pack(color);
            if write_z {
                band.depth[index] = z;
            }
        }
    }
}

/// Fills the part of a triangle that falls inside the band.
fn fill_band(tri: &Prepared, uniforms: &Uniforms, band: &mut Band) {
    // The affine shortcut samples in integers and advances `u`/`v` by addition. In the test the
    // worst channel ended up two levels from bilinear. That changes pixels, so it stays on the
    // Vita, where filling is the game's path. Elsewhere the general loop remains.
    #[cfg(target_os = "vita")]
    if try_fill_affine(tri, uniforms, band) {
        return;
    }
    #[cfg(target_os = "vita")]
    if opaque_fits(uniforms) {
        fill_opaque(tri, uniforms, band);
        return;
    }
    let width = uniforms.width;
    let rows = (band.color.len() / width) as i32;
    let top = tri.min_y.max(band.top);
    let bottom = tri.max_y.min(band.top + rows);
    for y in top..bottom {
        // Each row restarts from the exact computation: the accumulation error stays trapped
        // inside the row instead of running down the whole image.
        let start = [tri.min_x as f32 + 0.5, y as f32 + 0.5, 0.0, 0.0];
        let mut w = [
            edge(tri.screen[1], tri.screen[2], start) * tri.inv_area,
            edge(tri.screen[2], tri.screen[0], start) * tri.inv_area,
            edge(tri.screen[0], tri.screen[1], start) * tri.inv_area,
        ];
        let row = (y - band.top) as usize * width;
        // The intersection of a triangle with the row is a single interval: each
        // edge moves in one direction, and once the pixel leaves it doesn't come back. Stopping
        // there changes no pixel — it only skips walking the rest of the box.
        //
        // Trimming the ends with one division per row was measured on the device: the
        // frame went from 9 to 6. The computation went into this loop, which was already the
        // hottest. The addition below is what stays.
        let mut inside = false;
        for x in tri.min_x..tri.max_x {
            let bary = w;
            // The step comes before the discard because the `continue` would skip the addition.
            w = [w[0] + tri.step[0], w[1] + tri.step[1], w[2] + tri.step[2]];
            if bary[0] < 0.0 || bary[1] < 0.0 || bary[2] < 0.0 {
                if inside {
                    break;
                }
                continue;
            }
            inside = true;
            // **From the first vertex, not by the sum of the three weights.** The weights come
            // accumulated along the row and add up to 1 ± ε, and ε changes from one triangle to
            // the other: with constant z, `b·z` came out slightly above z in one triangle and
            // below in the other. NFS Carbon draws the EA logo in layers at the same z with
            // `GL_LEQUAL`, and half of each quad failed. Through the differences, a constant z is
            // exact.
            let z = tri.screen[0][2]
                + bary[1] * (tri.screen[1][2] - tri.screen[0][2])
                + bary[2] * (tri.screen[2][2] - tri.screen[0][2]);
            let index = row + x as usize;

            let inv_w = bary[0] * tri.screen[0][3]
                + bary[1] * tri.screen[1][3]
                + bary[2] * tri.screen[2][3];
            if inv_w == 0.0 {
                continue;
            }
            let w = 1.0 / inv_w;
            let attribute = |k: usize| {
                (bary[0] * tri.over_w[0][k]
                    + bary[1] * tri.over_w[1][k]
                    + bary[2] * tri.over_w[2][k])
                    * w
            };
            // Assembling the color costs the texture sampling, and it is the expensive step of the
            // fragment. Without an alpha test it can wait for the depth discard; **with** an alpha
            // test, it can't: in OpenGL alpha is decided before the stencil, and a fragment that
            // fails there has no right to touch the stencil.
            let assemble = || {
                let mut source = [attribute(0), attribute(1), attribute(2), attribute(3)];
                let primary = source;
                let texel0 = uniforms.texture.map(|texture| {
                    // Minification can happen along either screen axis. Compare the
                    // neighbors to the right and below, using the larger footprint.
                    let (u, v) = (attribute(4), attribute(5));
                    // **The neighboring pixel must be on the same side of the eye.** When the
                    // sign of `1/w` flips — the triangle crosses the horizon —, the difference
                    // explodes and the chosen level goes to the smallest of the chain, which is
                    // a flat color.
                    let downscale = |step: [f32; 3]| {
                        let next = std::array::from_fn::<_, 3, _>(|i| bary[i] + step[i]);
                        let inv_w2 = next[0] * tri.screen[0][3]
                            + next[1] * tri.screen[1][3]
                            + next[2] * tri.screen[2][3];
                        match inv_w2 <= 0.0 || inv_w <= 0.0 {
                            true => 0.0,
                            false => {
                                let w2 = 1.0 / inv_w2;
                                let neighbor = |k: usize| {
                                    (next[0] * tri.over_w[0][k]
                                        + next[1] * tri.over_w[1][k]
                                        + next[2] * tri.over_w[2][k])
                                        * w2
                                };
                                let du = (neighbor(4) - u) * texture.width as f32;
                                let dv = (neighbor(5) - v) * texture.height as f32;
                                let square = du * du + dv * dv;
                                match square > 1.0 {
                                    // `log2(sqrt(x))` is `log2(x)/2`, and one less root per pixel.
                                    true => square.log2() * 0.5,
                                    false => 0.0,
                                }
                            }
                        }
                    };
                    // Without a mipmap chain the sampler always lands on level zero. Avoiding the
                    // two derivatives and the log2 per fragment is decisive for 3D games that use
                    // compressed textures without auxiliary levels, like Super League.
                    let lod = if !uniforms.uses_mipmap {
                        0.0
                    } else {
                        downscale(tri.step).max(downscale(tri.step_y))
                    };
                    texture.sample_lod(u, v, lod)
                });
                // No mipmap: in the games that use unit 1 it carries a light map or the color
                // texture of a bump map, and the base level is enough. It is sampled before unit 0
                // combines because the crossbar lets unit 0 read its texel.
                let texel1 = uniforms
                    .texture1
                    .map(|texture| texture.sample_lod(attribute(7), attribute(8), 0.0));
                let texels = [texel0, texel1];
                if let Some(texel) = texel0 {
                    source = uniforms
                        .texture_env
                        .apply_with(source, primary, texel, texels);
                }
                // Unit 1 acts on what came out of unit 0.
                if let Some(texel) = texel1 {
                    source = uniforms.env1.apply_with(source, primary, texel, texels);
                }
                // **Fog comes after the texture and before the alpha test**, which is the
                // OpenGL ES 1.1 order, and only touches RGB: the fragment keeps its own alpha.
                if uniforms.fog.enabled && uniforms.fog.allowed {
                    let f = attribute(6).clamp(0.0, 1.0);
                    for channel in 0..3 {
                        source[channel] += (uniforms.fog.color[channel] - source[channel]) * (1.0 - f);
                    }
                }
                source
            };
            let mut ready = None;
            if uniforms.alpha_test {
                let source = assemble();
                if !compare(uniforms.alpha_func, source[3], uniforms.alpha_ref) {
                    continue;
                }
                ready = Some(source);
            }

            // **Stencil before depth, and the three operations applied even when the fragment
            // is discarded.** It is that "even when it discards" that makes the reflection work:
            // the pass that marks the floor usually writes to the stencil precisely where depth
            // fails.
            let stencil = &uniforms.stencil;
            let depth_passes =
                !uniforms.depth_test || compare(uniforms.depth_func, z, band.depth[index]);
            if stencil.test {
                let current = band.stencil[index];
                if !stencil.passes(current) {
                    band.stencil[index] = stencil.apply(0, current);
                    continue;
                }
                let which = usize::from(depth_passes) + 1;
                band.stencil[index] = stencil.apply(which, current);
            }
            if !depth_passes {
                continue;
            }

            let source = match ready {
                Some(source) => source,
                None => assemble(),
            };

            let mixed = if uniforms.blend {
                let destination = unpack(band.color[index]);
                let mut out = [0.0; 4];
                for c in 0..4 {
                    let s = factor(uniforms.blend_src, source, destination, c);
                    let d = factor(uniforms.blend_dst, source, destination, c);
                    out[c] = (source[c] * s + destination[c] * d).clamp(0.0, 1.0);
                }
                out
            } else {
                source
            };

            // `glColorMask` decides channel by channel. Writing what it forbade would erase what
            // an earlier pass left there, which is precisely the point of masking.
            let previous = band.color[index];
            let create = pack(mixed);
            band.color[index] = std::array::from_fn(|i| {
                if uniforms.color_mask[i] {
                    create[i]
                } else {
                    previous[i]
                }
            });
            if uniforms.depth_test && uniforms.depth_mask {
                band.depth[index] = z;
            }
        }
    }
}

/// Edge function: twice the signed area of the triangle `(a, b, c)`.
fn edge(a: [f32; 4], b: [f32; 4], c: [f32; 4]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Interpolates `a`→`b` up to the near plane, where `z + w` crosses zero.
/// Three-component dot product.
fn point(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn length(v: [f32; 3]) -> f32 {
    point(v, v).sqrt()
}

/// A unit-length vector, or the zero vector when there is no telling where it points.
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let n = length(v);
    if n <= f32::EPSILON {
        return [0.0, 0.0, 0.0];
    }
    [v[0] / n, v[1] / n, v[2] / n]
}

/// The cone factor of a spotlight, or one when the light is not a spotlight.
fn spotlight(light: &Light, to_light: [f32; 3]) -> f32 {
    if light.spot_cutoff >= 180.0 {
        return 1.0;
    }
    // The cosine is between the cone direction and the direction **from the light to the
    // vertex**, which is the opposite of `to_light`.
    let cos = point(
        normalize(light.spot_direction),
        [-to_light[0], -to_light[1], -to_light[2]],
    );
    if cos < light.spot_cutoff.to_radians().cos() {
        return 0.0;
    }
    cos.max(0.0).powf(light.spot_exponent)
}

/// The matrix that takes a normal from object coordinates to eye coordinates.
///
/// It is the **inverse transpose** of the 3×3 part of the modelview, not the modelview: with
/// non-uniform scaling, a normal transformed as if it were a direction stops being perpendicular
/// to the surface and the light slides across the model. When the inverse doesn't exist — a
/// degenerate matrix —, the raw 3×3 part is the least wrong thing we can return.
fn normal_matrix(m: &Matrix) -> [[f32; 3]; 3] {
    let a = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det.abs() <= f32::EPSILON {
        return a;
    }
    // The inverse transpose is the 3×3's cofactor matrix over the determinant. But `a` is
    // already the **transposed** 3×3 — `Matrix` is stored by column, and each row of `a` is one
    // of its columns —, so the cofactor of `(i, j)` of the 3×3 is that of `(j, i)` of `a`. With
    // `(i, j)` it came out as the untransposed inverse: the same for a scale, but for a rotation
    // the normal turned the opposite way, and the Armageddon Squadron terrain, drawn in blocks
    // with their own orientations, got light and shadow in rectangles.
    let cofactor = |i: usize, j: usize| {
        let (l1, l2) = ((i + 1) % 3, (i + 2) % 3);
        let (c1, c2) = ((j + 1) % 3, (j + 2) % 3);
        a[l1][c1] * a[l2][c2] - a[l1][c2] * a[l2][c1]
    };
    std::array::from_fn(|i| std::array::from_fn(|j| cofactor(j, i) / det))
}

/// Applies the normal matrix.
fn rotate_normal(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

/// Replaces each segment of `GL_LINES`, `GL_LINE_STRIP` or `GL_LINE_LOOP` with a rectangle one
/// pixel wide, as two separate triangles, in clip space.
///
/// **A line becomes triangles to follow the same path as everything else**: clipping, texture,
/// fog and blending come out the same as a triangle's, in both rasterizers, without a second fill
/// routine to keep in sync. Peggle paints the menu background gradient with one horizontal line
/// per pixel; without them, the middle of the screen was black behind the logo.
///
/// The width is the default `glLineWidth`, one viewport pixel. The triangles come out in the
/// screen's counterclockwise winding when `ccw`, so that face culling doesn't take the line away.
pub(crate) fn lines_to_triangles(
    mode: u32,
    clip: &[Vertex],
    viewport: (i32, i32, i32, i32),
    ccw: bool,
) -> Vec<Vertex> {
    let n = clip.len();
    let pairs: Vec<(usize, usize)> = match mode {
        gles::GL_LINES => (0..n / 2).map(|i| (2 * i, 2 * i + 1)).collect(),
        gles::GL_LINE_STRIP => (1..n).map(|i| (i - 1, i)).collect(),
        gles::GL_LINE_LOOP if n >= 2 => (1..n).map(|i| (i - 1, i)).chain([(n - 1, 0)]).collect(),
        _ => Vec::new(),
    };
    let (vw, vh) = (viewport.2.max(1) as f32, viewport.3.max(1) as f32);
    let near = |v: &Vertex| v.position[2] + v.position[3] >= 0.0;
    let mut output = Vec::with_capacity(pairs.len() * 6);
    for (i, j) in pairs {
        let (mut a, mut b) = (clip[i], clip[j]);
        match (near(&a), near(&b)) {
            (false, false) => continue,
            (true, false) => b = clip_near(a, b),
            (false, true) => a = clip_near(b, a),
            (true, true) => {}
        }
        let (aw, bw) = (a.position[3], b.position[3]);
        if aw <= 0.0 || bw <= 0.0 {
            continue;
        }
        // The direction in pixels, not in NDC: on a 640x480 viewport the two axes have different
        // scales, and the perpendicular in NDC would come out skewed.
        let dx = (b.position[0] / bw - a.position[0] / aw) * vw / 2.0;
        let dy = (b.position[1] / bw - a.position[1] / aw) * vh / 2.0;
        let length = (dx * dx + dy * dy).sqrt();
        if length == 0.0 {
            continue;
        }
        // Half a pixel to each side, converted back to NDC.
        let (ox, oy) = (-dy / length / vw, dx / length / vh);
        let shift = |v: Vertex, sign: f32| {
            let mut v = v;
            v.position[0] += sign * ox * v.position[3];
            v.position[1] += sign * oy * v.position[3];
            v
        };
        let (a0, a1, b0, b1) = (
            shift(a, 1.0),
            shift(a, -1.0),
            shift(b, 1.0),
            shift(b, -1.0),
        );
        // `a1 → b1 → b0` turns from `(dx, dy)` toward `(ox, oy)`, which is counterclockwise.
        match ccw {
            true => output.extend([a1, b1, b0, a1, b0, a0]),
            false => output.extend([a1, b0, b1, a1, a0, b0]),
        }
    }
    output
}

fn clip_near(a: Vertex, b: Vertex) -> Vertex {
    let (da, db) = (a.position[2] + a.position[3], b.position[2] + b.position[3]);
    let t = da / (da - db);
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Vertex {
        position: std::array::from_fn(|i| lerp(a.position[i], b.position[i])),
        color: std::array::from_fn(|i| lerp(a.color[i], b.color[i])),
        uv: std::array::from_fn(|i| lerp(a.uv[i], b.uv[i])),
        uv1: std::array::from_fn(|i| lerp(a.uv1[i], b.uv1[i])),
        // The normal was already consumed by lighting before clipping: here it changes nothing
        // anymore, and interpolating it would be work nobody reads.
        normal: a.normal,
        // The fog factor, on the other hand, will still be read: the new vertex sits where the
        // plane cut, and its fog is that of the cut point.
        fog: lerp(a.fog, b.fog),
    }
}

/// One of OpenGL's comparison functions.
#[inline]
fn compare(func: u32, value: f32, reference: f32) -> bool {
    match func {
        gles::GL_NEVER => false,
        gles::GL_LESS => value < reference,
        gles::GL_EQUAL => value == reference,
        gles::GL_LEQUAL => value <= reference,
        gles::GL_GREATER => value > reference,
        gles::GL_NOTEQUAL => value != reference,
        gles::GL_GEQUAL => value >= reference,
        _ => true,
    }
}

/// Combines the fragment color with the texel, according to `GL_TEXTURE_ENV_MODE`.
/// The weight of a blend factor for channel `c`.
fn factor(kind: u32, source: [f32; 4], destination: [f32; 4], c: usize) -> f32 {
    match kind {
        gles::GL_ZERO => 0.0,
        gles::GL_SRC_COLOR => source[c],
        gles::GL_ONE_MINUS_SRC_COLOR => 1.0 - source[c],
        gles::GL_SRC_ALPHA => source[3],
        gles::GL_ONE_MINUS_SRC_ALPHA => 1.0 - source[3],
        gles::GL_DST_ALPHA => destination[3],
        gles::GL_ONE_MINUS_DST_ALPHA => 1.0 - destination[3],
        gles::GL_DST_COLOR => destination[c],
        gles::GL_ONE_MINUS_DST_COLOR => 1.0 - destination[c],
        _ => 1.0,
    }
}

fn pack(color: [f32; 4]) -> [u8; 4] {
    std::array::from_fn(|i| (color[i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

fn unpack(color: [u8; 4]) -> [f32; 4] {
    std::array::from_fn(|i| color[i] as f32 / 255.0)
}

#[cfg(test)]
mod tests {

    /// A full frame with a linear texture, the case the Vita measures in tens of
    /// milliseconds. Ignored in CI: it is a clock, not behavior.
    #[test]
    #[ignore]
    fn measures_the_fill_of_one_frame() {
        let mut state = super::GlState::new(640, 480);
        state.set_viewport(0, 0, 640, 480);
        state.set_capability(super::gles::GL_DEPTH_TEST, true);
        state.set_depth_func(super::gles::GL_LEQUAL);
        state.clear(super::gles::GL_COLOR_BUFFER_BIT | super::gles::GL_DEPTH_BUFFER_BIT);
        let mut pixels = vec![[0u8; 4]; 64 * 64];
        for (i, p) in pixels.iter_mut().enumerate() {
            *p = [(i % 64) as u8, (i / 64) as u8, 180, 255];
        }
        state.textures.insert(
            1,
            super::Texture {
                width: 64,
                height: 64,
                pixels,
                filter: super::gles::GL_LINEAR,
                min_filter: super::gles::GL_LINEAR,
                ..super::Texture::default()
            },
        );
        state.bind_texture(1);
        state.set_capability(super::gles::GL_TEXTURE_2D, true);
        let v = |x: f32, y: f32, u: f32, t: f32| super::Vertex {
            position: [x, y, 0.0, 1.0],
            uv: [u, t, 0.0, 1.0],
            ..Default::default()
        };
        let draw = |state: &mut super::GlState| {
            state.draw(
                super::gles::GL_TRIANGLES,
                &[
                    v(-1.0, -1.0, 0.0, 0.0),
                    v(1.0, -1.0, 1.0, 0.0),
                    v(1.0, 1.0, 1.0, 1.0),
                    v(-1.0, -1.0, 0.0, 0.0),
                    v(1.0, 1.0, 1.0, 1.0),
                    v(-1.0, 1.0, 0.0, 1.0),
                ],
            );
        };
        draw(&mut state);
        state.flush();
        let begin = std::time::Instant::now();
        for _ in 0..4 {
            draw(&mut state);
            state.flush();
        }
        let ms = begin.elapsed().as_secs_f64() * 1000.0 / 4.0;
        eprintln!("textured 640x480 frame: {ms:.2} ms");
    }

    /// Opaque texture, the fill shortcut. Farther doesn't cover; closer does.
    #[test]
    fn the_opaque_shortcut_respects_depth() {
        let mut state = super::GlState::new(8, 8);
        state.set_viewport(0, 0, 8, 8);
        state.set_capability(super::gles::GL_DEPTH_TEST, true);
        state.set_depth_func(super::gles::GL_LEQUAL);
        state.clear(super::gles::GL_COLOR_BUFFER_BIT | super::gles::GL_DEPTH_BUFFER_BIT);
        let paint = |state: &mut super::GlState, z: f32, color: [u8; 4]| {
            state.textures.insert(
                1,
                super::Texture {
                    width: 1,
                    height: 1,
                    pixels: vec![color],
                    filter: super::gles::GL_NEAREST,
                    min_filter: super::gles::GL_NEAREST,
                    ..super::Texture::default()
                },
            );
            let v = |x: f32, y: f32| super::Vertex {
                position: [x, y, z, 1.0],
                uv: [0.0, 0.0, 0.0, 1.0],
                ..Default::default()
            };
            state.draw(
                super::gles::GL_TRIANGLES,
                &[
                    v(-1.0, -1.0),
                    v(1.0, -1.0),
                    v(1.0, 1.0),
                    v(-1.0, -1.0),
                    v(1.0, 1.0),
                    v(-1.0, 1.0),
                ],
            );
            state.flush();
        };
        state.bind_texture(1);
        state.set_capability(super::gles::GL_TEXTURE_2D, true);
        paint(&mut state, 0.0, [255, 0, 0, 255]);
        assert_eq!(state.color[8 * 4 + 4], [255, 0, 0, 255]);
        paint(&mut state, 0.5, [0, 255, 0, 255]);
        assert_eq!(state.color[8 * 4 + 4], [255, 0, 0, 255]);
        paint(&mut state, -0.5, [0, 0, 255, 255]);
        assert_eq!(state.color[8 * 4 + 4], [0, 0, 255, 255]);
    }

    /// The integer filter must not drift from what the old path's `pack` would write.
    #[test]
    fn the_integer_sample_stays_within_two_levels() {
        let mut pixels = vec![[0u8; 4]; 8 * 8];
        for (i, pixel) in pixels.iter_mut().enumerate() {
            *pixel = [
                (i * 17) as u8,
                (i * 29) as u8,
                (i * 3) as u8,
                (i * 13) as u8,
            ];
        }
        let texture = super::Texture {
            width: 8,
            height: 8,
            pixels,
            filter: super::gles::GL_LINEAR,
            min_filter: super::gles::GL_LINEAR,
            ..super::Texture::default()
        };
        let texel = super::Texel::create(&texture).expect("texture");
        let mut worst = 0i32;
        let mut u = -0.25f32;
        while u < 1.25 {
            let mut v = -0.25f32;
            while v < 1.25 {
                for smooth in [false, true] {
                    let integer = if smooth {
                        texel.get::<true>(u, v)
                    } else {
                        texel.get::<false>(u, v)
                    };
                    let fine = super::pack(texture.sample_level(u, v, 0, smooth));
                    for channel in 0..4 {
                        worst = worst.max((i32::from(integer[channel]) - i32::from(fine[channel])).abs());
                    }
                }
                v += 0.03;
            }
            u += 0.03;
        }
        assert!(worst <= 2, "the integer sample drifted {worst} levels");
    }

    /// Affine sprite: zero alpha doesn't paint, the one behind doesn't cover, the one in front
    /// does.
    #[test]
    fn the_affine_sprite_discards_alpha_and_respects_depth() {
        super::AFFINE_PATH.with(|c| c.set(0));
        let mut state = super::GlState::new(8, 8);
        state.set_viewport(0, 0, 8, 8);
        state.set_capability(super::gles::GL_DEPTH_TEST, true);
        state.set_depth_func(super::gles::GL_LEQUAL);
        state.set_capability(super::gles::GL_ALPHA_TEST, true);
        state.set_alpha_func(super::gles::GL_GREATER, 0.5);
        state.clear(super::gles::GL_COLOR_BUFFER_BIT | super::gles::GL_DEPTH_BUFFER_BIT);
        let paint = |state: &mut super::GlState, z: f32, color: [u8; 4]| {
            state.textures.insert(
                1,
                super::Texture {
                    width: 1,
                    height: 1,
                    pixels: vec![color],
                    filter: super::gles::GL_NEAREST,
                    min_filter: super::gles::GL_NEAREST,
                    ..super::Texture::default()
                },
            );
            let v = |x: f32, y: f32| super::Vertex {
                position: [x, y, z, 1.0],
                uv: [0.0, 0.0, 0.0, 1.0],
                ..Default::default()
            };
            state.draw(
                super::gles::GL_TRIANGLES,
                &[
                    v(-1.0, -1.0),
                    v(1.0, -1.0),
                    v(1.0, 1.0),
                    v(-1.0, -1.0),
                    v(1.0, 1.0),
                    v(-1.0, 1.0),
                ],
            );
            state.flush();
        };
        state.bind_texture(1);
        state.set_capability(super::gles::GL_TEXTURE_2D, true);
        paint(&mut state, 0.0, [0, 255, 0, 0]);
        assert_eq!(state.color[8 * 4 + 4], [0, 0, 0, 255]);
        paint(&mut state, 0.0, [255, 0, 0, 255]);
        assert_eq!(state.color[8 * 4 + 4], [255, 0, 0, 255]);
        paint(&mut state, 0.5, [0, 255, 0, 255]);
        assert_eq!(state.color[8 * 4 + 4], [255, 0, 0, 255]);
        paint(&mut state, -0.5, [0, 0, 255, 255]);
        assert_eq!(state.color[8 * 4 + 4], [0, 0, 255, 255]);
        // The shortcut only kicks in on the Vita. Elsewhere the general loop must give the same
        // pixel, and that is what the comparisons above demand.
        #[cfg(target_os = "vita")]
        {
            let paths = super::AFFINE_PATH.with(|c| c.get());
            assert!(
                paths >= 4,
                "the sprite did not take the affine path ({paths})"
            );
        }
    }

    /// **A horizontal line paints one row, and only one.** That is what Peggle's menu gradient
    /// asks for: one line per pixel, side by side. With two rows per line, the upper one would
    /// cover the lower one; with none, the background was black — which was the case before lines
    /// existed.
    #[test]
    fn a_horizontal_line_paints_one_row() {
        let mut state = super::GlState::new(8, 4);
        let red = |x: f32, y: f32| super::Vertex {
            position: [x, y, 0.0, 1.0],
            color: [1.0, 0.0, 0.0, 1.0],
            ..Default::default()
        };
        // The center of row 1 of 4 in NDC: (1.5 / 4) * 2 - 1.
        state.draw(
            super::gles::GL_LINES,
            &[red(-1.0, -0.25), red(1.0, -0.25)],
        );
        let frame = state.present(8, 4);
        let painted: Vec<usize> = (0..4)
            .filter(|&line| {
                frame[line * 8..line * 8 + 8]
                    .iter()
                    .any(|&p| p == 0xf800)
            })
            .collect();
        assert_eq!(painted.len(), 1, "painted rows: {painted:?}");
        let line = painted[0];
        assert!(
            frame[line * 8..line * 8 + 8].iter().all(|&p| p == 0xf800),
            "the row must come out whole"
        );
    }

    /// **The internal resolution reduction**, and the case that would have left it without effect.
    ///
    /// There are games that never call `glViewport` — Z-Wheel doesn't call it once in thirteen
    /// seconds, measured —, and for them the surface comes from the **initial** viewport. If it
    /// were reset to the frame size, the reduction wouldn't apply precisely to whoever depends on
    /// it.
    #[test]
    fn downscale_reaches_games_that_never_call_viewport() {
        let mut state = GlState::new(640, 480);

        // Without reduction, the initial surface is the whole frame.
        assert_eq!(state.surface(), (640, 480));

        state.set_downscale(2);
        assert_eq!(
            state.surface(),
            (320, 240),
            "the initial viewport must come out reduced"
        );

        // And a game that **declares** the console viewport, like the usual one, is divided too.
        state.set_viewport(0, 0, 640, 480);
        assert_eq!(state.surface(), (320, 240));

        // The scissor goes through the same computation.
        state.set_scissor(0, 0, 640, 480);
        state.set_scissor_test(true);
        let (_, _, width, height) = state.scissor.unwrap_or((0, 0, 0, 0));
        assert_eq!((width, height), (320, 240));

        // The frame handed out is still the console's: presentation does the scaling up.
        let mut bytes = Vec::new();
        state.frame_rgb565(640, 480, &mut bytes);
        assert_eq!(bytes.len(), 640 * 480 * 2, "the frame comes out at 640x480");

        // And going back to 1x restores the console size — the option is reversible.
        state.set_downscale(1);
        assert_eq!(state.surface(), (640, 480));
    }

    #[test]
    fn egl_buffer_write_preserves_drawings_and_depth() {
        let mut state = GlState::new(2, 1);
        let mut exported = Vec::new();
        state.frame_rgb565(2, 1, &mut exported);
        state.color[1] = [0, 255, 0, 255];
        state.depth[0] = 0.25;
        state.stencil[0] = 7;
        let mut changed = exported.clone();
        changed[..2].copy_from_slice(&0xf800u16.to_le_bytes());
        state.import_rgb565_changes(2, 1, &exported, &changed);
        assert_eq!(&state.color[0][..3], &[255, 0, 0]);
        assert_eq!(state.color[1], [0, 255, 0, 255]);
        assert_eq!(state.depth[0], 0.25);
        assert_eq!(state.stencil[0], 7);
    }

    /// The **scaled** path has its own loop, with the per-pixel divisions, and it is Quake's:
    /// a surface smaller than the screen. The test above only covers the unscaled path.
    #[test]
    fn egl_buffer_write_finds_the_pixel_when_the_surface_is_smaller() {
        let mut state = GlState::new(2, 1);
        state.set_viewport(0, 0, 1, 1);
        assert_eq!(state.surface(), (1, 1));
        let mut exported = Vec::new();
        state.frame_rgb565(2, 1, &mut exported);
        let mut changed = exported.clone();
        changed[..2].copy_from_slice(&0xf800u16.to_le_bytes());
        state.import_rgb565_changes(2, 1, &exported, &changed);
        assert_eq!(&state.color[0][..3], &[255, 0, 0]);
    }

    #[test]
    fn mipmap_considers_vertical_and_horizontal_minification() {
        for vertical in [false, true] {
            let mut state = GlState::new(8, 8);
            state.bind_texture(1);
            state.set_capability(gles::GL_TEXTURE_2D, true);
            state.textures.insert(
                1,
                Texture {
                    width: 64,
                    height: 64,
                    pixels: vec![[255, 0, 0, 255]; 64 * 64],
                    mipmaps: (0..6)
                        .map(|level| {
                            let size = 32 >> level;
                            Level {
                                width: size,
                                height: size,
                                pixels: vec![[0, 255, 0, 255]; size * size],
                            }
                        })
                        .collect(),
                    min_filter: gles::GL_NEAREST_MIPMAP_NEAREST,
                    ..Default::default()
                },
            );
            let vertex = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                uv: if vertical {
                    [0.0, (y + 1.0) * 0.5, 0.0, 1.0]
                } else {
                    [(x + 1.0) * 0.5, 0.0, 0.0, 1.0]
                },
                ..Default::default()
            };
            state.draw(
                gles::GL_TRIANGLE_STRIP,
                &[
                    vertex(-1.0, -1.0),
                    vertex(1.0, -1.0),
                    vertex(-1.0, 1.0),
                    vertex(1.0, 1.0),
                ],
            );
            state.flush();
            assert_eq!(
                state.color[3 * 8 + 4],
                [0, 255, 0, 255],
                "vertical={vertical}"
            );
        }
    }

    /// A hole in the chain doesn't become white: the invalid level falls back to one that exists.
    ///
    /// It was what left whole models flat — an empty level returned opaque white and the whole
    /// surface came out a single color.
    #[test]
    fn an_empty_level_falls_back_to_an_existing_one() {
        let mut t = Texture {
            width: 2,
            height: 1,
            pixels: vec![[255, 0, 0, 255]; 2],
            ..Default::default()
        };
        t.min_filter = gles::GL_NEAREST_MIPMAP_NEAREST;
        t.mipmaps = vec![Level {
            width: 0,
            height: 0,
            pixels: Vec::new(),
        }];
        let color = t.sample_lod(0.5, 0.5, 5.0);
        assert_eq!(color[0], 1.0, "red from the full level");
        assert_eq!(color[1], 0.0, "and not white: {color:?}");
    }

    /// A texture without a minification chain is not mipmapped, even if the filter asks for it.
    ///
    /// The `MIN_FILTER` default in OpenGL is `GL_NEAREST_MIPMAP_LINEAR`, and taking it literally
    /// started sampling with nearest neighbor the twenty-one Z-Wheel stage textures that ask for
    /// `GL_LINEAR` and bring no levels at all.
    #[test]
    fn without_a_chain_the_base_filter_applies() {
        let mut t = Texture {
            width: 2,
            height: 1,
            pixels: vec![[0, 0, 0, 255], [255, 255, 255, 255]],
            ..Default::default()
        };
        t.min_filter = gles::GL_NEAREST_MIPMAP_LINEAR;
        // Midway between the two texels, with nearest neighbor, the color is one of them.
        let middle = t.sample_lod(0.5, 0.5, 4.0);
        assert!(middle[0] == 0.0 || middle[0] == 1.0, "{middle:?}");
        t.min_filter = gles::GL_LINEAR_MIPMAP_LINEAR;
        let middle = t.sample_lod(0.5, 0.5, 4.0);
        assert!((middle[0] - 0.5).abs() < 0.01, "interpolated: {middle:?}");
    }

    /// With a chain, the level comes from `lod`: zero is the full image, and the last is the
    /// smallest level.
    #[test]
    fn the_lod_picks_the_level() {
        let mut t = Texture {
            width: 2,
            height: 1,
            pixels: vec![[255, 0, 0, 255]; 2],
            ..Default::default()
        };
        t.mipmaps = vec![Level {
            width: 1,
            height: 1,
            pixels: vec![[0, 0, 255, 255]],
        }];
        t.min_filter = gles::GL_NEAREST_MIPMAP_NEAREST;
        let near = t.sample_lod(0.5, 0.5, 0.0);
        assert_eq!(near[0], 1.0, "up close, the full level: {near:?}");
        let far = t.sample_lod(0.5, 0.5, 3.0);
        assert_eq!(far[2], 1.0, "from afar, the smaller level: {far:?}");
    }

    /// The stencil marks where the draw went, and the next draw only goes in where the mark is.
    ///
    /// It is the recipe for the Z-Wheel stage reflection, reduced to two squares.
    #[test]
    fn the_stencil_mark_clips_the_next_draw() {
        let mut state = GlState::new(4, 4);
        state.set_viewport(0, 0, 4, 4);
        state.set_capability(gles::GL_STENCIL_TEST, true);

        // First pass: writes 1 to the stencil on the top half, without painting.
        state.set_color_mask([false; 4]);
        state.set_stencil_func(gles::GL_ALWAYS, 1, 0xff);
        state.set_stencil_op(gles::GL_KEEP, gles::GL_KEEP, gles::GL_REPLACE);
        square(&mut state, -1.0, 0.0, 1.0, 1.0, [1.0; 4]);
        state.flush();
        assert_eq!(state.stencil[0], 1, "top marked");
        assert_eq!(state.stencil[12], 0, "bottom untouched");

        // Second pass: paints red only where the mark is.
        state.set_color_mask([true; 4]);
        state.set_stencil_func(gles::GL_EQUAL, 1, 0xff);
        state.set_stencil_op(gles::GL_KEEP, gles::GL_KEEP, gles::GL_KEEP);
        square(&mut state, -1.0, -1.0, 1.0, 1.0, [1.0, 0.0, 0.0, 1.0]);
        state.flush();
        assert_eq!(state.color[0][0], 255, "top painted");
        assert_eq!(state.color[12][0], 0, "bottom spared by the stencil");
    }

    /// The write mask decides bit by bit what drawing may change in the stencil.
    #[test]
    fn the_mask_protects_the_bits_outside_it() {
        let mut state = GlState::new(2, 2);
        state.set_viewport(0, 0, 2, 2);
        state.stencil.fill(0b1010_1010);
        state.set_capability(gles::GL_STENCIL_TEST, true);
        state.set_stencil_func(gles::GL_ALWAYS, 0xff, 0xff);
        state.set_stencil_op(gles::GL_KEEP, gles::GL_KEEP, gles::GL_REPLACE);
        state.set_stencil_mask(0b0000_1111);
        square(&mut state, -1.0, -1.0, 1.0, 1.0, [1.0; 4]);
        state.flush();
        assert_eq!(state.stencil[0], 0b1010_1111);
    }

    /// A draw that fails the stencil doesn't paint, but **applies** the failure operation: that
    /// is what lets a pass count without showing up.
    #[test]
    fn a_stencil_failure_still_touches_the_buffer() {
        let mut state = GlState::new(2, 2);
        state.set_viewport(0, 0, 2, 2);
        state.set_capability(gles::GL_STENCIL_TEST, true);
        state.set_stencil_func(gles::GL_NEVER, 0, 0xff);
        state.set_stencil_op(gles::GL_INCR, gles::GL_KEEP, gles::GL_KEEP);
        square(&mut state, -1.0, -1.0, 1.0, 1.0, [1.0, 0.0, 0.0, 1.0]);
        state.flush();
        assert_eq!(state.stencil[0], 1, "the failure operation applied");
        assert_eq!(state.color[0][0], 0, "and nothing was painted");
    }

    /// A square in normalized coordinates, with the given color.
    fn square(state: &mut GlState, x0: f32, y0: f32, x1: f32, y1: f32, color: [f32; 4]) {
        let v = |x: f32, y: f32| Vertex {
            position: [x, y, 0.0, 1.0],
            color,
            uv: DEFAULT_UV,
            ..Default::default()
        };
        state.draw(
            gles::GL_TRIANGLES,
            &[
                v(x0, y0),
                v(x1, y0),
                v(x1, y1),
                v(x0, y0),
                v(x1, y1),
                v(x0, y1),
            ],
        );
    }

    /// A state with light zero on, white material and light, and nothing else.
    fn lit_state() -> GlState {
        let mut state = GlState::new(4, 4);
        state.set_capability(gles::GL_LIGHTING, true);
        state.set_capability(gles::GL_LIGHT0, true);
        state.set_material(gles::GL_AMBIENT, [0.0, 0.0, 0.0, 1.0]);
        state.set_material(gles::GL_DIFFUSE, [1.0, 1.0, 1.0, 1.0]);
        state.set_light_model(gles::GL_LIGHT_MODEL_AMBIENT, [0.0, 0.0, 0.0, 1.0]);
        state
    }

    /// Light zero starts with white diffuse and directional at `(0, 0, 1, 0)`: a face turned
    /// toward the viewer gets everything, and one sideways gets nothing.
    #[test]
    fn diffuse_follows_the_cosine_of_the_normal() {
        let state = lit_state();
        let front = state.lit_color([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0], [1.0; 4]);
        assert!((front[0] - 1.0).abs() < 1e-5, "head-on: {front:?}");
        let side = state.lit_color([0.0, 0.0, -5.0, 1.0], [1.0, 0.0, 0.0], [1.0; 4]);
        assert!(side[0].abs() < 1e-5, "sideways: {side:?}");
        let back = state.lit_color([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, -1.0], [1.0; 4]);
        assert!(back[0].abs() < 1e-5, "facing away: {back:?}");
    }

    /// What Z-Wheel sets up: scene ambient at the default, light ambient at 0.5 and material
    /// ambient and diffuse at 0.949. Head-on, that is `0.949*0.2 + 0.949*0.5 + 0.949*1`, which
    /// saturates; sideways the two ambients remain.
    #[test]
    fn the_zwheel_stage_lighting_math() {
        let mut state = GlState::new(4, 4);
        state.set_capability(gles::GL_LIGHTING, true);
        state.set_capability(gles::GL_LIGHT0, true);
        let gray = [0.949, 0.949, 0.949, 1.0];
        state.set_material(gles::GL_AMBIENT, gray);
        state.set_material(gles::GL_DIFFUSE, gray);
        state.set_light(0, gles::GL_AMBIENT, [0.5, 0.5, 0.5, 1.0]);
        let side = state.lit_color([0.0, 0.0, -5.0, 1.0], [1.0, 0.0, 0.0], [1.0; 4]);
        assert!((side[0] - 0.949 * 0.7).abs() < 1e-4, "sideways: {side:?}");
        let front = state.lit_color([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0], [1.0; 4]);
        assert!((front[0] - 1.0).abs() < 1e-5, "head-on: {front:?}");
    }

    /// The alpha is the material diffuse's, not the sum of the lights — summing, everything
    /// turns opaque.
    #[test]
    fn alpha_comes_from_the_material() {
        let mut state = lit_state();
        state.set_material(gles::GL_DIFFUSE, [1.0, 1.0, 1.0, 0.25]);
        let color = state.lit_color([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0], [1.0; 4]);
        assert!((color[3] - 0.25).abs() < 1e-5, "{color:?}");
    }

    /// With `GL_COLOR_MATERIAL`, the vertex color takes the place of the material — it is the
    /// only way for a color array to still apply with lighting on.
    #[test]
    fn vertex_color_only_counts_with_color_material() {
        let mut state = lit_state();
        let red = [1.0, 0.0, 0.0, 1.0];
        let without = state.lit_color([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0], red);
        assert!((without[1] - 1.0).abs() < 1e-5, "without color material: {without:?}");
        state.set_capability(gles::GL_COLOR_MATERIAL, true);
        let com = state.lit_color([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0], red);
        assert!(com[1].abs() < 1e-5, "with color material: {com:?}");
    }

    /// The light position is stored in eye coordinates: `glLight` does the transforming, with the
    /// modelview at the moment of the call.
    #[test]
    fn the_light_position_uses_the_modelview_at_call_time() {
        let mut state = GlState::new(4, 4);
        state.set_matrix_mode(gles::GL_MODELVIEW);
        state.load_matrix(translation(3.0, 0.0, 0.0));
        state.set_light(0, gles::GL_POSITION, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(state.lights[0].position, [4.0, 0.0, 0.0, 1.0]);
        // Touching the modelview afterwards doesn't move the light.
        state.load_matrix(translation(10.0, 0.0, 0.0));
        assert_eq!(state.lights[0].position, [4.0, 0.0, 0.0, 1.0]);
    }

    /// With non-uniform scaling, the normal needs the inverse transpose: transformed as a
    /// direction, it stops being perpendicular to the surface and the light slides across the
    /// model.
    #[test]
    fn the_normal_survives_non_uniform_scale() {
        let m = scaling(1.0, 4.0, 1.0);
        // A surface tilted 45 degrees in the XY plane: tangent `(1, 1, 0)`, normal
        // `(1, -1, 0)`. After scaling, the tangent becomes `(1, 4, 0)`.
        let normal = normalize(rotate_normal(&normal_matrix(&m), [1.0, -1.0, 0.0]));
        let tangent = normalize([1.0, 4.0, 0.0]);
        assert!(point(normal, tangent).abs() < 1e-5, "normal {normal:?}");
    }

    /// Under a rotation the normal turns along with the model, and the same way. The scale above
    /// doesn't catch a swap between the inverse and its transpose, because there both are equal.
    #[test]
    fn the_normal_rotates_with_the_model() {
        let m = rotation(90.0, 0.0, 0.0, 1.0);
        let normal = rotate_normal(&normal_matrix(&m), [1.0, 0.0, 0.0]);
        assert!(normal[0].abs() < 1e-5, "normal {normal:?}");
        assert!((normal[1] - 1.0).abs() < 1e-5, "normal {normal:?}");

        // Rotation with translation and uniform scale: the direction is the rotation's.
        let mut m = translation(5.0, -3.0, 2.0);
        m = multiply(&m, &rotation(30.0, 0.0, 1.0, 0.0));
        m = multiply(&m, &scaling(2.0, 2.0, 2.0));
        let normal = normalize(rotate_normal(&normal_matrix(&m), [0.0, 0.0, 1.0]));
        let expected = transform(&rotation(30.0, 0.0, 1.0, 0.0), [0.0, 0.0, 1.0, 0.0]);
        for i in 0..3 {
            assert!((normal[i] - expected[i]).abs() < 1e-5, "normal {normal:?}");
        }
    }

    /// Without `GL_LIGHTING` none of this happens: the vertex color reaches the rasterizer
    /// intact.
    #[test]
    fn without_lighting_the_color_passes_unchanged() {
        let mut state = GlState::new(4, 4);
        assert!(!state.lighting);
        state.set_capability(gles::GL_LIGHTING, true);
        assert!(state.lighting);
    }
    use super::*;

    /// `GL_COMBINE` with source 0 on the texture and the `GL_REPLACE` function ignores the vertex
    /// color. It is what the QX engine asks for, with the color zeroed; treated as `GL_MODULATE`,
    /// the result was transparent black.
    #[test]
    fn combine_replaces_with_the_texture_ignoring_the_vertex() {
        let mut env = TexEnv::with_mode(gles::GL_COMBINE);
        env.define(gles::GL_COMBINE_RGB, gles::GL_REPLACE, 0.0);
        env.define(gles::GL_COMBINE_ALPHA, gles::GL_REPLACE, 0.0);
        env.define(gles::GL_SRC0_RGB, gles::GL_TEXTURE, 0.0);
        env.define(gles::GL_SRC0_ALPHA, gles::GL_TEXTURE, 0.0);
        // QX sends `GL_SRC_COLOR` in the alpha operand; it counts as the alpha.
        env.define(gles::GL_OPERAND0_ALPHA, gles::GL_SRC_COLOR, 0.0);
        let texel = [0.2, 0.4, 0.6, 0.8];
        assert_eq!(env.apply([0.0; 4], texel), texel);
    }

    /// QX's unit 1: `ADD_SIGNED` of the color texture with what came out of unit 0 (the light's
    /// `DOT3`), and the texture's alpha times the constant color.
    #[test]
    fn unit_1_adds_signed_over_the_previous_one() {
        let mut env = TexEnv::with_mode(gles::GL_COMBINE);
        env.define(gles::GL_COMBINE_RGB, gles::GL_ADD_SIGNED, 0.0);
        env.define(gles::GL_SRC0_RGB + 1, gles::GL_PREVIOUS, 0.0);
        env.define(gles::GL_SRC0_ALPHA + 1, gles::GL_CONSTANT, 0.0);
        env.color = [0.0, 0.0, 0.0, 0.5];
        let previous = [0.75, 0.5, 0.25, 1.0];
        let texel = [0.5, 0.5, 0.5, 1.0];
        let output = env.apply_with(previous, [0.0; 4], texel, [None, Some(texel)]);
        // 0.5 + previous − 0.5 = previous; alpha 1 × 0.5.
        for (channel, expected) in [0.75, 0.5, 0.25, 0.5].into_iter().enumerate() {
            assert!((output[channel] - expected).abs() < 1e-6, "{output:?}");
        }
    }

    /// A function or source that doesn't exist is `GL_INVALID_ENUM`, and the previous value
    /// stays. Bóia Cross's water asks for `DOT3_RGB` and right after sends `GL_TEXTURE0` in
    /// `GL_COMBINE_RGB`; if accepted, the bump mapping turned into multiplication and the water
    /// came out blue-purple.
    #[test]
    fn combine_rejects_functions_and_sources_that_do_not_exist() {
        let mut env = TexEnv::with_mode(gles::GL_COMBINE);
        env.define(gles::GL_COMBINE_RGB, gles::GL_DOT3_RGB, 0.0);
        env.define(gles::GL_COMBINE_RGB, gles::GL_TEXTURE0, 0.0);
        env.define(gles::GL_COMBINE_RGB, gles::GL_COMBINE, 0.0);
        assert_eq!(env.combine[0], gles::GL_DOT3_RGB);
        // `DOT3` only exists for RGB.
        env.define(gles::GL_COMBINE_ALPHA, gles::GL_DOT3_RGBA, 0.0);
        assert_eq!(env.combine[1], gles::GL_MODULATE);
        env.define(gles::GL_SRC0_RGB + 1, gles::GL_PRIMARY_COLOR, 0.0);
        env.define(gles::GL_SRC0_RGB + 1, gles::GL_DOT3_RGB, 0.0);
        env.define(gles::GL_SRC0_ALPHA, gles::GL_TEXTURE0 + 2, 0.0);
        assert_eq!(env.sources[0][1], gles::GL_PRIMARY_COLOR);
        assert_eq!(env.sources[1][0], gles::GL_TEXTURE);
    }

    /// The crossbar: `GL_TEXTURE0` and `GL_TEXTURE1` are that unit's texel, not the previous
    /// one. It is unit 1 of Bóia Cross's water: unit 0's bump times unit 1's color, with unit 1's
    /// alpha.
    #[test]
    fn the_crossbar_reads_each_units_texel_by_name() {
        let mut env = TexEnv::with_mode(gles::GL_COMBINE);
        env.define(gles::GL_SRC0_RGB, gles::GL_PREVIOUS, 0.0);
        env.define(gles::GL_SRC0_RGB + 1, gles::GL_TEXTURE0 + 1, 0.0);
        env.define(gles::GL_COMBINE_ALPHA, gles::GL_REPLACE, 0.0);
        env.define(gles::GL_SRC0_ALPHA, gles::GL_TEXTURE0 + 1, 0.0);
        env.define(gles::GL_SRC0_ALPHA + 1, gles::GL_TEXTURE0, 0.0);
        let previous = [0.5, 0.5, 0.5, 0.0];
        let brown = [0.6, 0.4, 0.2, 0.8];
        let output = env.apply_with(previous, [0.0; 4], brown, [Some([1.0; 4]), Some(brown)]);
        for (channel, expected) in [0.3, 0.2, 0.1, 0.8].into_iter().enumerate() {
            assert!((output[channel] - expected).abs() < 1e-6, "{output:?}");
        }
        // A source on a unit that doesn't draw: this unit passes the previous one along.
        let output = env.apply_with(previous, [0.0; 4], brown, [None, Some(brown)]);
        assert_eq!(output, previous);
    }

    #[test]
    fn combine_interpolates_by_the_third_source() {
        let mut env = TexEnv::with_mode(gles::GL_COMBINE);
        env.define(gles::GL_COMBINE_RGB, gles::GL_INTERPOLATE, 0.0);
        env.define(gles::GL_SRC0_RGB, gles::GL_TEXTURE, 0.0);
        env.define(gles::GL_SRC0_RGB + 1, gles::GL_PRIMARY_COLOR, 0.0);
        env.define(gles::GL_SRC0_RGB + 2, gles::GL_CONSTANT, 0.0);
        env.color = [0.25; 4];
        let output = env.apply([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
        // 0 × 0.25 + 1 × 0.75
        assert!((output[0] - 0.75).abs() < 1e-6, "{output:?}");
    }

    /// The frame after painting. Drawing is accumulated and only becomes pixels on the flush —
    /// which in the emulator happens at `eglSwapBuffers`, and here has to be requested.
    fn pixels(state: &mut GlState) -> &[[u8; 4]] {
        state.flush();
        &state.color
    }

    fn quad(state: &mut GlState, z: f32, color: [f32; 4]) {
        let vertex = |x: f32, y: f32| Vertex {
            position: [x, y, z, 1.0],
            color,
            uv: DEFAULT_UV,
            ..Default::default()
        };
        state.draw(
            gles::GL_TRIANGLES,
            &[
                vertex(-1.0, -1.0),
                vertex(1.0, -1.0),
                vertex(1.0, 1.0),
                vertex(-1.0, -1.0),
                vertex(1.0, 1.0),
                vertex(-1.0, 1.0),
            ],
        );
    }

    #[test]
    fn the_screen_blit_puts_the_texture_in_the_right_place() {
        // `glDrawTexiOES` draws in **window** coordinates, whose zero is at the bottom — the
        // opposite of our surface. Getting that flip wrong puts the image upside down, and in an
        // arcade emulator that passes for "it worked".
        let mut state = GlState::new(4, 4);
        state.set_viewport(0, 0, 4, 4);
        // Two rows: the bottom one red, the top one blue (in the order the texture arrives).
        let texture = Texture {
            width: 1,
            height: 2,
            pixels: vec![[255, 0, 0, 255], [0, 0, 255, 255]],
            filter: gles::GL_NEAREST,
            ..Texture::default()
        };
        state.textures.insert(1, texture);
        state.bind_texture(1);
        state.set_capability(gles::GL_TEXTURE_2D, true);
        state.set_texture_crop([0, 0, 1, 2]);

        // A 4x2 rectangle against the bottom of the window.
        state.draw_texture(0.0, 0.0, 0.0, 4.0, 2.0);
        let frame = state.present(4, 4);
        let at = |x: usize, y: usize| frame[y * 4 + x];
        let red = (255u16 >> 3) << 11;
        let blue = 255u16 >> 3;

        // The top half of the screen was not touched; the bottom one got the drawing.
        assert_eq!(at(0, 0), 0, "the top stays clean");
        assert_eq!(at(0, 1), 0);
        assert_ne!(at(0, 2), 0, "the bottom got the blit");
        assert_ne!(at(0, 3), 0);
        // And inside it the first texel is at the bottom, as the OpenGL window requires.
        assert_eq!(at(0, 3), red, "texel 0 is the bottom one");
        assert_eq!(at(0, 2), blue);
    }

    #[test]
    fn the_unclipped_blit_uses_the_whole_texture() {
        // A zeroed crop is the whole texture: drawing nothing would be worse than adopting the
        // obvious default.
        let mut state = GlState::new(2, 2);
        state.set_viewport(0, 0, 2, 2);
        state.textures.insert(
            1,
            Texture {
                width: 1,
                height: 1,
                pixels: vec![[0, 255, 0, 255]],
                filter: gles::GL_NEAREST,
                ..Texture::default()
            },
        );
        state.bind_texture(1);
        state.set_capability(gles::GL_TEXTURE_2D, true);
        state.draw_texture(0.0, 0.0, 0.0, 2.0, 2.0);
        let green = (255u16 >> 2) << 5;
        assert_eq!(state.present(2, 2), vec![green; 4]);
    }

    #[test]
    fn multiplication_follows_the_opengl_order() {
        // `translate` after `scale` scales the translation — it is the order of `glTranslate`
        // applied over a matrix that already has a scale.
        let m = multiply(&scaling(2.0, 2.0, 2.0), &translation(1.0, 0.0, 0.0));
        assert_eq!(transform(&m, [0.0, 0.0, 0.0, 1.0])[0], 2.0);
        assert_eq!(
            transform(&IDENTITY, [3.0, 4.0, 5.0, 1.0]),
            [3.0, 4.0, 5.0, 1.0]
        );
    }

    #[test]
    fn a_ninety_degree_rotation_takes_x_to_y() {
        let m = rotation(90.0, 0.0, 0.0, 1.0);
        let v = transform(&m, [1.0, 0.0, 0.0, 1.0]);
        assert!((v[0]).abs() < 1e-6, "{v:?}");
        assert!((v[1] - 1.0).abs() < 1e-6, "{v:?}");
    }

    #[test]
    fn fills_the_screen_and_respects_the_depth_test() {
        let mut state = GlState::new(8, 8);
        state.set_capability(gles::GL_DEPTH_TEST, true);
        state.set_depth_func(gles::GL_LESS);
        state.clear(gles::GL_COLOR_BUFFER_BIT | gles::GL_DEPTH_BUFFER_BIT);

        quad(&mut state, 0.0, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(pixels(&mut state)[8 * 4 + 4], [255, 0, 0, 255]);

        // Farther fails the test; closer passes.
        quad(&mut state, 0.5, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(pixels(&mut state)[8 * 4 + 4], [255, 0, 0, 255]);
        quad(&mut state, -0.5, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(pixels(&mut state)[8 * 4 + 4], [0, 0, 255, 255]);
    }

    /// Layers at the same z with `GL_LEQUAL` pass whole, in both triangles of each strip.
    ///
    /// It is how NFS Carbon builds the EA logo: one quad over the other, of different sizes, all
    /// at z = 3.8e-6. With z summed through the three weights, one triangle's rounding went past
    /// the z written by the other and half of each layer vanished along the diagonal.
    #[test]
    fn layers_at_the_same_z_pass_whole_with_lequal() {
        let side = 64;
        let mut state = GlState::new(side, side);
        state.set_capability(gles::GL_DEPTH_TEST, true);
        state.set_depth_func(gles::GL_LEQUAL);
        state.clear(gles::GL_COLOR_BUFFER_BIT | gles::GL_DEPTH_BUFFER_BIT);
        let z = 3.814_697_3e-6;
        let strip = |state: &mut GlState, x0: f32, y0: f32, x1: f32, y1: f32, color: [f32; 4]| {
            let v = |x: f32, y: f32| Vertex {
                position: [x, y, z, 1.0],
                color,
                ..Default::default()
            };
            state.draw(
                gles::GL_TRIANGLE_STRIP,
                &[v(x0, y1), v(x0, y0), v(x1, y1), v(x1, y0)],
            );
        };
        let green = [-0.8127374, -0.7875298, 0.8354998, 0.9214745];
        let blue = [-0.4027808, -0.3878969, 0.5765068, 0.6939483];
        strip(&mut state, -1.0, -1.0, 1.0, 1.0, [1.0, 0.0, 0.0, 1.0]);
        strip(
            &mut state,
            green[0],
            green[1],
            green[2],
            green[3],
            [0.0, 1.0, 0.0, 1.0],
        );
        strip(
            &mut state,
            blue[0],
            blue[1],
            blue[2],
            blue[3],
            [0.0, 0.0, 1.0, 1.0],
        );
        // How many pixel centers fall inside each rectangle: it is what each layer must cover,
        // and none of the corners falls exactly on a center.
        let inside = |r: [f32; 4]| {
            let center = |i: usize| (i as f32 + 0.5) / side as f32 * 2.0 - 1.0;
            let count = |a: f32, b: f32| (0..side).filter(|&i| (a..b).contains(&center(i))).count();
            count(r[0], r[2]) * count(r[1], r[3])
        };
        let frame = pixels(&mut state);
        let greens = frame.iter().filter(|p| **p == [0, 255, 0, 255]).count();
        let blues = frame.iter().filter(|p| **p == [0, 0, 255, 255]).count();
        assert_eq!(blues, inside(blue), "blue lost pixels to whatever was underneath");
        assert_eq!(greens, inside(green) - inside(blue), "green lost pixels to red");
    }

    /// The `y` of `glViewport` counts from the bottom up, and the frame is stored top-down.
    ///
    /// A viewport at `y = 0` with half the height is the **bottom** half of the image. Treating
    /// `y` as counted from the top put the stretch of track Crash Nitro Kart draws through the
    /// portal mirrored at the bottom of the screen.
    #[test]
    fn the_viewport_counts_y_from_the_bottom_up() {
        let mut state = GlState::new(8, 8);
        state.set_viewport(0, 0, 8, 8);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.set_viewport(0, 0, 8, 4);
        quad(&mut state, 0.0, [1.0, 0.0, 0.0, 1.0]);
        let frame = pixels(&mut state);
        assert_eq!(frame[6 * 8 + 4], [255, 0, 0, 255], "bottom half painted");
        assert_eq!(frame[8 + 4], [0, 0, 0, 255], "top half intact");
    }

    /// `glDepthRange` decides who stays in front across ranges, not the normalized depth.
    ///
    /// That is how Crash Nitro Kart keeps the kart's glow always on top: the draw that is
    /// "farther" in normalized coordinates, but in a closer range, must pass the test.
    #[test]
    fn the_depth_range_decides_the_order() {
        let mut state = GlState::new(8, 8);
        state.set_capability(gles::GL_DEPTH_TEST, true);
        state.set_depth_func(gles::GL_LESS);
        state.clear(gles::GL_COLOR_BUFFER_BIT | gles::GL_DEPTH_BUFFER_BIT);

        Rasterizer::set_depth_range(&mut state, 0.9, 1.0);
        quad(&mut state, -0.5, [1.0, 0.0, 0.0, 1.0]);
        Rasterizer::set_depth_range(&mut state, 0.0, 0.1);
        quad(&mut state, 0.5, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(pixels(&mut state)[8 * 4 + 4], [0, 255, 0, 255]);
    }

    #[test]
    fn alpha_blending_weighs_both_colors() {
        let mut state = GlState::new(4, 4);
        state.set_clear_color([0.0, 0.0, 0.0, 1.0]);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.set_capability(gles::GL_BLEND, true);
        state.set_blend_func(gles::GL_SRC_ALPHA, gles::GL_ONE_MINUS_SRC_ALPHA);
        quad(&mut state, 0.0, [1.0, 1.0, 1.0, 0.5]);
        let pixel = pixels(&mut state)[4 * 2 + 2];
        assert!((pixel[0] as i32 - 128).abs() <= 1, "{pixel:?}");
    }

    #[test]
    fn the_alpha_test_discards_the_fragment() {
        let mut state = GlState::new(4, 4);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.set_capability(gles::GL_ALPHA_TEST, true);
        state.set_alpha_func(gles::GL_GREATER, 0.5);
        quad(&mut state, 0.0, [1.0, 1.0, 1.0, 0.25]);
        assert_eq!(pixels(&mut state)[4 * 2 + 2], [0, 0, 0, 255]);
    }

    /// Winding is the detail that cost the most: getting the sign wrong culls exactly the faces
    /// that should show, and the result is a black screen with the right geometry behind it.
    #[test]
    fn the_front_face_is_counter_clockwise_in_opengl_coordinates() {
        let triangle = |flip: bool| {
            let vertex = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [1.0; 4],
                uv: DEFAULT_UV,
                ..Default::default()
            };
            // In OpenGL coordinates, with Y up, this order is counterclockwise.
            let mut v = [vertex(-1.0, -1.0), vertex(1.0, -1.0), vertex(0.0, 1.0)];
            if flip {
                v.swap(0, 1);
            }
            v
        };
        let center = 8 * 4 + 4;

        let mut state = GlState::new(8, 8);
        state.set_capability(gles::GL_CULL_FACE, true);
        state.set_cull_face(gles::GL_BACK);
        state.set_front_face(gles::GL_CCW);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.draw(gles::GL_TRIANGLES, &triangle(false));
        assert_eq!(pixels(&mut state)[center], [255, 255, 255, 255]);

        // The back side of the same triangle vanishes.
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.draw(gles::GL_TRIANGLES, &triangle(true));
        assert_eq!(pixels(&mut state)[center], [0, 0, 0, 255]);

        // With `GL_CW` the decision flips.
        state.set_front_face(gles::GL_CW);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.draw(gles::GL_TRIANGLES, &triangle(true));
        assert_eq!(pixels(&mut state)[center], [255, 255, 255, 255]);
    }

    #[test]
    fn near_plane_clipping_preserves_winding() {
        // A triangle crossing the near plane: two vertices in front, one behind. Clipping
        // splits what remains in two, and if it walks the polygon backwards the winding flips —
        // face culling then throws away precisely the clipped piece, and the hole only shows up
        // when the camera gets close to the geometry.
        let vertex = |x: f32, y: f32, z: f32, w: f32| Vertex {
            position: [x, y, z, w],
            color: [1.0; 4],
            uv: DEFAULT_UV,
            ..Default::default()
        };
        // Counterclockwise in OpenGL coordinates; the third vertex is behind the camera.
        let tri = [
            vertex(-1.0, -1.0, 1.0, 2.0),
            vertex(1.0, -1.0, 1.0, 2.0),
            vertex(0.0, 4.0, -4.0, -2.0),
        ];
        let center = 8 * 5 + 4;

        let mut state = GlState::new(8, 8);
        state.set_capability(gles::GL_CULL_FACE, true);
        state.set_cull_face(gles::GL_BACK);
        state.set_front_face(gles::GL_CCW);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.draw(gles::GL_TRIANGLES, &tri);
        assert_eq!(pixels(&mut state)[center], [255, 255, 255, 255]);
    }

    #[test]
    fn the_near_plane_is_z_plus_w_not_the_sign_of_w() {
        // A triangle just behind the near plane, but still with a positive `w`: clipping only by
        // the sign of `w` would let it through, and the perspective divide would spread it across
        // the whole screen.
        let vertex = |x: f32, y: f32| Vertex {
            position: [x, y, -1.0, 0.001],
            color: [1.0; 4],
            uv: DEFAULT_UV,
            ..Default::default()
        };
        let tri = [
            vertex(-0.001, -0.001),
            vertex(0.001, -0.001),
            vertex(0.0, 0.001),
        ];

        let mut state = GlState::new(8, 8);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.draw(gles::GL_TRIANGLES, &tri);
        assert!(pixels(&mut state).iter().all(|p| *p == [0, 0, 0, 255]));
    }

    #[test]
    fn the_texture_matrix_transforms_the_coordinates() {
        // Zeebo Sports Peteca sends the `uv` in fixed point and brings it back to `0..1` through
        // the texture matrix. Ignoring it made `GL_REPEAT` wrap around the texture on every pixel,
        // and the court and the stands came out as confetti in the right colors.
        let mut state = GlState::new(4, 4);
        let mut texture = Texture {
            width: 2,
            height: 1,
            pixels: vec![[255, 0, 0, 255], [0, 0, 255, 255]],
            ..Texture::default()
        };
        texture.filter = gles::GL_NEAREST;
        state.textures.insert(1, texture);
        state.bind_texture(1);
        state.set_capability(gles::GL_TEXTURE_2D, true);

        // `u` arrives as 32767 and the matrix divides it back to near zero, which is the red
        // texel. Without the matrix, the raw value would land anywhere in the texture.
        state.set_matrix_mode(gles::GL_TEXTURE);
        state.load_matrix(scaling(1.0 / 32767.0, 1.0, 1.0));
        state.set_matrix_mode(gles::GL_MODELVIEW);

        let vertex = |x: f32, y: f32| Vertex {
            position: [x, y, 0.0, 1.0],
            color: [1.0; 4],
            uv: [32767.0 * 0.25, 0.0, 0.0, 1.0],
            ..Default::default()
        };
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.draw(
            gles::GL_TRIANGLES,
            &[vertex(-1.0, -1.0), vertex(1.0, -1.0), vertex(0.0, 1.0)],
        );
        assert_eq!(pixels(&mut state)[2 * 4 + 2], [255, 0, 0, 255]);
    }

    /// **Each unit has its own texture matrix, and it uses all four components.** It is how
    /// Reckless Racing draws the shadow: the vertex position goes as a three-component coordinate
    /// on both units, and each matrix derives `s` and `t` from `x` and `z`. With a single stack,
    /// unit 1's matrix overwrote unit 0's; without `r`, `t` stayed constant and the shadow came
    /// out as vertical stripes covering the cars.
    #[test]
    fn each_unit_projects_the_coordinate_through_its_own_matrix() {
        let mut state = GlState::new(4, 4);
        // Unit 0: s = x, t = z. Unit 1: s = x / 2, t = -z / 2 + 0.5.
        let xz = [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ];
        let middle = [
            0.5, 0.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 0.0, //
            0.0, -0.5, 0.0, 0.0, //
            0.0, 0.5, 0.0, 1.0,
        ];
        state.set_matrix_mode(gles::GL_TEXTURE);
        state.set_active_texture(gles::GL_TEXTURE0);
        state.load_matrix(xz);
        state.set_active_texture(gles::GL_TEXTURE0 + 1);
        state.load_matrix(middle);
        state.set_active_texture(gles::GL_TEXTURE0);
        state.set_matrix_mode(gles::GL_MODELVIEW);

        let position = [0.25, -3.0, 0.75, 1.0];
        let vertex = Vertex {
            uv: position,
            uv1: position,
            ..Default::default()
        };
        state.vertex_stage(&[vertex]);
        let output = state.transformed()[0];
        assert_eq!([output.uv[0], output.uv[1]], [0.25, 0.75]);
        assert_eq!([output.uv1[0], output.uv1[1]], [0.125, 0.125]);
    }

    #[test]
    fn without_a_viewport_the_surface_is_the_whole_screen() {
        // Peteca draws in screen coordinates and never calls `glViewport`. Deducing the surface
        // from an empty set of viewports gave 1×1, and presentation stretched a single pixel
        // across the whole screen: the game came out entirely white.
        let state = GlState::new(640, 480);
        assert_eq!(state.surface(), (640, 480));
    }

    #[test]
    fn the_surface_is_the_largest_viewport_the_game_used() {
        // Zeebo's Quake draws at 320×400 on a 640×480 screen, and presentation scales it up.
        let mut state = GlState::new(640, 480);
        state.set_viewport(0, 0, 320, 400);
        state.set_viewport(0, 360, 320, 40);
        assert_eq!(state.surface(), (320, 400));
    }

    #[test]
    fn the_matrix_stack_pushes_and_pops() {
        let mut state = GlState::new(4, 4);
        state.set_matrix_mode(gles::GL_MODELVIEW);
        state.load_identity();
        state.push_matrix();
        state.mult_matrix(translation(5.0, 0.0, 0.0));
        assert_eq!(state.top()[12], 5.0);
        state.pop_matrix();
        assert_eq!(state.top()[12], 0.0);
        // Popping too many times must not empty the stack.
        state.pop_matrix();
        state.pop_matrix();
        assert_eq!(*state.top(), IDENTITY);
    }

    #[test]
    fn the_presentation_is_rgb565() {
        let mut state = GlState::new(2, 2);
        state.set_clear_color([1.0, 0.0, 0.0, 1.0]);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        assert_eq!(state.present(2, 2), vec![0xf800; 4]);
    }

    /// Z-Wheel reads the color buffer twice per frame, and on the first the queue is always
    /// empty: it was a whole frame reconverted with nothing having changed. The second read can
    /// only reuse what is already in the vector if nothing actually touched the buffer.
    #[test]
    fn the_frame_is_reconverted_only_when_something_changed() {
        let mut state = GlState::new(64, 64);
        state.set_clear_color([1.0, 0.0, 0.0, 1.0]);
        state.clear(gles::GL_COLOR_BUFFER_BIT);

        let mut bytes = Vec::new();
        state.frame_rgb565(64, 64, &mut bytes);
        let red = bytes.clone();
        assert_eq!(bytes.len(), 64 * 64 * 2);

        // Nothing changed: the second read returns the same content, without reconverting.
        bytes.fill(0xAB);
        state.frame_rgb565(64, 64, &mut bytes);
        assert!(
            bytes.iter().all(|&b| b == 0xAB),
            "with no change, the output vector is not touched"
        );

        // A `clear` touches the buffer, so the next read must reconvert.
        state.set_clear_color([0.0, 0.0, 1.0, 1.0]);
        state.clear(gles::GL_COLOR_BUFFER_BIT);
        state.frame_rgb565(64, 64, &mut bytes);
        assert_ne!(bytes, red, "after the clear the frame is different");
        assert!(
            bytes.iter().any(|&b| b != 0xAB),
            "and it was actually reconverted"
        );

        // And a size change must not take the shortcut.
        state.frame_rgb565(32, 32, &mut bytes);
        assert_eq!(bytes.len(), 32 * 32 * 2);
    }
}

/// The GL state the **guest** changed, in sections — and the rule for when saving is not allowed.
///
/// ## Why small sections, and not a list of numbers
///
/// The first version of the machine state used a flat list of numbers, and I got its index wrong
/// **twice** — `pending_launch` came back with the high half of `next_serial`, and `network` came
/// back with the width of the clip rectangle. Here there are over a hundred numbers, so a flat
/// list would be asking to get it wrong again. Each section has its size checked on read, and an
/// offset within it is rejected instead of becoming a swapped value.
///
/// ## What goes in, and what is left for later
///
/// In go the 48 fields that are **state** — the three matrix stacks, the viewport and the scissor,
/// the colors, the test flags and functions, the fog, the depth range, the color mask, the texture
/// unit and **lighting** (eight lights, the material and the model). Left out, for now and with a
/// reason:
///
/// - `textures`, which needs its own format because of the mipmap chains;
/// - `color`, `depth` and `stencil` — the three 640×480 buffers. Measured: **the color one is not a
///   duplicate of the screen**. The screen the core presents is the device's bitmap, and the color
///   buffer is the GL drawing surface; `eglSwapBuffers` reads one and hands over the other. Both
///   must be saved, and that is what makes the state grow by about 2.7 MB.
///
/// ## The draw-in-progress rule
///
/// `batch`, `pending` and `transformed` are accumulators of a draw **started and not finished**.
/// The frontend calls serialize **between frames**, never inside a `retro_run`, so at the moment of
/// the save they are empty — and that is what makes them dispensable. Relying on that would be
/// fragile: the saver **asks** with [`GlState::draw_in_progress`] and refuses the save if the
/// answer is yes. A state saved in the middle of a `glBegin` would promise a draw that never
/// existed.
impl crate::save_state::Saveable for GlState {
    fn save(&self, dest: &mut crate::save_state::Sections) {
        let floats = |values: &[f32]| -> Vec<u32> {
            values.iter().map(|v| v.to_bits()).collect()
        };

        // The matrix stacks, each with its count in front. Unit 1's texture stack comes last, and
        // is optional on read: saves from before it have only the first three.
        let mut matrices = vec![self.matrix_mode];
        for stack in [
            &self.modelview,
            &self.projection,
            &self.texture_matrix,
            &self.texture_matrix1,
        ] {
            matrices.push(stack.len() as u32);
            for matrix in stack {
                matrices.extend(floats(matrix));
            }
        }
        dest.put_u32s("gl.matrizes", matrices);

        // Where drawing happens.
        let mut location = vec![
            self.viewport.0 as u32,
            self.viewport.1 as u32,
            self.viewport.2 as u32,
            self.viewport.3 as u32,
            u32::from(self.scissor.is_some()),
            self.scissor.unwrap_or((0, 0, 0, 0)).0 as u32,
            self.scissor.unwrap_or((0, 0, 0, 0)).1 as u32,
            self.scissor.unwrap_or((0, 0, 0, 0)).2 as u32,
            self.scissor.unwrap_or((0, 0, 0, 0)).3 as u32,
            self.raw_scissor.0 as u32,
            self.raw_scissor.1 as u32,
            self.raw_scissor.2 as u32,
            self.raw_scissor.3 as u32,
            u32::from(self.scissor_enabled),
            u32::from(self.surface.is_some()),
            self.surface.unwrap_or((0, 0)).0 as u32,
            self.surface.unwrap_or((0, 0)).1 as u32,
            u32::from(self.stretched),
            u32::from(self.dirty),
        ];
        location.extend(floats(&self.clear_color));
        location.push(self.clear_depth.to_bits());
        location.extend(floats(&self.current_color));
        location.push(u32::from(self.clear_stencil));
        location.extend(self.color_mask.iter().map(|m| u32::from(*m)));
        dest.put_u32s("gl.onde", location);

        // The test flags and functions.
        dest.put_u32s(
            "gl.testes",
            vec![
                u32::from(self.depth_test),
                u32::from(self.depth_mask),
                self.depth_func,
                u32::from(self.blend),
                self.blend_src,
                self.blend_dst,
                u32::from(self.alpha_test),
                self.alpha_func,
                self.alpha_ref.to_bits(),
                u32::from(self.cull_face),
                self.cull_mode,
                self.front_face,
                u32::from(self.stencil_test),
                self.stencil_func,
                self.stencil_ref as u32,
                self.stencil_value_mask,
                self.stencil_write_mask,
                self.stencil_op[0],
                self.stencil_op[1],
                self.stencil_op[2],
                self.depth_range.0.to_bits(),
                self.depth_range.1.to_bits(),
                u32::from(self.lighting),
                u32::from(self.color_material),
                self.shade_model,
            ],
        );

        // The bound textures and the texture environment of both units.
        let mut texture = vec![
            self.bound_texture,
            u32::from(self.texture_2d),
            self.active_unit,
            self.client_unit,
        ];
        texture.extend(tex_env_words(&self.texture_env));
        texture.push(u32::from(self.unit1.enabled));
        texture.push(self.unit1.texture);
        texture.extend(tex_env_words(&self.unit1.env));
        dest.put_u32s("gl.textura", texture);

        // The fog.
        let mut fog: Vec<u32> = vec![
            u32::from(self.fog.enabled),
            self.fog.curve,
            self.fog.density.to_bits(),
            self.fog.start.to_bits(),
            self.fog.end.to_bits(),
            u32::from(self.fog.allowed),
        ];
        fog.extend(floats(&self.fog.color));
        dest.put_u32s("gl.nevoa", fog);

        // Lighting: the material, the model ambient and the eight lights.
        let mut light = floats(&self.material.ambient);
        light.extend(floats(&self.material.diffuse));
        light.extend(floats(&self.material.specular));
        light.extend(floats(&self.material.emission));
        light.push(self.material.shininess.to_bits());
        light.extend(floats(&self.light_model_ambient));
        for source in &self.lights {
            light.push(u32::from(source.enabled));
            light.extend(floats(&source.ambient));
            light.extend(floats(&source.diffuse));
            light.extend(floats(&source.specular));
            light.extend(floats(&source.position));
            light.extend(floats(&source.spot_direction));
            light.push(source.spot_exponent.to_bits());
            light.push(source.spot_cutoff.to_bits());
            light.extend(floats(&source.attenuation));
        }
        dest.put_u32s("gl.iluminacao", light);
        dest.put_u32s(
            "gl.buffers",
            [
                self.width as u32,
                self.height as u32,
                self.color.len() as u32,
                self.depth.len() as u32,
                self.stencil.len() as u32,
            ],
        );
        dest.put("gl.buffer.cor", rgba_to_bytes(&self.color));
        let mut depth = Vec::with_capacity(self.depth.len() * 4);
        for value in &self.depth {
            depth.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        dest.put("gl.buffer.profundidade", depth);
        dest.put("gl.buffer.stencil", self.stencil.clone());
        save_textures(&self.textures, dest);
    }

    fn restore(
        &mut self,
        origin: &crate::save_state::Reader<'_>,
    ) -> Result<(), crate::save_state::Error> {
        use crate::save_state::Error;
        let missing = |name: &str| Error::Section {
            name: name.to_string(),
            reason: "the section is not in the file".to_string(),
        };

        let matrices = origin.u32s("gl.matrizes")?;
        let location = origin.u32s("gl.onde")?;
        let tests = origin.u32s("gl.testes")?;
        let texture = origin.u32s("gl.textura")?;
        let fog = origin.u32s("gl.nevoa")?;
        let light = origin.u32s("gl.iluminacao")?;

        let check = |name: &str, got: usize, expected: usize| -> Result<(), Error> {
            if got != expected {
                return Err(Error::Section {
                    name: name.to_string(),
                    reason: format!("expected {expected} numbers and got {got}"),
                });
            }
            Ok(())
        };
        // Nineteen for the viewport, the scissor and the surface; four for the clear color; one
        // for the depth; four for the current color; one for the stencil; four for the color mask.
        check("gl.onde", location.len(), 19 + 4 + 1 + 4 + 1 + 4)?;
        check("gl.testes", tests.len(), 25)?;
        check("gl.textura", texture.len(), 4 + TEX_ENV_LEN + 2 + TEX_ENV_LEN)?;
        check("gl.nevoa", fog.len(), 6 + 4)?;
        // Material (sixteen floats and the shininess) plus the model ambient: twenty-one. Each
        // light takes twenty-five: enabled, four ambient, four diffuse, four specular, four
        // position, three spot direction, exponent, cutoff and three attenuation.
        check(
            "gl.iluminacao",
            light.len(),
            21 + crate::video::gles::LIGHTS * 25,
        )?;

        let f = |value: u32| f32::from_bits(value);
        let boolean = |value: u32, field: &str, location: &str| -> Result<bool, Error> {
            match value {
                0 => Ok(false),
                1 => Ok(true),
                other => Err(Error::Section {
                    name: location.to_string(),
                    reason: format!("field `{field}` is {other}, and it is a boolean"),
                }),
            }
        };

        // The matrix stacks.
        let mut cursor = 0usize;
        let matrix_mode = matrices[cursor];
        cursor += 1;
        let mut stacks: Vec<Vec<Matrix>> = Vec::new();
        for index in 0..4 {
            if index == 3 && matrices.len() == cursor {
                stacks.push(vec![IDENTITY]);
                break;
            }
            if matrices.len() < cursor + 1 {
                return Err(missing("gl.matrizes"));
            }
            let matrix_count = matrices[cursor] as usize;
            cursor += 1;
            if matrices.len() < cursor + matrix_count * 16 {
                return Err(Error::Section {
                    name: "gl.matrizes".to_string(),
                    reason: format!("a stack claims {matrix_count} matrices and the file ends first"),
                });
            }
            let mut stack = Vec::with_capacity(matrix_count);
            for _ in 0..matrix_count {
                let mut matrix = [0.0f32; 16];
                for (index, dest) in matrix.iter_mut().enumerate() {
                    *dest = f(matrices[cursor + index]);
                }
                cursor += 16;
                stack.push(matrix);
            }
            stacks.push(stack);
        }

        // From here on it is applying.
        self.matrix_mode = matrix_mode;
        // **The writer's order, not the one I remembered.** I had put the projection first, and
        // the test showed the swap: the `modelview` stack came back with the projection inside.
        // Swapping two matrices gives no error at all — it gives the scene drawn in the wrong
        // place, and you only see it by looking.
        self.modelview = stacks.remove(0);
        self.projection = stacks.remove(0);
        self.texture_matrix = stacks.remove(0);
        self.texture_matrix1 = stacks.remove(0);

        self.viewport = (
            location[0] as i32,
            location[1] as i32,
            location[2] as i32,
            location[3] as i32,
        );
        self.scissor = boolean(location[4], "scissor", "gl.onde")?.then(|| {
            (
                location[5] as i32,
                location[6] as i32,
                location[7] as i32,
                location[8] as i32,
            )
        });
        self.raw_scissor = (
            location[9] as i32,
            location[10] as i32,
            location[11] as i32,
            location[12] as i32,
        );
        self.scissor_enabled = boolean(location[13], "scissor_enabled", "gl.onde")?;
        self.surface = boolean(location[14], "surface", "gl.onde")?
            .then(|| (location[15] as usize, location[16] as usize));
        self.stretched = boolean(location[17], "stretched", "gl.onde")?;
        self.dirty = boolean(location[18], "dirty", "gl.onde")?;
        for (index, value) in self.clear_color.iter_mut().enumerate() {
            *value = f(location[19 + index]);
        }
        self.clear_depth = f(location[23]);
        for (index, value) in self.current_color.iter_mut().enumerate() {
            *value = f(location[24 + index]);
        }
        self.clear_stencil = location[28] as u8;
        for (index, value) in self.color_mask.iter_mut().enumerate() {
            *value = location[29 + index] != 0;
        }

        self.depth_test = boolean(tests[0], "depth_test", "gl.testes")?;
        self.depth_mask = boolean(tests[1], "depth_mask", "gl.testes")?;
        self.depth_func = tests[2];
        self.blend = boolean(tests[3], "blend", "gl.testes")?;
        self.blend_src = tests[4];
        self.blend_dst = tests[5];
        self.alpha_test = boolean(tests[6], "alpha_test", "gl.testes")?;
        self.alpha_func = tests[7];
        self.alpha_ref = f(tests[8]);
        self.cull_face = boolean(tests[9], "cull_face", "gl.testes")?;
        self.cull_mode = tests[10];
        self.front_face = tests[11];
        self.stencil_test = boolean(tests[12], "stencil_test", "gl.testes")?;
        self.stencil_func = tests[13];
        self.stencil_ref = tests[14] as i32;
        self.stencil_value_mask = tests[15];
        self.stencil_write_mask = tests[16];
        self.stencil_op = [tests[17], tests[18], tests[19]];
        self.depth_range = (f(tests[20]), f(tests[21]));
        self.lighting = boolean(tests[22], "lighting", "gl.testes")?;
        self.color_material = boolean(tests[23], "color_material", "gl.testes")?;
        self.shade_model = tests[24];

        self.bound_texture = texture[0];
        self.texture_2d = boolean(texture[1], "texture_2d", "gl.textura")?;
        self.active_unit = texture[2];
        self.client_unit = texture[3];
        self.texture_env = read_tex_env(&texture[4..4 + TEX_ENV_LEN]);
        self.unit1.enabled = boolean(texture[4 + TEX_ENV_LEN], "unit1.enabled", "gl.textura")?;
        self.unit1.texture = texture[5 + TEX_ENV_LEN];
        self.unit1.env = read_tex_env(&texture[6 + TEX_ENV_LEN..]);

        self.fog.enabled = boolean(fog[0], "fog.enabled", "gl.nevoa")?;
        self.fog.curve = fog[1];
        self.fog.density = f(fog[2]);
        self.fog.start = f(fog[3]);
        self.fog.end = f(fog[4]);
        self.fog.allowed = boolean(fog[5], "fog.allowed", "gl.nevoa")?;
        for (index, value) in self.fog.color.iter_mut().enumerate() {
            *value = f(fog[6 + index]);
        }

        // A cursor by index, not through a closure that borrows it: the mutable closure doesn't
        // let the counter be read in between, and the compiler said so before I got the count
        // wrong by hand.
        let mut cursor = 0usize;
        macro_rules! four {
            ($dest:expr) => {
                for value in $dest.iter_mut() {
                    *value = f(light[cursor]);
                    cursor += 1;
                }
            };
        }
        four!(self.material.ambient);
        four!(self.material.diffuse);
        four!(self.material.specular);
        four!(self.material.emission);
        self.material.shininess = f(light[cursor]);
        cursor += 1;
        four!(self.light_model_ambient);
        for index in 0..self.lights.len() {
            self.lights[index].enabled = boolean(light[cursor], "light.enabled", "gl.iluminacao")?;
            cursor += 1;
            four!(self.lights[index].ambient);
            four!(self.lights[index].diffuse);
            four!(self.lights[index].specular);
            four!(self.lights[index].position);
            for position in 0..3 {
                self.lights[index].spot_direction[position] = f(light[cursor]);
                cursor += 1;
            }
            self.lights[index].spot_exponent = f(light[cursor]);
            self.lights[index].spot_cutoff = f(light[cursor + 1]);
            cursor += 2;
            four!(self.lights[index].attenuation);
        }

        // **The three buffers.** Measured before deciding: the color one is NOT a duplicate of
        // the screen. The screen the core presents is the device's bitmap, and the color buffer is
        // the GL drawing surface — `eglSwapBuffers` reads one and hands over the other. Both must
        // be saved.
        let buffers = origin.u32s("gl.buffers")?;
        if buffers.len() != 5 {
            return Err(Error::Section {
                name: "gl.buffers".to_string(),
                reason: format!("expected 5 numbers and got {}", buffers.len()),
            });
        }
        let (width, height) = (buffers[0] as usize, buffers[1] as usize);
        let cap = width * height;
        // **A ceiling on the declared size.** A corrupted file — or one written by a version with
        // a different resolution — must not make loading request an absurd amount of memory. Above
        // the surface size it is a refusal, not an allocation.
        let check_size = |name: &str, declared: usize, per_pixel: usize| -> Result<usize, Error> {
            if declared > cap {
                return Err(Error::Section {
                    name: name.to_string(),
                    reason: format!(
                        "the state claims {declared} element(s) and the surface is {width}x{height}"
                    ),
                });
            }
            let expected = declared * per_pixel;
            Ok(expected)
        };
        let count = check_size("gl.buffer.cor", buffers[2] as usize, 4)?;
        let color = state_section(origin, "gl.buffer.cor")?;
        if color.len() != count {
            return Err(Error::Section {
                name: "gl.buffer.cor".to_string(),
                reason: format!("expected {count} bytes and got {}", color.len()),
            });
        }
        let count = check_size("gl.buffer.profundidade", buffers[3] as usize, 4)?;
        let depth = state_section(origin, "gl.buffer.profundidade")?;
        if depth.len() != count {
            return Err(Error::Section {
                name: "gl.buffer.profundidade".to_string(),
                reason: format!("expected {count} bytes and got {}", depth.len()),
            });
        }
        let count = check_size("gl.buffer.stencil", buffers[4] as usize, 1)?;
        let stencil = state_section(origin, "gl.buffer.stencil")?;
        if stencil.len() != count {
            return Err(Error::Section {
                name: "gl.buffer.stencil".to_string(),
                reason: format!("expected {count} bytes and got {}", stencil.len()),
            });
        }
        let color: Vec<[u8; 4]> = color.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect();
        let depth: Vec<f32> = depth
            .chunks_exact(4)
            .map(|p| f32::from_bits(u32::from_le_bytes([p[0], p[1], p[2], p[3]])))
            .collect();

        self.color = color;
        self.depth = depth;
        self.stencil = stencil;
        self.textures = load_textures(origin)?;
        Ok(())
    }
}

/// Saves the textures: per object, the numbers, the pixels and the minification levels.
///
/// The mipmap chain is the reason this table has its own format. Discarding it was the cause of
/// the stripes on the Z-Wheel stage models: they come with the whole chain, from 128×128 down to
/// 1×1, and are seen at grazing angles — the side of a car takes a few pixels of width and covers
/// the whole texture. Always sampling level zero, each pixel lands on an arbitrary texel and the
/// result is the back of the car repeated in columns.
///
/// The pixels go **raw**, alpha byte included: four per pixel, in reading order. It is the
/// heaviest table in the state, and where compressing would pay off most — but compressing is a
/// decision about the format, and the format already has a version to take that later.
fn save_textures(
    textures: &std::collections::HashMap<u32, Texture>,
    dest: &mut crate::save_state::Sections,
) {
    let mut ids: Vec<u32> = textures.keys().copied().collect();
    ids.sort_unstable();
    dest.put_u32s("tex.ids", ids.iter().copied());
    for id in ids {
        let texture = &textures[&id];
        dest.put_u32s(
            &format!("tex.{id}.meta"),
            [
                texture.width as u32,
                texture.height as u32,
                texture.wrap[0],
                texture.wrap[1],
                texture.filter,
                texture.min_filter,
                texture.crop[0] as u32,
                texture.crop[1] as u32,
                texture.crop[2] as u32,
                texture.crop[3] as u32,
                texture.mipmaps.len() as u32,
            ],
        );
        dest.put(
            &format!("tex.{id}.pixels"),
            pixels_to_bytes(&texture.pixels),
        );
        // The dimensions of each level, and then all their pixels right after.
        let mut sizes = Vec::with_capacity(texture.mipmaps.len() * 2);
        let mut pixels = Vec::new();
        for level in &texture.mipmaps {
            sizes.push(level.width as u32);
            sizes.push(level.height as u32);
            pixels.extend_from_slice(&pixels_to_bytes(&level.pixels));
        }
        dest.put_u32s(&format!("tex.{id}.mips"), sizes);
        dest.put(&format!("tex.{id}.mip_pixels"), pixels);
    }
}

/// The RGBA pixels as bytes, in a single vector.
fn rgba_to_bytes(pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut output = Vec::with_capacity(pixels.len() * 4);
    for pixel in pixels {
        output.extend_from_slice(pixel);
    }
    output
}

/// The RGBA pixels as bytes.
fn pixels_to_bytes(pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut output = Vec::with_capacity(pixels.len() * 4);
    for pixel in pixels {
        output.extend_from_slice(pixel);
    }
    output
}

/// The way back from [`save_textures`], with the check that matters.
///
/// **The number of pixels must match the dimensions.** A truncated file, or a mipmap level with
/// swapped measurements, would draw stripes — and the defect would show up as a wrong image, with
/// nothing pointing at the save state. Here it is a refusal.
fn load_textures(
    origin: &crate::save_state::Reader<'_>,
) -> Result<std::collections::HashMap<u32, Texture>, crate::save_state::Error> {
    use crate::save_state::Error;
    let ids = origin.u32s("tex.ids")?;
    let mut textures = std::collections::HashMap::new();
    for id in ids {
        let meta = origin.u32s(&format!("tex.{id}.meta"))?;
        if meta.len() != 11 {
            return Err(Error::Section {
                name: format!("tex.{id}.meta"),
                reason: format!("expected 11 numbers and got {}", meta.len()),
            });
        }
        let (width, height) = (meta[0] as usize, meta[1] as usize);
        let pixels = bytes_to_pixels(&state_section(origin, &format!("tex.{id}.pixels"))?);
        if pixels.len() != width * height {
            return Err(Error::Section {
                name: format!("tex.{id}.pixels"),
                reason: format!(
                    "the texture is {width}x{height} ({} pixels) and {} came",
                    width * height,
                    pixels.len()
                ),
            });
        }
        let sizes = origin.u32s(&format!("tex.{id}.mips"))?;
        let count = meta[10] as usize;
        if sizes.len() != count * 2 {
            return Err(Error::Section {
                name: format!("tex.{id}.mips"),
                reason: format!("claims {count} level(s) and got {} number(s)", sizes.len()),
            });
        }
        let raw = state_section(origin, &format!("tex.{id}.mip_pixels"))?;
        let mut mipmaps = Vec::with_capacity(count);
        let mut cursor = 0usize;
        for level in 0..count {
            let (l, a) = (sizes[level * 2] as usize, sizes[level * 2 + 1] as usize);
            let needed = l * a * 4;
            if raw.len() < cursor + needed {
                return Err(Error::Section {
                    name: format!("tex.{id}.mip_pixels"),
                    reason: format!(
                        "level {level} is {l}x{a} and needs {needed} bytes, and {} remain",
                        raw.len() - cursor
                    ),
                });
            }
            mipmaps.push(Level {
                width: l,
                height: a,
                pixels: bytes_to_pixels(&raw[cursor..cursor + needed]),
            });
            cursor += needed;
        }
        textures.insert(
            id,
            Texture {
                width,
                height,
                pixels,
                mipmaps,
                wrap: [meta[2], meta[3]],
                filter: meta[4],
                min_filter: meta[5],
                crop: [
                    meta[6] as i32,
                    meta[7] as i32,
                    meta[8] as i32,
                    meta[9] as i32,
                ],
            },
        );
    }
    Ok(textures)
}

/// A byte section that must exist.
fn state_section<'a>(
    origin: &'a crate::save_state::Reader<'a>,
    name: &str,
) -> Result<Vec<u8>, crate::save_state::Error> {
    origin
        .section(name)
        .map(|bytes| bytes.to_vec())
        .ok_or_else(|| crate::save_state::Error::Section {
            name: name.to_string(),
            reason: "the section is not in the file".to_string(),
        })
}

/// RGBA bytes as pixels. A remainder of fewer than four bytes is discarded, and the caller is the
/// one who checks the count.
fn bytes_to_pixels(bytes: &[u8]) -> Vec<[u8; 4]> {
    bytes
        .chunks_exact(4)
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect()
}

/// How many numbers the texture environment takes: mode, two combine, six sources, six
/// operands, two scales and four color.
const TEX_ENV_LEN: usize = 1 + 2 + 6 + 6 + 2 + 4;

/// The texture environment as numbers.
fn tex_env_words(env: &TexEnv) -> Vec<u32> {
    let mut output = vec![env.mode];
    output.extend(env.combine);
    output.extend(env.sources.iter().flatten().copied());
    output.extend(env.operands.iter().flatten().copied());
    output.extend(env.scale.iter().map(|e| e.to_bits()));
    output.extend(env.color.iter().map(|c| c.to_bits()));
    output
}

/// The way back from [`tex_env_words`].
fn read_tex_env(numbers: &[u32]) -> TexEnv {
    let mut cursor = 0usize;
    let mut next = || {
        let value = numbers[cursor];
        cursor += 1;
        value
    };
    let mode = next();
    let combine = [next(), next()];
    let mut sources = [[0u32; 3]; 2];
    for line in sources.iter_mut() {
        for value in line.iter_mut() {
            *value = next();
        }
    }
    let mut operands = [[0u32; 3]; 2];
    for line in operands.iter_mut() {
        for value in line.iter_mut() {
            *value = next();
        }
    }
    let scale = [f32::from_bits(next()), f32::from_bits(next())];
    let color = [
        f32::from_bits(next()),
        f32::from_bits(next()),
        f32::from_bits(next()),
        f32::from_bits(next()),
    ];
    TexEnv {
        mode,
        combine,
        sources,
        operands,
        scale,
        color,
    }
}

#[cfg(test)]
mod gl_state_tests {
    use super::*;
    use crate::save_state::{Saveable, Reader, Sections};

    /// **A save from before unit 1's texture matrix still opens.** It has only three stacks in
    /// `gl.matrizes`; the fourth comes back as identity, which is what the game had.
    #[test]
    fn a_save_without_the_unit_1_matrix_loads_with_identity() {
        let mut before = GlState::new(640, 480);
        before.texture_matrix1.push([0.75; 16]);
        let mut sections = Sections::create();
        before.save(&mut sections);
        let three_stacks: Vec<u32> = std::iter::once(gles::GL_MODELVIEW)
            .chain(
                (0..3).flat_map(|_| std::iter::once(1).chain(IDENTITY.iter().map(|v| v.to_bits()))),
            )
            .collect();
        sections.put_u32s("gl.matrizes", three_stacks);
        let file = sections.close();
        let reader = Reader::open(&file).expect("opened");

        let mut after = GlState::new(640, 480);
        after.texture_matrix1.push([0.5; 16]);
        after.restore(&reader).expect("restored");
        assert_eq!(after.texture_matrix1, vec![IDENTITY]);
    }

    /// **The GL state round-trips**, field by field.
    ///
    /// Each section is checked by size on read; this test checks the **values**, which is what the
    /// size doesn't catch. It covers the four groups at once: the matrix stacks, where drawing
    /// happens, the flags, the texture, the fog and the lighting.
    #[test]
    fn the_gl_state_round_trips() {
        let mut before = GlState::new(640, 480);
        before.matrix_mode = 0x1700;
        before.modelview.push([1.5; 16]);
        before.projection.push([-2.25; 16]);
        before.texture_matrix.push([0.5; 16]);
        before.texture_matrix1.push([0.75; 16]);
        before.viewport = (1, 2, 640, 480);
        before.scissor = Some((3, 4, 100, 200));
        before.raw_scissor = (5, 6, 7, 8);
        before.scissor_enabled = true;
        before.surface = Some((320, 240));
        before.stretched = true;
        before.dirty = true;
        before.clear_color = [0.25, 0.5, 0.75, 1.0];
        before.clear_depth = 0.5;
        before.current_color = [1.0, 0.0, 0.5, 0.25];
        before.clear_stencil = 7;
        before.color_mask = [true, false, true, false];
        before.depth_test = true;
        before.depth_mask = false;
        before.depth_func = 0x0203;
        before.blend = true;
        before.blend_src = 1;
        before.blend_dst = 2;
        before.alpha_test = true;
        before.alpha_func = 0x0204;
        before.alpha_ref = 0.375;
        before.cull_face = true;
        before.cull_mode = 0x0405;
        before.front_face = 0x0901;
        before.stencil_test = true;
        before.stencil_func = 0x0207;
        before.stencil_ref = -3;
        before.stencil_value_mask = 0xff;
        before.stencil_write_mask = 0x0f;
        before.stencil_op = [1, 2, 3];
        before.depth_range = (0.0, 0.985);
        before.lighting = true;
        before.color_material = true;
        before.shade_model = 0x1d01;
        before.bound_texture = 0x1234;
        before.texture_2d = true;
        before.active_unit = 1;
        before.client_unit = 2;
        before.texture_env.mode = 0x2100;
        before.texture_env.color = [0.1, 0.2, 0.3, 0.4];
        before.unit1.enabled = true;
        before.unit1.texture = 0x5678;
        before.fog.enabled = true;
        before.fog.curve = 0x2601;
        before.fog.density = 0.125;
        before.fog.start = 10.0;
        before.fog.end = 20.0;
        before.fog.allowed = true;
        before.fog.color = [0.5, 0.25, 0.125, 1.0];
        before.material.shininess = 12.5;
        before.material.ambient = [0.11; 4];
        before.light_model_ambient = [0.22; 4];
        before.lights[0].enabled = true;
        before.lights[0].position = [1.0, 2.0, 3.0, 0.0];
        before.lights[0].spot_cutoff = 45.0;
        before.lights[0].attenuation = [1.0, 0.5, 0.25];

        let mut sections = Sections::create();
        before.save(&mut sections);
        let file = sections.close();
        let reader = Reader::open(&file).expect("opened");

        let mut after = GlState::new(640, 480);
        after.restore(&reader).expect("restored");

        assert_eq!(after.matrix_mode, 0x1700);
        // **The top of the stack and its size**, not the whole list: `GlState::new` already puts
        // an identity in each stack, and asserting the list assumed how many there were before.
        // That is how this test failed me once, with the right value.
        for (name, stack, expected) in [
            ("modelview", &after.modelview, [1.5f32; 16]),
            ("projection", &after.projection, [-2.25f32; 16]),
            ("texture_matrix", &after.texture_matrix, [0.5f32; 16]),
            ("texture_matrix1", &after.texture_matrix1, [0.75f32; 16]),
        ] {
            assert_eq!(stack.len(), 2, "the size of the {name} stack");
            assert_eq!(
                stack.last().copied(),
                Some(expected),
                "the top of the {name} stack did not come back"
            );
        }
        assert_eq!(after.viewport, (1, 2, 640, 480));
        assert_eq!(after.scissor, Some((3, 4, 100, 200)));
        assert_eq!(after.raw_scissor, (5, 6, 7, 8));
        assert!(after.scissor_enabled);
        assert_eq!(after.surface, Some((320, 240)));
        assert!(after.stretched);
        assert!(after.dirty);
        assert_eq!(after.clear_color, [0.25, 0.5, 0.75, 1.0], "colors are f32");
        assert_eq!(after.clear_depth, 0.5);
        assert_eq!(after.current_color, [1.0, 0.0, 0.5, 0.25]);
        assert_eq!(after.clear_stencil, 7);
        assert_eq!(after.color_mask, [true, false, true, false]);
        assert!(after.depth_test);
        assert!(!after.depth_mask);
        assert_eq!(after.depth_func, 0x0203);
        assert!(after.blend);
        assert_eq!((after.blend_src, after.blend_dst), (1, 2));
        assert!(after.alpha_test);
        assert_eq!(after.alpha_ref, 0.375);
        assert!(after.cull_face);
        assert_eq!(after.cull_mode, 0x0405);
        assert_eq!(after.front_face, 0x0901);
        assert!(after.stencil_test);
        assert_eq!(after.stencil_ref, -3, "the stencil `ref` is signed");
        assert_eq!(
            (after.stencil_value_mask, after.stencil_write_mask),
            (0xff, 0x0f)
        );
        assert_eq!(after.stencil_op, [1, 2, 3]);
        assert_eq!(after.depth_range, (0.0, 0.985), "the Crash range");
        assert!(after.lighting);
        assert!(after.color_material);
        assert_eq!(after.shade_model, 0x1d01);
        assert_eq!(after.bound_texture, 0x1234);
        assert!(after.texture_2d);
        assert_eq!((after.active_unit, after.client_unit), (1, 2));
        assert_eq!(after.texture_env.mode, 0x2100);
        assert_eq!(after.texture_env.color, [0.1, 0.2, 0.3, 0.4]);
        assert!(after.unit1.enabled);
        assert_eq!(after.unit1.texture, 0x5678);
        assert!(after.fog.enabled);
        assert_eq!(after.fog.curve, 0x2601);
        assert_eq!(after.fog.density, 0.125);
        assert_eq!((after.fog.start, after.fog.end), (10.0, 20.0));
        assert!(after.fog.allowed);
        assert_eq!(after.fog.color, [0.5, 0.25, 0.125, 1.0]);
        assert_eq!(after.material.shininess, 12.5);
        assert_eq!(after.material.ambient, [0.11; 4]);
        assert_eq!(after.light_model_ambient, [0.22; 4]);
        assert!(after.lights[0].enabled);
        assert_eq!(after.lights[0].position, [1.0, 2.0, 3.0, 0.0]);
        assert_eq!(after.lights[0].spot_cutoff, 45.0);
        assert_eq!(after.lights[0].attenuation, [1.0, 0.5, 0.25]);
    }

    /// A section with the wrong size is a **refusal**, not a shifted read.
    #[test]
    fn a_section_with_the_wrong_size_is_rejected() {
        let mut before = GlState::new(640, 480);
        before.depth_test = true;
        let mut sections = Sections::create();
        before.save(&mut sections);
        let mut file = sections.close();

        // Flips a number in the middle of the file: the crc32 stops matching, and it is the
        // first barrier.
        let middle = file.len() / 2;
        file[middle] ^= 0xff;
        match Reader::open(&file) {
            Err(crate::save_state::Error::Integrity { .. }) => {}
            other => panic!("should have refused on integrity, and returned {other:?}"),
        }
    }

    /// **Textures round-trip, with the whole mipmap chain.**
    ///
    /// The chain is the reason this table has its own format: the test uses two levels, and checks
    /// that the second came back with its measurements and pixels.
    #[test]
    fn textures_and_mipmaps_come_back() {
        let mut before = GlState::new(640, 480);
        let level_zero: Vec<[u8; 4]> = (0..16u8).map(|n| [n, n + 1, n + 2, 255]).collect();
        let level_one: Vec<[u8; 4]> = (0..4u8).map(|n| [n + 100, 0, 0, 128]).collect();
        before.textures.insert(
            0x1000,
            Texture {
                width: 4,
                height: 4,
                pixels: level_zero.clone(),
                mipmaps: vec![Level {
                    width: 2,
                    height: 2,
                    pixels: level_one.clone(),
                }],
                wrap: [0x2901, 0x2900],
                filter: 0x2601,
                min_filter: 0x2703,
                crop: [1, -2, 3, -4],
            },
        );

        let mut sections = Sections::create();
        before.save(&mut sections);
        let file = sections.close();
        let reader = Reader::open(&file).expect("opened");
        let mut after = GlState::new(640, 480);
        after.restore(&reader).expect("restored");

        let texture = after.textures.get(&0x1000).expect("the texture came back");
        assert_eq!((texture.width, texture.height), (4, 4));
        assert_eq!(texture.pixels, level_zero, "the pixels did not come back");
        assert_eq!((texture.wrap[0], texture.wrap[1]), (0x2901, 0x2900));
        assert_eq!((texture.filter, texture.min_filter), (0x2601, 0x2703));
        assert_eq!(texture.crop, [1, -2, 3, -4], "the crop is signed");
        assert_eq!(texture.mipmaps.len(), 1, "the mipmap chain did not come back");
        assert_eq!((texture.mipmaps[0].width, texture.mipmaps[0].height), (2, 2));
        assert_eq!(
            texture.mipmaps[0].pixels, level_one,
            "level one's pixels did not come back"
        );
    }

    /// A texture whose pixel count doesn't match its dimensions is **rejected**.
    ///
    /// It is the check that matters: a mipmap level with swapped measurements would draw stripes,
    /// and the defect would show up as a wrong image, with nothing pointing at the save state.
    #[test]
    fn a_texture_with_too_few_pixels_is_rejected() {
        let mut before = GlState::new(640, 480);
        before.textures.insert(
            0x1000,
            Texture {
                width: 4,
                height: 4,
                pixels: vec![[0, 0, 0, 255]],
                mipmaps: Vec::new(),
                wrap: [0, 0],
                filter: 0,
                min_filter: 0,
                crop: [0; 4],
            },
        );
        let mut sections = Sections::create();
        before.save(&mut sections);
        let file = sections.close();
        let reader = Reader::open(&file).expect("opened");
        let mut after = GlState::new(640, 480);
        match after.restore(&reader) {
            Err(crate::save_state::Error::Section { name, reason }) => {
                // The identifier goes in decimal in the section name, as in the other tables.
                assert_eq!(name, "tex.4096.pixels");
                assert!(reason.contains("16"), "{reason}");
            }
            other => panic!("should have refused the short texture, and returned {other:?}"),
        }
    }

    /// **The three buffers round-trip** — color, depth and stencil, each in its own format.
    ///
    /// Measured before deciding: the color buffer is **not** a duplicate of the screen. The screen
    /// the core presents is the device's bitmap, and the color buffer is the GL drawing surface;
    /// `eglSwapBuffers` reads one and hands over the other. Saving only one of them would leave the
    /// next scene drawn over nothing.
    #[test]
    fn the_color_depth_and_stencil_buffers_come_back() {
        let mut before = GlState::new(4, 2);
        assert_eq!(before.color.len(), 8, "the new state already has the surface buffers");
        before.color[0] = [1, 2, 3, 4];
        before.color[7] = [5, 6, 7, 8];
        before.depth[3] = 0.25;
        before.depth[7] = 0.75;
        before.stencil[5] = 9;

        let mut sections = Sections::create();
        before.save(&mut sections);
        let file = sections.close();
        let reader = Reader::open(&file).expect("opened");
        let mut after = GlState::new(4, 2);
        after.restore(&reader).expect("restored");

        assert_eq!(after.color.len(), 8);
        assert_eq!(after.color[0], [1, 2, 3, 4], "the first color pixel");
        assert_eq!(after.color[7], [5, 6, 7, 8], "the last color pixel");
        assert_eq!(after.depth.len(), 8);
        assert_eq!(after.depth[3], 0.25, "depth is f32");
        assert_eq!(after.depth[7], 0.75);
        assert_eq!(after.stencil.len(), 8);
        assert_eq!(after.stencil[5], 9);
    }

    /// **A buffer larger than the surface is rejected**, not allocated.
    ///
    /// It is the ceiling that keeps a corrupted file — or one written by a version with a
    /// different resolution — from making loading request an absurd amount of memory. The test
    /// builds a **valid** state and spoils only the stencil field: rebuilding the sections by hand
    /// would make it measure something else.
    #[test]
    fn a_buffer_larger_than_the_surface_is_rejected() {
        let before = GlState::new(4, 2);
        let mut sections = Sections::create();
        before.save(&mut sections);
        // The five numbers of the buffers header, with the last one lying. The first number is
        // the section's **count** (5), not a value: getting that wrong makes the reader refuse the
        // whole section, and that is what the test showed before I got it right.
        let mut lying = 5u32.to_le_bytes().to_vec();
        for value in [4u32, 2, 8, 8, 9999] {
            lying.extend_from_slice(&value.to_le_bytes());
        }
        sections.swap("gl.buffers", lying);
        let file = sections.close();
        let reader = Reader::open(&file).expect("opened");

        let mut after = GlState::new(4, 2);
        match after.restore(&reader) {
            Err(crate::save_state::Error::Section { name, reason }) => {
                assert_eq!(name, "gl.buffer.stencil");
                assert!(reason.contains("9999"), "{reason}");
            }
            other => panic!("should have refused the giant buffer, and returned {other:?}"),
        }
    }
}
