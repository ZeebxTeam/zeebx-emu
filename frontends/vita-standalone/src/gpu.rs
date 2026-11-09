//! The Vita's GPU. vitaGL opens the context; the game's fill is the core's shader.
//!
//! vitaGL translates GLSL ES 1.00 and presents itself as OpenGL ES 2.0. If initialization fails,
//! the caller carries on with the kernel framebuffer. If it works, vitaGL owns the screen until
//! the process ends — there is no `vglEnd` — and the menu is uploaded as a texture instead of
//! going through `sceDisplaySetFrameBuf`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};

use glow::HasContext;

use super::note;

const WIDTH: f32 = 960.0;
const HEIGHT: f32 = 544.0;

struct State {
    gl: Arc<glow::Context>,
    /// The GPU quad, when the frame comes back from memory. The list and the game
    /// on the CPU do not use it: for them, the blit is what shows up.
    program: glow::Program,
    vbo: glow::Buffer,
    /// Where the blit reads the image that came from the CPU.
    reading: glow::Framebuffer,
    cpu_texture: glow::Texture,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

unsafe extern "C" {
    fn vglGetProcAddress(name: *const i8) -> *const c_void;
    fn vglInitWithCustomSizes(
        legacy_pool_size: i32,
        width: i32,
        height: i32,
        ram_size: i32,
        cdram_size: i32,
        phycont_size: i32,
        cdlg_size: i32,
        msaa: i32,
    ) -> i32;
    fn vglSwapBuffers(has_common_dialog: u8);
    fn vglWaitVblankStart(enable: u8);
    fn vglUseTripleBuffering(usage: u8);
    fn vglSetShaderCachePath(path: *const i8);
    fn vglMemalign(alignment: u32, size: u32) -> *mut u8;
    fn vglBufferData(target: u32, data: *const u8);
    /// Only to find, in the already relocated binary, the circular vertex pool.
    fn vglAllocFromScratch(size: usize) -> *mut u8;
}

const VERTEX: &str = r#"
attribute vec2 pos;
attribute vec2 uv;
varying vec2 vuv;
void main() {
    vuv = uv;
    gl_Position = vec4(pos, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"
precision mediump float;
varying vec2 vuv;
uniform sampler2D imagem;
void main() {
    gl_FragColor = texture2D(imagem, vuv);
}
"#;

/// What could be done with the GPU.
pub enum Opening {
    /// Context and screen shader ready. vitaGL owns the screen.
    Ready,
    /// vitaGL refused the init. The kernel framebuffer still works.
    NoGpu,
    /// The init went through and vitaGL took the screen, but the presentation shader did not
    /// link. Going back to the kernel framebuffer would fight with it.
    TookScreen(String),
}

/// Brings up vitaGL.
pub fn open() -> Opening {
    let _ = std::fs::create_dir_all("ux0:data/zeebx/shader_cache");
    // The cache has to exist before the init: the first compilation goes through SceShaccCg.
    unsafe {
        vglSetShaderCachePath(c"ux0:data/zeebx/shader_cache".as_ptr());
        // The byte starts at 3. The third buffer is one extra frame between the
        // input and the image: the character has already moved, and the screen still
        // shows the previous frame. The same byte says into how many pools the init
        // splits the vertex ring, so it has to come before.
        vglUseTripleBuffering(0);
        // `vglInitExtended` treats the fourth argument as what is left over, not as the
        // pool size. With a 16 MiB threshold it took the rest of the user partition: in
        // coredump `0x000136399b` the `ram_mempool` had 137 MiB, the CDRAM 96 MiB and
        // the PHYCONT 26 MiB, on top of the 192 MiB heap. The following `glClear` died
        // in `sceGxmReserveVertexDefaultUniformBuffer` with no physical page
        // (`ScePhyMemPartGame`). Here the size is the block. 16 MiB of RAM leaves the
        // rest of the partition to the GXM and to the game's large blocks; 64 MiB of
        // CDRAM holds the 32 MiB vertex ring and the textures; 8 MiB of PHYCONT holds
        // the render targets. The dialog keeps the same `0x8C6000` that
        // `vglInitExtended` used to set.
        let code = vglInitWithCustomSizes(
            0x10_0000, 960, 544, 0x100_0000, 0x400_0000, 0x80_0000, 0x8C_6000, 0,
        );
        // The header says `GLboolean`, but the binary returns 0 when 960×544 fits the
        // screen and 1 when it needs adjusting. Both have already brought up the GXM.
        // Reading 0 as failure asked again for the PHYCONT and the CDRAM, which the init
        // had just taken: `0x80024302` and `0x80024309`, with no free physical page.
        note(&format!("vglInit {code}"));
        if code < 0 {
            note("vitaGL did not come up; the frame stays on the CPU");
            return Opening::NoGpu;
        }
        super::note_free_memory();
        vglWaitVblankStart(1);
        // The init leaves a thread drawing the vitaGL logo. It only lets go of the screen on
        // the first swap. Compiling the shader before that leaves the logo in front the whole
        // time, and the two threads fight over the same context.
        note("vitaGL up; removing the logo");
        vglSwapBuffers(0);
    }
    let gl = Arc::new(unsafe {
        glow::Context::from_loader_function(|name| {
            let Ok(c) = std::ffi::CString::new(name) else {
                return std::ptr::null();
            };
            vglGetProcAddress(c.as_ptr())
        })
    });
    unsafe {
        note(&format!(
            "GL {} / {} / {} / {}",
            gl.get_parameter_string(glow::VENDOR),
            gl.get_parameter_string(glow::RENDERER),
            gl.get_parameter_string(glow::VERSION),
            gl.get_parameter_string(glow::SHADING_LANGUAGE_VERSION),
        ));
    }
    note("compiling the screen shader");
    let program = match unsafe { present_program(&gl) } {
        Ok(program) => program,
        Err(error) => {
            // vitaGL has already taken the screen. Without this shader there is no way to show
            // the menu or the game, and the kernel framebuffer does not get the screen back.
            return Opening::TookScreen(format!("the screen shader did not link ({error})"));
        }
    };
    let (vbo, reading, cpu_texture) = unsafe {
        let vbo = match gl.create_buffer() {
            Ok(vbo) => vbo,
            Err(error) => {
                return Opening::TookScreen(format!("no screen buffer ({error})"));
            }
        };
        let reading = match gl.create_framebuffer() {
            Ok(reading) => reading,
            Err(error) => {
                return Opening::TookScreen(format!("no screen framebuffer ({error})"));
            }
        };
        let texture = match gl.create_texture() {
            Ok(texture) => texture,
            Err(error) => {
                return Opening::TookScreen(format!("no screen texture ({error})"));
            }
        };
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        // 960×544 is not a power of two. On ES 2 such a texture only samples with clamp and no
        // mips.
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
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        (vbo, reading, texture)
    };
    // A `glClear` on our own framebuffer has to come back when read. On this Vita3K
    // it does not: the next swap only republishes the list. The game, then, goes out
    // through the CPU, which is the frame the list's blit is able to show.
    let frame_kept = frame_reads_back(&gl);
    if frame_kept {
        note("the GPU framebuffer returned the frame; the game stays on the GPU");
    } else {
        note("the GPU framebuffer does not return the frame; the game stays on the CPU");
    }
    KEEPS_FRAME.store(frame_kept, Ordering::Relaxed);
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(State {
        gl,
        program,
        vbo,
        reading,
        cpu_texture,
    });
    Opening::Ready
}

static KEEPS_FRAME: AtomicBool = AtomicBool::new(false);

/// The GPU returned the green that was requested in a 2×2 framebuffer.
fn frame_reads_back(gl: &glow::Context) -> bool {
    unsafe {
        let Ok(texture) = gl.create_texture() else {
            return false;
        };
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            2,
            2,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        let Ok(fbo) = gl.create_framebuffer() else {
            gl.delete_texture(texture);
            return false;
        };
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        gl.disable(glow::SCISSOR_TEST);
        gl.color_mask(true, true, true, true);
        gl.clear_color(0.0, 1.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.finish();
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
        gl.delete_framebuffer(fbo);
        gl.delete_texture(texture);
        loaded[1] > 200 && loaded[3] > 200
    }
}

/// The game can draw on the GPU. False when the frame does not come back from memory.
pub fn keeps_frame() -> bool {
    KEEPS_FRAME.load(Ordering::Relaxed)
}

pub fn is_active() -> bool {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

pub fn context() -> Option<Arc<glow::Context>> {
    STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|state| Arc::clone(&state.gl))
}

/// The menu and the software frame. The word is `0xAABBGGRR`, which in memory comes out R, G, B, A.
pub fn show_rgba(pixels: &[u32], w: i32, h: i32) {
    let guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = guard.as_ref() else {
        return;
    };
    unsafe {
        let gl = &state.gl;
        gl.bind_texture(glow::TEXTURE_2D, Some(state.cpu_texture));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        let bytes = std::slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), pixels.len() * 4);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            w,
            h,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(bytes)),
        );
        // The clear comes first: it is what waits for the logo to go away. The attachment
        // marks the scene dirty and the blit opens another, with only the image. The list
        // fills the whole screen, so the black of the previous scene does not show.
        prepare_screen(gl);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(state.reading));
        gl.framebuffer_texture_2d(
            glow::READ_FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(state.cpu_texture),
            0,
        );
        copy(gl, w, h, 0, 0, w, h);
        vglSwapBuffers(0);
    }
}

