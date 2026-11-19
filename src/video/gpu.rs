//! The rasterizer on the GPU: the same pipeline, with the fill done by the host's OpenGL.
//!
//! **This is not a reimplementation of the GLES1 fixed pipeline.** It would not be needed: the
//! [`GlState`] already does the whole vertex stage on the CPU — modelview, projection, texture
//! matrix and per-vertex lighting —, and by the time the triangles reach the fill the vertices are
//! in clip space with final color and `uv`. That is exactly what a GPU expects to receive.
//!
//! So this backend **contains** a `GlState`, uses it for all the state bookkeeping and for the
//! vertex stage, and replaces only what the measurement pointed out as expensive: the fill. Having
//! a single source for matrices and light is what makes the comparison between the two honest — if
//! the light is wrong, it will be wrong the same way in both, and whatever difference remains
//! belongs to the fill.
//!
//! A welcome side effect: the ~100 thousand `glEnable`/`glDisable` calls that the Z-Wheel and Crash
//! make stop being GL calls one by one. The state is applied **once per draw**, from what was noted
//! down here.

// Creating **our own** context belongs to the `gpu` feature, which brings in glutin. With only
// `gl`, the backend draws on the context the caller handed over — which is the case of the Libretro
// core, and the reason the two features exist separately.
#[cfg(feature = "gpu")]
use super::context::GlContext;

/// Our own context, when this build knows how to open one.
///
/// With only the `gl` feature the type is unit: the field exists and stays empty, and type
/// inference in the `match` below needs no annotation on two arms that exist in only one of them.
#[cfg(feature = "gpu")]
type OwnContext = GlContext;
#[cfg(not(feature = "gpu"))]
type OwnContext = ();
use super::gles;
use super::rasterizer::{
    GlState, Matrix, GpuFrame, Rasterizer, TexEnv, Texture as SavedTexture, DEFAULT_UV,
    TextureUnit, Vertex, lines_to_triangles,
};
use glow::{self, HasContext};
use std::collections::HashMap;

// vitaGL keeps here the block of the texture bound on the active unit.
// `vglBufferData` only stores the pointer in the bound buffer: it does not allocate memory.
#[cfg(target_os = "vita")]
unsafe extern "C" {
    fn vglGetTexDataPointer(target: u32) -> *mut u8;
    fn vglMemalign(alignment: u32, size: u32) -> *mut u8;
    fn vglBufferData(target: u32, data: *const u8);
}

/// Copies the batch into a stable region and points the buffer at it.
///
/// On vitaGL the draw does not take the vertices: it keeps the pointer and the GPU reads
/// later, at the end of the scene. Rewriting the same block for every sprite makes the
/// next one get the previous one's `uv` — a square from another sheet. The ring
/// only wraps back to the start when it fills up, and `glFinish` waits for the GPU to release the
/// pointers. `glBufferData` won't do: it asks for the whole ring again and, without a
/// page, sleeps for a second.
#[cfg(target_os = "vita")]
fn write_vita_block(gl: &glow::Context, vbo: glow::Buffer, bytes: &[u8]) -> bool {
    use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
    static BLOCK: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
    static BLOCK_BYTES: AtomicUsize = AtomicUsize::new(0);
    static CURSOR: AtomicUsize = AtomicUsize::new(0);
    if bytes.is_empty() {
        return true;
    }
    // 8 MiB fit a whole frame of sprites. Running out in the middle of the frame
    // would call `glFinish` with the scene open.
    const RING: usize = 8 << 20;
    let step = bytes.len().div_ceil(16) * 16;
    let mut base = BLOCK.load(Ordering::Relaxed);
    let mut capacity = BLOCK_BYTES.load(Ordering::Relaxed);
    if base.is_null() {
        let size = step.max(RING);
        let Ok(size_u32) = u32::try_from(size) else {
            return false;
        };
        base = unsafe { vglMemalign(16, size_u32) };
        if base.is_null() {
            return false;
        }
        BLOCK.store(base, Ordering::Relaxed);
        BLOCK_BYTES.store(size, Ordering::Relaxed);
        capacity = size;
        CURSOR.store(0, Ordering::Relaxed);
    }
    if step > capacity {
        return false;
    }
    let mut cursor = CURSOR.load(Ordering::Relaxed);
    if cursor.saturating_add(step) > capacity {
        unsafe { gl.finish() };
        cursor = 0;
    }
    let dest = unsafe { base.add(cursor) };
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dest, bytes.len());
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        vglBufferData(glow::ARRAY_BUFFER, dest);
    }
    CURSOR.store(cursor + step, Ordering::Relaxed);
    true
}

/// Writes RGBA8 into the block vitaGL already allocated for the bound texture.
///
/// `glTexImage2D` and `glTexSubImage2D` ask for a new block. When the game's physical partition
/// runs out, the next `glClear` dies in `sceGxmReserveVertexDefaultUniformBuffer`:
/// that was the coredump with the stack in `GpuState::dest`. The returned pointer is the block
/// of the first image, linear RGBA, with the width rounded up to 8 — the stride
/// that `gpu_alloc_texture` stored.
#[cfg(target_os = "vita")]
unsafe fn write_rgba_vita(
    pixels: &[u8],
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    total_width: usize,
    total_height: usize,
) -> bool {
    if width == 0
        || height == 0
        || total_width == 0
        || total_height == 0
        || x.saturating_add(width) > total_width
        || y.saturating_add(height) > total_height
    {
        return false;
    }
    let src_stride = width * 4;
    let needed = src_stride.saturating_mul(height);
    if pixels.len() < needed {
        return false;
    }
    let ptr = unsafe { vglGetTexDataPointer(glow::TEXTURE_2D) };
    if ptr.is_null() {
        return false;
    }
    let dest_stride = total_width.div_ceil(8) * 8 * 4;
    unsafe {
        if x == 0 && width == total_width && src_stride == dest_stride {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr.add(y * dest_stride), needed);
        } else {
            for line in 0..height {
                std::ptr::copy_nonoverlapping(
                    pixels.as_ptr().add(line * src_stride),
                    ptr.add((y + line) * dest_stride + x * 4),
                    src_stride,
                );
            }
        }
    }
    true
}

/// `GL_TEXTURE_MAX_ANISOTROPY` and the maximum the GPU accepts, from the
/// `EXT_texture_filter_anisotropic` extension (core in OpenGL 4.6).
const TEXTURE_MAX_ANISOTROPY: u32 = 0x84fe;
const MAX_TEXTURE_MAX_ANISOTROPY: u32 = 0x84ff;

/// How many `f32` each vertex takes in the buffer: position, color and texture coordinate.
const FLOATS_PER_VERTEX: usize = 4 + 4 + 2 + 1 + 2;

/// How many vertices fit in the vertex buffer ring. See [`GpuState::ring`].
/// On the Vita the batch does not go through this ring.
#[cfg_attr(target_os = "vita", allow(dead_code))]
const RING_VERTICES: usize = 1 << 16;

/// How many vertices a batch gathers before going to the GPU even without a state change.
const BATCH_VERTICES: usize = 1 << 14;

/// What the GPU needs to know about a game texture, beyond the pixels already in it.
struct Texture {
    object: glow::Texture,
    width: usize,
    height: usize,
    /// The highest mipmap level already uploaded. A filter that asks for mipmaps without the full
    /// chain draws black in OpenGL — so, when only level zero exists, the filter is downgraded.
    max_level: u32,
    /// Level zero already has storage on the GPU. Repeating the image at the same size
    /// does not call `glTexImage2D` again: on vitaGL that asks for another physical block.
    level_zero: bool,
    crop: [i32; 4],
    filter: u32,
    min_filter: u32,
    wrap: [u32; 2],
}

/// The fill state, noted down from the calls and applied once per draw.
///
/// It is also the batch key: consecutive draws with the same state go to the GPU together. See
/// [`GpuState::batch`].
#[derive(Clone, PartialEq)]
struct State {
    depth_test: bool,
    depth_mask: bool,
    /// The `glDepthRange`, `(near, far)`.
    depth_range: (f32, f32),
    /// The fog at draw time. The per-vertex factor comes from the vertex stage; here there is
    /// only the "is it on" and the color, which the fragment shader reads.
    fog: crate::video::rasterizer::Fog,
    depth_func: u32,
    blend: bool,
    blend_src: u32,
    blend_dst: u32,
    alpha_test: bool,
    alpha_func: u32,
    alpha_ref: f32,
    color_mask: [bool; 4],
    cull_face: bool,
    cull_mode: u32,
    front_face: u32,
    stencil_test: bool,
    stencil_func: u32,
    stencil_ref: i32,
    stencil_value_mask: u32,
    stencil_write_mask: u32,
    stencil_op: [u32; 3],
    texture_env: TexEnv,
    /// Texture unit 1, mirrored from the software state.
    unit1: TextureUnit,
    bound_texture: u32,
    texturing: bool,
    /// The viewport as the game passed it, with `y` going bottom to top.
    viewport: (i32, i32, i32, i32),
    /// An internal viewport already counted from the top, which does not go through the
    /// conversion — the one from [`GpuState::import_rgb565_changes`]. See
    /// [`GpuState::top_down_viewport`].
    fixed_top_down_viewport: Option<(i32, i32, i32, i32)>,
    /// The `glScissor` as the game passed it, and whether `GL_SCISSOR_TEST` is on.
    scissor: (i32, i32, i32, i32),
    scissor_enabled: bool,
    clear_color: [f32; 4],
    clear_depth: f32,
    clear_stencil: i32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            depth_test: false,
            depth_mask: true,
            depth_range: (0.0, 1.0),
            fog: crate::video::rasterizer::Fog::default(),
            depth_func: gles::GL_LESS,
            blend: false,
            blend_src: gles::GL_ONE,
            blend_dst: gles::GL_ZERO,
            alpha_test: false,
            alpha_func: gles::GL_ALWAYS,
            alpha_ref: 0.0,
            color_mask: [true; 4],
            cull_face: false,
            cull_mode: gles::GL_BACK,
            front_face: gles::GL_CCW,
            stencil_test: false,
            stencil_func: gles::GL_ALWAYS,
            stencil_ref: 0,
            stencil_value_mask: u32::MAX,
            stencil_write_mask: u32::MAX,
            stencil_op: [gles::GL_KEEP; 3],
            texture_env: TexEnv::default(),
            unit1: TextureUnit::default(),
            bound_texture: 0,
            texturing: false,
            viewport: (0, 0, 0, 0),
            fixed_top_down_viewport: None,
            scissor: (0, 0, 0, 0),
            scissor_enabled: false,
            clear_color: [0.0, 0.0, 0.0, 1.0],
            clear_depth: 1.0,
            clear_stencil: 0,
        }
    }
}

/// Whether this GPU's `glBlitFramebuffer` delivers what it promises.
///
/// **The advertisement is not enough.** Flycast, which runs on the bad Android and handheld
/// drivers, does not trust it: it does a real blit and, when the result does not match, draws a
/// quad instead. Here the blit is not decoration either — it is the MSAA `resolve` and the
/// downscale of the big frame before readback —, and a blit that lies **raises no error**: it gives
/// a wrong image silently, which is exactly the kind of defect that only shows up on the device of
/// whoever is playing.
///
/// The internal color format. In OpenGL ES 2 the internal format must be the same as `format`:
/// `RGBA8` is from ES 3, and vitaGL rejects the sized format even though it has the token.
fn color_format(gl: &glow::Context) -> u32 {
    let version = gl.version();
    if version.is_embedded && version.major < 3 {
        glow::RGBA
    } else {
        glow::RGBA8
    }
}

/// The test is minimal on purpose: 2×2, a flat color, a round trip through the CPU. It costs two
/// textures and two framebuffers, once per context.
#[cfg(not(target_arch = "wasm32"))]
fn blit_works(gl: &glow::Context) -> bool {
    use glow::HasContext;

    /// The color written on one side and checked on the other.
    const MARKER: [u8; 4] = [0x00, 0xff, 0x00, 0xff];

    (|| -> Option<bool> {
        unsafe {
            let create = |width: u32| -> Option<(glow::Texture, glow::Framebuffer)> {
                let color = gl.create_texture().ok()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(color));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    color_format(&gl) as i32,
                    width as i32,
                    width as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                let fbo = gl.create_framebuffer().ok()?;
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(color),
                    0,
                );
                Some((color, fbo))
            };

            let (color_a, fbo_a) = create(2)?;
            let (color_b, fbo_b) = create(2)?;

            // Out: paint side A. The scissor stays off because it belongs to the game and may be
            // clipping outside this piece.
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo_a));
            gl.clear_color(
                f32::from(MARKER[0]) / 255.0,
                f32::from(MARKER[1]) / 255.0,
                f32::from(MARKER[2]) / 255.0,
                f32::from(MARKER[3]) / 255.0,
            );
            gl.clear(glow::COLOR_BUFFER_BIT);

            // Back: copy A to B through the same path that `resolve` and the downscale use.
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fbo_a));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(fbo_b));
            gl.blit_framebuffer(
                0,
                0,
                2,
                2,
                0,
                0,
                2,
                2,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo_b));
            let mut loaded = [0u8; 4];
            gl.read_pixels(
                0,
                0,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut loaded)),
            );

            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.delete_framebuffer(fbo_a);
            gl.delete_framebuffer(fbo_b);
            gl.delete_texture(color_a);
            gl.delete_texture(color_b);

            // **A pending error counts as failure.** A blit that returns the right image and leaves
            // an error pending is a driver that accepted by luck, and the error would show up later,
            // far from here, in the first call that consumed it.
            let clean = gl.get_error() == glow::NO_ERROR;
            Some(loaded == MARKER && clean)
        }
    })()
    .unwrap_or(false)
}

/// What is already on the GPU, so as not to resend what has not changed.
///
/// **Every field starts as `None`, and `None` means "unknown".** That is what makes invalidation
/// trivial: forgetting the mirror is going back to the default value, and from there everything is
/// resent once — which is what [`GpuState::forget_mirror`] does when someone else touches the
/// context.
///
/// There are eighteen GL calls per batch in [`GpuState::apply`], and they are almost always the
/// same eighteen as the previous batch: the same program, the same fog, the same scissor. A game
/// that draws hundreds of times per frame pays for that hundreds of times.
#[derive(Clone, Copy, PartialEq)]
struct Mirror {
    viewport: Option<(i32, i32, i32, i32)>,
    scissor: Option<(i32, i32, i32, i32)>,
    scissor_enabled: Option<bool>,
    depth_clamp: Option<bool>,
    depth_test: Option<bool>,
    depth_func: Option<u32>,
    depth_mask: Option<bool>,
    depth_range: Option<(f32, f32)>,
    blend: Option<bool>,
    blend_func: Option<(u32, u32)>,
    color_mask: Option<[bool; 4]>,
    cull_face: Option<bool>,
    cull_mode: Option<u32>,
    front_face: Option<u32>,
    stencil_test: Option<bool>,
    stencil_func: Option<(u32, i32, u32)>,
    stencil_mask: Option<u32>,
    stencil_ops: Option<[u32; 3]>,
}

impl Default for Mirror {
    /// Everything unknown: it is the "I just got the context from someone else" state.
    fn default() -> Self {
        Self {
            viewport: None,
            scissor: None,
            scissor_enabled: None,
            depth_clamp: None,
            depth_test: None,
            depth_func: None,
            depth_mask: None,
            depth_range: None,
            blend: None,
            blend_func: None,
            color_mask: None,
            cull_face: None,
            cull_mode: None,
            front_face: None,
            stencil_test: None,
            stencil_func: None,
            stencil_mask: None,
            stencil_ops: None,
        }
    }
}

impl Mirror {
    /// Marks the value as sent and says whether it **changed** — that is, whether the call is
    /// needed.
    ///
    /// Counts both sides on purpose: without the numbers there is no way to know whether the
    /// mirror is saving calls or just repeating what was already there.
    fn changed<T: PartialEq + Copy>(
        slot: &mut Option<T>,
        create: T,
        sent: &mut u64,
        saved: &mut u64,
    ) -> bool {
        if *slot == Some(create) {
            *saved += 1;
            false
        } else {
            *slot = Some(create);
            *sent += 1;
            true
        }
    }
}

/// **The GPU the frontend reported as dead**, by the address of the `glow::Context`, or zero.
///
/// The notice that a context is no longer valid comes from outside the emulator, **in the middle
/// of a frame**, and whoever receives it can no longer touch the context: the function pointers it
/// resolved no longer exist. An address, compared without dereferencing anything, is what is left —
/// and it is enough, because two `Arc` copies of the same GPU have the same address.
///
/// **An `AtomicUsize`, and not a lock**, because whoever writes here is the frontend's video thread
/// while the emulator draws: see the race note in `frontends/libretro/src/lib.rs`. A `Mutex` on the
/// draw path would trade a race for a deadlock.
static DEAD_GPU: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The address of a GPU: its identity between whoever created it and whoever draws on it.
pub fn gpu_address(gl: &glow::Context) -> usize {
    gl as *const glow::Context as usize
}

/// The frontend reported that the GPU at `address` is no longer valid.
///
/// In `libretro` both callbacks say this: `context_destroy` warns that the context is about to die
/// and `context_reset` that it was born again — and the ABI is explicit that the core's GL objects
/// are invalid in both cases (`libretro.h`: "When context_reset is called, OpenGL resources in
/// the libretro implementation are guaranteed to be invalid"), and that a `context_reset` may
/// arrive **without** `context_destroy`, when the context was lost from outside ("resources should
/// just be recreated without any attempt to free old resources").
pub fn gpu_destroyed(address: usize) {
    DEAD_GPU.store(address, std::sync::atomic::Ordering::Relaxed);
}

