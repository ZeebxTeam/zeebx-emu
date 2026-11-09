//! The console frame painted by the host's GL, inside the egui window.
//!
//! The normal path uploads the frame as an egui texture: for that it is converted from RGB565 to
//! ARGB and then to RGBA, two vectors per frame. Here the same frame goes to the GPU **in the
//! format it is already in** — `GL_UNSIGNED_SHORT_5_6_5` is a format OpenGL accepts
//! directly —, and scaling it up to the window size is the GPU's job.
//!
//! **This is not the rasterizer on the GPU.** 3D is still rasterized in software; all that
//! changes is the last step, putting the finished frame on screen. It exists to prove we know how
//! to draw with GL inside egui without touching the emulator — and it is on top of this piece that
//! a GPU backend would be built later.
//!
//! The desktop context is **core 3.3** (Qt asks for it that way, and the headless glutin resolves
//! "no profile, no version" the same way), so there is no fixed pipeline: even a textured quad
//! needs a shader.

// `egui` comes directly, and **not** through `eframe`: the re-export tied this module to the
// desktop, and what draws here is the engine, which has no window at all.
use glow::{self, HasContext};

/// The triangle that covers the whole screen, generated without a vertex array.
///
/// Three vertices cover the [-1, 1] rectangle with room to spare, and the GPU's clipping discards
/// the rest. It is cheaper than a quad made of two triangles, and — what matters more here — needs
/// no vertex buffer: `gl_VertexID` is the only input.
const VERTEX: &str = r#"
uniform vec2 recorte;
out vec2 uv;
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    // The texture's origin is at the top; OpenGL's, at the bottom. Flipping here avoids flipping the
    // whole frame on the CPU. `recorte` (the crop) is the fraction of the texture that is image: the
    // GPU's 3D frame can have the game surface smaller than the attachment.
    uv = vec2(p.x, 1.0 - p.y) * recorte;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"
in vec2 uv;
uniform sampler2D quadro;
out vec4 cor;
void main() {
    cor = texture(quadro, uv);
}
"#;

/// The rectangle of the window where the frame goes, in physical pixels, counted **from the
/// bottom** — which is how `glViewport` wants it.
///
/// It belongs to the painter, and not to egui, because whoever paints does not always have egui on
/// screen: the headless one builds the area by hand, and the Qt interface gets it from Qt Quick.
/// egui still comes in through `From`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub x: i32,
    pub bottom_y: i32,
    pub width: i32,
    pub height: i32,
}

impl From<egui::epaint::ViewportInPixels> for Viewport {
    fn from(vp: egui::epaint::ViewportInPixels) -> Self {
        Self {
            x: vp.left_px,
            bottom_y: vp.from_bottom_px,
            width: vp.width_px,
            height: vp.height_px,
        }
    }
}

/// The program and the texture that paint the frame.
pub struct GlPainter {
    program: glow::Program,
    vao: glow::VertexArray,
    texture: glow::Texture,
    /// Size of the texture already on the GPU, to reuse it while it does not change.
    measured: (i32, i32),
    smooth: bool,
    /// Identity/contents of the last framebuffer uploaded through the cached path.
    last_frame: Option<(u64, u64)>,
}

impl GlPainter {
    /// Builds the program. Tries GLSL 3.30 and falls back to ES 3.00 — the desktop provides OpenGL
    /// and Android provides GLES, so both cases exist.
    pub fn create(gl: &glow::Context) -> Result<Self, String> {
        let program = ["#version 330 core\n", "#version 300 es\nprecision mediump float;\n"]
            .into_iter()
            .find_map(|header| unsafe { compile(gl, header) }.ok())
            .ok_or("no GLSL version accepted")?;
        unsafe {
            let vao = gl.create_vertex_array()?;
            let texture = gl.create_texture()?;
            Ok(Self {
                program,
                vao,
                texture,
                measured: (0, 0),
                smooth: false,
                last_frame: None,
            })
        }
    }

    /// Uploads the frame and draws it in the requested rectangle.
    ///
    /// `bytes` is the frame in RGB565, the way the console surface stores it.
    pub fn draw(
        &mut self,
        gl: &glow::Context,
        bytes: &[u8],
        width: i32,
        height: i32,
        vp: Viewport,
        smooth: bool,
    ) {
        self.draw_impl(gl, bytes, width, height, None, vp, smooth);
    }