/// Rewinds vitaGL's vertex pool to the start when the headroom runs out.
///
/// Every `glDrawArrays` copies the vertices into that pool, and the GXM still reads the
/// copy after the frame swap. The cursor only moves forward: past the end, the draw
/// requests a block and queues it for the collector, which stops returning what it
/// queued. Rewinding the cursor to the start on every frame stomps on the copy the
/// on-screen frame still uses — the sprite comes out with another one's uv, a square
/// from another sheet. The headroom holds several frames. We only rewind when it runs
/// out, and `glFinish` waits for the GXM to release the pool first.
pub fn prepare_frame() {
    let Some(layout) = pool_struct() else {
        return;
    };
    // A frame of sprites stays below this. With less headroom, the next
    // draw overflows the pool.
    const MARGIN: u32 = 6 << 20;
    let (base, cursor, limit) = unsafe {
        let fields = layout as *const u32;
        (
            fields.add(0x58 / 4).read(),
            fields.add(0x70 / 4).read(),
            fields.add(0x88 / 4).read(),
        )
    };
    if base != 0 && cursor >= base && limit.saturating_sub(cursor) > MARGIN {
        return;
    }
    if let Some(gl) = context() {
        unsafe { gl.finish() };
    }
    if base != 0 {
        unsafe {
            let fields = layout as *mut u32;
            fields.add(0x70 / 4).write(base);
        }
    }
}