/// The GPU at `address` was born again: if the death mark belonged to the previous GPU **at that
/// same address**, it is cleared.
///
/// The address comes from the allocator, and a new GPU's may be that of one that died. Without
/// this cleanup the new GPU would be born marked as dead and would draw nothing, silently.
pub fn gpu_created(address: usize) {
    let _ = DEAD_GPU.compare_exchange(
        address,
        0,
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// What this state knows about the GPU it draws on: whose context it is, whether it is still valid
/// and what was left bound on it.
///
/// **It is separate from [`GpuState`] because of the house rule — keep no change without a
/// measurement.** A GL context does not exist in a unit test, but the three decisions below depend
/// on no driver at all, and they are the ones the GPU audit pointed out as wrong (findings 1, 2
/// and 4):
///
/// - [`Gpu::deletes_on_drop`]: whether `Drop` calls the `delete_*`. With the GPU dead, **it does
///   not**;
/// - [`Gpu::needs_bind`]: whether the next batch rebinds program, VAO and VBO, or reuses the cache;
/// - [`Gpu::is_dead`]: whether the borrowed GPU is gone — which can happen **within** a frame.
struct Gpu {
    /// Whether the context belongs to someone else — the window, or the frontend. In that case the
    /// state has to be handed back after each use (see [`GpuState::hand_back_context`]) and can be
    /// taken back by the owner.
    borrowed: bool,
    /// The address of the context, to recognize the notice that it died. See [`DEAD_GPU`].
    address: usize,
    /// Whether the **program**, the `vao` and the `vbo` are already bound on the GPU.
    ///
    /// The three are created once and never change, so binding them on every draw meant paying
    /// three driver calls per batch — and Quake's batches reach about 370 per frame. Worse:
    /// `submit_with` **unbound** both at the end of every draw, and on a thin ARM driver, like the
    /// Mali in handhelds, unbinding program and VAO is precisely what revalidates the most. Measured
    /// on RetroArch's GPU path, before and after (commit `e6a436e`).
    ///
    /// **What can drop one of the three is other code drawing with the same context — and there
    /// is some.** The window's `GlPainter` ends each paint with `use_program(None)` and
    /// `bind_vertex_array(None)` **on purpose**, to hand the context back to `egui`
    /// (`src/ui/gpu.rs`), and the context it paints on is the same one the rasterizer draws on (the
    /// headless window, `frontends/headless/src/window.rs`, and Android). That is why the cache is
    /// not invalidated only by the context being recreated: what invalidates it is
    /// [`GpuState::hand_back_context`] and [`GpuState::draw_to_fbo`].
    enabled: std::cell::Cell<bool>,
}

impl Gpu {
    /// This state's GPU, with the context freshly created and nothing bound on it.
    fn create(borrowed: bool, address: usize) -> Self {
        Self {
            borrowed,
            address,
            enabled: std::cell::Cell::new(false),
        }
    }

    /// Whether the context belongs to someone else. See [`GpuState::hand_back_context`].
    fn borrowed(&self) -> bool {
        self.borrowed
    }

    /// Whether the borrowed GPU is no longer valid.
    ///
    /// An own context never dies before its owner: whoever would close it is this very state, in
    /// `Drop`.
    fn is_dead(&self) -> bool {
        self.borrowed && self.address == DEAD_GPU.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether this state's GL names can be deleted.
    ///
    /// **With the GPU dead, they cannot.** Deleting means calling `delete_*` on the driver, and the
    /// functions this state kept died with the context: at best the driver records an error, at
    /// worst the pointer points to code that no longer exists. The names are **dropped**, and
    /// whoever would delete them no longer exists: releasing them later would mean touching a dead
    /// context.
    fn deletes_on_drop(&self) -> bool {
        !self.is_dead()
    }

    /// Whether the next batch needs to rebind program, VAO and VBO.
    fn needs_bind(&self) -> bool {
        !self.enabled.get()
    }

    /// The three went onto the GPU: the next batch can skip the three calls.
    fn mark_bound(&self) {
        self.enabled.set(true);
    }

    /// The context left our hands: what was bound on it is no longer ours.
    fn forget_bound(&self) {
        self.enabled.set(false);
    }
}

pub struct GpuState {
    /// The state bookkeeping and the vertex stage, shared with the software path.
    state: GlState,
    /// The context that **we** opened, when there was none.
    ///
    /// It is never read: it exists so it is not dropped while the backend lives. Dropping it would
    /// destroy the context the GL functions that `gl` just stored come from.
    _own: Option<OwnContext>,
    /// The GL functions: borrowed from the window, or from our own context.
    gl: std::sync::Arc<glow::Context>,
    /// The GPU this state draws on: whose context it is, whether it is still valid and what was
    /// left bound on it. See [`Gpu`].
    gpu: Gpu,
    fill: State,
    /// The destination: a color texture plus depth and stencil together.
    frame: Option<Destination>,
    program: glow::Program,
    /// Where each uniform of the program lives, and what was last sent to it.
    uniforms: UniformCache,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    /// The vertex buffer is a ring: `(capacity, next free)`, in vertices.
    ///
    /// Each draw writes to the next range and draws from it, and only when the ring fills up is the
    /// buffer requested again, whole. Redefining the buffer on every draw — the `buffer_data` —
    /// and redoing the attribute pointers cost 4 µs per call, and Quake makes about 370 per
    /// frame: that was what kept it below console speed, in slow motion.
    ring: (usize, usize),
    /// Whether the attribute pointers are already stored in the `vao`. They do not change: the
    /// vertex layout is a single one, and each draw's position in the ring goes in the `first` of
    /// `draw_arrays`.
    vao_ready: bool,
    /// **The gathered draws that have not gone to the GPU yet**, as loose triangles, with the state
    /// and the perspective in which they were requested.
    /// Quake draws face by face: one fan `glDrawArrays` for each polygon, about 370 per frame,
    /// almost all consecutive with the same texture and the same state. Each one went to the GPU
    /// alone — state, uniforms and the driver call, about 5 µs —, and the game did not fit in the
    /// 60 Hz frame. Gathered, the fan and the strip become triangles and the batch goes in a single
    /// call, when the state changes or when someone needs what was already drawn. See
    /// [`GpuState::flush`].
    batch: Vec<f32>,
    batch_state: Option<(State, bool)>,
    /// The `glDrawTex` calls not yet sent. Each one is a rectangle; consecutive ones, with the same
    /// texture and the same state, become a single draw. See [`GpuState::draw_texture`].
    sprites: Vec<f32>,
    sprite_state: Option<State>,
    /// A draw's transformed vertices, before they become triangles in the batch.
    loose: Vec<Vertex>,
    /// The support texture of [`GpuState::import_rgb565_changes`].
    bridge: glow::Texture,
    /// The already allocated size of [`GpuState::bridge`]. `None` until the first upload.
    bridge_size: Option<(i32, i32)>,
    textures: HashMap<u32, Texture>,
    /// Buffers reused between calls, so as not to allocate memory per frame.
    vertices: Vec<f32>,
    pixels: Vec<u8>,
    /// Whether anything was drawn since the last frame conversion.
    dirty: bool,
    /// The internal resolution, in multiples of the console frame. See [`Rasterizer::set_scale`].
    scale: usize,
    /// The frame reduced to the console size, where readbacks come from when `scale > 1`.
    downscaled: Option<(glow::Framebuffer, glow::Texture, (usize, usize))>,
    /// The front: the copy of the last presented frame, while the game is already drawing the next
    /// one in the destination. See [`GpuState::protect_front`].
    front: Option<(glow::Framebuffer, glow::Texture, (i32, i32))>,
    /// There was an `eglSwapBuffers`, and the destination is still the presented frame: the next
    /// write to it first copies to the front.
    swapped: bool,
    /// The front holds the presented frame and the destination has already changed: whoever
    /// presents reads the front.
    front_valid: bool,
    /// Samples per pixel of the antialias (MSAA); 1 is off. See [`Rasterizer::set_antialias`].
    samples: usize,
    /// The anisotropic filter applied to the game's textures; 1 is off.
    anisotropy: f32,
    /// The aspect ratio requested for 3D, width over height. `None` is the console's 4:3. See
    /// [`GpuState::extra`].
    ratio: Option<f32>,
    /// Whether the batch going to the GPU was transformed by a perspective projection.
    in_perspective: bool,
    /// Whether this GPU's `glBlitFramebuffer` passed the test in [`blit_works`].
    ///
    /// False turns off antialias and internal resolution: both depend on copying between
    /// framebuffers, and a blit that lies would give a wrong image silently.
    blit_reliable: bool,
    /// The external framebuffer to draw into, when the frontend provides one. `None` is our own.
    ///
    /// `Some(0)` is the frontend's default framebuffer — which `glow` writes as `None` in `bind`.
    /// See [`Rasterizer::draw_to_fbo`].
    external_fbo: Option<u32>,
    /// Whether the depth and stencil attachments are discarded after the frame. See
    /// [`GpuState::set_tile_discard`].
    discard_tiles: bool,
    /// What is already on the GPU. See [`Mirror`].
    mirror: std::cell::Cell<Mirror>,
    /// State calls sent and saved by the mirror, for checking.
    state_sends: std::cell::Cell<u64>,
    state_savings: std::cell::Cell<u64>,
    /// What goes into the periodic log report. See [`GpuState::end_frame`].
    measured: Stats,
}

/// Counts over a window of [`FRAMES_PER_REPORT`] frames.
///
/// It exists because of issue #70: Ridge Racer oscillates between 10 and 25 frames per second
/// after the second lap on an RTX 4090 on Windows, and here (AMD, Mesa) it stayed at 30.3 from
/// start to finish. Without the reporter's GPU, what is left is the log of whoever has one: if
/// textures or uploads grow over time, it shows here; if they stay flat while the real time per
/// frame rises, the weight is in the driver.
#[derive(Default)]
struct Stats {
    frames: u32,
    bytes_uploaded: u64,
    batches: u64,
    start: Option<std::time::Instant>,
    last: Option<std::time::Instant>,
    worst_frame: std::time::Duration,
}

/// 300 frames are 10 s at the console's rate of 30: spaced enough not to fill the log ring, short
/// enough to separate the first lap from the second.
const FRAMES_PER_REPORT: u32 = 300;

struct Destination {
    fbo: glow::Framebuffer,
    color: glow::Texture,
    depth: glow::Renderbuffer,
    /// In console pixels; the attachment has `(measured.0 + 2 * extra, measured.1) * scale`.
    measured: (usize, usize),
    scale: usize,
    /// The extra columns on each side, in console pixels, for the wide aspect ratio.
    extra: usize,
    samples: usize,
    /// With antialias, drawing goes to this multisample framebuffer — color and the depth above
    /// —, and is resolved into `color` before any readback.
    multi: Option<(glow::Framebuffer, glow::Renderbuffer)>,
}

impl Destination {
    /// The framebuffer being drawn into.
    fn drawing(&self) -> glow::Framebuffer {
        self.multi.map_or(self.fbo, |(fbo, _)| fbo)
    }
}

impl GpuState {
    /// Builds the program on a context, or says why it could not.
    ///
    /// `borrowed` is the window's context, when there is one. **Receiving instead of creating is
    /// not economy, it is correctness:** the core runs on the same thread as the interface, and a
    /// context of ours made current there turns off the window's — the interface stops painting
    /// and freezes, while the audio, which is another thread, keeps playing.
    ///
    /// Without a window — the command line `run`, where the measurement is done — there is nothing
    /// to borrow and we open the pbuffer.
    pub fn create(
        width: usize,
        height: usize,
        borrowed: Option<std::sync::Arc<glow::Context>>,
    ) -> Result<Self, String> {
        #[allow(unused_variables)]
        let (own, gl, borrowed) = match borrowed {
            Some(gl) => (None, gl, true),
            // **Without a borrowed context, this feature does not open one.** Whoever needs its
            // own context uses `gpu`; whoever receives the frontend's — the core — cannot open
            // one, and answering that is better than failing with a link error.
            #[cfg(feature = "gpu")]
            None => {
                let own = GlContext::create()?;
                let gl = own.gl.clone();
                (Some(own), gl, false)
            }
            #[cfg(not(feature = "gpu"))]
            None => {
                return Err(
                    "no borrowed context: this build does not open a GPU context"
                        .to_string(),
                );
            }
        };
        let (program, vao, vbo, bridge) = unsafe {
            let program = compile(&gl)?;
            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            let bridge = gl.create_texture()?;
            (program, vao, vbo, bridge)
        };
        // The blit test before any draw, once per context.
        #[cfg(not(target_arch = "wasm32"))]
        let blit_reliable = blit_works(&gl);
        // `wasm32` has no `blitFramebuffer`: there WebGL2 does not expose the function, and the
        // GPU path is not even used. Answering `true` keeps the rest of the code with a single
        // answer.
        #[cfg(target_arch = "wasm32")]
        let blit_reliable = true;
        if !blit_reliable {
            crate::logging!(
                crate::logging::Level::Warning,
                "gl",
                "this GPU's glBlitFramebuffer does not return what it was asked to copy: antialias and internal resolution stay off for this session"
            );
        }
        let gpu = Gpu::create(borrowed, gpu_address(&gl));
        Ok(Self {
            state: GlState::new(width, height),
            _own: own,
            gl,
            gpu,
            blit_reliable,
            discard_tiles: false,
            // **The viewport starts with the whole screen**, which is what OpenGL specifies as
            // the default and what `GlState::new` does. Starting at zero was what erased all of
            // the Z-Wheel's geometry: it never calls `glViewport` — zero times in thirteen
            // seconds — and was left with a `glViewport(0, 0, 0, 0)`, from which nothing comes
            // out. Only the `import` showed up, because it sets its own.
            fill: State {
                viewport: (0, 0, width as i32, height as i32),
                scissor: (0, 0, width as i32, height as i32),
                ..State::default()
            },
            frame: None,
            program,
            uniforms: UniformCache::default(),
            vao,
            vbo,
            ring: (0, 0),
            vao_ready: false,
            batch: Vec::new(),
            batch_state: None,
            sprites: Vec::new(),
            sprite_state: None,
            loose: Vec::new(),
            bridge,
            bridge_size: None,
            textures: HashMap::new(),
            vertices: Vec::new(),
            pixels: Vec::new(),
            dirty: true,
            scale: 1,
            ratio: None,
            in_perspective: false,
            external_fbo: None,
            mirror: std::cell::Cell::new(Mirror::default()),
            state_sends: std::cell::Cell::new(0),
            state_savings: std::cell::Cell::new(0),
            measured: Stats::default(),
            downscaled: None,
            front: None,
            swapped: false,
            front_valid: false,
            samples: 1,
            anisotropy: 1.0,
        })
    }

    /// Ensures the destination exists at the frame size and leaves it bound.
    /// The extra columns on each side, in surface pixels, for the requested aspect ratio.
    ///
    /// Only when the surface goes full screen: a pbuffer smaller than the frame (the Z-Wheel
    /// draws at 640×330) has no "sides" to open.
    ///
    /// **The math is in surface pixels.** On the surface stretched by Qualcomm each of its
    /// columns is worth `fw / sw` screen columns: Quake draws at 320×400, and the extra 16:9
    /// columns are the screen's divided by two. Counted in screen pixels, as before, the wide
    /// image came out with twice the sides. And there were only sides when the surface was the
    /// frame's size — in Quake, never: 16:9 opened nothing, and with internal resolution above 1
    /// the frame went to the window at the surface's aspect ratio, narrow and smaller.
    fn extra(&self) -> usize {
        let (fw, fh) = self.state.frame_size();
        let Some(aspect) = self.ratio else {
            return 0;
        };
        let (sw, sh) = self.state.surface();
        let full_screen = (sw, sh) == (fw, fh) || self.state.stretched_surface();
        if !full_screen || fh == 0 || fw == 0 {
            return 0;
        }
        let width = (fh as f32 * aspect).round() as usize;
        width.saturating_sub(fw) / 2 * sw / fw
    }

    /// The framebuffer being drawn into **now**.
    ///
    /// When the frontend provides a framebuffer, that is the target — not ours. This method exists
    /// because the choice was written in two places, and the two disagreed: the path that creates
    /// the destination respected the frontend's framebuffer, and the short path (destination
    /// already ready, unchanged) rebound ours. Since the short one is what runs on every frame
    /// after the first, the drawing stayed in our framebuffer, the frontend presented its own —
    /// black screen — and the GPU worked just as hard. The symptom was exactly that: **GPU busy and
    /// nothing on screen**.
    fn draw_target(&self) -> Option<glow::Framebuffer> {
        match self.external_fbo {
            // `0` is the framebuffer already bound; the frontend is the one in charge of it.
            Some(0) => None,
            Some(id) => std::num::NonZeroU32::new(id).map(glow::NativeFramebuffer),
            None => self.frame.as_ref().map(Destination::drawing),
        }
    }

    fn dest(&mut self) {
        let measured = self.state.frame_size();
        let (scale, samples, extra) = (self.scale, self.samples, self.extra());
        if self.frame.as_ref().is_some_and(|d| {
            d.measured == measured && d.scale == scale && d.samples == samples && d.extra == extra
        }) {
            let target = self.draw_target();
            unsafe { self.gl.bind_framebuffer(glow::FRAMEBUFFER, target) };
            return;
        }
        // The front belongs to the outgoing destination: its frame has another size.
        self.release_front();
        let gl = &self.gl;
        unsafe {
            if let Some(old) = self.frame.take() {
                release_target(gl, old);
            }
            let (width, height) = (
                ((measured.0 + 2 * extra) * scale) as i32,
                (measured.1 * scale) as i32,
            );
            let color = gl.create_texture().expect("color texture");
            gl.bind_texture(glow::TEXTURE_2D, Some(color));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                color_format(&gl) as i32,
                width,
                height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            // Depth and stencil in the same attachment: it is the combination OpenGL guarantees.
            // With antialias it has the same samples as the color, and lives in the draw
            // framebuffer.
            let depth = gl.create_renderbuffer().expect("depth buffer");
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
            match samples > 1 {
                true => gl.renderbuffer_storage_multisample(
                    glow::RENDERBUFFER,
                    samples as i32,
                    glow::DEPTH24_STENCIL8,
                    width,
                    height,
                ),
                false => gl.renderbuffer_storage(
                    glow::RENDERBUFFER,
                    glow::DEPTH24_STENCIL8,
                    width,
                    height,
                ),
            }
            let fbo = gl.create_framebuffer().expect("framebuffer");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(color),
                0,
            );
            let multi = match samples > 1 {
                false => {
                    gl.framebuffer_renderbuffer(
                        glow::FRAMEBUFFER,
                        glow::DEPTH_STENCIL_ATTACHMENT,
                        glow::RENDERBUFFER,
                        Some(depth),
                    );
                    None
                }
                true => {
                    let color_multi = gl.create_renderbuffer().expect("multisample color");
                    gl.bind_renderbuffer(glow::RENDERBUFFER, Some(color_multi));
                    gl.renderbuffer_storage_multisample(
                        glow::RENDERBUFFER,
                        samples as i32,
                        color_format(&gl),
                        width,
                        height,
                    );
                    let fbo_multi = gl.create_framebuffer().expect("multisample framebuffer");
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo_multi));
                    gl.framebuffer_renderbuffer(
                        glow::FRAMEBUFFER,
                        glow::COLOR_ATTACHMENT0,
                        glow::RENDERBUFFER,
                        Some(color_multi),
                    );
                    gl.framebuffer_renderbuffer(
                        glow::FRAMEBUFFER,
                        glow::DEPTH_STENCIL_ATTACHMENT,
                        glow::RENDERBUFFER,
                        Some(depth),
                    );
                    Some((fbo_multi, color_multi))
                }
            };
            // A new framebuffer has undefined contents, while the software rasterizer's vectors
            // start as opaque black, depth 1 and stencil 0. Matching here keeps the first frame
            // from depending on what the GPU left in memory. The draw one stays bound at the end,
            // which is what the caller expects.
            gl.viewport(0, 0, width, height);
            gl.disable(glow::SCISSOR_TEST);
            gl.color_mask(true, true, true, true);
            gl.depth_mask(true);
            gl.stencil_mask(u32::MAX);
            // **A new destination touches state outside `apply`.** From here on the mirror does
            // not know what is on the GPU, and would rather resend everything than lie.
            self.forget_mirror();
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear_depth_f32(1.0);
            gl.clear_stencil(0);
            for target in [Some(fbo), multi.map(|(f, _)| f)].into_iter().flatten() {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target));
                gl.clear(
                    glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT | glow::STENCIL_BUFFER_BIT,
                );
            }
            let dest = Destination {
                fbo,
                color,
                depth,
                measured,
                scale,
                extra,
                samples,
                multi,
            };
            self.frame = Some(dest);
            // The frontend is in charge of the target, when it provided a framebuffer; without
            // that, the destination is ours, with the antialias resolved later. The choice is the
            // same as on the short path, and that is why it lives in `draw_target`.
            let target = self.draw_target();
            gl.bind_framebuffer(glow::FRAMEBUFFER, target);
            let mut state = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            // ES 2 does not guarantee depth and stencil in the same attachment. vitaGL has the
            // `DEPTH24_STENCIL8` token, but if the framebuffer is not complete, 16-bit depth —
            // which ES 2 requires — still lets the Z test work.
            if state != glow::FRAMEBUFFER_COMPLETE && samples <= 1 {
                gl.framebuffer_renderbuffer(
                    glow::FRAMEBUFFER,
                    glow::DEPTH_STENCIL_ATTACHMENT,
                    glow::RENDERBUFFER,
                    None,
                );
                gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
                gl.renderbuffer_storage(
                    glow::RENDERBUFFER,
                    glow::DEPTH_COMPONENT16,
                    width,
                    height,
                );
                gl.framebuffer_renderbuffer(
                    glow::FRAMEBUFFER,
                    glow::DEPTH_ATTACHMENT,
                    glow::RENDERBUFFER,
                    Some(depth),
                );
                state = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            }
            if state != glow::FRAMEBUFFER_COMPLETE {
                static WARNED: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    crate::logging!(
                        crate::logging::Level::Warning,
                        "gl",
                        "incomplete GPU framebuffer ({state:#x})"
                    );
                }
            }
        }
    }

    /// Saves the presented frame before the game writes the next one over it.
    ///
    /// **The destination is a single buffer, and whoever presents was reading it live.** On the
    /// device `eglSwapBuffers` swaps front and back, and the front stays intact until the next
    /// swap. Here the window showed the destination — and a window pass can end in the middle of
    /// the next frame, when its deadline expires: it caught the destination after the `glClear`
    /// and before the draws. Ultimate Chess 3D, which reuploads texture rows every frame and so
    /// rarely ends the pass at the swap, flickered between the scene and black at 3x resolution,
    /// and at 1x, where the CPU screen reads the frame, it stayed black.
    ///
    /// **The copy is on demand**: at the swap nothing is copied, because the destination *is* the
    /// presented frame; only the first write after it copies the destination to the front. A pass
    /// that ends at the swap — the case of almost every light game — pays for no copy at all. The
    /// game's `glReadPixels` keeps reading the live destination, which is the back, as on the
    /// device.
    fn protect_front(&mut self) {
        if !self.swapped || self.external_fbo.is_some() || self.gpu.is_dead() {
            return;
        }
        self.swapped = false;
        // On the Vita presentation reads the texture at the swap, when the destination is still
        // the frame shown. The copy below would only serve someone reading the old image
        // after the next frame had already written, and that reader is the window in the middle
        // of the pass. The blit also opens a scene, draws the whole frame twice
        // and invalidates the viewport of the draw that comes next.
        #[cfg(not(target_os = "vita"))]
        self.copy_front();
    }

    #[cfg(not(target_os = "vita"))]
    fn copy_front(&mut self) {
        let Some(dest) = self.frame.as_ref() else {
            return;
        };
        let (w, h) = (
            ((dest.measured.0 + 2 * dest.extra) * dest.scale) as i32,
            (dest.measured.1 * dest.scale) as i32,
        );
        let origin = dest.fbo;
        let gl = self.gl.clone();
        unsafe {
            if self.front.is_none_or(|f| f.2 != (w, h)) {
                self.release_front();
                let color = gl.create_texture().expect("front texture");
                gl.bind_texture(glow::TEXTURE_2D, Some(color));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    color_format(&gl) as i32,
                    w,
                    h,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                // The same filters as `color`: whoever presents samples one or the other.
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::NEAREST as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::NEAREST as i32,
                );
                let fbo = gl.create_framebuffer().expect("front framebuffer");
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(color),
                    0,
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
                self.front = Some((fbo, color, (w, h)));
            }
            let front = self.front.map(|f| f.0);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(origin));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, front);
            gl.disable(glow::SCISSOR_TEST);
            self.forget_scissor();
            gl.blit_framebuffer(
                0,
                0,
                w,
                h,
                0,
                0,
                w,
                h,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, self.draw_target());
        }
        self.front_valid = true;
    }

    /// Drops the front: the destination changed size, or the context ended.
    fn release_front(&mut self) {
        if let Some((fbo, color, _)) = self.front.take() {
            unsafe {
                self.gl.delete_framebuffer(fbo);
                self.gl.delete_texture(color);
            }
        }
        self.swapped = false;
        self.front_valid = false;
    }

    /// Resolves the antialias: the samples of the draw framebuffer become `color`.
    fn resolve(&self) {
        let Some(dest) = self.frame.as_ref() else {
            return;
        };
        let Some((multi, _)) = dest.multi else {
            return;
        };
        let (w, h) = (
            ((dest.measured.0 + 2 * dest.extra) * dest.scale) as i32,
            (dest.measured.1 * dest.scale) as i32,
        );
        let gl = &self.gl;
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(multi));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(dest.fbo));
            gl.disable(glow::SCISSOR_TEST);
            self.forget_scissor();
            gl.blit_framebuffer(
                0,
                0,
                w,
                h,
                0,
                0,
                w,
                h,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(dest.fbo));
        }
    }

    /// The viewport with `y` counted from the top, which is how the destination is stored.
    ///
    /// `glViewport` counts bottom to top; see the same method in the software rasterizer for what
    /// that broke in Crash Nitro Kart.
    fn top_down_viewport(&self) -> (i32, i32, i32, i32) {
        if let Some(fixed) = self.fill.fixed_top_down_viewport {
            return fixed;
        }
        let (x, y, width, height) = self.fill.viewport;
        let surface_height = self.state.surface().1 as i32;
        (x, surface_height - y - height, width, height)
    }

    /// Puts the noted-down state onto the GPU. Called once per draw.
    /// Copies the draw state from `GlState` into the mirror that goes to GL.
    ///
    /// It exists because of save states. The mirror (`fill`) is updated **field by field** by each
    /// `Rasterizer` method, and is not derived from `GlState` on every batch. Loading a state
    /// replaces the whole `GlState` and leaves the mirror describing the previous world — and since
    /// the mirror is what `apply` writes to the GPU, the scene would come out with the matrix, the
    /// test flags and the clear colors from before. This function is the single point where the two
    /// agree again.
    fn resync_mirror(&mut self) {
        let e = &self.state;
        self.fill.depth_test = e.depth_test;
        self.fill.depth_mask = e.depth_mask;
        self.fill.depth_range = e.depth_range;
        self.fill.fog = e.fog;
        self.fill.depth_func = e.depth_func;
        self.fill.blend = e.blend;
        self.fill.blend_src = e.blend_src;
        self.fill.blend_dst = e.blend_dst;
        self.fill.alpha_test = e.alpha_test;
        self.fill.alpha_func = e.alpha_func;
        self.fill.alpha_ref = e.alpha_ref;
        self.fill.color_mask = e.color_mask;
        self.fill.cull_face = e.cull_face;
        self.fill.cull_mode = e.cull_mode;
        self.fill.front_face = e.front_face;
        self.fill.stencil_test = e.stencil_test;
        self.fill.stencil_func = e.stencil_func;
        self.fill.stencil_ref = e.stencil_ref;
        self.fill.stencil_value_mask = e.stencil_value_mask;
        self.fill.stencil_write_mask = e.stencil_write_mask;
        self.fill.stencil_op = e.stencil_op;
        self.fill.texture_env = e.texture_env;
        self.fill.unit1 = e.unit1();
        self.fill.bound_texture = e.bound_texture;
        self.fill.texturing = e.texture_2d;
        self.fill.viewport = e.viewport;
        self.fill.fixed_top_down_viewport = None;
        // The mirror keeps the **raw** scissor and a flag: it has no "no scissor" state, and the
        // flag is what `apply` checks to turn it on or not.
        self.fill.scissor = e.raw_scissor;
        self.fill.scissor_enabled = e.scissor_enabled;
        self.fill.clear_color = e.clear_color;
        self.fill.clear_depth = e.clear_depth;
        self.fill.clear_stencil = i32::from(e.clear_stencil);
    }

    /// Recreates the GPU texture objects from the authoritative copy of the GL state.
    fn recreate_restored_textures(&mut self) {
        let gl = self.gl.clone();
        for (_, texture) in self.textures.drain() {
            unsafe { gl.delete_texture(texture.object) };
        }

        let saved_textures: Vec<(u32, SavedTexture)> = self
            .state
            .textures
            .iter()
            .map(|(&name, texture)| (name, texture.clone()))
            .collect();
        unsafe { gl.active_texture(glow::TEXTURE0) };

        for (name, save) in saved_textures {
            if save.width == 0
                || save.height == 0
                || save.pixels.len() < save.width * save.height
            {
                continue;
            }
            let Ok(object) = (unsafe { gl.create_texture() }) else {
                continue;
            };
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(object));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    color_format(&gl) as i32,
                    save.width as i32,
                    save.height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(rgba_bytes(&save.pixels))),
                );
            }

            let mut max_level = 0u32;
            for (index, level) in save.mipmaps.iter().enumerate() {
                if level.width == 0
                    || level.height == 0
                    || level.pixels.len() < level.width * level.height
                {
                    break;
                }
                let gl_level = (index + 1) as u32;
                unsafe {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        gl_level as i32,
                        color_format(&gl) as i32,
                        level.width as i32,
                        level.height as i32,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(rgba_bytes(&level.pixels))),
                    );
                }
                max_level = gl_level;
            }
            unsafe { gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4) };

            self.textures.insert(
                name,
                Texture {
                    object,
                    width: save.width,
                    height: save.height,
                    max_level,
                    level_zero: true,
                    crop: save.crop,
                    filter: save.filter,
                    min_filter: save.min_filter,
                    wrap: save.wrap,
                },
            );
            if let Some(texture) = self.textures.get(&name) {
                self.parameters(texture);
            }
        }
        unsafe { gl.bind_texture(glow::TEXTURE_2D, None) };
    }

    /// Discards the resources that represent the host's frame, not the guest's state.
    fn drop_frame_caches(&mut self) {
        let gl = self.gl.clone();
        // **With the GPU dead, the objects are dropped, not deleted.** Whoever gets here after the
        // context ended — a state load between two frames, for example — would call the
        // `delete_*` of a context that is gone. See [`Gpu::deletes_on_drop`].
        if self.gpu.deletes_on_drop() {
            unsafe {
                if let Some(dest) = self.frame.take() {
                    release_target(&gl, dest);
                }
                if let Some((fbo, color, _)) = self.downscaled.take() {
                    gl.delete_framebuffer(fbo);
                    gl.delete_texture(color);
                }
                if let Some((fbo, color, _)) = self.front.take() {
                    gl.delete_framebuffer(fbo);
                    gl.delete_texture(color);
                }
            }
        } else {
            self.frame = None;
            self.downscaled = None;
            self.front = None;
        }
        self.swapped = false;
        self.front_valid = false;
        self.uniforms = UniformCache::default();
        self.ring = (0, 0);
        self.vao_ready = false;
        // The context was recreated: what was bound on it no longer is.
        self.gpu.forget_bound();
        self.batch.clear();
        self.batch_state = None;
        self.sprites.clear();
        self.sprite_state = None;
        self.loose.clear();
        self.vertices.clear();
        self.dirty = true;
    }

    /// Forgets everything the mirror knew: the next [`GpuState::apply`] resends the whole
    /// state.
    ///
    /// Called when **someone else** touches the context — `egui`, the frontend — or when the
    /// rasterizer itself touches it outside `apply` (the new `dest`, the `resolve`, the frame
    /// readback). Without this the mirror would lie, and lying here is a wrong drawing.
    fn forget_mirror(&self) {
        self.mirror.set(Mirror::default());
    }

    /// Only the scissor part: the `glScissor` and `GL_SCISSOR_TEST`.
    ///
    /// That is enough at the points that turn the scissor off for a blit — see the comments in
    /// `resolve`, `bind_for_read` and `clear`.
    fn forget_scissor(&self) {
        let mut m = self.mirror.get();
        m.scissor = None;
        m.scissor_enabled = None;
        self.mirror.set(m);
    }

    /// The `eglSwapBuffers`: the destination becomes the presented frame.
    ///
    /// **It belongs only to the swap, and not to [`Rasterizer::flush_draw`].** That one is also
    /// called by the save state, which rewind takes every second in the middle of the frame. Done
    /// there, this took the swap as having happened right after the `glClear`: the front was
    /// dropped, and the window showed the freshly cleared destination — Ultimate Chess 3D flashed
    /// black once per second, and with antialias the `resolve` carried half the frame into `color`.
    ///
    /// **The antialias is resolved here, at the swap, and not only at readback.** The `color`
    /// texture is what the window presents through the GPU ([`GpuState::gpu_frame`]), and with MSAA
    /// the scene lives in the multisample framebuffer until the `resolve`. Before the deferred
    /// readback, reading the frame at every swap resolved it along the way; with the deferred
    /// readback, whoever presents through the GPU never reads, and the texture stayed as it was —
    /// black with antialias on, while `zeebx session`, which reads, came out right. The frontend's
    /// framebuffer does not go through here: there the frontend is the one presenting. `resolve`
    /// leaves the destination bound, and the presenter's context goes back to the state it
    /// expects.
    fn swap(&mut self) {
        if self.external_fbo.is_some() {
            return;
        }
        self.flush();
        let multisampled = self.frame.as_ref().is_some_and(|d| d.multi.is_some());
        if multisampled && !self.gpu.is_dead() {
            self.resolve();
            self.hand_back_context();
        }
        // The destination is now the presented frame; the front, if there was one, is stale. See
        // [`GpuState::protect_front`]. And what the screen shows moved from the front to the
        // destination, even with no new drawing since the last readback: the next one must reread.
        self.swapped = true;
        if std::mem::take(&mut self.front_valid) {
            self.dirty = true;
        }
    }

    /// Counts the frame and, every [`FRAMES_PER_REPORT`], writes to the log what the GPU holds and
    /// received. See [`Stats`].
    fn end_frame(&mut self) {
        let now = std::time::Instant::now();
        let m = &mut self.measured;
        let start = *m.start.get_or_insert(now);
        if let Some(last) = m.last.replace(now) {
            m.worst_frame = m.worst_frame.max(now - last);
        }
        m.frames += 1;
        if m.frames < FRAMES_PER_REPORT {
            return;
        }
        // The smaller levels add a third to the largest; the math ignores that on purpose, because
        // what matters is the trend between reports, not the absolute number.
        let live_bytes: usize = self
            .textures
            .values()
            .map(|t| t.width * t.height * 4)
            .sum();
        let seconds = (now - start).as_secs_f64();
        crate::logging!(
            crate::logging::Level::Info,
            "gl",
            "gpu: {} frames in {:.1} real s ({:.1} per second, worst {} ms), {} live textures \
({:.1} MB), {:.2} MB uploaded and {} batches in the window",
            m.frames,
            seconds,
            m.frames as f64 / seconds.max(f64::EPSILON),
            m.worst_frame.as_millis(),
            self.textures.len(),
            live_bytes as f64 / 1_048_576.0,
            m.bytes_uploaded as f64 / 1_048_576.0,
            m.batches,
        );
        self.measured = Stats {
            start: Some(now),
            last: Some(now),
            ..Stats::default()
        };
    }

    /// How many state calls the mirror sent and how many it saved.
    pub fn state_sent_and_spared(&self) -> (u64, u64) {
        (self.state_sends.get(), self.state_savings.get())
    }

    fn apply(&mut self) {
        let (x, y, w, h) = self.top_down_viewport();
        let extra = self.frame.as_ref().map_or(0, |d| d.extra) as i32;
        // **At a wide aspect ratio, perspective gains sides and the rest only moves to the
        // center.** The viewport of a perspective batch grows by the ratio `k` and the clip `x`
        // shrinks by the same ratio (in `draw`): what was on screen lands on the same pixel as
        // before, shifted to the center, and what was outside the clip shows up on the sides. HUD
        // and 2D, in orthographic, only shift.
        let surface = self.state.surface().0 as i32;
        let to_attachment = |x: i32, w: i32| match (self.in_perspective, extra > 0) {
            (true, true) => {
                let width = w * (surface + 2 * extra) / surface.max(1);
                (extra + x + w / 2 - width / 2, width)
            }
            _ => (x + extra, w),
        };
        let (x, w) = to_attachment(x, w);
        let gl = &self.gl;
        let e = &self.fill;
        // **Everything that follows goes through the mirror.** There are eighteen calls per batch,
        // almost always with the same values as the previous batch.
        let mut m = self.mirror.get();
        let mut sent_n = 0u64;
        let mut p_n = 0u64;
        unsafe {
            // The viewport comes in console pixels; the attachment is `scale` times larger.
            let n = self.scale as i32;
            let viewport = (x * n, y * n, w.max(0) * n, h.max(0) * n);
            if Mirror::changed(&mut m.viewport, viewport, &mut sent_n, &mut p_n) {
                gl.viewport(viewport.0, viewport.1, viewport.2, viewport.3);
            }
            // **The software rasterizer only clips at the near plane.** OpenGL clips at the six
            // frustum planes, and the far plane made whole surfaces disappear — on the Z-Wheel it
            // was a strip of the background, between the horizon line and the ground. Clamped
            // instead of clipped, the behavior is back to the software one.
            //
            // It is core in desktop OpenGL, but does not exist in GLES. Emitting the invalid enum
            // on every batch costs validation in the Mali driver and leaves `GL_INVALID_ENUM`
            // pending — and that is why it has its own value here, instead of going straight to
            // the call.
            let embedded = gl.version().is_embedded;
            if !embedded
                && Mirror::changed(&mut m.depth_clamp, true, &mut sent_n, &mut p_n)
            {
                gl.enable(glow::DEPTH_CLAMP);
            }
            // The game's `glScissor` comes in console pixels, with `y` going bottom to top —
            // the same convention as the viewport —, and the attachment is `scale` times larger.
            // See [`scissor_on_attachment`].
            if Mirror::changed(&mut m.scissor_enabled, e.scissor_enabled, &mut sent_n, &mut p_n) {
                set_enabled(gl, glow::SCISSOR_TEST, e.scissor_enabled);
            }
            if e.scissor_enabled {
                let (sx, sy, sw, sh) = scissor_on_attachment(e.scissor, self.state.surface(), extra);
                let scissor = (sx * n, sy * n, sw.max(0) * n, sh.max(0) * n);
                if Mirror::changed(&mut m.scissor, scissor, &mut sent_n, &mut p_n) {
                    gl.scissor(scissor.0, scissor.1, scissor.2, scissor.3);
                }
            }
            if Mirror::changed(
                &mut m.depth_test,
                e.depth_test,
                &mut sent_n,
                &mut p_n,
            ) {
                set_enabled(gl, glow::DEPTH_TEST, e.depth_test);
            }
            if Mirror::changed(
                &mut m.depth_func,
                e.depth_func,
                &mut sent_n,
                &mut p_n,
            ) {
                gl.depth_func(e.depth_func);
            }
            if Mirror::changed(
                &mut m.depth_mask,
                e.depth_mask,
                &mut sent_n,
                &mut p_n,
            ) {
                gl.depth_mask(e.depth_mask);
            }
            if Mirror::changed(
                &mut m.depth_range,
                e.depth_range,
                &mut sent_n,
                &mut p_n,
            ) {
                gl.depth_range_f32(e.depth_range.0, e.depth_range.1);
            }
            if Mirror::changed(&mut m.blend, e.blend, &mut sent_n, &mut p_n) {
                set_enabled(gl, glow::BLEND, e.blend);
            }
            let blend_func = (e.blend_src, e.blend_dst);
            if Mirror::changed(&mut m.blend_func, blend_func, &mut sent_n, &mut p_n) {
                gl.blend_func(blend_func.0, blend_func.1);
            }
            if Mirror::changed(&mut m.color_mask, e.color_mask, &mut sent_n, &mut p_n) {
                let [r, g, b, a] = e.color_mask;
                gl.color_mask(r, g, b, a);
            }
            if Mirror::changed(&mut m.cull_face, e.cull_face, &mut sent_n, &mut p_n) {
                set_enabled(gl, glow::CULL_FACE, e.cull_face);
            }
            if Mirror::changed(&mut m.cull_mode, e.cull_mode, &mut sent_n, &mut p_n) {
                gl.cull_face(e.cull_mode);
            }
            // Row 0 of the framebuffer is treated as the top of the image, and Y is flipped in the
            // vertex shader. That inverts the orientation seen by face culling, so the front face
            // is swapped here to compensate — without this, culling eats the wrong side.
            let face = match e.front_face {
                gles::GL_CCW => glow::CW,
                _ => glow::CCW,
            };
            if Mirror::changed(&mut m.front_face, face, &mut sent_n, &mut p_n) {
                gl.front_face(face);
            }
            if Mirror::changed(
                &mut m.stencil_test,
                e.stencil_test,
                &mut sent_n,
                &mut p_n,
            ) {
                set_enabled(gl, glow::STENCIL_TEST, e.stencil_test);
            }
            let stencil_func = (e.stencil_func, e.stencil_ref, e.stencil_value_mask);
            if Mirror::changed(&mut m.stencil_func, stencil_func, &mut sent_n, &mut p_n) {
                gl.stencil_func(stencil_func.0, stencil_func.1, stencil_func.2);
            }
            if Mirror::changed(
                &mut m.stencil_mask,
                e.stencil_write_mask,
                &mut sent_n,
                &mut p_n,
            ) {
                gl.stencil_mask(e.stencil_write_mask);
            }
            if Mirror::changed(&mut m.stencil_ops, e.stencil_op, &mut sent_n, &mut p_n) {
                gl.stencil_op(e.stencil_op[0], e.stencil_op[1], e.stencil_op[2]);
            }
        }
        self.mirror.set(m);
        self.state_sends
            .set(self.state_sends.get() + sent_n);
        self.state_savings
            .set(self.state_savings.get() + p_n);
    }

    /// Sends the batch of already transformed vertices to the GPU.
    ///
    /// `texture` exists because the bridge of [`GpuState::import_rgb565_changes`] does not belong
    /// to the game and so is not in its texture map.
    /// Sends the pending batch to the GPU, with the state in which it was gathered.
    ///
    /// **Everyone who touches the GPU outside drawing calls this first**: whoever clears, whoever
    /// uploads or deletes a texture, whoever reads the frame, whoever changes the destination.
    /// What was requested earlier has to arrive earlier — a `glClear` that jumped ahead of the
    /// batch would erase what the game drew before it. State changes do not need to: they change
    /// `fill`, and a different `fill` already closes the batch on the next draw.
    fn flush(&mut self) {
        // The rectangle came before the waiting polygon, or the other way around: the order in
        // which the game asked has to be the order on the GPU.
        self.flush_sprites();
        self.flush_batch();
    }

    fn flush_batch(&mut self) {
        let Some((state, perspective)) = self.batch_state.take() else {
            return;
        };
        if self.batch.is_empty() {
            return;
        }
        let current = std::mem::replace(&mut self.fill, state);
        let prior = std::mem::replace(&mut self.vertices, std::mem::take(&mut self.batch));
        self.in_perspective = perspective;
        self.submit_with(glow::TRIANGLES, -1.0, None);
        self.in_perspective = false;
        self.batch = std::mem::replace(&mut self.vertices, prior);
        self.batch.clear();
        self.fill = current;
    }

    fn flush_sprites(&mut self) {
        let Some(state) = self.sprite_state.take() else {
            return;
        };
        if self.sprites.is_empty() {
            return;
        }
        let current = std::mem::replace(&mut self.fill, state);
        let prior = std::mem::replace(&mut self.vertices, std::mem::take(&mut self.sprites));
        self.submit_with(glow::TRIANGLES, -1.0, None);
        self.sprites = std::mem::replace(&mut self.vertices, prior);
        self.sprites.clear();
        self.fill = current;
    }

    fn submit_with(&mut self, mode: u32, flip: f32, texture: Option<glow::Texture>) {
        let count = self.vertices.len() / FLOATS_PER_VERTEX;
        if count == 0 {
            return;
        }
        self.measured.batches += 1;
        // **The GPU can die in the middle of the frame.** The frontend's notice does not wait for
        // `retro_run` to finish, and drawing after it means calling GL functions that no longer
        // exist. What was on the GPU stays — the frame comes out of what was already drawn —, and
        // the session is born again at the start of the next frame, which is where the loss becomes
        // a rasterizer swap.
        if self.gpu.is_dead() {
            self.vertices.clear();
            return;
        }
        self.protect_front();
        self.dest();
        self.apply();
        let has_bridge_texture = texture.is_some();
        let texture = texture.or_else(|| {
            self.fill
                .texturing
                .then(|| self.textures.get(&self.fill.bound_texture))
                .flatten()
                .map(|t| t.object)
        });
        let texture1 = match (has_bridge_texture, self.fill.unit1.enabled) {
            (false, true) => self.textures.get(&self.fill.unit1.texture).map(|t| t.object),
            _ => None,
        };
        let gl = &self.gl;
        unsafe {
            // Bound **once**, and not on every draw: see [`Gpu::enabled`].
            if self.gpu.needs_bind() {
                gl.use_program(Some(self.program));
                gl.bind_vertex_array(Some(self.vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
                self.gpu.mark_bound();
            }
            let step = (FLOATS_PER_VERTEX * 4) as i32;
            if !self.vao_ready {
                for (index, size, offset) in [
                    (0u32, 4i32, 0i32),
                    (1, 4, 16),
                    (2, 2, 32),
                    (3, 1, 40),
                    (4, 2, 44),
                ] {
                    gl.enable_vertex_attrib_array(index);
                    gl.vertex_attrib_pointer_f32(
                        index,
                        size,
                        glow::FLOAT,
                        false,
                        step,
                        offset,
                    );
                }
                self.vao_ready = true;
            }
            let bytes = f32_bytes(&self.vertices);
            // On the Vita each batch gets its own region: the GPU reads the pointer at the end
            // of the scene. On the others, the ring asks for another block when it fills, and the
            // old one is only released when the draws using it finish.
            #[cfg(target_os = "vita")]
            let first = if write_vita_block(gl, self.vbo, bytes) {
                0
            } else {
                self.hand_back_context();
                return;
            };
            #[cfg(not(target_os = "vita"))]
            let first = {
                let (capacity, mut free) = self.ring;
                if free + count > capacity {
                    let capacity = RING_VERTICES.max(count);
                    gl.buffer_data_size(
                        glow::ARRAY_BUFFER,
                        (capacity * FLOATS_PER_VERTEX * 4) as i32,
                        glow::STREAM_DRAW,
                    );
                    self.ring.0 = capacity;
                    free = 0;
                }
                gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, free as i32 * step, bytes);
                self.ring.1 = free + count;
                free
            };
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, texture);
            uniform_i32(gl, &self.uniforms, self.program, "amostra", 0);
            uniform_i32(
                gl,
                &self.uniforms,
                self.program,
                "texturando",
                i32::from(texture.is_some()),
            );
            send_env(
                gl,
                &self.uniforms,
                self.program,
                "",
                &self.fill.texture_env,
            );
            // Unit 1 only comes in with a real texture: bound without a loaded texture, it would
            // pass the previous one along with a black texel.
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, texture1);
            gl.active_texture(glow::TEXTURE0);
            uniform_i32(gl, &self.uniforms, self.program, "amostra1", 1);
            uniform_i32(
                gl,
                &self.uniforms,
                self.program,
                "texturando1",
                i32::from(texture1.is_some()),
            );
            if texture1.is_some() {
                send_env(
                    gl,
                    &self.uniforms,
                    self.program,
                    "1",
                    &self.fill.unit1.env,
                );
            }
            uniform_i32(
                gl,
                &self.uniforms,
                self.program,
                "func_alfa",
                match self.fill.alpha_test {
                    true => alpha_code(self.fill.alpha_func),
                    false => 7,
                },
            );
            uniform_f32(
                gl,
                &self.uniforms,
                self.program,
                "ref_alfa",
                self.fill.alpha_ref,
            );
            uniform_f32(gl, &self.uniforms, self.program, "virar", flip);
            let fog = self.fill.fog;
            uniform_i32(
                gl,
                &self.uniforms,
                self.program,
                "com_neblina",
                i32::from(fog.enabled && fog.allowed),
            );
            uniform_vec3(
                gl,
                &self.uniforms,
                self.program,
                "cor_neblina",
                fog.color,
            );
            gl.draw_arrays(mode, first as i32, count as i32);
            // **Do not unbind here.** Unbinding at the end of every draw was the cost this cache
            // came to remove: one program less per batch and one revalidation less in each.
        }
        // On the Vita the main other user of the context is presentation, once per frame, and it
        // binds framebuffer, scissor and program itself. Handing back here turned off blending,
        // depth and the program on every sprite — the cost that invalidating the cache on
        // every batch had already shown on RetroArch. Running the character means more sprites.
        // The `resume_context` after the swap forgets the mirror.
        #[cfg(not(target_os = "vita"))]
        self.hand_back_context();
        self.dirty = true;
    }

    /// Pushes a vertex into the upload buffer.
    fn put(&mut self, v: &Vertex) {
        push_vertex(&mut self.vertices, v);
    }

    /// Leaves bound, for reading, a framebuffer with the frame at the console size.
    ///
    /// With `scale` 1 it is the destination itself. Above that the big frame is reduced on the
    /// GPU, with a linear filter, before any readback: it is what the game sees — the
    /// `GetColorBufferQUALCOMM`, the `glReadPixels`, the copy to the screen —, and reading the big
    /// frame would mean moving the square of the factor in bytes only to throw almost all of it
    /// away.
    ///
    /// `presented` says what is read: the frame of the last swap, which is what the screen shows —
    /// the front, when the game has already written to the destination after it —, or the live
    /// destination, which is what the game's `glReadPixels` asks for. See
    /// [`GpuState::protect_front`].
    fn bind_for_read(&mut self, presented: bool) {
        self.dest();
        let from_front = presented && self.front_valid && self.external_fbo.is_none();
        // The frontend's framebuffer does not go through our resolve. On the internal path,
        // resolve MSAA before reading — except for the front, which is already the resolved copy.
        if self.external_fbo.is_none() && !from_front {
            self.resolve();
        }
        let origin = match from_front {
            true => self.front.map(|f| f.0),
            false => self.frame.as_ref().map(|d| d.fbo),
        };
        let extra = self.frame.as_ref().map_or(0, |d| d.extra) as i32;
        if self.scale <= 1 && extra == 0 {
            let fbo = match self.external_fbo {
                Some(_) => self.draw_target(),
                None => origin,
            };
            unsafe { self.gl.bind_framebuffer(glow::FRAMEBUFFER, fbo) };
            return;
        }
        let measured = self.state.frame_size();
        let (fw, fh) = (measured.0 as i32, measured.1 as i32);
        let n = self.scale as i32;
        let gl = &self.gl;
        unsafe {
            if self.downscaled.as_ref().is_none_or(|r| r.2 != measured) {
                if let Some((fbo, color, _)) = self.downscaled.take() {
                    gl.delete_framebuffer(fbo);
                    gl.delete_texture(color);
                }
                let color = gl.create_texture().expect("downscaled texture");
                gl.bind_texture(glow::TEXTURE_2D, Some(color));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    color_format(&gl) as i32,
                    fw,
                    fh,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                let fbo = gl.create_framebuffer().expect("downscaled framebuffer");
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(color),
                    0,
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
                self.downscaled = Some((fbo, color, measured));
            }
            let dest = self.downscaled.as_ref().map(|r| r.0);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, origin);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, dest);
            gl.disable(glow::SCISSOR_TEST);
            self.forget_scissor();
            // At a wide aspect ratio, the game reads only the center: that is where the 640×480
            // image it drew is, and the sides are ours.
            gl.blit_framebuffer(
                extra * n,
                0,
                (extra + fw) * n,
                fh * n,
                0,
                0,
                fw,
                fh,
                glow::COLOR_BUFFER_BIT,
                glow::LINEAR,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, dest);
        }
    }

    /// Reads the frame into `self.pixels` as RGB565, two bytes per pixel in host order, with
    /// row 0 at the top. Returns `false` when it read as RGBA: on GLES reading in 5-6-5 is not
    /// guaranteed, and there the format that always works is used.
    fn read_frame_rgb565(&mut self, width: usize, height: usize) -> bool {
        if self.gl.version().is_embedded {
            self.read_frame(width, height);
            return false;
        }
        self.bind_for_read(true);
        self.pixels.clear();
        self.pixels.resize(width * height * 2, 0);
        unsafe {
            // Each row has `width * 2` bytes; the default alignment of four would skew odd
            // widths.
            self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 2);
            self.gl.read_pixels(
                0,
                0,
                width as i32,
                height as i32,
                glow::RGB,
                glow::UNSIGNED_SHORT_5_6_5,
                glow::PixelPackData::Slice(Some(&mut self.pixels)),
            );
            self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
        }
        self.hand_back_context();
        true
    }

    /// Reads the GPU frame into `self.pixels`, as RGBA, with row 0 at the top.
    fn read_frame(&mut self, width: usize, height: usize) {
        self.bind_for_read(true);
        self.pixels.clear();
        self.pixels.resize(width * height * 4, 0);
        unsafe {
            self.gl.read_pixels(
                0,
                0,
                width as i32,
                height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut self.pixels)),
            );
        }
        self.hand_back_context();
    }

    /// Restores the state egui assumes, when the context belongs to someone else.
    ///
    /// On the windowed path egui paints on the **same thread and the same context**, right after
    /// us, and it does not reconfigure everything it uses: leaving our framebuffer bound, or the
    /// depth test on with a depth that is not its own, makes the interface disappear.
    ///
    /// With an own context this costs nothing because it does not run: nobody else uses it.
    ///
    /// **Here, and not only on the recreated context, the cache of bound objects dies** when the
    /// presenter is the host. See the end of this function.
    fn hand_back_context(&self) {
        if !self.gpu.borrowed() {
            return;
        }
        let gl = &self.gl;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::STENCIL_TEST);
            // The mirror below says the scissor was left off, and it really needs to be: with it
            // on in GL, the next draw without scissor resends nothing and comes out clipped by the
            // previous one's rectangle. That was Peggle on the GPU, with no background at all.
            gl.disable(glow::SCISSOR_TEST);
            if !gl.version().is_embedded {
                gl.disable(glow::DEPTH_CLAMP);
            }
            gl.disable(glow::BLEND);
            gl.depth_mask(true);
            gl.depth_range_f32(0.0, 1.0);
            gl.stencil_mask(u32::MAX);
            gl.color_mask(true, true, true, true);
        }
        // **Here the context goes to someone else — and the mirror learns what was left.**
        //
        // Forgetting everything would be safer and would save nothing: this function runs on every
        // batch, and a mirror reset on every batch answers "changed" eighteen times, always.
        // Measured: zero calls saved. What is done instead is to record the **known** state it
        // leaves, which is what the next `apply` will compare — and the keys that are not here stay
        // unknown, because the other party may have touched them.
        let mut m = self.mirror.get();
        m.scissor = None;
        m.scissor_enabled = Some(false);
        m.depth_test = Some(false);
        m.blend = Some(false);
        m.cull_face = Some(false);
        m.stencil_test = Some(false);
        m.depth_mask = Some(true);
        m.depth_range = Some((0.0, 1.0));
        m.stencil_mask = Some(u32::MAX);
        m.color_mask = Some([true, true, true, true]);
        if !self.gl.version().is_embedded {
            m.depth_clamp = Some(false);
        }
        self.mirror.set(m);
        // **And the cache of the three bound objects dies here too**, when the presenter of the
        // frame is the host: without an external framebuffer, whoever draws our destination is the
        // interface, on the same context, right after us — and its `GlPainter` ends every paint
        // **unbinding** program and VAO on purpose (`src/ui/gpu.rs`). Trusting the cache after that
        // means drawing with `egui`'s program: in the core profile, VAO zero is
        // `GL_INVALID_OPERATION` and nothing on screen, with no error at all on our side.
        //
        // **Not when the frontend provides its own framebuffer**: there libretro is the one
        // drawing, the frame is its own, and `hand_back_context` runs on every batch — invalidating
        // here would mean paying three driver calls per batch again, which is precisely the cost
        // that `e6a436e` removed from the RetroArch path (370 batches per frame in Quake). On that
        // path the invalidation is done by [`GpuState::draw_to_fbo`], once per frame, when the
        // frontend takes the context.
        if self.external_fbo.is_none() {
            self.gpu.forget_bound();
        }
    }

    /// Reapplies a texture's parameters, downgrading the filter when the chain is missing.
    fn parameters(&self, t: &Texture) {
        let gl = &self.gl;
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(t.object));
            let has_chain = t.max_level > 0;
            let min = match (t.min_filter, has_chain) {
                (f, true) => f,
                // Without auxiliary levels, a mipmap filter draws black in OpenGL. The software
                // rasterizer falls back to level zero in that case; here the fallback is choosing
                // the equivalent filter without mipmap.
                (gles::GL_NEAREST_MIPMAP_NEAREST | gles::GL_NEAREST_MIPMAP_LINEAR, false) => {
                    gles::GL_NEAREST
                }
                (gles::GL_LINEAR_MIPMAP_NEAREST | gles::GL_LINEAR_MIPMAP_LINEAR, false) => {
                    gles::GL_LINEAR
                }
                (f, false) => f,
            };
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, t.filter as i32);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAX_LEVEL,
                t.max_level as i32,
            );
            for (axis, mode) in [
                (glow::TEXTURE_WRAP_S, t.wrap[0]),
                (glow::TEXTURE_WRAP_T, t.wrap[1]),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, axis, mode as i32);
            }
            // It is only requested when on, and it only gets here on if the GPU has the
            // extension — see [`Rasterizer::set_anisotropic`].
            if self.anisotropy > 1.0 {
                gl.tex_parameter_f32(glow::TEXTURE_2D, TEXTURE_MAX_ANISOTROPY, self.anisotropy);
            }
        }
    }
}