    /// Like draw, but does not resend the pixels if the framebuffer has not changed.
    ///
    /// The key is the framebuffer's (serial, writes): the serial tells different surfaces apart and
    /// writes grows every time the game touches that surface.
    pub fn draw_frame(
        &mut self,
        gl: &glow::Context,
        bytes: &[u8],
        width: i32,
        height: i32,
        key_id: (u64, u64),
        vp: Viewport,
        smooth: bool,
    ) {
        self.draw_impl(gl, bytes, width, height, Some(key_id), vp, smooth);
    }

    fn draw_impl(
        &mut self,
        gl: &glow::Context,
        bytes: &[u8],
        width: i32,
        height: i32,
        key_id: Option<(u64, u64)>,
        vp: Viewport,
        smooth: bool,
    ) {
        if width <= 0 || height <= 0 || bytes.len() < (width * height * 2) as usize {
            return;
        }
        unsafe {
            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            // Each row has `width * 2` bytes: the default alignment of four would only hold for an
            // even width, and a frame of odd width would come out skewed.
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 2);
            // **The storage is allocated once, and each frame only swaps the contents.** Calling
            // `tex_image_2d` every frame reallocates the whole texture on the GPU — it is the
            // stumble zeebulator documents, and it was what was here.
            if self.measured != (width, height) {
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGB as i32,
                    width,
                    height,
                    0,
                    glow::RGB,
                    glow::UNSIGNED_SHORT_5_6_5,
                    glow::PixelUnpackData::Slice(None),
                );
            }
            let needs_upload =
                self.measured != (width, height) || key_id.is_none() || self.last_frame != key_id;
            if needs_upload {
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    width,
                    height,
                    glow::RGB,
                    glow::UNSIGNED_SHORT_5_6_5,
                    glow::PixelUnpackData::Slice(Some(bytes)),
                );
                self.last_frame = key_id;
            }
            if self.measured != (width, height) || self.smooth != smooth {
                self.measured = (width, height);
                self.smooth = smooth;
                // The console pixel is big and square: scaling up without interpolation is the
                // default, and smoothing is the viewer's choice — the same rule as the egui path.
                let filter = match smooth {
                    true => glow::LINEAR,
                    false => glow::NEAREST,
                } as i32;
                for (name, value) in [
                    (glow::TEXTURE_MIN_FILTER, filter),
                    (glow::TEXTURE_MAG_FILTER, filter),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, name, value);
                }
            }
            self.paint(gl, [1.0, 1.0], vp);
        }
    }

    /// Draws a texture that is already on the GPU: the 3D frame at the internal resolution.
    ///
    /// The texture belongs to the rasterizer, in the same context egui uses. Scaling up or down to
    /// the window rectangle is the GPU's job, with the same filter chosen for the normal path.
    pub fn draw_texture(
        &mut self,
        gl: &glow::Context,
        frame: crate::video::rasterizer::GpuFrame,
        vp: Viewport,
        smooth: bool,
    ) {
        unsafe {
            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(frame.texture));
            let filter = match smooth {
                true => glow::LINEAR,
                false => glow::NEAREST,
            } as i32;
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
            self.paint(gl, frame.clip, vp);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }

    /// The triangle with the texture bound to unit zero, and the state handed back to egui.
    unsafe fn paint(&self, gl: &glow::Context, clip: [f32; 2], vp: Viewport) {
        unsafe {
            if let Some(local) = gl.get_uniform_location(self.program, "quadro") {
                gl.uniform_1_i32(Some(&local), 0);
            }
            if let Some(local) = gl.get_uniform_location(self.program, "recorte") {
                gl.uniform_2_f32(Some(&local), clip[0], clip[1]);
            }
            gl.viewport(vp.x, vp.bottom_y, vp.width, vp.height);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            // Hand back the state we touched: egui draws the rest of the interface after us, and
            // a program or VAO left bound shows up as an untextured interface.
            //
            // **This is what breaks the rasterizer's cache when it draws in the same context.** It
            // binds program, VAO and VBO once and not on every batch, and it is the one that has to
            // know someone else may touch them — see `GpuState::hand_back_context`: the cache is
            // invalidated there, on every hand-back, when the one presenting the frame is the host.
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    /// Returns to the GPU what was created. Whoever built the painter calls this when closing the window.
    pub fn release(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_texture(self.texture);
        }
    }
}

/// Compiles and links the shader pair with the given version header.
unsafe fn compile(gl: &glow::Context, header: &str) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, source) in [
            (glow::VERTEX_SHADER, VERTEX),
            (glow::FRAGMENT_SHADER, FRAGMENT),
        ] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, &format!("{header}{source}"));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let error = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                for shader in shaders {
                    gl.delete_shader(shader);
                }
                gl.delete_program(program);
                return Err(error);
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            let error = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(error);
        }
        Ok(program)
    }
}