static POOL_WARNED: AtomicBool = AtomicBool::new(false);
static PRESENT_BLOCK: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static PRESENT_BLOCK_BOUND: AtomicBool = AtomicBool::new(false);

/// `vglAllocFromScratch` loads the pool's address with `movw`/`movt`. In the process
/// those immediates are already relocated; the value in the ELF is no good.
fn pool_struct() -> Option<*mut u8> {
    static CACHE: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
    let cached = CACHE.load(Ordering::Relaxed);
    if !cached.is_null() {
        return Some(cached);
    }
    let entry = (vglAllocFromScratch as *const () as usize) & !1;
    let code = unsafe { std::slice::from_raw_parts(entry as *const u16, 16) };
    let mut down = None;
    let mut high = None;
    let mut i = 0;
    while i + 1 < code.len() {
        let hw1 = code[i];
        let hw2 = code[i + 1];
        let opcode = hw1 & 0xfbf0;
        let movt = match opcode {
            0xf240 => Some(false),
            0xf2c0 => Some(true),
            _ => None,
        };
        if let Some(movt) = movt {
            let rd = (hw2 >> 8) & 0xf;
            if rd == 3 {
                let imm = ((u32::from(hw1 & 0xf)) << 12)
                    | ((u32::from((hw1 >> 10) & 1)) << 11)
                    | ((u32::from((hw2 >> 12) & 7)) << 8)
                    | u32::from(hw2 & 0xff);
                if movt {
                    high.get_or_insert(imm);
                } else {
                    down.get_or_insert(imm);
                }
            }
            i += 2;
            continue;
        }
        // 32-bit Thumb starts with 11101, 1111 or 11110. The rest is 16 bits.
        let is_32bit = hw1 >= 0xe800;
        i += if is_32bit { 2 } else { 1 };
    }
    let (Some(down), Some(high)) = (down, high) else {
        if !POOL_WARNED.swap(true, Ordering::Relaxed) {
            note("could not find vitaGL's vertex pool");
        }
        return None;
    };
    let address = ((high << 16) | down) as *mut u8;
    // Base and limit of pool 0. The init writes both; the headroom is 32 MiB
    // split across the screen buffers. With two, each pool gets 16 MiB.
    let (base, limit) = unsafe {
        let fields = address as *const u32;
        (fields.add(0x58 / 4).read(), fields.add(0x88 / 4).read())
    };
    let slack = limit.wrapping_sub(base);
    if base == 0 || !(1 << 20..17 << 20).contains(&slack) {
        if !POOL_WARNED.swap(true, Ordering::Relaxed) {
            note("vitaGL's vertex pool is not where the scratch points");
        }
        return None;
    }
    CACHE.store(address, Ordering::Relaxed);
    if !POOL_WARNED.swap(true, Ordering::Relaxed) {
        note("vitaGL's vertex pool rewinds when the headroom runs out");
    }
    Some(address)
}