impl Drop for GpuState {
    fn drop(&mut self) {
        // **With the GPU dead, nothing is deleted.** The frontend reports that the context ended
        // and the live session still holds this state: on the next `retro_run` it is replaced, and
        // `Drop` runs here. Calling `delete_*` at this point would mean using the function pointers
        // of a context that no longer exists — at best the driver records an error, at worst the
        // address points to code that is no longer there (GPU audit, finding 1). The names are
        // dropped with the context, which belongs to whoever reported it. See
        // [`Gpu::deletes_on_drop`].
        if !self.gpu.deletes_on_drop() {
            return;
        }
        let gl = &self.gl;
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            gl.delete_texture(self.bridge);
            for t in self.textures.values() {
                gl.delete_texture(t.object);
            }
            if let Some(d) = self.frame.take() {
                release_target(gl, d);
            }
            if let Some((fbo, color, _)) = self.downscaled.take() {
                gl.delete_framebuffer(fbo);
                gl.delete_texture(color);
            }
        }
    }
}

/// Returns to the GPU what a destination created.
unsafe fn release_target(gl: &glow::Context, d: Destination) {
    unsafe {
        gl.delete_framebuffer(d.fbo);
        gl.delete_texture(d.color);
        gl.delete_renderbuffer(d.depth);
        if let Some((fbo, color)) = d.multi {
            gl.delete_framebuffer(fbo);
            gl.delete_renderbuffer(color);
        }
    }
}

fn set_enabled(gl: &glow::Context, capacity: u32, enabled: bool) {
    unsafe {
        match enabled {
            true => gl.enable(capacity),
            false => gl.disable(capacity),
        }
    }
}

/// The positions of the program's uniforms, and the last value each one received.
///
/// Quake makes about 370 draw calls per frame, and each one looked up by name the position of a
/// dozen uniforms — glow builds a C string and the driver looks up the name every time — and
/// resent all of them, even when unchanged. The position does not change while the program
/// exists, and a uniform keeps its value in the program: sending the same value again changes
/// nothing. Measured, it is close to one microsecond less per draw; the bulk of the cost was the
/// buffer, see [`GpuState::ring`].
#[derive(Default)]
struct UniformCache {
    map: std::cell::RefCell<HashMap<String, (Option<glow::UniformLocation>, Option<[u32; 4]>)>>,
}

impl UniformCache {
    /// Sends `value` to the uniform `name` through `send`, if it exists and the value is new.
    fn define(
        &self,
        gl: &glow::Context,
        program: glow::Program,
        name: &str,
        value: [u32; 4],
        send: impl FnOnce(&glow::UniformLocation),
    ) {
        let mut map = self.map.borrow_mut();
        if !map.contains_key(name) {
            let location = unsafe { gl.get_uniform_location(program, name) };
            map.insert(name.to_string(), (location, None));
        }
        let Some((location, last)) = map.get_mut(name) else {
            return;
        };
        if *last == Some(value) {
            return;
        }
        *last = Some(value);
        if let Some(location) = location {
            send(location);
        }
    }
}

fn uniform_i32(
    gl: &glow::Context,
    u: &UniformCache,
    program: glow::Program,
    name: &str,
    value: i32,
) {
    u.define(gl, program, name, [value as u32, 0, 0, 0], |location| unsafe {
        gl.uniform_1_i32(Some(location), value)
    });
}

fn uniform_f32(
    gl: &glow::Context,
    u: &UniformCache,
    program: glow::Program,
    name: &str,
    value: f32,
) {
    u.define(
        gl,
        program,
        name,
        [value.to_bits(), 0, 0, 0],
        |location| unsafe { gl.uniform_1_f32(Some(location), value) },
    );
}

/// A four-component uniform, for the `GL_TEXTURE_ENV_COLOR` color.
fn uniform_vec4(
    gl: &glow::Context,
    u: &UniformCache,
    program: glow::Program,
    name: &str,
    value: [f32; 4],
) {
    u.define(gl, program, name, value.map(f32::to_bits), |location| unsafe {
        gl.uniform_4_f32(Some(location), value[0], value[1], value[2], value[3])
    });
}

/// A three-component uniform, for the fog color.
fn uniform_vec3(
    gl: &glow::Context,
    u: &UniformCache,
    program: glow::Program,
    name: &str,
    value: [f32; 4],
) {
    let bits = [
        value[0].to_bits(),
        value[1].to_bits(),
        value[2].to_bits(),
        0,
    ];
    u.define(gl, program, name, bits, |location| unsafe {
        gl.uniform_3_f32(Some(location), value[0], value[1], value[2])
    });
}

/// Writes a vertex into the GPU buffer, in the layout that [`FLOATS_PER_VERTEX`] declares.
///
/// **It is the only place that knows this layout.** The fog factor comes ready from the vertex
/// stage, which is the same for both rasterizers; here it is just one more attribute to
/// interpolate.
fn push_vertex(dest: &mut Vec<f32>, v: &Vertex) {
    dest.extend_from_slice(&v.position);
    dest.extend_from_slice(&v.color);
    dest.extend_from_slice(&v.uv[..2]);
    dest.push(v.fog);
    dest.extend_from_slice(&v.uv1[..2]);
}

/// The bytes of a `f32` vector, for `buffer_data`.
fn f32_bytes(data: &[f32]) -> &[u8] {
    // An f32 has no bit invariant, and an alignment of four satisfies an alignment of one.
    unsafe { std::slice::from_raw_parts(data.as_ptr().cast(), std::mem::size_of_val(data)) }
}

/// The RGBA texels are already in the four-byte format the GPU receives.
fn rgba_bytes(data: &[[u8; 4]]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(data.as_ptr().cast(), data.len() * 4) }
}

/// The code the shader uses for each `glTexEnv` mode. See [`TexEnv::apply`].
fn env_code(mode: u32) -> i32 {
    match mode {
        gles::GL_REPLACE => 0,
        gles::GL_DECAL => 1,
        gles::GL_ADD => 2,
        gles::GL_COMBINE => 4,
        _ => 3,
    }
}

/// The `GL_COMBINE` function in the shader. See `TexEnv::combine`.
fn func_code(callback: u32) -> i32 {
    match callback {
        gles::GL_REPLACE => 0,
        gles::GL_ADD => 2,
        gles::GL_ADD_SIGNED => 3,
        gles::GL_INTERPOLATE => 4,
        gles::GL_SUBTRACT => 5,
        gles::GL_DOT3_RGB => 6,
        gles::GL_DOT3_RGBA => 7,
        _ => 1,
    }
}

/// The `GL_COMBINE` source in the shader: texture, constant, primary color, previous unit or,
/// through the crossbar, the texel of unit 0 or unit 1. See `is_combine_source`, in the rasterizer.
fn src_code(source: u32) -> i32 {
    match source {
        gles::GL_TEXTURE => 0,
        gles::GL_CONSTANT => 1,
        gles::GL_PRIMARY_COLOR => 2,
        f if f == gles::GL_TEXTURE0 => 4,
        f if f == gles::GL_TEXTURE0 + 1 => 5,
        _ => 3,
    }
}

/// The environment uniforms of one unit; `suffix` is `""` for unit 0 and `"1"` for unit 1.
fn send_env(
    gl: &glow::Context,
    u: &UniformCache,
    program: glow::Program,
    suffix: &str,
    env: &TexEnv,
) {
    // The mode name comes ready-made: it is sent on every draw, and the rest only in `GL_COMBINE`.
    let mode = if suffix.is_empty() { "env" } else { "env1" };
    uniform_i32(gl, u, program, mode, env_code(env.mode));
    if env.mode != gles::GL_COMBINE {
        return;
    }
    for side in 0..2 {
        let name = ["rgb", "alfa"][side];
        uniform_i32(
            gl,
            u,
            program,
            &format!("cmb_{name}{suffix}"),
            func_code(env.combine[side]),
        );
        uniform_f32(
            gl,
            u,
            program,
            &format!("escala_{name}{suffix}"),
            env.scale[side],
        );
        for i in 0..3 {
            uniform_i32(
                gl,
                u,
                program,
                &format!("src_{name}{suffix}[{i}]"),
                src_code(env.sources[side][i]),
            );
            uniform_i32(
                gl,
                u,
                program,
                &format!("op_{name}{suffix}[{i}]"),
                operand_code(env.operands[side][i]),
            );
        }
    }
    uniform_vec4(gl, u, program, &format!("cor_env{suffix}"), env.color);
}

/// The `GL_COMBINE` operand in the shader.
fn operand_code(operand: u32) -> i32 {
    match operand {
        gles::GL_SRC_COLOR => 0,
        gles::GL_ONE_MINUS_SRC_COLOR => 1,
        gles::GL_SRC_ALPHA => 2,
        _ => 3,
    }
}

/// The code the shader uses for each alpha test function.
fn alpha_code(func: u32) -> i32 {
    match func {
        gles::GL_NEVER => 0,
        gles::GL_LESS => 1,
        gles::GL_EQUAL => 2,
        gles::GL_LEQUAL => 3,
        gles::GL_GREATER => 4,
        gles::GL_NOTEQUAL => 5,
        gles::GL_GEQUAL => 6,
        _ => 7,
    }
}

/// The vertex shader is a **straight pass-through**: the positions already arrive in clip space,
/// because the vertex stage happens on the CPU, shared with the software rasterizer.
///
/// The `flip` inverts Y. It exists because row 0 of the framebuffer is treated as the top of the
/// image — that way `glReadPixels` returns the rows in the same order the software surface uses,
/// and no frame needs to be mirrored on the CPU. The `import` bridge passes `+1` because it
/// already delivers ready coordinates.
///
/// There is no flat shading path because the software rasterizer has none: it stores
/// `glShadeModel` and never reads it. See [`GpuState::set_shade_model`].
const VERTEX: &str = r#"
layout(location = 0) in vec4 pos;
layout(location = 1) in vec4 cor;
layout(location = 2) in vec2 uv;
layout(location = 3) in float fog;
layout(location = 4) in vec2 uv1;
uniform float virar;
out vec4 vcor;
out vec2 vuv;
out float vfog;
out vec2 vuv1;
void main() {
    vcor = cor;
    vuv = uv;
    vfog = fog;
    vuv1 = uv1;
    gl_Position = vec4(pos.x, pos.y * virar, pos.z, pos.w);
}
"#;

/// The fragment does what is left of the fixed pipeline: combine the texture and test the alpha.
///
/// The combination follows the software rasterizer's `combine`, case by case, so that both
/// backends respond to `glTexEnv` the same way.
const FRAGMENT: &str = r#"
in vec4 vcor;
in vec2 vuv;
in float vfog;
in vec2 vuv1;
uniform sampler2D amostra;
uniform sampler2D amostra1;
uniform int com_neblina;
uniform vec3 cor_neblina;
uniform int texturando;
uniform int texturando1;
// Each unit's environment: the mode and, under `GL_COMBINE`, function, sources, operands, scales and the
// constant color. The `1` suffix is unit 1.
uniform int env;
uniform int cmb_rgb;
uniform int cmb_alfa;
uniform int src_rgb[3];
uniform int op_rgb[3];
uniform int src_alfa[3];
uniform int op_alfa[3];
uniform float escala_rgb;
uniform float escala_alfa;
uniform vec4 cor_env;
uniform int env1;
uniform int cmb_rgb1;
uniform int cmb_alfa1;
uniform int src_rgb1[3];
uniform int op_rgb1[3];
uniform int src_alfa1[3];
uniform int op_alfa1[3];
uniform float escala_rgb1;
uniform float escala_alfa1;
uniform vec4 cor_env1;
uniform int func_alfa;
uniform float ref_alfa;
out vec4 saida;

// The `GL_COMBINE` source: 0 the texture, 1 the constant, 2 the primary color, 3 the previous unit,
// 4 and 5 the texel of unit 0 and of unit 1 (the crossbar).
vec4 fonte_de(int qual, vec4 anterior, vec4 primaria, vec4 texel, vec4 constante, vec4 texel0,
              vec4 texel1) {
    if (qual == 0) { return texel; }
    if (qual == 1) { return constante; }
    if (qual == 2) { return primaria; }
    if (qual == 4) { return texel0; }
    if (qual == 5) { return texel1; }
    return anterior;
}

// If any source is the texel of a unit that does not draw, that unit acts as disabled: the
// `ARB_texture_env_crossbar` rule. Same as software's `TexEnv::combine`.
bool falta_unidade(int srgb[3], int salfa[3]) {
    for (int i = 0; i < 3; i++) {
        if ((srgb[i] == 4 || salfa[i] == 4) && texturando != 1) { return true; }
        if ((srgb[i] == 5 || salfa[i] == 5) && texturando1 != 1) { return true; }
    }
    return false;
}

// The RGB operand: the color, its complement, the alpha or its complement.
vec3 operando_rgb(int op, vec4 v) {
    if (op == 0) { return v.rgb; }
    if (op == 1) { return vec3(1.0) - v.rgb; }
    if (op == 2) { return vec3(v.a); }
    return vec3(1.0 - v.a);
}

// For alpha there are only the alpha operands; the color ones count as them, as in software.
float operando_alfa(int op, vec4 v) {
    if (op == 1 || op == 3) { return 1.0 - v.a; }
    return v.a;
}

vec3 funcao_rgb(int f, vec3 a0, vec3 a1, vec3 a2) {
    if (f == 0) { return a0; }
    if (f == 2) { return a0 + a1; }
    if (f == 3) { return a0 + a1 - 0.5; }
    if (f == 4) { return a0 * a2 + a1 * (1.0 - a2); }
    if (f == 5) { return a0 - a1; }
    if (f == 6 || f == 7) { return vec3(4.0 * dot(a0 - 0.5, a1 - 0.5)); }
    return a0 * a1;
}

float funcao_alfa(int f, float a0, float a1, float a2) {
    if (f == 0) { return a0; }
    if (f == 2) { return a0 + a1; }
    if (f == 3) { return a0 + a1 - 0.5; }
    if (f == 4) { return a0 * a2 + a1 * (1.0 - a2); }
    if (f == 5) { return a0 - a1; }
    return a0 * a1;
}

// One texture unit, case by case the same as software's `TexEnv::apply_with`: the classic
// modes act on what came out of the previous unit.
vec4 unidade(int modo, int crgb, int calfa, int srgb[3], int orgb[3], int salfa[3], int oalfa[3],
             float ergb, float ealfa, vec4 constante, vec4 anterior, vec4 primaria, vec4 texel,
             vec4 texel0, vec4 texel1) {
    if (modo == 4) {
        if (falta_unidade(srgb, salfa)) { return anterior; }
        vec3 r[3];
        float a[3];
        for (int i = 0; i < 3; i++) {
            r[i] = operando_rgb(orgb[i],
                                fonte_de(srgb[i], anterior, primaria, texel, constante, texel0, texel1));
            a[i] = operando_alfa(oalfa[i],
                                 fonte_de(salfa[i], anterior, primaria, texel, constante, texel0, texel1));
        }
        vec3 rgb = clamp(funcao_rgb(crgb, r[0], r[1], r[2]) * ergb, 0.0, 1.0);
        float alfa = clamp(funcao_alfa(calfa, a[0], a[1], a[2]) * ealfa, 0.0, 1.0);
        if (crgb == 7) { alfa = rgb.r; }
        return vec4(rgb, alfa);
    }
    if (modo == 0) { return texel; }
    if (modo == 1) {
        return vec4(anterior.rgb * (1.0 - texel.a) + texel.rgb * texel.a, anterior.a);
    }
    if (modo == 2) {
        return vec4(min(anterior.rgb + texel.rgb, vec3(1.0)), anterior.a * texel.a);
    }
    return anterior * texel;
}

void main() {
    vec4 primaria = vcor;
    vec4 cor = primaria;
    // Both units sample before either one combines: through the crossbar, unit 0 can read unit
    // 1's texel.
    vec4 texel0 = texturando == 1 ? texture(amostra, vuv) : vec4(0.0);
    vec4 texel1 = texturando1 == 1 ? texture(amostra1, vuv1) : vec4(0.0);
    if (texturando == 1) {
        cor = unidade(env, cmb_rgb, cmb_alfa, src_rgb, op_rgb, src_alfa, op_alfa, escala_rgb,
                      escala_alfa, cor_env, cor, primaria, texel0, texel0, texel1);
    }
    if (texturando1 == 1) {
        cor = unidade(env1, cmb_rgb1, cmb_alfa1, src_rgb1, op_rgb1, src_alfa1, op_alfa1,
                      escala_rgb1, escala_alfa1, cor_env1, cor, primaria, texel1, texel0, texel1);
    }
    // Fog comes after the texture and before the alpha test, and only touches RGB — the same
    // order as the software rasterizer.
    if (com_neblina == 1) {
        cor = vec4(mix(cor_neblina, cor.rgb, clamp(vfog, 0.0, 1.0)), cor.a);
    }
    bool passa;
    if      (func_alfa == 0) { passa = false; }
    else if (func_alfa == 1) { passa = cor.a <  ref_alfa; }
    else if (func_alfa == 2) { passa = cor.a == ref_alfa; }
    else if (func_alfa == 3) { passa = cor.a <= ref_alfa; }
    else if (func_alfa == 4) { passa = cor.a >  ref_alfa; }
    else if (func_alfa == 5) { passa = cor.a != ref_alfa; }
    else if (func_alfa == 6) { passa = cor.a >= ref_alfa; }
    else                     { passa = true; }
    if (!passa) { discard; }
    saida = cor;
}
"#;

/// The same vertex shader, in GLSL ES 1.00. vitaGL translates this, not `#version 300 es`.
const VERTEX_ES2: &str = r#"
attribute vec4 pos;
attribute vec4 cor;
attribute vec2 uv;
attribute float fog;
attribute vec2 uv1;
uniform float virar;
varying vec4 vcor;
varying vec2 vuv;
varying float vfog;
varying vec2 vuv1;
void main() {
    vcor = cor;
    vuv = uv;
    vfog = fog;
    vuv1 = uv1;
    gl_Position = vec4(pos.x, pos.y * virar, pos.z, pos.w);
}
"#;

/// The ES 3 fragment rewritten for ES 1.00: no `in`/`out`, no `texture()`, no array as a
/// function argument. The uniform indices are constants — vitaGL's translator does not accept
/// indexing an array with the loop variable.
const FRAGMENT_ES2: &str = r#"
precision mediump float;
precision mediump int;
varying vec4 vcor;
varying vec2 vuv;
varying float vfog;
varying vec2 vuv1;
uniform sampler2D amostra;
uniform sampler2D amostra1;
uniform int com_neblina;
uniform vec3 cor_neblina;
uniform int texturando;
uniform int texturando1;
uniform int env;
uniform int cmb_rgb;
uniform int cmb_alfa;
uniform int src_rgb[3];
uniform int op_rgb[3];
uniform int src_alfa[3];
uniform int op_alfa[3];
uniform float escala_rgb;
uniform float escala_alfa;
uniform vec4 cor_env;
uniform int env1;
uniform int cmb_rgb1;
uniform int cmb_alfa1;
uniform int src_rgb1[3];
uniform int op_rgb1[3];
uniform int src_alfa1[3];
uniform int op_alfa1[3];
uniform float escala_rgb1;
uniform float escala_alfa1;
uniform vec4 cor_env1;
uniform int func_alfa;
uniform float ref_alfa;

vec4 fonte_de(int qual, vec4 anterior, vec4 primaria, vec4 texel, vec4 constante, vec4 texel0,
              vec4 texel1) {
    if (qual == 0) { return texel; }
    if (qual == 1) { return constante; }
    if (qual == 2) { return primaria; }
    if (qual == 4) { return texel0; }
    if (qual == 5) { return texel1; }
    return anterior;
}

vec3 operando_rgb(int op, vec4 v) {
    if (op == 0) { return v.rgb; }
    if (op == 1) { return vec3(1.0) - v.rgb; }
    if (op == 2) { return vec3(v.a); }
    return vec3(1.0 - v.a);
}

float operando_alfa(int op, vec4 v) {
    if (op == 1 || op == 3) { return 1.0 - v.a; }
    return v.a;
}

vec3 funcao_rgb(int f, vec3 a0, vec3 a1, vec3 a2) {
    if (f == 0) { return a0; }
    if (f == 2) { return a0 + a1; }
    if (f == 3) { return a0 + a1 - 0.5; }
    if (f == 4) { return a0 * a2 + a1 * (1.0 - a2); }
    if (f == 5) { return a0 - a1; }
    if (f == 6 || f == 7) { return vec3(4.0 * dot(a0 - 0.5, a1 - 0.5)); }
    return a0 * a1;
}

float funcao_alfa(int f, float a0, float a1, float a2) {
    if (f == 0) { return a0; }
    if (f == 2) { return a0 + a1; }
    if (f == 3) { return a0 + a1 - 0.5; }
    if (f == 4) { return a0 * a2 + a1 * (1.0 - a2); }
    if (f == 5) { return a0 - a1; }
    return a0 * a1;
}

bool falta(int s0, int s1, int s2, int a0, int a1, int a2) {
    if ((s0 == 4 || s1 == 4 || s2 == 4 || a0 == 4 || a1 == 4 || a2 == 4) && texturando != 1) {
        return true;
    }
    if ((s0 == 5 || s1 == 5 || s2 == 5 || a0 == 5 || a1 == 5 || a2 == 5) && texturando1 != 1) {
        return true;
    }
    return false;
}