/// The game frame, already on the GPU. `x` and `y` are the top-left corner, in screen pixels.
pub fn show_texture(id: u32, x: i32, y: i32, w: i32, h: i32) {
    let guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = guard.as_ref() else {
        return;
    };
    unsafe {
        paint(state, id, x, y, w, h);
        vglSwapBuffers(0);
    }
}

unsafe fn present_program(gl: &glow::Context) -> Result<glow::Program, String> {
    let mut last = String::from("screen shader");
    for header in ["#version 100\n", "\n"] {
        match unsafe { set_enabled(gl, header) } {
            Ok(program) => return Ok(program),
            Err(error) => last = error,
        }
    }
    Err(last)
}

unsafe fn set_enabled(gl: &glow::Context, header: &str) -> Result<glow::Program, String> {
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
        gl.bind_attrib_location(program, 0, "pos");
        gl.bind_attrib_location(program, 1, "uv");
        gl.link_program(program);
        // vitaGL's `vglGetProcAddress` has no `glDetachShader`. glow aborts
        // when the pointer comes back null ("called glDetachShader but it was not loaded",
        // in the coredump). Deleting the shader while still attached is what the spec
        // provides for: it is freed when the program dies.
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

/// Draws the texture. `v` zero is the top: the game's fill flips Y, and the menu's first row is
/// also the visual top.
unsafe fn paint(state: &State, texture: u32, x: i32, y: i32, w: i32, h: i32) {
    let (x0, y1) = ndc(x as f32, y as f32);
    let (x1, y0) = ndc((x + w) as f32, (y + h) as f32);
    // Two triangles. The top of the screen (larger NDC y) samples v = 0.
    let vertices: [f32; 24] = [
        x0, y1, 0.0, 0.0, x1, y1, 1.0, 0.0, x0, y0, 0.0, 1.0, x1, y1, 1.0, 0.0, x1, y0, 1.0, 1.0,
        x0, y0, 0.0, 1.0,
    ];
    let gl = &state.gl;
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.viewport(0, 0, WIDTH as i32, HEIGHT as i32);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::BLEND);
        gl.disable(glow::CULL_FACE);
        gl.disable(glow::STENCIL_TEST);
        gl.color_mask(true, true, true, true);
        // vitaGL has two buffers. The game quad only covers 640×480 in the center;
        // the rest kept the game list, and it showed behind the scene. The clear is
        // of this buffer, on every swap — clearing only once leaves the other one on
        // the alternation.
        gl.clear_color(0.0, 0.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.bind_vertex_array(None);
        gl.use_program(Some(state.program));
        let image = gl.get_uniform_location(state.program, "imagem");
        gl.uniform_1_i32(image.as_ref(), 0);
        gl.active_texture(glow::TEXTURE0);
        let native = glow::NativeTexture(std::num::NonZeroU32::new(texture).unwrap());
        gl.bind_texture(glow::TEXTURE_2D, Some(native));
        // The game's color is 640×480, also not a power of two.
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
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(state.vbo));
        // `glBufferData` requests a new block on every swap. The screen quad fits in a
        // single block, overwritten: the draw copies the six vertices before returning.
        let bytes = bytemuck_f32(&vertices);
        let mut block = PRESENT_BLOCK.load(Ordering::Relaxed);
        if block.is_null() {
            block = vglMemalign(16, bytes.len() as u32);
            if !block.is_null() {
                PRESENT_BLOCK.store(block, Ordering::Relaxed);
            }
        }
        if block.is_null() {
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
        } else {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), block, bytes.len());
            if !PRESENT_BLOCK_BOUND.swap(true, Ordering::Relaxed) {
                vglBufferData(glow::ARRAY_BUFFER, block);
            }
        }
        let step = 4 * 4;
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, step, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, step, 8);
        for index in 2..5 {
            gl.disable_vertex_attrib_array(index);
        }
        gl.draw_arrays(glow::TRIANGLES, 0, 6);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.use_program(None);
    }
}