vec4 unidade(int modo, int crgb, int calfa, int srgb0, int srgb1, int srgb2, int orgb0, int orgb1,
             int orgb2, int salfa0, int salfa1, int salfa2, int oalfa0, int oalfa1, int oalfa2,
             float ergb, float ealfa, vec4 constante, vec4 anterior, vec4 primaria, vec4 texel,
             vec4 texel0, vec4 texel1) {
    if (modo == 4) {
        if (falta(srgb0, srgb1, srgb2, salfa0, salfa1, salfa2)) { return anterior; }
        vec4 f0 = fonte_de(srgb0, anterior, primaria, texel, constante, texel0, texel1);
        vec4 f1 = fonte_de(srgb1, anterior, primaria, texel, constante, texel0, texel1);
        vec4 f2 = fonte_de(srgb2, anterior, primaria, texel, constante, texel0, texel1);
        vec3 r0 = operando_rgb(orgb0, f0);
        vec3 r1 = operando_rgb(orgb1, f1);
        vec3 r2 = operando_rgb(orgb2, f2);
        vec4 g0 = fonte_de(salfa0, anterior, primaria, texel, constante, texel0, texel1);
        vec4 g1 = fonte_de(salfa1, anterior, primaria, texel, constante, texel0, texel1);
        vec4 g2 = fonte_de(salfa2, anterior, primaria, texel, constante, texel0, texel1);
        float a0 = operando_alfa(oalfa0, g0);
        float a1 = operando_alfa(oalfa1, g1);
        float a2 = operando_alfa(oalfa2, g2);
        vec3 rgb = clamp(funcao_rgb(crgb, r0, r1, r2) * ergb, 0.0, 1.0);
        float alfa = clamp(funcao_alfa(calfa, a0, a1, a2) * ealfa, 0.0, 1.0);
        if (crgb == 7) { alfa = rgb.r; }
        return vec4(rgb, alfa);
    }
    if (modo == 0) { return texel; }
    if (modo == 1) {
        return vec4(anterior.rgb * (1.0 - texel.a) + texel.rgb * texel.a, anterior.a);
    }
    if (modo == 2) {
        return vec4(min(anterior.rgb + texel.rgb, vec3(1.0)), anterior.a * texel.a);
    }
    return anterior * texel;
}

void main() {
    vec4 primaria = vcor;
    vec4 cor = primaria;
    vec4 texel0 = texturando == 1 ? texture2D(amostra, vuv) : vec4(0.0);
    vec4 texel1 = texturando1 == 1 ? texture2D(amostra1, vuv1) : vec4(0.0);
    if (texturando == 1) {
        cor = unidade(env, cmb_rgb, cmb_alfa, src_rgb[0], src_rgb[1], src_rgb[2], op_rgb[0],
                      op_rgb[1], op_rgb[2], src_alfa[0], src_alfa[1], src_alfa[2], op_alfa[0],
                      op_alfa[1], op_alfa[2], escala_rgb, escala_alfa, cor_env, cor, primaria,
                      texel0, texel0, texel1);
    }
    if (texturando1 == 1) {
        cor = unidade(env1, cmb_rgb1, cmb_alfa1, src_rgb1[0], src_rgb1[1], src_rgb1[2], op_rgb1[0],
                      op_rgb1[1], op_rgb1[2], src_alfa1[0], src_alfa1[1], src_alfa1[2], op_alfa1[0],
                      op_alfa1[1], op_alfa1[2], escala_rgb1, escala_alfa1, cor_env1, cor, primaria,
                      texel1, texel0, texel1);
    }
    if (com_neblina == 1) {
        cor = vec4(mix(cor_neblina, cor.rgb, clamp(vfog, 0.0, 1.0)), cor.a);
    }
    bool passa;
    if      (func_alfa == 0) { passa = false; }
    else if (func_alfa == 1) { passa = cor.a <  ref_alfa; }
    else if (func_alfa == 2) { passa = cor.a == ref_alfa; }
    else if (func_alfa == 3) { passa = cor.a <= ref_alfa; }
    else if (func_alfa == 4) { passa = cor.a >  ref_alfa; }
    else if (func_alfa == 5) { passa = cor.a != ref_alfa; }
    else if (func_alfa == 6) { passa = cor.a >= ref_alfa; }
    else                     { passa = true; }
    if (!passa) { discard; }
    gl_FragColor = cor;
}
"#;

/// Without `GL_COMBINE`. ES 2 guarantees 16 uniform vectors in the fragment shader, and the
/// combine chain goes over that on cards that do not pack `int`. Mode 4 falls back to modulate.
const FRAGMENT_ES2_SHORT: &str = r#"
precision mediump float;
varying vec4 vcor;
varying vec2 vuv;
varying float vfog;
varying vec2 vuv1;
uniform sampler2D amostra;
uniform sampler2D amostra1;
uniform int com_neblina;
uniform vec3 cor_neblina;
uniform int texturando;
uniform int texturando1;
uniform int env;
uniform int env1;
uniform int func_alfa;
uniform float ref_alfa;

vec4 aplica(int modo, vec4 anterior, vec4 texel) {
    if (modo == 0) { return texel; }
    if (modo == 1) {
        return vec4(anterior.rgb * (1.0 - texel.a) + texel.rgb * texel.a, anterior.a);
    }
    if (modo == 2) {
        return vec4(min(anterior.rgb + texel.rgb, vec3(1.0)), anterior.a * texel.a);
    }
    return anterior * texel;
}

void main() {
    vec4 cor = vcor;
    if (texturando == 1) { cor = aplica(env, cor, texture2D(amostra, vuv)); }
    if (texturando1 == 1) { cor = aplica(env1, cor, texture2D(amostra1, vuv1)); }
    if (com_neblina == 1) {
        cor = vec4(mix(cor_neblina, cor.rgb, clamp(vfog, 0.0, 1.0)), cor.a);
    }
    bool passa;
    if      (func_alfa == 0) { passa = false; }
    else if (func_alfa == 1) { passa = cor.a <  ref_alfa; }
    else if (func_alfa == 2) { passa = cor.a == ref_alfa; }
    else if (func_alfa == 3) { passa = cor.a <= ref_alfa; }
    else if (func_alfa == 4) { passa = cor.a >  ref_alfa; }
    else if (func_alfa == 5) { passa = cor.a != ref_alfa; }
    else if (func_alfa == 6) { passa = cor.a >= ref_alfa; }
    else                     { passa = true; }
    if (!passa) { discard; }
    gl_FragColor = cor;
}
"#;

/// Compiles the shader pair. Desktop and GLES 3 use the new text; vitaGL only translates ES 1.00.
unsafe fn compile(gl: &glow::Context) -> Result<glow::Program, String> {
    let version = gl.version();
    // On ES 2 the full combine text does compile in SceShaccCg and dies
    // inside `glLinkProgram`: vitaGL calls `sceGxmProgramGetParameterCount`
    // on an invalid GXM program, and the process ends without a link error that
    // would let it fall back to the short text. The coredump when opening the game
    // was at that `blx`, with the combine source (`src_rgb1`) still on the heap. The
    // short one fits in the fragment's 16 uniform vectors; `GL_COMBINE` becomes modulate.
    let es2 = version.is_embedded && version.major < 3;
    let complete = [
        ("#version 330 core\n", VERTEX, FRAGMENT),
        (
            "#version 300 es\nprecision highp float;\n",
            VERTEX,
            FRAGMENT,
        ),
        ("#version 100\n", VERTEX_ES2, FRAGMENT_ES2),
        ("#version 100\n", VERTEX_ES2, FRAGMENT_ES2_SHORT),
        ("\n", VERTEX_ES2, FRAGMENT_ES2_SHORT),
    ];
    let short = [
        ("#version 100\n", VERTEX_ES2, FRAGMENT_ES2_SHORT),
        ("\n", VERTEX_ES2, FRAGMENT_ES2_SHORT),
    ];
    let attempts: &[(&str, &str, &str)] = if es2 { &short } else { &complete };
    let mut last = String::from("no GLSL version accepted");
    for &(header, vertex, fragment) in attempts {
        match unsafe { link_program(gl, header, vertex, fragment) } {
            Ok(program) => return Ok(program),
            Err(error) => {
                // The first version the driver rejects explains, on the device, which text
                // got as far as linking. On desktop 3.30 passes and this line does not run.
                crate::logging!(
                    crate::logging::Level::Warning,
                    "gl",
                    "shader rejected: {error}"
                );
                last = error;
            }
        }
    }
    Err(last)
}

unsafe fn link_program(
    gl: &glow::Context,
    header: &str,
    vertex: &str,
    fragment: &str,
) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, source) in [
            (glow::VERTEX_SHADER, vertex),
            (glow::FRAGMENT_SHADER, fragment),
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
        // ES 1.00 has no `layout(location)`. The indices are those of `vertex_attrib_pointer`.
        for (index, name) in [(0, "pos"), (1, "cor"), (2, "uv"), (3, "fog"), (4, "uv1")] {
            gl.bind_attrib_location(program, index, name);
        }
        gl.link_program(program);
        // No `glDetachShader`: vitaGL does not export that entry point, and glow aborts
        // if the pointer is null. The shader marked for deletion goes away with the program.
        for shader in shaders {
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

/// An RGB565 pixel expanded to RGB888. Same expansion as the software rasterizer.
fn expand565(bytes: &[u8], offset: usize) -> [u8; 3] {
    let pixel = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    let r = ((pixel >> 11) & 31) as u8;
    let g = ((pixel >> 5) & 63) as u8;
    let b = (pixel & 31) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}

impl Rasterizer for GpuState {
    fn save_state(&self, dest: &mut crate::save_state::Sections) {
        // The state is the same `GlState` the software rasterizer uses — this one just draws it
        // on the graphics card instead of on the processor. Saving is delegating.
        crate::save_state::Saveable::save(&self.state, dest);
    }

    fn restore_state(
        &mut self,
        origin: &crate::save_state::Reader<'_>,
    ) -> Result<(), crate::save_state::Error> {
        crate::save_state::Saveable::restore(&mut self.state, origin)?;
        self.drop_frame_caches();
        self.recreate_restored_textures();
        // The draw state mirror is kept **incrementally** by the trait methods, not rebuilt from
        // the GlState. After loading, it would describe the previous world, and it is what goes to
        // GL on every batch — the scene would come out with the previous matrix and flags.
        self.resync_mirror();
        self.hand_back_context();
        Ok(())
    }

    fn flush_draw(&mut self) {
        self.flush();
    }

    fn draw_in_progress(&self) -> bool {
        self.state.draw_in_progress()
    }

    /// The outside framebuffer, when the frontend hands one over — and with it the **frame
    /// boundary**.
    ///
    /// **This is where the cache of bound objects is invalidated on the libretro path.** Between
    /// two frames the frontend draws its FBO in the same context, binding what it needs — and
    /// `e6a436e` measured that, in that consumer, what it leaves behind is harmless to us. What we
    /// cannot do is *assume* that: this call is the only one the core makes once per frame, before
    /// any drawing, and it is the right moment to stop trusting what was left from before. It costs
    /// three driver calls per frame, not per batch, plus the draw state resent once in the first
    /// batch — the mirror forgets too, for the same reason.
    fn frame_texture(&self) -> Option<u32> {
        // At the swap the destination is still the presented frame. The front only receives the
        // copy when the next frame starts writing over it, and that has not happened yet.
        let color = self.frame.as_ref().map(|dest| dest.color)?;
        Some(color.0.get())
    }

    fn draw_to_fbo(&mut self, fbo: Option<u32>) {
        if fbo.is_some() {
            self.forget_mirror();
            self.gpu.forget_bound();
        }
        self.external_fbo = fbo;
    }

    /// **The core's VAO must not stay bound when the frontend draws.** `e6a436e` stopped unbinding
    /// the program and VAO on every batch, and on the libretro path the frame ended with ours
    /// bound. RetroArch's `gl` driver on GLES does not bind a VAO of its own: its attribute
    /// pointers landed in our VAO, and with a non-zero VAO GLES 3 rejects vertices coming from
    /// processor memory (`GL_INVALID_OPERATION`). Drawing the FBO to the screen failed silently:
    /// black screen, with normal audio, on the GLES handhelds.
    ///
    /// Once per frame, not per batch: the gain from `e6a436e` stays.
    /// **When the window paints in the same context, the mirror no longer knows what is on the
    /// graphics card.** `hand_back_context` records what it leaves behind itself, and that holds
    /// until `egui` paints: it swaps the viewport for the window's, turns on the scissor and turns
    /// on premultiplied alpha blending. The mirror kept the old values and did not resend them —
    /// Ridge Racer drew in a corner of the window, with the texture backgrounds white.
    ///
    /// It is the counterpart of libretro's [`GpuState::draw_to_fbo`], and costs the same: once per
    /// window frame, not per batch.
    fn resume_context(&mut self) {
        if self.gpu.borrowed() {
            self.forget_mirror();
            self.gpu.forget_bound();
        }
    }

    fn hand_back_to_frontend(&mut self) {
        if self.external_fbo.is_none() || self.gpu.is_dead() {
            return;
        }
        let gl = &self.gl;
        unsafe {
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        self.gpu.forget_bound();
    }

    fn set_matrix_mode(&mut self, mode: u32) {
        self.state.set_matrix_mode(mode);
    }
    fn top_matrix(&self) -> Matrix {
        self.state.top_matrix()
    }
    fn load_identity(&mut self) {
        self.state.load_identity();
    }
    fn load_matrix(&mut self, m: Matrix) {
        self.state.load_matrix(m);
    }
    fn mult_matrix(&mut self, m: Matrix) {
        self.state.mult_matrix(m);
    }
    fn push_matrix(&mut self) {
        self.state.push_matrix();
    }
    fn pop_matrix(&mut self) {
        self.state.pop_matrix();
    }

    fn set_viewport(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.state.set_viewport(x, y, width, height);
        self.fill.viewport = (x, y, width, height);
    }
    fn set_scissor(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.state.set_scissor(x, y, width, height);
        self.fill.scissor = (x, y, width, height);
    }
    fn set_surface(&mut self, width: usize, height: usize) {
        self.flush();
        self.state.set_surface(width, height);
    }
    fn set_stretched_surface(&mut self, width: usize, height: usize) {
        self.flush();
        self.state.set_stretched_surface(width, height);
    }
    fn surface(&self) -> (usize, usize) {
        self.state.surface()
    }
    fn frame_size(&self) -> (usize, usize) {
        self.state.frame_size()
    }

    fn set_clear_color(&mut self, color: [f32; 4]) {
        self.state.set_clear_color(color);
        self.fill.clear_color = color;
    }
    fn set_clear_depth(&mut self, depth: f32) {
        self.state.set_clear_depth(depth);
        self.fill.clear_depth = depth;
    }
    fn set_clear_stencil(&mut self, value: i32) {
        self.state.set_clear_stencil(value);
        self.fill.clear_stencil = value;
    }
    fn set_color(&mut self, color: [f32; 4]) {
        self.state.set_color(color);
    }
    fn current_color(&self) -> [f32; 4] {
        self.state.current_color()
    }

    fn clear(&mut self, mask: u32) {
        self.flush();
        self.protect_front();
        self.dest();
        // **The software rasterizer's `clear` ignores the masks**: it fills the arrays directly.
        // `glClear` honors `glDepthMask`, `glStencilMask` and `glColorMask`, so they are opened
        // here — otherwise a clear requested with a closed mask would simply not happen, and the
        // next frame would draw over the previous one.
        let gl = &self.gl;
        let mut bits = 0;
        unsafe {
            if mask & gles::GL_COLOR_BUFFER_BIT != 0 {
                let [r, g, b, a] = self.fill.clear_color;
                gl.color_mask(true, true, true, true);
                gl.clear_color(r, g, b, a);
                bits |= glow::COLOR_BUFFER_BIT;
            }
            if mask & gles::GL_DEPTH_BUFFER_BIT != 0 {
                gl.depth_mask(true);
                gl.clear_depth_f32(self.fill.clear_depth);
                bits |= glow::DEPTH_BUFFER_BIT;
            }
            if mask & gles::GL_STENCIL_BUFFER_BIT != 0 {
                gl.stencil_mask(u32::MAX);
                gl.clear_stencil(self.fill.clear_stencil);
                bits |= glow::STENCIL_BUFFER_BIT;
            }
            if bits != 0 {
                // The `glScissor` clipping is left out: the software rasterizer does not have it,
                // and diverging here would make the comparison between the two useless.
                gl.disable(glow::SCISSOR_TEST);
                self.forget_scissor();
                gl.clear(bits);
            }
        }
        self.hand_back_context();
        if mask & gles::GL_COLOR_BUFFER_BIT != 0 {
            self.dirty = true;
        }
    }

    fn set_capability(&mut self, capability: u32, on: bool) {
        self.state.set_capability(capability, on);
        match capability {
            gles::GL_DEPTH_TEST => self.fill.depth_test = on,
            gles::GL_BLEND => self.fill.blend = on,
            gles::GL_ALPHA_TEST => self.fill.alpha_test = on,
            gles::GL_CULL_FACE => self.fill.cull_face = on,
            gles::GL_STENCIL_TEST => self.fill.stencil_test = on,
            gles::GL_SCISSOR_TEST => {
                self.fill.scissor_enabled = on;
                self.state.set_scissor_test(on);
            }
            // Turning texturing on and off is per unit; the first two draw.
            gles::GL_TEXTURE_2D if self.state.base_active_unit() => self.fill.texturing = on,
            gles::GL_TEXTURE_2D => self.fill.unit1 = self.state.unit1(),
            gles::GL_FOG => self.fill.fog = self.state.fog(),
            _ => {}
        }
    }
    fn set_shade_model(&mut self, mode: u32) {
        // **Both backends ignore `glShadeModel`**, on purpose: the software rasterizer stores the
        // mode and never reads it — it always interpolates. It is a divergence from GLES1 that they
        // share, and making the graphics card honor flat mode here would only make the comparison
        // between the two noisy without fixing anything visible.
        self.state.set_shade_model(mode);
    }
    fn set_light(&mut self, index: usize, pname: u32, values: [f32; 4]) {
        self.state.set_light(index, pname, values);
    }
    fn set_material(&mut self, pname: u32, values: [f32; 4]) {
        self.state.set_material(pname, values);
    }
    fn set_light_model(&mut self, pname: u32, values: [f32; 4]) {
        self.state.set_light_model(pname, values);
    }

    fn set_blend_func(&mut self, src: u32, dst: u32) {
        self.state.set_blend_func(src, dst);
        self.fill.blend_src = src;
        self.fill.blend_dst = dst;
    }
    fn set_alpha_func(&mut self, func: u32, reference: f32) {
        self.state.set_alpha_func(func, reference);
        self.fill.alpha_func = func;
        self.fill.alpha_ref = reference;
    }
    fn set_depth_func(&mut self, func: u32) {
        self.state.set_depth_func(func);
        self.fill.depth_func = func;
    }
    fn set_depth_mask(&mut self, on: bool) {
        self.state.set_depth_mask(on);
        self.fill.depth_mask = on;
    }
    fn set_depth_range(&mut self, near: f32, far: f32) {
        self.state.set_depth_range(near, far);
        self.fill.depth_range = (near.clamp(0.0, 1.0), far.clamp(0.0, 1.0));
    }
    fn set_fog(&mut self, pname: u32, values: [f32; 4]) {
        self.state.set_fog(pname, values);
        self.fill.fog = self.state.fog();
    }
    fn set_fog_allowed(&mut self, allowed: bool) {
        self.state.set_fog_allowed(allowed);
        self.fill.fog = self.state.fog();
    }
    fn set_color_mask(&mut self, mask: [bool; 4]) {
        self.state.set_color_mask(mask);
        self.fill.color_mask = mask;
    }
    fn set_cull_face(&mut self, mode: u32) {
        self.state.set_cull_face(mode);
        self.fill.cull_mode = mode;
    }
    fn set_front_face(&mut self, face: u32) {
        self.state.set_front_face(face);
        self.fill.front_face = face;
    }
    fn set_stencil_func(&mut self, func: u32, reference: i32, mask: u32) {
        self.state.set_stencil_func(func, reference, mask);
        self.fill.stencil_func = func;
        self.fill.stencil_ref = reference;
        self.fill.stencil_value_mask = mask;
    }
    fn set_stencil_op(&mut self, failure: u32, zfail: u32, passes: u32) {
        self.state.set_stencil_op(failure, zfail, passes);
        self.fill.stencil_op = [failure, zfail, passes];
    }
    fn set_stencil_mask(&mut self, mask: u32) {
        self.state.set_stencil_mask(mask);
        self.fill.stencil_write_mask = mask;
    }

    fn set_active_texture(&mut self, unit: u32) {
        self.state.set_active_texture(unit);
    }
    fn set_client_active_texture(&mut self, unit: u32) {
        self.state.set_client_active_texture(unit);
    }
    fn client_unit(&self) -> u32 {
        Rasterizer::client_unit(&self.state)
    }
    // **The binding belongs to the active unit**, here as in the software state: Resident Evil 4
    // ends each block binding a texture on unit 1, and letting that binding apply to the base unit
    // swapped its texture and painted the village white. See `GlState`'s `active_unit`.
    fn bind_texture(&mut self, name: u32) {
        self.state.bind_texture(name);
        if self.state.base_active_unit() {
            self.fill.bound_texture = name;
        }
        self.fill.unit1 = self.state.unit1();
    }
    fn bound_texture(&self) -> u32 {
        self.state.bound_texture()
    }
    fn set_texture_env(&mut self, mode: u32) {
        self.state.set_texture_env(mode);
        self.fill.texture_env = self.state.texture_env();
        self.fill.unit1 = self.state.unit1();
    }
    fn set_texture_env_param(&mut self, pname: u32, enum_value: u32, number: f32) {
        self.state.set_texture_env_param(pname, enum_value, number);
        self.fill.texture_env = self.state.texture_env();
        self.fill.unit1 = self.state.unit1();
    }
    fn set_texture_env_color(&mut self, color: [f32; 4]) {
        self.state.set_texture_env_color(color);
        self.fill.texture_env = self.state.texture_env();
        self.fill.unit1 = self.state.unit1();
    }
    fn set_texture_parameter(&mut self, name: u32, value: u32) {
        self.flush();
        self.state.set_texture_parameter(name, value);
        if !self.state.active_unit_draws() {
            return;
        }
        let enabled = self.state.bound_texture();
        if let Some(t) = self.textures.get_mut(&enabled) {
            match name {
                gles::GL_TEXTURE_MIN_FILTER => t.min_filter = value,
                gles::GL_TEXTURE_MAG_FILTER => t.filter = value,
                gles::GL_TEXTURE_WRAP_S => t.wrap[0] = value,
                gles::GL_TEXTURE_WRAP_T => t.wrap[1] = value,
                _ => {}
            }
        }
        if let Some(t) = self.textures.get(&enabled) {
            self.parameters(t);
        }
    }
    fn set_texture_crop(&mut self, crop: [i32; 4]) {
        self.state.set_texture_crop(crop);
        if !self.state.active_unit_draws() {
            return;
        }
        let enabled = self.state.bound_texture();
        if let Some(t) = self.textures.get_mut(&enabled) {
            t.crop = crop;
        }
    }
    fn delete_texture(&mut self, name: u32) {
        self.flush();
        self.state.delete_texture(name);
        if let Some(t) = self.textures.remove(&name) {
            // With the graphics card dead the name is dropped, not deleted: see
            // [`Gpu::deletes_on_drop`].
            if self.gpu.deletes_on_drop() {
                unsafe { self.gl.delete_texture(t.object) };
            }
        }
    }

    fn upload_level(
        &mut self,
        name: u32,
        level: u32,
        width: usize,
        height: usize,
        pixels: Vec<[u8; 4]>,
    ) {
        self.flush();
        let bytes: Vec<u8> = pixels.iter().flatten().copied().collect();
        self.measured.bytes_uploaded += bytes.len() as u64;
        let gl = &self.gl;
        let object = match self.textures.get(&name) {
            Some(t) => t.object,
            None => {
                let object = match unsafe { gl.create_texture() } {
                    Ok(object) => object,
                    Err(_) => return,
                };
                // **The parameters come before the image.** The game usually binds the name, asks
                // for filter and wrap and only then sends the image; until here the texture did
                // not exist on the graphics card, and `set_texture_parameter` only stored it in the
                // software state. Being born with a fixed `GL_LINEAR` threw away the `GL_NEAREST`
                // that Treino Cerebral asks for, and bilinear marked the seam of every piece of the
                // buttons. It is the same thing state restoration does in
                // `recreate_restored_textures`.
                let (filter, min_filter, wrap, crop) = match self.state.textures.get(&name) {
                    Some(t) => (t.filter, t.min_filter, t.wrap, t.crop),
                    None => (
                        gles::GL_LINEAR,
                        gles::GL_LINEAR,
                        [gles::GL_REPEAT; 2],
                        [0; 4],
                    ),
                };
                self.textures.insert(
                    name,
                    Texture {
                        object,
                        width,
                        height,
                        max_level: 0,
                        level_zero: false,
                        crop,
                        filter,
                        min_filter,
                        wrap,
                    },
                );
                object
            }
        };
        // On vitaGL, `glTexImage2D` and `glTexSubImage2D` ask for a new block. An image the
        // game resends at the same size writes to the pointer the first allocation
        // left behind; running that partition dry is what brought down `glClear`.
        let reuse = level == 0
            && self
                .textures
                .get(&name)
                .is_some_and(|t| t.level_zero && t.width == width && t.height == height);
        let mut needs_image = !reuse;
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(object));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            if reuse {
                #[cfg(target_os = "vita")]
                {
                    needs_image = !write_rgba_vita(&bytes, 0, 0, width, height, width, height);
                }
                #[cfg(not(target_os = "vita"))]
                {
                    gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        0,
                        0,
                        width as i32,
                        height as i32,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&bytes)),
                    );
                }
            }
            if needs_image {
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    level as i32,
                    color_format(&gl) as i32,
                    width as i32,
                    height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&bytes)),
                );
            }
        }
        if let Some(t) = self.textures.get_mut(&name) {
            if level == 0 {
                t.width = width;
                t.height = height;
                t.level_zero = true;
            }
            t.max_level = t.max_level.max(level);
        }
        if needs_image && let Some(t) = self.textures.get(&name) {
            self.parameters(t);
        }
        self.state.upload_level(name, level, width, height, pixels);
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
        self.flush();
        let result = self.state.sub_image(name, x, y, width, height, pixels);
        if result.is_ok() {
            if let Some(t) = self.textures.get(&name) {
                let bytes: Vec<u8> = pixels.iter().flatten().copied().collect();
                self.measured.bytes_uploaded += bytes.len() as u64;
                let (tex_width, tex_height, object) = (t.width, t.height, t.object);
                unsafe {
                    let gl = &self.gl;
                    gl.bind_texture(glow::TEXTURE_2D, Some(object));
                    gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                    #[cfg(target_os = "vita")]
                    let wrote = write_rgba_vita(
                        &bytes,
                        x as usize,
                        y as usize,
                        width as usize,
                        height as usize,
                        tex_width,
                        tex_height,
                    );
                    #[cfg(not(target_os = "vita"))]
                    let wrote = false;
                    if !wrote {
                        gl.tex_sub_image_2d(
                            glow::TEXTURE_2D,
                            0,
                            x as i32,
                            y as i32,
                            width as i32,
                            height as i32,
                            glow::RGBA,
                            glow::UNSIGNED_BYTE,
                            glow::PixelUnpackData::Slice(Some(&bytes)),
                        );
                    }
                }
            }
        }
        result
    }

    fn draw(&mut self, mode: u32, vertices: &[Vertex]) {
        // Lines become loose triangles by the same math as the software rasterizer — see
        // [`lines_to_triangles`]. Points are still not handled, there or here.
        let lines = matches!(
            mode,
            gles::GL_LINES | gles::GL_LINE_STRIP | gles::GL_LINE_LOOP
        );
        let glow_mode = match mode {
            gles::GL_TRIANGLES => glow::TRIANGLES,
            gles::GL_TRIANGLE_STRIP => glow::TRIANGLE_STRIP,
            gles::GL_TRIANGLE_FAN => glow::TRIANGLE_FAN,
            _ if lines => glow::TRIANGLES,
            _ => return,
        };
        // The polygon comes after the rectangles still in the batch.
        self.flush_sprites();
        self.state.vertex_stage(vertices);
        let extra = self.extra();
        let perspective = extra > 0 && self.state.is_perspective_projection();
        // The clip `x` shrinks in the ratio the viewport grows — see [`GpuState::apply`].
        let k = match perspective {
            true => {
                let sw = self.state.surface().0 as f32;
                sw / (sw + 2.0 * extra as f32)
            }
            false => 1.0,
        };
        // A state different from the current batch: what was already gathered goes first, with
        // its own state.
        if self
            .batch_state
            .as_ref()
            .is_some_and(|(state, p)| *p != perspective || *state != self.fill)
            || self.batch.len() >= BATCH_VERTICES * FLOATS_PER_VERTEX
        {
            self.flush();
        }
        if self.batch_state.is_none() {
            self.batch_state = Some((self.fill.clone(), perspective));
        }
        let mut loose = std::mem::take(&mut self.loose);
        loose.clear();
        let from_lines = lines.then(|| {
            lines_to_triangles(
                mode,
                self.state.transformed(),
                self.state.viewport,
                self.state.front_face != gles::GL_CW,
            )
        });
        let origin = from_lines.as_deref().unwrap_or(self.state.transformed());
        loose.extend(origin.iter().map(|v| {
            let [px, py, pz, pw] = v.position;
            Vertex {
                position: [px * k, py, pz, pw],
                ..*v
            }
        }));
        // Fans and strips become loose triangles in the order OpenGL assembles them, which is what
        // keeps the winding — and with it face culling. In a strip, the winding alternates at
        // every step, and swapping the first two keeps it.
        let n = loose.len();
        let mut put = |i: usize| push_vertex(&mut self.batch, &loose[i]);
        match glow_mode {
            glow::TRIANGLES => (0..n / 3 * 3).for_each(&mut put),
            glow::TRIANGLE_STRIP => {
                for i in 0..n.saturating_sub(2) {
                    let (a, b) = if i % 2 == 0 { (i, i + 1) } else { (i + 1, i) };
                    put(a);
                    put(b);
                    put(i + 2);
                }
            }
            _ => {
                for i in 1..n.saturating_sub(1) {
                    put(0);
                    put(i);
                    put(i + 1);
                }
            }
        }
        self.loose = loose;
    }

    fn draw_texture(&mut self, x: f32, y: f32, z: f32, width: f32, height: f32) {
        // Outside the Vita each rectangle goes to the graphics card right away, as before. The
        // batch exists because vitaGL charges for the whole scene on every `glDrawArrays`, and
        // gathering the sprites of the same sheet removes that charge. On other cards the
        // batch delayed the drawing: the frontend presents in `hand_back_to_frontend`,
        // before the swap, and whatever was still in the batch was left out of the frame.
        #[cfg(target_os = "vita")]
        self.flush_batch();
        #[cfg(not(target_os = "vita"))]
        self.flush();
        let enabled = self.fill.bound_texture;
        let Some(t) = self.textures.get(&enabled) else {
            return;
        };
        let (tw, th) = (t.width as f32, t.height as f32);
        if tw == 0.0 || th == 0.0 || width == 0.0 || height == 0.0 {
            return;
        }
        let [ucr, vcr, wcr, hcr] = t.crop.map(|v| v as f32);
        // A zeroed crop is the whole texture, as in the software rasterizer.
        let (wcr, hcr) = match (wcr, hcr) {
            (0.0, 0.0) => (tw, th),
            _ => (wcr, hcr),
        };
        let (s0, s1) = (ucr / tw, (ucr + wcr) / tw);
        let (t0, t1) = (vcr / th, (vcr + hcr) / th);
        let surface_height = self.surface().1 as f32;
        let (left, right) = (x, x + width);
        let (top, base) = (surface_height - y - height, surface_height - y);
        let color = self.current_color();
        let (vx, vy, vw, vh) = self.top_down_viewport();
        if vw <= 0 || vh <= 0 {
            return;
        }
        let corners = [
            ([left, top], [s0, t1]),
            ([left, base], [s0, t0]),
            ([right, base], [s1, t0]),
            ([right, top], [s1, t1]),
        ];
        let vertex = |sx: f32, sy: f32, uv: [f32; 2]| Vertex {
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
        };
        let quad = [
            vertex(corners[0].0[0], corners[0].0[1], corners[0].1),
            vertex(corners[1].0[0], corners[1].0[1], corners[1].1),
            vertex(corners[2].0[0], corners[2].0[1], corners[2].1),
            vertex(corners[3].0[0], corners[3].0[1], corners[3].1),
        ];
        // The extension defines no winding for the rectangle, so culling it by face would be
        // discarding a drawing the game expects to see.
        #[cfg(target_os = "vita")]
        {
            // Running the character means more of these rectangles, almost all on the same
            // sheet. Together, they go in a single call. The fan becomes two triangles so it can
            // sit next to the next one.
            let mut state = self.fill.clone();
            state.cull_face = false;
            state.texturing = true;
            let same = self.sprite_state.as_ref() == Some(&state)
                && self.sprites.len() + 6 * FLOATS_PER_VERTEX
                    <= BATCH_VERTICES * FLOATS_PER_VERTEX;
            if !same {
                self.flush_sprites();
                self.sprite_state = Some(state);
            }
            for i in [0, 1, 2, 0, 2, 3] {
                push_vertex(&mut self.sprites, &quad[i]);
            }
        }
        #[cfg(not(target_os = "vita"))]
        {
            self.vertices.clear();
            for i in [0, 1, 2, 3] {
                push_vertex(&mut self.vertices, &quad[i]);
            }
            let guard = self.fill.clone();
            self.fill.cull_face = false;
            self.fill.texturing = true;
            self.submit_with(glow::TRIANGLE_FAN, -1.0, None);
            self.fill = guard;
        }
    }

    /// **The graphics card's frame is stored with row 0 at the top**, because Y is flipped in the
    /// vertex shader — see [`VERTEX`]. The game's `glReadPixels`, however, counts `y` from the
    /// bottom and expects the first row of the result to be the bottom one, which is what the
    /// software rasterizer delivers. Reading it raw returned the image **upside down**: it was the
    /// Zeeboids photo saved inverted, and with it the character's face upside down in every game
    /// that uses it afterwards.
    ///
    /// So the requested band is converted to the corresponding framebuffer row and the result
    /// comes out mirrored back.
    fn read_rect(&mut self, x: i32, y: i32, width: usize, height: usize) -> Vec<[u8; 4]> {
        self.flush();
        if width == 0 || height == 0 {
            return Vec::new();
        }
        self.bind_for_read(false);
        let surface_height = self.surface().1 as i32;
        let from_bottom = surface_height - y - height as i32;
        let mut bytes = vec![0u8; width * height * 4];
        unsafe {
            self.gl.read_pixels(
                x,
                from_bottom,
                width as i32,
                height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut bytes)),
            );
        }
        self.hand_back_context();
        let mut output = Vec::with_capacity(width * height);
        for line in bytes.chunks_exact(width * 4).rev() {
            output.extend(line.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]));
        }
        output
    }

    fn frame_waits_for_gpu(&self) -> bool {
        true
    }

    fn end_frame(&mut self) {
        self.swap();
        GpuState::end_frame(self)
    }

    fn state_sent_and_spared(&self) -> (u64, u64) {
        GpuState::state_sent_and_spared(self)
    }

    fn frame_rgb565(&mut self, width: usize, height: usize, out: &mut Vec<u8>) {
        self.flush();
        // **The graphics card died in the middle of the frame: the read does not happen.**
        // `glReadPixels` is a call like the others, and after the notice the functions this state
        // stored do not exist. A black frame of the requested size comes out — just one, until the
        // session is born again on the next frame (see [`Gpu::is_dead`] and the race note in
        // `frontends/libretro`).
        if self.gpu.is_dead() {
            out.clear();
            out.resize(width * height * 2, 0);
            self.dirty = false;
            return;
        }
        // Same shortcut as the software rasterizer: a frame equal to what is already in `out` has
        // nothing to reconvert. On the graphics card this matters even more, because the read is
        // a round trip.
        if !self.dirty && out.len() == width * height * 2 {
            return;
        }
        let (sw, sh) = self.surface();
        if sw == 0 || sh == 0 {
            return;
        }
        // **The graphics card delivers the frame already in RGB565.** Converting RGBA on the CPU
        // cost 1.8 ms per frame in Need for Speed — six times the read itself — and at 60 frames
        // per second it was the biggest slice of `eglSwapBuffers`. At the same size it is a copy;
        // with resampling, each column's index is computed once per frame, not once per pixel.
        let in_565 = self.read_frame_rgb565(sw, sh);
        out.clear();
        out.resize(width * height * 2, 0);
        if in_565 && sw == width && sh == height {
            out.copy_from_slice(&self.pixels[..width * height * 2]);
        } else {
            let (step, convert): (usize, fn(&[u8]) -> [u8; 2]) = match in_565 {
                true => (2, |p| [p[0], p[1]]),
                false => (4, |p| {
                    (((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3))
                        .to_le_bytes()
                }),
            };
            let columns: Vec<usize> = (0..width)
                .map(|x| (x * sw / width).min(sw - 1) * step)
                .collect();
            for (y, output) in out.chunks_exact_mut(width * 2).enumerate() {
                let start = (y * sh / height).min(sh - 1) * sw * step;
                let line = &self.pixels[start..start + sw * step];
                for (pair, &column) in output.chunks_exact_mut(2).zip(&columns) {
                    pair.copy_from_slice(&convert(&line[column..column + step]));
                }
            }
        }
        self.dirty = false;
    }

    fn set_tile_discard(&mut self, discard: bool) {
        self.discard_tiles = discard;
        crate::logging!(
            crate::logging::Level::Info,
            "gl",
            "depth and stencil discard after the frame: {}",
            match discard {
                true => "on (experimental, for tiled GPUs)",
                false => "off",
            }
        );
    }

    fn frame_rgb565_words(&mut self, width: usize, height: usize, out: &mut Vec<u16>) {
        self.flush();
        // Dead graphics card: not even this read touches it. See [`GpuState::frame_rgb565`].
        if self.gpu.is_dead() {
            out.clear();
            out.resize(width * height, 0);
            self.dirty = false;
            return;
        }
        if !self.dirty && out.len() == width * height {
            return;
        }
        let (sw, sh) = self.surface();
        if sw == 0 || sh == 0 {
            return;
        }
        let in_565 = self.read_frame_rgb565(sw, sh);
        out.clear();
        out.resize(width * height, 0);
        let pixel = |p: &[u8]| -> u16 {
            if in_565 {
                u16::from_le_bytes([p[0], p[1]])
            } else {
                ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3)
            }
        };
        let step = if in_565 { 2 } else { 4 };
        let columns: Vec<usize> = (0..width)
            .map(|x| (x * sw / width).min(sw - 1) * step)
            .collect();
        for (y, output) in out.chunks_exact_mut(width).enumerate() {
            let start = (y * sh / height).min(sh - 1) * sw * step;
            let line = &self.pixels[start..start + sw * step];
            for (dest, &column) in output.iter_mut().zip(&columns) {
                *dest = pixel(&line[column..column + step]);
            }
        }
        self.dirty = false;
        // **Discarding the attachments nobody is going to read.** The color frame was just read
        // into CPU memory, and this frame's depth and stencil are not needed to draw the next
        // one — as long as the game clears them, which is the common case.
        //
        // **It is experimental and off by default**, because a game that does **not** clear the
        // depth counts on it from one frame to the next — the console is a real framebuffer, and
        // the depth there persists. On a tiled GPU, which is the case of the handhelds' Mali,
        // telling the driver this avoids writing the attachments back to memory: it pays off
        // there, and not on desktop, which is where it cannot be measured.
        if self.discard_tiles && self.external_fbo.is_none() {
            let gl = &self.gl;
            unsafe {
                gl.invalidate_framebuffer(glow::FRAMEBUFFER, &[glow::DEPTH, glow::STENCIL]);
            }
        }
    }

    fn import_rgb565_changes(&mut self, width: usize, height: usize, old: &[u8], new: &[u8]) {
        self.flush();
        let (sw, sh) = self.surface();
        if width == 0 || height == 0 || old.len() != width * height * 2 || new.len() != old.len() {
            return;
        }
        let (fw, fh) = self.frame_size();
        // **The source and the destination can have different sizes, and almost always do.** On
        // the Z-Wheel the request is 640x330 — the pbuffer — and the surface is 640x480. The
        // software rasterizer falls into a scaled loop and stretches the source over the surface;
        // drawing 1:1 into the first 330 rows squeezed everything into 0..226 after the frame was
        // resampled.
        //
        // Here the stretching is done by the graphics card's sampler, for free: the texture is the
        // whole source and the rectangle covers the surface.
        let (columns, lines) = (width, height);
        let dest_x = sw.min(fw);
        let dest_y = sh.min(fh);
        if columns == 0 || lines == 0 || dest_x == 0 || dest_y == 0 {
            return;
        }
        // **The alpha carries the mask.** The software rasterizer writes only the pixels that
        // changed and preserves the destination alpha; here the same effect comes out of a single
        // transfer: alpha 255 where it changed, 0 where it did not, the alpha test discards the
        // rest and `glColorMask` closes the alpha channel so as not to overwrite the destination's.
        self.pixels.clear();
        self.pixels.resize(columns * lines * 4, 0);
        let mut changed = false;
        for y in 0..lines {
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
                let [r, g, b] = expand565(new, offset);
                let dest = (y * columns + x) * 4;
                self.pixels[dest..dest + 4].copy_from_slice(&[r, g, b, 255]);
                changed = true;
            }
        }
        if !changed {
            return;
        }
        self.protect_front();
        self.dest();
        unsafe {
            let gl = &self.gl;
            gl.bind_texture(glow::TEXTURE_2D, Some(self.bridge));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            let measured = (columns as i32, lines as i32);
            // The bridge changes on every frame in which the 2D changes. On vitaGL this writes to
            // the block of the first image; `glTexSubImage2D` asked for another one and the
            // physical partition ran out at `glClear`.
            let same = self.bridge_size == Some(measured);
            let mut needs_image = !same;
            if same {
                #[cfg(target_os = "vita")]
                {
                    needs_image =
                        !write_rgba_vita(&self.pixels, 0, 0, columns, lines, columns, lines);
                }
                #[cfg(not(target_os = "vita"))]
                {
                    gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        0,
                        0,
                        measured.0,
                        measured.1,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&self.pixels)),
                    );
                }
            }
            if needs_image {
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    color_format(&gl) as i32,
                    measured.0,
                    measured.1,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&self.pixels)),
                );
                for (name, value) in [
                    (glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32),
                    (glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32),
                    (glow::TEXTURE_MAX_LEVEL, 0),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, name, value);
                }
                self.bridge_size = Some(measured);
            }
        }
        // The rectangle covers the surface inside the frame. The coordinates come out ready, so
        // the shader does not flip Y: `flip` is +1.
        let x1 = (dest_x as f32 / fw as f32) * 2.0 - 1.0;
        let y1 = (dest_y as f32 / fh as f32) * 2.0 - 1.0;
        self.vertices.clear();
        for ([px, py], uv) in [
            ([-1.0f32, -1.0f32], [0.0f32, 0.0f32]),
            ([x1, -1.0], [1.0, 0.0]),
            ([x1, y1], [1.0, 1.0]),
            ([-1.0, y1], [0.0, 1.0]),
        ] {
            self.put(&Vertex {
                position: [px, py, 0.0, 1.0],
                color: [1.0; 4],
                uv: [uv[0], uv[1], 0.0, 1.0],
                normal: [0.0, 0.0, 1.0],
                uv1: DEFAULT_UV,
                fog: 1.0,
            });
        }
        let guard = self.fill.clone();
        self.fill.fixed_top_down_viewport = Some((0, 0, fw as i32, fh as i32));
        self.fill.depth_test = false;
        self.fill.stencil_test = false;
        self.fill.blend = false;
        self.fill.cull_face = false;
        self.fill.depth_mask = false;
        self.fill.color_mask = [true, true, true, false];
        self.fill.texture_env = TexEnv::with_mode(gles::GL_REPLACE);
        self.fill.alpha_test = true;
        self.fill.alpha_func = gles::GL_GREATER;
        self.fill.alpha_ref = 0.5;
        let bridge = self.bridge;
        self.submit_with(glow::TRIANGLE_FAN, 1.0, Some(bridge));
        self.fill = guard;
        self.hand_back_context();
    }

    fn set_scale(&mut self, scale: usize) {
        // Without a reliable blit there is no downscaling of the big frame, and scaling is exactly
        // that: the larger drawing reduced before any read. Staying at 1x is the right answer.
        if !self.blit_reliable {
            return;
        }
        self.flush();
        // The ceiling is the largest attachment the graphics card accepts: a factor above it would
        // not create the destination.
        let maximum = unsafe {
            self.gl
                .get_parameter_i32(glow::MAX_RENDERBUFFER_SIZE)
                .min(self.gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE))
        }
        .max(1) as usize;
        let (fw, fh) = self.state.frame_size();
        let fits = (maximum / fw.max(fh).max(1)).max(1);
        let scale = scale.clamp(1, fits);
        if scale != self.scale {
            self.scale = scale;
            self.dirty = true;
        }
    }

    fn read_large_frame(&mut self) -> Option<(usize, usize, Vec<u8>)> {
        self.flush();
        let extra = self.extra();
        if self.scale <= 1 && extra == 0 {
            return None;
        }
        self.dest();
        // What the screen shows: the front, when the game has already written to the destination
        // after the swap.
        match self.front_valid && self.external_fbo.is_none() {
            true => unsafe { self.gl.bind_framebuffer(glow::FRAMEBUFFER, self.front.map(|f| f.0)) },
            false => self.resolve(),
        }
        let (sw, sh) = self.state.surface();
        let (w, h) = ((sw + 2 * extra) * self.scale, sh * self.scale);
        let mut bytes = vec![0u8; w * h * 4];
        unsafe {
            self.gl.read_pixels(
                0,
                0,
                w as i32,
                h as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut bytes)),
            );
        }
        self.hand_back_context();
        Some((w, h, bytes))
    }

    fn set_aspect_ratio(&mut self, aspect: Option<f32>) {
        self.flush();
        // Narrower than native opens nothing; the ceiling avoids an absurd attachment.
        let aspect = aspect.filter(|a| a.is_finite()).map(|a| a.clamp(4.0 / 3.0, 3.6));
        if aspect != self.ratio {
            self.ratio = aspect;
            self.dirty = true;
        }
    }

    fn set_antialias(&mut self, samples: usize) {
        // The MSAA `resolve` is a blit between framebuffers. Without a reliable blit, asking for
        // samples would give dirty edges instead of smooth ones.
        if !self.blit_reliable {
            return;
        }
        self.flush();
        let maximum = unsafe { self.gl.get_parameter_i32(glow::MAX_SAMPLES) }.max(1) as usize;
        // Powers of two are what graphics cards offer; 1 is off.
        let request = match samples {
            0 | 1 => 1,
            n => n.next_power_of_two().min(maximum.max(1)),
        };
        if request != self.samples {
            self.samples = request;
            self.dirty = true;
        }
    }

    fn set_anisotropic(&mut self, level: usize) {
        self.flush();
        let has = self.gl.supported_extensions().iter().any(|e| {
            e == "GL_EXT_texture_filter_anisotropic" || e == "GL_ARB_texture_filter_anisotropic"
        });
        let maximum = match has {
            true => unsafe { self.gl.get_parameter_f32(MAX_TEXTURE_MAX_ANISOTROPY) }.max(1.0),
            false => 1.0,
        };
        let level = (level.max(1) as f32).min(maximum);
        if level == self.anisotropy {
            return;
        }
        // Going back to 1 also has to be written to the textures: the parameter lives in them.
        let reset_to_one = level <= 1.0 && self.anisotropy > 1.0;
        self.anisotropy = level;
        let gl = self.gl.clone();
        for t in self.textures.values() {
            self.parameters(t);
            if reset_to_one {
                unsafe { gl.tex_parameter_f32(glow::TEXTURE_2D, TEXTURE_MAX_ANISOTROPY, 1.0) };
            }
        }
        unsafe { gl.bind_texture(glow::TEXTURE_2D, None) };
    }

    fn gpu_frame(&self) -> Option<GpuFrame> {
        let dest = self.frame.as_ref()?;
        if dest.scale <= 1 && dest.extra == 0 {
            return None;
        }
        let (fw, fh) = dest.measured;
        let (sw, sh) = self.state.surface();
        let extra = dest.extra;
        // The front, when the game has already started the next frame in the destination: see
        // [`GpuState::protect_front`].
        let texture = match self.front_valid {
            true => self.front.map_or(dest.color, |f| f.1),
            false => dest.color,
        };
        Some(GpuFrame {
            texture,
            clip: [
                (sw.min(fw) + 2 * extra) as f32 / (fw + 2 * extra).max(1) as f32,
                sh.min(fh) as f32 / fh.max(1) as f32,
            ],
            ratio: match self.state.stretched_surface() {
                // Stretched, the surface fills the screen: the height is the screen's, and the
                // width is its own plus the extra columns, at the stretch scale.
                true => (sw + 2 * extra) as f32 / sw.max(1) as f32 * fw as f32 / fh.max(1) as f32,
                false => (sw.min(fw) + 2 * extra) as f32 / sh.min(fh).max(1) as f32,
            },
        })
    }
}