fn ndc(px: f32, py: f32) -> (f32, f32) {
    (px / WIDTH * 2.0 - 1.0, 1.0 - py / HEIGHT * 2.0)
}

fn bytemuck_f32(vertices: &[f32; 24]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(vertices.as_ptr().cast::<u8>(), vertices.len() * 4) }
}

/// Opens the screen scene and paints it black. The bound read target stays in effect.
unsafe fn prepare_screen(gl: &glow::Context) {
    unsafe {
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::BLEND);
        gl.disable(glow::CULL_FACE);
        gl.disable(glow::STENCIL_TEST);
        gl.color_mask(true, true, true, true);
        // Two buffers. The game covers 640×480; clearing only once leaves the list
        // in the other one, and it comes back on the alternation.
        gl.clear_color(0.0, 0.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.viewport(0, 0, WIDTH as i32, HEIGHT as i32);
    }
}

/// Copies what is bound for reading. `v` zero is the top of the image.
///
/// The blit puts `v` 1 at the top of the screen. The source Y is swapped so the
/// visual top, which is at `v` 0, stays at the top. The draw target goes back to
/// what it was before the blit; a caller that will draw to the screen again needs
/// to bind it again.
unsafe fn blit(gl: &glow::Context, fw: i32, fh: i32, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        gl.blit_framebuffer(
            0,
            fh,
            fw,
            0,
            x,
            y,
            x + w,
            y + h,
            glow::COLOR_BUFFER_BIT,
            glow::NEAREST,
        );
    }
}

unsafe fn copy(gl: &glow::Context, fw: i32, fh: i32, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        blit(gl, fw, fh, x, y, w, h);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
}