/// The game's scissor in attachment pixels, still without the scale.
///
/// **The `y` becomes counted from the top, like the viewport's.** `glScissor` counts bottom to top
/// and the attachment stores the image top to bottom; the viewport goes through
/// `top_down_viewport` and the scissor went raw. On the full screen (`0 0 640 480`) both readings
/// coincide, which is why the bug only showed up on rectangles: Crash Nitro Kart draws the next
/// stretch of the track through a portal, with viewport and scissor on its rectangle, and the
/// scissor landed on the mirrored band of the screen — the portal came out empty and the scenery
/// "rose out of nowhere" when the kart went through it. It is the same symptom the viewport once
/// had, and it came back when the scissor started being honored on the graphics card.
///
/// **In widescreen, the scissor shifts, and only grows up to the edges it already touched.**
/// Whatever was on screen lands on the same pixel, shifted by `extra`; a full-screen scissor — the
/// one Resident Evil 4 and Crash Nitro Kart turn on in game — has to gain the new sides, or they
/// keep the attachment's background color. Widening it by the viewport ratio, as before, made a
/// portal's rectangle leak outside its frame.
fn scissor_on_attachment(
    (x, y, width, height): (i32, i32, i32, i32),
    (surface_width, surface_height): (usize, usize),
    extra: i32,
) -> (i32, i32, i32, i32) {
    let (s_width, s_height) = (surface_width as i32, surface_height as i32);
    let top = s_height - y - height;
    let left = match x <= 0 {
        true => 0,
        false => x + extra,
    };
    let right = match x + width >= s_width {
        true => s_width + 2 * extra,
        false => x + width + extra,
    };
    (left, top, right - left, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opens both rasterizers at the same size, or gives up when there is no graphics card.
    ///
    /// Without a context there is nothing to compare, and demanding a graphics card from whoever
    /// runs the suite would be asking it to fail on a machine without EGL. The tests here report
    /// and pass in that case.
    fn pair(width: usize, height: usize) -> Option<(GpuState, GlState)> {
        match GpuState::create(width, height, None) {
            Ok(gpu) => Some((gpu, GlState::new(width, height))),
            Err(reason) => {
                println!("no graphics card on this machine: {reason}");
                None
            }
        }
    }

    /// The graphics card restore has to recreate the real texture objects and both units.
    ///
    /// RE4 uses unit 1 to combine the base texture with the second layer. The software state always
    /// stored both, but 0.3.0 cleared the GPU object cache and forgot to copy unit 1 into the draw
    /// mirror; the result was a white scene after Load.
    #[test]
    fn save_state_recreates_textures_and_the_second_unit() {
        let Some((mut gpu, _)) = pair(16, 16) else {
            return;
        };
        // **The unit comes as a GL enum, not as an index.** `set_active_texture` receives what the
        // guest sends in `glActiveTexture`, and the state stores `enum - GL_TEXTURE0`. Passing
        // `1`, the subtraction gives a huge number, unit 1 is never set up — and the test failed
        // blaming the code for losing what the test itself had never put there. This test was red
        // on `development` because of that: it was born in the Android save states PR, whose CI
        // was cancelled before running it.
        gpu.set_active_texture(gles::GL_TEXTURE0);
        gpu.bind_texture(10);
        gpu.upload_level(10, 0, 1, 1, vec![[255, 0, 0, 255]]);
        gpu.set_capability(gles::GL_TEXTURE_2D, true);
        gpu.set_active_texture(gles::GL_TEXTURE0 + 1);
        gpu.bind_texture(20);
        gpu.upload_level(20, 0, 1, 1, vec![[0, 255, 0, 255]]);
        gpu.set_capability(gles::GL_TEXTURE_2D, true);

        let mut sections = crate::save_state::Sections::create();
        gpu.save_state(&mut sections);
        let file = sections.close();
        let reader = crate::save_state::Reader::open(&file).unwrap();

        gpu.delete_texture(10);
        gpu.delete_texture(20);
        gpu.restore_state(&reader).unwrap();

        assert!(gpu.textures.contains_key(&10), "the base texture did not come back to the graphics card");
        assert!(gpu.textures.contains_key(&20), "the unit 1 texture did not come back to the graphics card");
        assert!(gpu.fill.unit1.enabled, "unit 1 lost GL_TEXTURE_2D");
        assert_eq!(gpu.fill.unit1.texture, 20, "unit 1 came back with another name");
    }

    /// Draws the same thing in both and returns the frames in RGB565, for comparison.
    fn render_both(
        gpu: &mut GpuState,
        sw: &mut GlState,
        width: usize,
        height: usize,
        scene: impl Fn(&mut dyn Rasterizer),
    ) -> (Vec<u8>, Vec<u8>) {
        let mut a = Vec::new();
        let mut b = Vec::new();
        scene(gpu);
        scene(sw);
        gpu.frame_rgb565(width, height, &mut a);
        sw.frame_rgb565(width, height, &mut b);
        (a, b)
    }

    /// The `(x, y)` pixel of an RGB565 frame, as five/six-bit `(r, g, b)`.
    fn pixel(frame: &[u8], width: usize, x: usize, y: usize) -> (u16, u16, u16) {
        let i = (y * width + x) * 2;
        let v = u16::from_le_bytes([frame[i], frame[i + 1]]);
        ((v >> 11) & 31, (v >> 5) & 63, v & 31)
    }

    /// A triangle covering the top-left corner, to catch orientation and fill.
    ///
    /// It is the test that tells "the graphics card did not draw" apart from "the graphics card
    /// drew in the wrong place" — and the wrong place is the easiest mistake to make here, because
    /// OpenGL counts rows bottom to top and the console surface counts them top to bottom.
    #[test]
    fn the_triangle_lands_in_the_same_corner_in_both_rasterizers() {
        let (width, height) = (16, 16);
        let Some((mut gpu, mut sw)) = pair(width, height) else {
            return;
        };
        let scene = |r: &mut dyn Rasterizer| {
            r.set_viewport(0, 0, width as i32, height as i32);
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            // In clip space: the top-left corner is (-1, +1).
            let corner = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [1.0, 0.0, 0.0, 1.0],
                uv: DEFAULT_UV,
                uv1: DEFAULT_UV,
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[corner(-1.0, 1.0), corner(-1.0, -1.0), corner(1.0, 1.0)],
            );
        };
        let (a, b) = render_both(&mut gpu, &mut sw, width, height, scene);

        let corner_gpu = pixel(&a, width, 1, 1);
        let corner_software = pixel(&b, width, 1, 1);
        assert_eq!(
            corner_software.0, 31,
            "the software should paint the top corner red"
        );
        assert_eq!(
            corner_gpu, corner_software,
            "top corner: graphics card {corner_gpu:?} vs software {corner_software:?}"
        );
        let bottom_gpu = pixel(&a, width, 14, 14);
        let bottom_software = pixel(&b, width, 14, 14);
        assert_eq!(
            bottom_gpu, bottom_software,
            "bottom corner: graphics card {bottom_gpu:?} vs software {bottom_software:?}"
        );
    }

    /// `glReadPixels` counts `y` from the bottom, and both rasterizers have to agree.
    ///
    /// The graphics card stores the frame with row 0 at the top, because Y is flipped in the
    /// shader; the software stores it the same way, but converts on read. Reading the graphics card
    /// raw returned the image upside down — it was the Zeeboids photo saved inverted, and with it
    /// the character's face upside down in every game that uses it afterwards.
    #[test]
    fn pixel_readback_has_the_same_orientation_in_both_rasterizers() {
        let (width, height) = (16, 16);
        let Some((mut gpu, mut sw)) = pair(width, height) else {
            return;
        };
        // Top half red: a frame that is not vertically symmetric.
        let scene = |r: &mut dyn Rasterizer| {
            r.set_viewport(0, 0, width as i32, height as i32);
            r.set_clear_color([0.0, 0.0, 1.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            let corner = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [1.0, 0.0, 0.0, 1.0],
                uv: DEFAULT_UV,
                uv1: DEFAULT_UV,
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[corner(-1.0, 1.0), corner(-1.0, 0.0), corner(1.0, 1.0)],
            );
            r.draw(
                gles::GL_TRIANGLES,
                &[corner(1.0, 1.0), corner(-1.0, 0.0), corner(1.0, 0.0)],
            );
        };
        scene(&mut gpu);
        scene(&mut sw);
        let from_gpu = gpu.read_rect(0, 0, width, height);
        let from_software = sw.read_rect(0, 0, width, height);
        // The first row of the result is the **bottom** row of the screen, which here is blue.
        assert_eq!(
            from_software[width + 1][2], 255,
            "in software the first row read should be the bottom one, blue"
        );
        assert_eq!(
            from_gpu[width + 1],
            from_software[width + 1],
            "bottom row: graphics card {:?} vs software {:?}",
            from_gpu[width + 1],
            from_software[width + 1]
        );
        let high = (height - 2) * width + 1;
        assert_eq!(
            from_gpu[high], from_software[high],
            "top row: graphics card {:?} vs software {:?}",
            from_gpu[high], from_software[high]
        );
    }

    /// Fog tints the fragment the same way in both rasterizers, and goes away when turned off.
    ///
    /// The factor comes from the distance in eye coordinates and from the vertex stage, which is
    /// shared by both; what is compared here is what each one does with it in the fragment.
    #[test]
    fn fog_tints_the_same_in_both_rasterizers() {
        let (width, height) = (16, 16);
        let Some((mut gpu, mut sw)) = pair(width, height) else {
            return;
        };
        // A red square ten units from the eye, with blue fog from 0 to 20: halfway, so half of
        // each color.
        let scene = |r: &mut dyn Rasterizer| {
            r.set_viewport(0, 0, width as i32, height as i32);
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            // An orthographic projection from 1 to 100 in `z`, so the square ten units from the
            // eye fits in the clip volume without touching `x` and `y`.
            r.set_matrix_mode(gles::GL_PROJECTION);
            r.load_identity();
            let (near, far) = (1.0f32, 100.0f32);
            let mut ortho = crate::video::rasterizer::IDENTITY;
            ortho[10] = -2.0 / (far - near);
            ortho[14] = -(far + near) / (far - near);
            r.mult_matrix(ortho);
            r.set_matrix_mode(gles::GL_MODELVIEW);
            r.load_identity();
            r.mult_matrix(crate::video::rasterizer::translation(0.0, 0.0, -10.0));
            r.set_fog(gles::GL_FOG_MODE, [gles::GL_LINEAR as f32, 0.0, 0.0, 0.0]);
            r.set_fog(gles::GL_FOG_START, [0.0; 4]);
            r.set_fog(gles::GL_FOG_END, [20.0, 0.0, 0.0, 0.0]);
            r.set_fog(gles::GL_FOG_COLOR, [0.0, 0.0, 1.0, 1.0]);
            let corner = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [1.0, 0.0, 0.0, 1.0],
                uv: DEFAULT_UV,
                uv1: DEFAULT_UV,
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            // In clip space the projection is the identity, so the eye `z` is the translation's:
            // ten units, halfway to the end of the fog.
            r.draw(
                gles::GL_TRIANGLES,
                &[corner(-1.0, 1.0), corner(-1.0, -1.0), corner(1.0, 1.0)],
            );
        };
        let draw_scene = |r: &mut dyn Rasterizer| {
            r.set_capability(gles::GL_FOG, true);
            scene(r);
        };
        let (a, b) = render_both(&mut gpu, &mut sw, width, height, draw_scene);
        let (gpu_pixel, software) = (pixel(&a, width, 1, 1), pixel(&b, width, 1, 1));
        assert!(
            software.0 > 10 && software.0 < 24 && software.2 > 5,
            "the software should mix red and blue, and gave {software:?}"
        );
        assert!(
            gpu_pixel.0.abs_diff(software.0) <= 1 && gpu_pixel.2.abs_diff(software.2) <= 1,
            "with fog: graphics card {gpu_pixel:?} vs software {software:?}"
        );
        // Turned off by the player, the color goes back to the game's in both.
        let without = |r: &mut dyn Rasterizer| {
            r.set_fog_allowed(false);
            r.set_capability(gles::GL_FOG, true);
            scene(r);
        };
        let (a, b) = render_both(&mut gpu, &mut sw, width, height, without);
        assert_eq!(
            pixel(&b, width, 1, 1),
            (31, 0, 0),
            "without fog the software should keep the game's red"
        );
        assert_eq!(
            pixel(&a, width, 1, 1),
            pixel(&b, width, 1, 1),
            "without fog both have to agree again"
        );
    }

    /// The Bóia Cross water, with the calls the game makes: `DOT3` between the normal map and the
    /// light vector in the vertex color, and unit 1 multiplying by the water color it reads
    /// through `GL_TEXTURE1`. The two invalid functions in the middle are the game's and must be
    /// ignored.
    ///
    /// Before, both rasterizers gave blue: the normal map times the vertex, squared.
    #[test]
    fn boia_cross_water_matches_in_both_rasterizers() {
        let (width, height) = (16, 16);
        let Some((mut gpu, mut sw)) = pair(width, height) else {
            return;
        };
        let scene = |r: &mut dyn Rasterizer| {
            r.set_viewport(0, 0, width as i32, height as i32);
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            let unit1 = gles::GL_TEXTURE0 + 1;
            r.set_active_texture(gles::GL_TEXTURE0);
            r.bind_texture(1);
            r.upload_level(1, 0, 1, 1, vec![[128, 128, 255, 255]]);
            r.set_capability(gles::GL_TEXTURE_2D, true);
            r.set_texture_env(gles::GL_COMBINE);
            r.set_texture_env_param(gles::GL_COMBINE_RGB, gles::GL_DOT3_RGB, 0.0);
            r.set_texture_env_param(gles::GL_COMBINE_RGB, gles::GL_TEXTURE0, 0.0);
            r.set_texture_env_param(gles::GL_SRC0_RGB + 1, gles::GL_PRIMARY_COLOR, 0.0);
            r.set_active_texture(unit1);
            r.bind_texture(2);
            r.upload_level(2, 0, 1, 1, vec![[160, 120, 80, 255]]);
            r.set_capability(gles::GL_TEXTURE_2D, true);
            r.set_texture_env(gles::GL_COMBINE);
            r.set_texture_env_param(gles::GL_COMBINE_RGB, gles::GL_COMBINE, 0.0);
            r.set_texture_env_param(gles::GL_SRC0_RGB, gles::GL_PREVIOUS, 0.0);
            r.set_texture_env_param(gles::GL_SRC0_RGB + 1, unit1, 0.0);
            r.set_texture_env_param(gles::GL_COMBINE_ALPHA, gles::GL_REPLACE, 0.0);
            r.set_texture_env_param(gles::GL_SRC0_ALPHA, unit1, 0.0);
            r.set_texture_env_param(gles::GL_SRC0_ALPHA + 1, gles::GL_TEXTURE0, 0.0);
            // The light (0, 0, ½) encoded as a color: the `DOT3` with the normal (0, 0, ½) gives ½.
            let corner = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [0.5, 0.5, 0.75, 0.0],
                uv: DEFAULT_UV,
                uv1: DEFAULT_UV,
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[corner(-1.0, 1.0), corner(-1.0, -1.0), corner(1.0, 1.0)],
            );
        };
        let (a, b) = render_both(&mut gpu, &mut sw, width, height, scene);
        let (gpu_pixel, software) = (pixel(&a, width, 1, 1), pixel(&b, width, 1, 1));
        // Half of the brown (80, 60, 40) in RGB565: close to (9.7, 14.8, 4.9).
        assert!(
            (9..=10).contains(&software.0)
                && (14..=15).contains(&software.1)
                && (4..=5).contains(&software.2),
            "the software should give half the brown, and gave {software:?}"
        );
        assert!(
            gpu_pixel.0.abs_diff(software.0) <= 1
                && gpu_pixel.1.abs_diff(software.1) <= 1
                && gpu_pixel.2.abs_diff(software.2) <= 1,
            "graphics card {gpu_pixel:?} vs software {software:?}"
        );
    }

    /// With a higher internal resolution, the game reads back the same frame as always.
    ///
    /// The drawing happens in an attachment `scale` times larger, and the read goes through a
    /// downscale on the graphics card. Away from the edges the pixel has to be the native frame's;
    /// at the edges the downscale mixes neighbors, and that is exactly what smooths — which is why
    /// only the interiors are compared. The texture for the window has to exist, and the clip has
    /// to cover the whole surface.
    #[test]
    fn with_scale_readback_stays_at_console_size() {
        let (width, height) = (16, 16);
        let Some((mut native, mut sw)) = pair(width, height) else {
            return;
        };
        let Ok(mut large) = GpuState::create(width, height, None) else {
            return;
        };
        large.set_scale(2);
        let scene = |r: &mut dyn Rasterizer| {
            r.set_viewport(0, 0, width as i32, height as i32);
            r.set_clear_color([0.0, 0.0, 1.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            let corner = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [1.0, 0.0, 0.0, 1.0],
                uv: DEFAULT_UV,
                uv1: DEFAULT_UV,
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[corner(-1.0, 1.0), corner(-1.0, -1.0), corner(1.0, 1.0)],
            );
        };
        let (a, _) = render_both(&mut native, &mut sw, width, height, scene);
        scene(&mut large);
        let mut b = Vec::new();
        large.frame_rgb565(width, height, &mut b);
        assert_eq!(b.len(), a.len(), "the read comes out at the console size");
        for (x, y) in [(1, 1), (2, 3), (14, 14), (13, 12)] {
            assert_eq!(
                pixel(&b, width, x, y),
                pixel(&a, width, x, y),
                "pixel ({x}, {y}) at scale 2"
            );
        }
        let frame = large.gpu_frame().expect("large texture for the window");
        assert_eq!(frame.clip, [1.0, 1.0]);
        assert!(native.gpu_frame().is_none(), "at scale 1 the window uses the usual screen");
    }

    /// With antialiasing the triangle's edge mixes the two colors, and the interior stays as it
    /// was.
    ///
    /// The drawing goes to a multisample framebuffer and is resolved before the read. On the
    /// diagonal, some pixel has to come out with red and blue at the same time — that is the proof
    /// that the samples reached the read; without resolving, the read would come out black or
    /// fail.
    #[test]
    fn with_antialias_the_diagonal_blends_the_colors() {
        let (width, height) = (16, 16);
        let Ok(mut gpu) = GpuState::create(width, height, None) else {
            return;
        };
        gpu.set_antialias(4);
        if gpu.samples <= 1 {
            println!("graphics card without multisampling");
            return;
        }
        gpu.set_viewport(0, 0, width as i32, height as i32);
        gpu.set_clear_color([0.0, 0.0, 1.0, 1.0]);
        gpu.clear(gles::GL_COLOR_BUFFER_BIT);
        let corner = |x: f32, y: f32| Vertex {
            position: [x, y, 0.0, 1.0],
            color: [1.0, 0.0, 0.0, 1.0],
            uv: DEFAULT_UV,
            uv1: DEFAULT_UV,
            normal: [0.0, 0.0, 1.0],
                fog: 1.0,
        };
        gpu.draw(
            gles::GL_TRIANGLES,
            &[corner(-1.0, 1.0), corner(-1.0, -1.0), corner(1.0, 1.0)],
        );
        let mut frame = Vec::new();
        gpu.frame_rgb565(width, height, &mut frame);
        assert_eq!(frame.len(), width * height * 2);
        assert_eq!(pixel(&frame, width, 1, 1), (31, 0, 0), "red interior");
        assert_eq!(pixel(&frame, width, 14, 14), (0, 0, 31), "blue interior");
        let blended = (0..width).any(|x| {
            let (r, _, b) = pixel(&frame, width, x, width - 1 - x);
            r > 0 && b > 0
        });
        assert!(blended, "some pixel on the diagonal should mix red and blue");
    }

    /// A rectangle's scissor lands in the same place as the rectangle, with `y` counted from the
    /// top.
    ///
    /// The Crash Nitro Kart portal is a viewport and a scissor on a rectangle; with the raw `y`
    /// the scissor landed on the mirrored band and the portal came out empty.
    #[test]
    fn a_rect_scissor_counts_y_from_the_top() {
        // 42×64 at 21 from the left and 42 from the bottom, on a 640×480 screen: the top is at 374.
        assert_eq!(scissor_on_attachment((21, 42, 42, 64), (640, 480), 0), (21, 374, 42, 64));
        // In widescreen it only shifts; the full screen gains the sides.
        assert_eq!(scissor_on_attachment((21, 42, 42, 64), (640, 480), 80), (101, 374, 42, 64));
        assert_eq!(scissor_on_attachment((0, 0, 640, 480), (640, 480), 80), (0, 0, 800, 480));
    }

    /// In widescreen, the game's scissor must not cut off the new sides.
    ///
    /// `glScissor` arrives in console pixels, and the attachment is wider: without the same
    /// conversion the viewport gets, a full-screen scissor — which is what Resident Evil 4 and
    /// Crash Nitro Kart turn on in game — cut everything beyond the console's 640, and the sides
    /// kept the attachment's background color.
    #[test]
    fn in_widescreen_the_game_scissor_does_not_eat_the_sides() {
        let (width, height) = (64, 48);
        let Ok(mut gpu) = GpuState::create(width, height, None) else {
            println!("no graphics card on this machine");
            return;
        };
        gpu.set_aspect_ratio(Some(16.0 / 9.0));
        gpu.set_viewport(0, 0, width as i32, height as i32);
        // Any perspective: it is what makes the batch gain the sides.
        gpu.set_matrix_mode(gles::GL_PROJECTION);
        gpu.load_identity();
        let mut perspective = crate::video::rasterizer::IDENTITY;
        perspective[11] = -1.0;
        perspective[15] = 0.0;
        perspective[10] = -1.0;
        perspective[14] = -2.0;
        gpu.mult_matrix(perspective);
        gpu.set_matrix_mode(gles::GL_MODELVIEW);
        gpu.load_identity();
        // The full-screen scissor, in console pixels.
        gpu.set_scissor(0, 0, width as i32, height as i32);
        gpu.set_capability(gles::GL_SCISSOR_TEST, true);
        gpu.set_clear_color([0.0, 0.0, 0.0, 1.0]);
        gpu.clear(gles::GL_COLOR_BUFFER_BIT);
        // A square much larger than the screen, to also cover the new sides.
        let corner = |x: f32, y: f32| Vertex {
            position: [x, y, -1.0, 1.0],
            color: [0.0, 1.0, 0.0, 1.0],
            uv: DEFAULT_UV,
            uv1: DEFAULT_UV,
            normal: [0.0, 0.0, 1.0],
            fog: 1.0,
        };
        for tri in [
            [corner(-4.0, 4.0), corner(-4.0, -4.0), corner(4.0, 4.0)],
            [corner(4.0, 4.0), corner(-4.0, -4.0), corner(4.0, -4.0)],
        ] {
            gpu.draw(gles::GL_TRIANGLES, &tri);
        }
        let (w, h, rgba) = gpu.read_large_frame().expect("the wide frame exists");
        assert!(w > width, "widescreen should widen the attachment");
        let green = |x: usize| {
            let i = ((h / 2) * w + x) * 4;
            rgba[i + 1]
        };
        assert!(green(w / 2) > 200, "the center should be painted");
        assert!(
            green(w - 2) > 200,
            "the right edge was {} — the scissor ate the new side",
            green(w - 2)
        );
        assert!(green(1) > 200, "the left edge was {}", green(1));
    }

    /// A game that never calls `glViewport` has to draw anyway.
    ///
    /// The Z-Wheel is like that — zero calls in thirteen seconds — and this was the bug that erased
    /// its wheel, floor and dialog on the first run on the graphics card: the viewport started at
    /// zero.
    #[test]
    fn without_glviewport_the_gpu_draws_on_the_whole_screen() {
        let (width, height) = (16, 16);
        let Some((mut gpu, mut sw)) = pair(width, height) else {
            return;
        };
        let scene = |r: &mut dyn Rasterizer| {
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            let corner = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [0.0, 1.0, 0.0, 1.0],
                uv: DEFAULT_UV,
                uv1: DEFAULT_UV,
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[corner(-1.0, 1.0), corner(-1.0, -1.0), corner(1.0, 1.0)],
            );
        };
        let (a, b) = render_both(&mut gpu, &mut sw, width, height, scene);
        assert_eq!(
            pixel(&b, width, 1, 1).1,
            63,
            "the software should paint the corner green without any viewport"
        );
        assert_eq!(
            pixel(&a, width, 1, 1),
            pixel(&b, width, 1, 1),
            "no viewport: graphics card {:?} vs software {:?}",
            pixel(&a, width, 1, 1),
            pixel(&b, width, 1, 1)
        );
    }

    /// Geometry beyond the far plane: the software draws it, and OpenGL would clip it.
    ///
    /// The software rasterizer only clips at the **near** plane; OpenGL clips at all six frustum
    /// planes. A game that goes past the far plane — and the Z-Wheel does — would see the surface
    /// simply disappear on the graphics card. That is what `GL_DEPTH_CLAMP` solves: instead of
    /// clipping in depth, it clamps the value to the range.
    #[test]
    fn what_goes_past_the_far_plane_shows_in_both() {
        let (width, height) = (16, 16);
        let Some((mut gpu, mut sw)) = pair(width, height) else {
            return;
        };
        let scene = |r: &mut dyn Rasterizer| {
            r.set_viewport(0, 0, width as i32, height as i32);
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            // Z beyond 1 in clip space: outside the frustum through the far plane.
            let corner = |x: f32, y: f32| Vertex {
                position: [x, y, 1.5, 1.0],
                color: [0.0, 0.0, 1.0, 1.0],
                uv: DEFAULT_UV,
                uv1: DEFAULT_UV,
                normal: [0.0, 0.0, 1.0],
                fog: 1.0,
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[corner(-1.0, 1.0), corner(-1.0, -1.0), corner(1.0, 1.0)],
            );
        };
        let (a, b) = render_both(&mut gpu, &mut sw, width, height, scene);
        assert_eq!(
            pixel(&b, width, 2, 2).2,
            31,
            "the software should draw, because it does not clip at the far plane"
        );
        assert_eq!(
            pixel(&a, width, 2, 2),
            pixel(&b, width, 2, 2),
            "beyond the far plane: graphics card {:?} vs software {:?}",
            pixel(&a, width, 2, 2),
            pixel(&b, width, 2, 2)
        );
    }

    /// Face culling has to agree in both, in all four combinations.
    ///
    /// It is the point where the conventions cross: the software rasterizer decides in screen
    /// coordinates and calls the **negative** area case CCW (the `counter_clockwise` line), while
    /// OpenGL calls the positive area case CCW. Since Y is flipped in the shader, the coordinates
    /// of both are numerically the same — so the front face has to be inverted on entering the
    /// graphics card. This test is what proves the inversion goes the right way, and not the
    /// opposite.
    #[test]
    fn face_culling_agrees_in_both() {
        let (width, height) = (16, 16);
        let Some((mut gpu, mut sw)) = pair(width, height) else {
            return;
        };
        for front in [gles::GL_CCW, gles::GL_CW] {
            for inverted in [false, true] {
                let scene = |r: &mut dyn Rasterizer| {
                    r.set_viewport(0, 0, width as i32, height as i32);
                    r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
                    r.clear(gles::GL_COLOR_BUFFER_BIT);
                    r.set_capability(gles::GL_CULL_FACE, true);
                    r.set_cull_face(gles::GL_BACK);
                    r.set_front_face(front);
                    let corner = |x: f32, y: f32| Vertex {
                        position: [x, y, 0.0, 1.0],
                        color: [1.0, 1.0, 1.0, 1.0],
                        uv: DEFAULT_UV,
                        uv1: DEFAULT_UV,
                        normal: [0.0, 0.0, 1.0],
                fog: 1.0,
                    };
                    // A triangle covering the center, in both vertex orders.
                    let tri = match inverted {
                        false => [corner(-1.0, 1.0), corner(-1.0, -1.0), corner(1.0, 1.0)],
                        true => [corner(1.0, 1.0), corner(-1.0, -1.0), corner(-1.0, 1.0)],
                    };
                    r.draw(gles::GL_TRIANGLES, &tri);
                };
                let (a, b) = render_both(&mut gpu, &mut sw, width, height, scene);
                let on_gpu = pixel(&a, width, 2, 2);
                let in_software = pixel(&b, width, 2, 2);
                assert_eq!(
                    on_gpu, in_software,
                    "front={front:#x} inverted={inverted}: graphics card {on_gpu:?} vs \
                     software {in_software:?}"
                );
            }
        }
    }

    /// `import_rgb565_changes` is how the game's 2D gets onto the stage: it is what brings the
    /// Z-Wheel's dialog and covers into the OpenGL frame.
    #[test]
    fn the_import_puts_the_game_pixels_in_the_same_place() {
        let (width, height) = (8, 8);
        let Some((mut gpu, mut sw)) = pair(width, height) else {
            return;
        };
        // An all-black frame, and a second one with the first row white: only that row should
        // change.
        let old = vec![0u8; width * height * 2];
        let mut create = old.clone();
        for x in 0..width {
            create[x * 2] = 0xff;
            create[x * 2 + 1] = 0xff;
        }
        let scene = |r: &mut dyn Rasterizer| {
            r.set_viewport(0, 0, width as i32, height as i32);
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
        };
        scene(&mut gpu);
        scene(&mut sw);
        gpu.import_rgb565_changes(width, height, &old, &create);
        sw.import_rgb565_changes(width, height, &old, &create);
        let mut a = Vec::new();
        let mut b = Vec::new();
        gpu.frame_rgb565(width, height, &mut a);
        sw.frame_rgb565(width, height, &mut b);

        assert_eq!(
            pixel(&b, width, 3, 0),
            (31, 63, 31),
            "the software should have the first row white"
        );
        assert_eq!(
            pixel(&a, width, 3, 0),
            pixel(&b, width, 3, 0),
            "first row: graphics card {:?} vs software {:?}",
            pixel(&a, width, 3, 0),
            pixel(&b, width, 3, 0)
        );
        assert_eq!(
            pixel(&a, width, 3, 5),
            pixel(&b, width, 3, 5),
            "the middle row should have stayed black in both"
        );
    }

    /// An address no graphics card has, for the tests that open no context at all.
    const FAKE_ADDRESS: usize = usize::MAX;
    /// The address of a second graphics card that does not exist either.
    const OTHER_ADDRESS: usize = usize::MAX - 8;

    /// **A graphics card that died drops its GL names without deleting any of them.**
    ///
    /// A GL context does not exist in a unit test — which is why the decisions that depend on it
    /// live in [`Gpu`], which can be tested without any graphics card. What is checked here is
    /// what `Drop` asks before calling `delete_*` (GPU audit, finding 1): after the notice that the
    /// context is gone, **no** deletion happens — and after a new graphics card, it happens
    /// again.
    #[test]
    fn a_dead_gpu_leaks_its_names_and_a_new_one_deletes_them() {
        // Borrowed, live graphics card: `Drop` deletes, as it always has.
        let alive = Gpu::create(true, FAKE_ADDRESS);
        assert!(
            alive.deletes_on_drop(),
            "live graphics card: `Drop` deletes the names it created"
        );

        // The notice that the context died — it is the same call the core's `context_lost` and
        // `context_reset` make.
        gpu_destroyed(FAKE_ADDRESS);
        assert!(alive.is_dead(), "the state did not recognize the dead graphics card notice");
        assert!(
            !alive.deletes_on_drop(),
            "`Drop` would call delete_* with the function pointers of a dead context"
        );

        // **Recreated on demand:** a new graphics card, at another address, is valid again — the
        // old mark does not reach it.
        let create = Gpu::create(true, OTHER_ADDRESS);
        assert!(!create.is_dead());
        assert!(create.deletes_on_drop());

        // And so is the one born **at the same address** as the dead one — the allocator reuses
        // addresses: what clears the mark is `gpu_created`, which is what the core's `enable_gpu`
        // calls.
        gpu_created(FAKE_ADDRESS);
        let reborn = Gpu::create(true, FAKE_ADDRESS);
        assert!(!reborn.is_dead());
        assert!(reborn.deletes_on_drop());

        // An own context never dies before its owner: what would close it is this very state.
        gpu_destroyed(FAKE_ADDRESS);
        let own = Gpu::create(false, FAKE_ADDRESS);
        assert!(!own.is_dead());
        assert!(
            own.deletes_on_drop(),
            "own context: its `Drop` is what deletes the names"
        );

        // The mark is process-wide: leaving it set would change the test that ran next.
        gpu_created(FAKE_ADDRESS);
        gpu_created(OTHER_ADDRESS);
    }

    /// **The cache of the three objects dies when the context leaves our hands.**
    ///
    /// The window's `GlPainter` ends each paint by **unbinding** the program and VAO, and paints in
    /// the same context the rasterizer draws in: without invalidating, the next batch would draw
    /// with `egui`'s program — or with none, which is `GL_INVALID_OPERATION` and nothing on screen,
    /// with no error at all on our side (GPU audit, finding 2).
    #[test]
    fn a_context_leaving_our_hands_invalidates_bound_objects() {
        let gpu = Gpu::create(true, FAKE_ADDRESS);
        assert!(gpu.needs_bind(), "the first batch binds all three");
        gpu.mark_bound();
        assert!(
            !gpu.needs_bind(),
            "the next batch reuses what is already bound — that is the gain from commit e6a436e"
        );
        gpu.forget_bound();
        assert!(
            gpu.needs_bind(),
            "the context was handed back to the host: the next batch has to rebind"
        );
    }

    /// An offscreen graphics card for the tests that need a **borrowed** context.
    ///
    /// It is the window's and the frontend's path: the context belongs to someone else, which is
    /// why it can be taken back in the middle of drawing. The `GlContext` comes back too because
    /// it is what keeps the GL functions alive: dropping it before the state would crash the test,
    /// as it crashes the program.
    #[cfg(feature = "gpu")]
    fn borrowed_gpu() -> Option<(GlContext, std::sync::Arc<glow::Context>)> {
        match GlContext::create() {
            Ok(context) => {
                let gl = context.gl.clone();
                // The address may have belonged to a graphics card another test marked as dead:
                // the mark is process-wide, and whoever sets up the graphics card clears it —
                // here, and in the core's `enable_gpu`.
                gpu_created(gpu_address(&gl));
                Some((context, gl))
            }
            Err(reason) => {
                println!("no offscreen graphics card: {reason}");
                None
            }
        }
    }

    /// A graphics card state on top of the borrowed context, like the game's in the window.
    #[cfg(feature = "gpu")]
    fn borrowed_state(
        width: usize,
        height: usize,
        gl: &std::sync::Arc<glow::Context>,
    ) -> Option<GpuState> {
        match GpuState::create(width, height, Some(gl.clone())) {
            Ok(state) => Some(state),
            Err(reason) => {
                println!("no graphics card state: {reason}");
                None
            }
        }
    }

    /// A triangle of the scene, in clip space, in the given color.
    #[cfg(feature = "gpu")]
    fn triangle(r: &mut dyn Rasterizer, color: [f32; 4], corners: [(f32, f32); 3]) {
        let vertex = |(x, y): (f32, f32)| Vertex {
            position: [x, y, 0.0, 1.0],
            color,
            uv: DEFAULT_UV,
            uv1: DEFAULT_UV,
            normal: [0.0, 0.0, 1.0],
            fog: 1.0,
        };
        r.set_color(color);
        r.draw(gles::GL_TRIANGLES, &corners.map(vertex));
    }

    /// The first batch of the scene of the two tests below: a red triangle in the top corner.
    #[cfg(feature = "gpu")]
    fn first_batch(r: &mut dyn Rasterizer, (width, height): (usize, usize)) {
        r.set_viewport(0, 0, width as i32, height as i32);
        r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
        r.clear(gles::GL_COLOR_BUFFER_BIT);
        triangle(
            r,
            [1.0, 0.0, 0.0, 1.0],
            [(-1.0, 1.0), (-1.0, -1.0), (1.0, 1.0)],
        );
        // Closing the batch is what puts it on the graphics card — and that is where the context
        // goes back to the host.
        r.flush_draw();
    }

    /// The second batch: another triangle, in the other half of the frame.
    #[cfg(feature = "gpu")]
    fn second_batch(r: &mut dyn Rasterizer) {
        triangle(
            r,
            [0.0, 1.0, 0.0, 1.0],
            [(1.0, -1.0), (1.0, 1.0), (-1.0, -1.0)],
        );
        r.flush_draw();
    }

    /// **A second painter in the same context does not erase the game's drawing.**
    ///
    /// It is the missing half of finding 2: no test put `egui`'s `GlPainter` between two batches of
    /// the rasterizer, **in the same context** — which is the `graphics.gpu_rasterizer = true` path
    /// (the headless window hands its context to the session, and `src/ui/gpu.rs` paints in it).
    /// What `GlPainter` does at the end of each paint is copied here: `use_program(None)` and
    /// `bind_vertex_array(None)`. The frame on both sides has to come out **the same**.
    /// **What the window changes between two frames goes back to the engine.** `egui` paints in
    /// the same context and leaves the viewport at the window size, the scissor on and
    /// premultiplied alpha blending. The mirror thought the viewport and blending were the
    /// engine's and did not resend them: Ridge Racer came out in a corner of the window, with white
    /// boxes in place of transparency.
    #[cfg(feature = "gpu")]
    #[test]
    fn state_left_by_the_window_does_not_leak_into_the_next_frame() {
        use glow::HasContext as _;

        let (width, height) = (16, 16);
        let Some((context, gl)) = borrowed_gpu() else {
            return;
        };
        let (Some(mut reference), Some(mut with_window)) = (
            borrowed_state(width, height, &gl),
            borrowed_state(width, height, &gl),
        ) else {
            return;
        };
        let _ = &context;
        let measured = (width, height);

        // The reference finishes **before** the window paints: there is a single context, and
        // what the window changes would apply to both.
        first_batch(&mut reference, measured);
        second_batch(&mut reference);
        first_batch(&mut with_window, measured);

        // **`egui` paints here**, and this is how it leaves the context.
        unsafe {
            gl.viewport(0, 0, 4, 4);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(0, 0, 4, 4);
            gl.enable(glow::BLEND);
            gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE_MINUS_DST_ALPHA,
                glow::ONE,
            );
        }
        with_window.resume_context();
        second_batch(&mut with_window);

        assert_eq!(
            with_window.read_rect(0, 0, width, height),
            reference.read_rect(0, 0, width, height),
            "the second batch drew with the viewport, scissor or blending the window left"
        );
    }

    /// **The filter requested before the image applies to the texture the image creates.**
    ///
    /// The common order is to generate the name, bind, set the parameters and only then send the
    /// image. The graphics card only stored parameters for textures that already existed on it,
    /// and the texture was born in `upload_level` with `GL_LINEAR`: Treino Cerebral asks for
    /// `GL_NEAREST` for the buttons and boxes, built from pieces side by side, and bilinear pulled
    /// the atlas neighbor into the seam of each piece — the light lines every few pixels.
    #[test]
    fn a_filter_set_before_the_image_applies_to_the_texture() {
        let Some((mut gpu, _)) = pair(4, 4) else {
            return;
        };
        gpu.bind_texture(7);
        gpu.set_texture_parameter(gles::GL_TEXTURE_MAG_FILTER, gles::GL_NEAREST);
        gpu.set_texture_parameter(gles::GL_TEXTURE_MIN_FILTER, gles::GL_NEAREST);
        gpu.set_texture_parameter(gles::GL_TEXTURE_WRAP_S, gles::GL_CLAMP_TO_EDGE);
        gpu.upload_level(7, 0, 2, 2, vec![[255; 4]; 4]);

        let t = &gpu.textures[&7];
        assert_eq!(t.filter, gles::GL_NEAREST, "magnification");
        assert_eq!(t.min_filter, gles::GL_NEAREST, "minification");
        assert_eq!(t.wrap[0], gles::GL_CLAMP_TO_EDGE, "wrap in s");
    }

    /// **The screen shows the frame from the last swap, not the destination in the middle of the
    /// next one.** The game clears to green after swapping the red frame, and reading what is
    /// presented stays red until the next swap; the game's `glReadPixels`, which reads the back,
    /// already sees the green. It was Ultimate Chess 3D flickering between the scene and black.
    /// See [`GpuState::protect_front`].
    ///
    /// The mid-frame save state, which rewind takes every second, drains the drawing and is not a
    /// swap: with it taken as a swap, the chess flickered black once per second. With and without
    /// antialiasing, which resolved half the frame into `color`. See [`GpuState::swap`].
    #[test]
    fn the_screen_shows_the_swapped_frame_not_half_of_the_next() {
        let (width, height) = (8, 8);
        let red = 0xf800u16;
        let green = 0x07e0u16;
        for samples in [1, 4] {
            let Some((mut gpu, _)) = pair(width, height) else {
                return;
            };
            Rasterizer::set_antialias(&mut gpu, samples);
            let swap = |gpu: &mut GpuState| {
                gpu.flush_draw();
                Rasterizer::end_frame(gpu);
            };
            gpu.set_viewport(0, 0, width as i32, height as i32);
            gpu.set_clear_color([1.0, 0.0, 0.0, 1.0]);
            gpu.clear(gles::GL_COLOR_BUFFER_BIT);
            swap(&mut gpu);

            gpu.set_clear_color([0.0, 1.0, 0.0, 1.0]);
            gpu.clear(gles::GL_COLOR_BUFFER_BIT);
            // The rewind save state, in the middle of the frame.
            gpu.flush_draw();
            let mut frame = Vec::new();
            gpu.frame_rgb565_words(width, height, &mut frame);
            assert!(
                frame.iter().all(|&p| p == red),
                "the screen read the middle of the next frame, with {samples} sample(s)"
            );
            assert_eq!(gpu.read_rect(0, 0, 1, 1)[0], [0, 255, 0, 255], "the game reads the back");

            swap(&mut gpu);
            gpu.frame_rgb565_words(width, height, &mut frame);
            assert!(
                frame.iter().all(|&p| p == green),
                "the next swap did not reach the screen, with {samples} sample(s)"
            );
        }
    }

    /// **The scissor the game turns off goes away in GL too, with the borrowed context.**
    ///
    /// Peggle clips almost every draw with `glScissor` and turns the scissor off for the
    /// background. `hand_back_context` told the mirror it had left the scissor off, without
    /// turning it off: the background resent nothing and came out clipped by the last sprite's
    /// rectangle. On the graphics card, the menu and the level scenery did not show up, and the
    /// previous screens stayed underneath.
    #[cfg(feature = "gpu")]
    #[test]
    fn a_scissor_disabled_by_the_game_does_not_clip_the_next_batch() {
        let (width, height) = (16, 16);
        let Some((context, gl)) = borrowed_gpu() else {
            return;
        };
        let Some(mut r) = borrowed_state(width, height, &gl) else {
            return;
        };
        let _ = &context;
        first_batch(&mut r, (width, height));

        // A sprite clipped to a 4×4 corner, and the batch closes: the context goes back to the
        // host.
        r.set_capability(gles::GL_SCISSOR_TEST, true);
        r.set_scissor(0, 0, 4, 4);
        second_batch(&mut r);

        // The background, without scissor, covers the whole screen.
        r.set_capability(gles::GL_SCISSOR_TEST, false);
        let blue = [0.0, 0.0, 1.0, 1.0];
        triangle(&mut r, blue, [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0)]);
        triangle(&mut r, blue, [(1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]);
        r.flush_draw();

        let not_blue = r
            .read_rect(0, 0, width, height)
            .iter()
            .filter(|p| **p != [0, 0, 255, 255])
            .count();
        assert_eq!(not_blue, 0, "the background came out clipped by the previous sprite's scissor");
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn the_painter_on_the_same_context_does_not_erase_the_game_drawing() {
        use glow::HasContext as _;

        let (width, height) = (16, 16);
        let Some((context, gl)) = borrowed_gpu() else {
            return;
        };
        let (Some(mut first_only), Some(mut reference), Some(mut with_painter)) = (
            borrowed_state(width, height, &gl),
            borrowed_state(width, height, &gl),
            borrowed_state(width, height, &gl),
        ) else {
            return;
        };
        let _ = &context;
        let measured = (width, height);

        first_batch(&mut first_only, measured);
        first_batch(&mut reference, measured);
        first_batch(&mut with_painter, measured);

        // **`egui`'s `GlPainter` paints here**, in the same context, and this is how it ends.
        unsafe {
            gl.use_program(None);
            gl.bind_vertex_array(None);
        }

        // In the reference nobody touched the context; in the other, `egui` just unbound both.
        second_batch(&mut reference);
        second_batch(&mut with_painter);

        let only_one = first_only.read_rect(0, 0, width, height);
        let expected = reference.read_rect(0, 0, width, height);
        let obtained = with_painter.read_rect(0, 0, width, height);
        assert_ne!(
            only_one, expected,
            "the test scene does not discriminate: the second batch drew nothing even in the reference"
        );
        assert_eq!(
            obtained, expected,
            "the second batch drew with the program egui left: the cache of bound objects \
             survived someone else using the context"
        );
    }

    /// **At the end of the libretro frame, nothing of the engine stays bound.** RetroArch draws the
    /// FBO to the screen with the same context, and on GLES the engine's bound VAO broke that
    /// draw: black screen with audio on the handhelds. See [`GpuState::hand_back_to_frontend`].
    #[cfg(feature = "gpu")]
    #[test]
    fn end_of_frame_returns_the_context_with_nothing_of_ours_bound() {
        use glow::HasContext as _;

        let (width, height) = (16, 16);
        let Some((context, gl)) = borrowed_gpu() else {
            return;
        };
        let Some(mut state) = borrowed_state(width, height, &gl) else {
            return;
        };
        let _ = &context;
        // The "frontend" FBO, as `get_current_framebuffer` would hand it over.
        let fbo = unsafe {
            let texture = gl.create_texture().unwrap();
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            let fbo = gl.create_framebuffer().unwrap();
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            fbo
        };
        let enabled = || unsafe {
            [
                gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING),
                gl.get_parameter_i32(glow::CURRENT_PROGRAM),
                gl.get_parameter_i32(glow::ARRAY_BUFFER_BINDING),
            ]
        };

        state.draw_to_fbo(Some(fbo.0.get()));
        first_batch(&mut state, (width, height));
        assert_ne!(
            enabled(),
            [0, 0, 0],
            "the test does not discriminate: after the batch the engine should have its own bound"
        );
        state.hand_back_to_frontend();
        assert_eq!(enabled(), [0, 0, 0], "the engine's VAO, program and VBO stayed bound");

        // The next frame rebinds what it needs and draws.
        state.draw_to_fbo(Some(fbo.0.get()));
        second_batch(&mut state);
        assert_ne!(enabled(), [0, 0, 0], "the next frame did not rebind the engine's objects");
    }

    /// **A graphics card that died in the middle of the frame receives no new drawing.**
    ///
    /// The notice that the context is gone comes from outside the emulator and **does not wait**
    /// for `retro_run` to finish (GPU audit, findings 1 and 4): what is checked is that the
    /// rasterizer stops calling the driver as soon as it knows, instead of drawing through function
    /// pointers that no longer exist. The frame that comes out is what was already on the graphics
    /// card.
    #[cfg(feature = "gpu")]
    #[test]
    fn a_gpu_that_died_mid_frame_draws_no_more() {
        let (width, height) = (16, 16);
        let Some((context, gl)) = borrowed_gpu() else {
            return;
        };
        // The test's two states are born before the notice: creating one after it would be asking
        // a dead graphics card for GL. Both belong to the **same** context — the death mark belongs
        // to the graphics card, not to the state, which is what the frontend's notice means.
        let (Some(mut live), Some(mut with_dead)) = (
            borrowed_state(width, height, &gl),
            borrowed_state(width, height, &gl),
        ) else {
            return;
        };
        let _ = &context;
        let measured = (width, height);

        // **The scene discriminates:** with the graphics card alive, the second batch changes the
        // frame. It is this test's control, and it runs in the same context and at the same size
        // as the case that matters.
        first_batch(&mut live, measured);
        let before = live.read_rect(0, 0, width, height);
        second_batch(&mut live);
        let after = live.read_rect(0, 0, width, height);
        assert_ne!(
            before, after,
            "the test scene does not discriminate: with a live GPU the second batch has to show up"
        );

        // The notice, in the middle of the frame — the same one `context_lost` and `context_reset`
        // give.
        first_batch(&mut with_dead, measured);
        let before = with_dead.read_rect(0, 0, width, height);
        gpu_destroyed(gpu_address(&gl));
        second_batch(&mut with_dead);
        let after = with_dead.read_rect(0, 0, width, height);
        assert_eq!(
            after,
            before,
            "the dead graphics card kept drawing: the state was not invalidated on the notice"
        );

        // The mark is process-wide: clear it, or the next test would inherit a dead graphics card.
        gpu_created(gpu_address(&gl));
    }
}
