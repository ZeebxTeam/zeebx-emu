//! Zeebx's Libretro core.
//!
//! The frontend owns video, audio and input; this crate only translates them for the `zeebx`
//! engine. No host window, sound card or controller is opened by this crate: the engine runs
//! through the virtual step of [`zeebx::session::Session::run_frame`].
//!
//! The core handles `.mod`/`.zip`/`.7z` content, RGB565 video, PCM16 audio at 44.1 kHz, RetroPad on
//! both ports, a USB keyboard and a Boomerang fed by the frontend's sensor. OpenGL/GLES 3 hardware
//! rendering, versioned save states and switching from the Z-Wheel are implemented too; the
//! guest's mouse remains outside the ABI.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use glow::HasContext;
use zeebx::audio::Mixer;
use zeebx::config::ZWheel;
use zeebx::input::Pad;
use zeebx::input::bindings::Device;
use zeebx::session::{Session, StartError, Step};
use zeebx::storage::StoragePaths;
use zeebx::speed::{SkipCounter, Frameskip, FpsLimit, Pace};

/// ABI version this core implements.
const API_VERSION: u32 = 1;

/// The core's nominal rate: 44.1 kHz stereo.
///
/// The amount delivered per call is **not** fixed: it comes from the virtual time that passed since
/// the previous call. See `retro_run`.
const SAMPLE_RATE: u32 = 44_100;

/// Pixel format negotiated with the frontend, in `retro_pixel_format`.
const PIXEL_FORMAT_RGB565: u32 = 2;

/// Environment commands used.
/// Asks the frontend to unload the content and go back to its menu.
const ENV_SHUTDOWN: u32 = 7;

/// Shows a notice to the player, on the frontend's own screen.
const ENV_SET_MESSAGE: u32 = 6;

/// Asks whether the frontend delivers all buttons in a single word.
const ENV_GET_INPUT_BITMASKS: u32 = 51;

/// Special RetroPad identifier that returns the buttons as a bit mask.
const ID_JOYPAD_MASK: u32 = 256;

/// Receives the frontend's keys. This is how the Z-Wheel navigates: it asks for `AVK_0` and
/// `AVK_CLR`, which do not exist on the RetroPad.
const ENV_SET_KEYBOARD_CALLBACK: u32 = 12;

/// The ABI's key codes (`enum retro_key`), which follow SDL 1.2's.
const RETROK_BACKSPACE: u32 = 8;
const RETROK_RETURN: u32 = 13;
const RETROK_ESCAPE: u32 = 27;
const RETROK_ASTERISK: u32 = 42;
const RETROK_HASH: u32 = 35;
const RETROK_0: u32 = 48;
const RETROK_UP: u32 = 273;
const RETROK_DOWN: u32 = 274;
const RETROK_RIGHT: u32 = 275;
const RETROK_LEFT: u32 = 276;

/// How many games the core found next to the content.
///
/// **It is the precondition for the Z-Wheel to list anything.** The wheel enumerates the installed
/// applets and asks for each one's `.mod` by ClassID; what knows where they are is the survey made
/// when opening the content. An empty wheel here is an empty wheel on screen, and without this
/// count the symptom "the Z-Wheel opens with no games at all" would have nowhere to be measured
/// without a window. **A test instrument, and only that.**
#[cfg(test)]
static GAMES_SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The last class the shell asked to open.
///
/// **A test instrument, and only that.** The core's log goes out through the frontend's callback,
/// which is **variadic** (an `extern "C" fn(...)` cannot be written in stable Rust), so the test
/// observes here what the interface would show as text. Outside `cfg(test)` it does not exist.
#[cfg(test)]
static LAST_OPENED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The class of the applet running now, on each frame.
///
/// **A test instrument, and only that**, for the same reason as [`LAST_OPENED`]. It is what lets
/// the Z-Wheel cycle test tell *who* is running (the wheel, or the game it opened) without a window
/// and without looking at pixels.
#[cfg(test)]
static CURRENT_CLASS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The session's virtual clock, in milliseconds, on each frame.
///
/// **A test instrument, and only that.** It tells whether the wheel is at the same point of the
/// timeline on both paths (the scan's and the core's), and it is the first thing to check when the
/// same script gives different outcomes: the key may be right and the instant, not. Measured with
/// it: each `retro_run` advances ~26 ms of virtual clock, not the 16 ms of a frame at 60 Hz,
/// because the core's pass ends when the machine **presents** a frame.
#[cfg(test)]
static CLOCK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The instructions the machine has executed so far, on each frame.
///
/// **A test instrument, and only that.** It is [`CLOCK`]'s partner for the question that separates
/// "the wheel is slow" from "the wheel stopped": the guest's clock advances by vsync and by
/// instruction, so instructions that rise with the clock stopped are the guest spinning without
/// presenting a frame. That is what showed that the core's path delivers 1,214 frames per virtual
/// millisecond after confirming, against ~240 frames per second in the normal regime.
#[cfg(test)]
static INSTRUCTIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The frontend offers a GPU context for the core to draw in.
const ENV_SET_HW_RENDER: u32 = 14;

/// Hardware profile the core asks for.
///
/// Desktop uses `RETRO_HW_CONTEXT_OPENGL_CORE` (3), measured with RetroArch/Mesa. The Linux
/// AArch64 handhelds (R36S/R35S/RGB20S with ArkOS/AeolusUX/dArkOS/dArkOSen and RG40XX-H with muOS)
/// expose OpenGL ES in RetroArch, not an OpenGL Core 3.3 context. On them we ask for GLES 3.0
/// (`RETRO_HW_CONTEXT_OPENGLES3`, 4): VAO, FBO blit, MSAA and `#version 300 es` are already ES 3.0.
/// Asking for 3.2 needlessly refused Panfrost drivers that offer 3.1 and sent all the drawing back
/// to the processor; we never depend on X11, Wayland or EGL.
///
/// On desktop the value 1 (`RETRO_HW_CONTEXT_OPENGL`, compatibility) does not work: on
/// RetroArch/EGL it delivered a different profile than the engine expected and failed with
/// `GL: Invalid enum`.
#[cfg(target_os = "emscripten")]
const HW_CONTEXT: u32 = 4; // RETRO_HW_CONTEXT_OPENGLES3 — WebGL2
#[cfg(target_os = "emscripten")]
const HW_VERSION_MAJOR: u32 = 3;
#[cfg(target_os = "emscripten")]
const HW_VERSION_MINOR: u32 = 0;

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const HW_CONTEXT: u32 = 4; // RETRO_HW_CONTEXT_OPENGLES3 — GLES 3.0
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const HW_VERSION_MAJOR: u32 = 3;
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const HW_VERSION_MINOR: u32 = 0;

#[cfg(not(any(
    target_os = "emscripten",
    all(target_os = "linux", target_arch = "aarch64")
)))]
const HW_CONTEXT: u32 = 3; // RETRO_HW_CONTEXT_OPENGL_CORE
#[cfg(not(any(
    target_os = "emscripten",
    all(target_os = "linux", target_arch = "aarch64")
)))]
const HW_VERSION_MAJOR: u32 = 3;
#[cfg(not(any(
    target_os = "emscripten",
    all(target_os = "linux", target_arch = "aarch64")
)))]
const HW_VERSION_MINOR: u32 = 3;

/// The value `retro_video_refresh` receives when the frame went out in the frontend's framebuffer.
///
/// It is `libretro`'s sentinel: passing pixels along with it would be a lie, and RetroArch presents
/// the framebuffer it provided itself.
const HW_FRAME_BUFFER_VALID: usize = usize::MAX;


/// The start of `retro_hw_render_callback`, in the order of the vendored `libretro.h`.
///
/// The fields are the ones the core needs to read and fill in: the context type (the core chooses),
/// the two pointers the **frontend** fills in later (the current framebuffer and the GL function
/// resolver) and `context_reset`, which is how the frontend announces the context is usable.
#[repr(C)]
#[derive(Clone, Copy)]
struct RetroHwRenderCallback {
    context_type: u32,
    context_reset: Option<unsafe extern "C" fn()>,
    get_current_framebuffer: Option<unsafe extern "C" fn() -> u32>,
    get_proc_address: Option<unsafe extern "C" fn(*const c_char) -> *const c_void>,
    depth: bool,
    stencil: bool,
    bottom_left_origin: bool,
    version_major: u32,
    version_minor: u32,
    /// Whether the frontend wants the core to keep GL resources across contexts.
    ///
    /// It is declared so the struct has the **size and offsets** of `libretro.h`: one field less
    /// here and `context_destroy` below would be read from the wrong place. We use no cache, and
    /// answering `false` (zero) is the right answer.
    cache_context: bool,
    /// Called by the frontend when the context **stops being valid**, when switching video driver,
    /// for example. Without it, the `glow::Context` we keep would go on pointing at functions that
    /// no longer exist, and the first draw after that breaks.
    context_destroy: Option<unsafe extern "C" fn()>,
    /// The rest of the struct does not concern us, but the size has to match the frontend's.
    _rest: [usize; 4],
}

/// What the frontend answered to the hardware render request.
static GPU_OFFER: Mutex<Option<RetroHwRenderCallback>> = Mutex::new(None);

/// Whether the frontend has already announced that the context is usable.
///
/// **The order matters:** `retro_load_game` asks for the context, and the frontend calls
/// `context_reset` **afterwards**; only then do the GL functions exist. That is why the GPU comes
/// in on the first `retro_run`, and not at load time.
static CONTEXT_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The GL context built from what the frontend delivered, once it delivered.
///
/// **This lock guards the pointer, not the use of the context.** It serializes whoever reads and
/// whoever swaps the `Arc` (`gpu()`, `enable_gpu` and the frontend's two callbacks), not the `gl*`
/// calls the emulator runs inside `retro_run`: the context itself is used **without** any lock, on
/// two threads (see the race note in [`context_lost`]). That is what the ABI allows: it says what
/// to do with the context (`libretro.h`: "Any GL state is lost, and must not be deinitialized
/// explicitly"; "If context_reset is called without any notification (context_destroy), the OpenGL
/// context was lost and resources should just be recreated without any attempt to free old
/// resources"), and it does not say which thread the callbacks come from, nor that they will not
/// arrive in the middle of a `retro_run`.
static GPU: std::sync::Mutex<Option<std::sync::Arc<glow::Context>>> =
    std::sync::Mutex::new(None);

/// The GPU context, once we are already drawing in it.
fn gpu() -> Option<std::sync::Arc<glow::Context>> {
    // **A poisoned lock must not turn into "there is no GPU".** A panic while it was held (there
    // is a `catch_unwind` in [`enable_gpu`], and what it protects touches GL) would leave the
    // emulator concluding there is no GPU and silently falling back to the processor, with
    // `gpu_enabled` true. The value left stored is what matters, and it is still readable.
    GPU
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Asks the frontend for the GPU context, once, and keeps the answer.
///
/// In `libretro`, the one who **offers** is the core: it fills in the struct and calls the
/// environment. The frontend returns `true` if it accepts, and after that it creates the context
/// and calls our `context_reset`.
fn request_gpu_context() {
    if GPU_OFFER.lock().is_ok_and(|g| g.is_some()) {
        return;
    }
    let mut offer = RetroHwRenderCallback {
        context_type: HW_CONTEXT,
        context_reset: Some(context_ready),
        get_current_framebuffer: None,
        get_proc_address: None,
        // Depth and stencil are requirements of the console, not decoration: the Z-Wheel's stage
        // marks the floor in the stencil to draw the reflection.
        depth: true,
        stencil: true,
        bottom_left_origin: false,
        version_major: HW_VERSION_MAJOR,
        version_minor: HW_VERSION_MINOR,
        // We do not keep GL resources across contexts, and `libretro` only offers the option.
        cache_context: false,
        context_destroy: Some(context_lost),
        _rest: [0; 4],
    };
    let target = &mut offer as *mut RetroHwRenderCallback as *mut c_void;
    if unsafe { environ(ENV_SET_HW_RENDER, target) } {
        log(&format!(
            "Zeebx: the frontend accepted hardware rendering (OpenGL {}.{}); drawing now happens on the GPU",
            offer.version_major, offer.version_minor
        ));
        if let Ok(mut g) = GPU_OFFER.lock() { *g = Some(offer); }
    } else {
        warning("Zeebx: the frontend does not offer hardware rendering; drawing stays on the processor");
    }
}

/// Builds the GL context from the frontend's resolver and **recreates the session** on the GPU.
///
/// **Once, on the first frame** (and not at load time), because only after `context_reset` do the
/// functions exist. Recreating the session here costs a restart nobody sees: no frame has been
/// delivered yet.
///
/// **Any failure falls back to software and says why.** A frontend that accepts the request and
/// does not deliver (no `get_proc_address`, no context) must not leave the emulator without an
/// image: the software path is the measured one and the one that already worked.
fn enable_gpu(state: &mut Core) {
    if state.gpu_enabled || !CONTEXT_READY.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    // Tries **once**: a context that did not come will not come on the next frame, and insisting
    // on every frame would use up the whole log.
    state.gpu_enabled = true;
    let Some(offer) = GPU_OFFER.lock().ok().and_then(|g| *g) else {
        return;
    };
    let Some(get_address) = offer.get_proc_address else {
        warning("Zeebx: the frontend accepted hardware rendering but does not offer get_proc_address; continuing on the processor");
        return;
    };
    let context = std::sync::Arc::new(unsafe {
        glow::Context::from_loader_function(|name| {
            let Ok(name) = CString::new(name) else {
                return std::ptr::null();
            };
            get_address(name.as_ptr())
        })
    });
    // **The current GPU may have landed at the address of one that died.** The death mark is by
    // address, and addresses belong to the allocator: without this cleanup the new GPU would be
    // born marked as dead and would draw nothing, silently. See [`zeebx::video::gpu::gpu_created`].
    zeebx::video::gpu::gpu_created(zeebx::video::gpu::gpu_address(&context));
    // The version requested through the callback does not prove the version the driver actually
    // delivered. Logging the four values avoids confusing the RK3326's libMali with the H700's
    // Mesa/Panfrost path.
    let (vendor, renderer, version, shading) = unsafe {
        (
            context.get_parameter_string(glow::VENDOR),
            context.get_parameter_string(glow::RENDERER),
            context.get_parameter_string(glow::VERSION),
            context.get_parameter_string(glow::SHADING_LANGUAGE_VERSION),
        )
    };
    log(&format!(
        "Zeebx: GL real vendor={vendor}; renderer={renderer}; version={version}; GLSL={shading}"
    ));
    // The GPU goes into the global **before** the switch: it is what `switch_to` checks to decide
    // whether the session is born with the GPU rasterizer. That way the following switches (the
    // Z-Wheel opening a game, the game going back to it) are born on the GPU as well.
    if let Ok(mut guard) = GPU.lock() {
        *guard = Some(context);
    }
    let before = state.path.clone();
    // **A `panic` here would take RetroArch down.** There is no safe boundary for a Rust failure to
    // cross and return to the frontend's C: the whole process goes down, and the user loses the
    // emulator because of a drawing optimization. `catch_unwind` turns that into the same path as
    // the ordinary failure: a notice in the log, a software session, the game running.
    //
    // `AssertUnwindSafe` is what the situation calls for: if the middle of building the context was
    // left inconsistent, what comes after does **not** continue from there: the session is
    // recreated from scratch, and the GPU global is cleared.
    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        switch_to(state, &before, false)
    }));
    let outcome = match attempt {
        Ok(result) => result,
        Err(_) => Err(StartError::NotLoadable(
            "building the GPU context panicked".to_string(),
        )),
    };
    match outcome {
        Ok(()) => log("Zeebx: drawing on the GPU"),
        Err(error) => {
            warning(&format!(
                "Zeebx: hardware rendering failed ({error}); continuing on the processor"
            ));
            // The software session was lost in the attempt. Recreate it without the GPU so as not
            // to be left with no image at all, and the GPU global is cleared, so the following
            // switches are born on the processor.
            state.gpu_enabled = false;
            if let Ok(mut guard) = GPU.lock() {
                *guard = None;
            }
            if let Err(also) = switch_to(state, &before, false) {
                warning(&format!("Zeebx: could not reopen even on the processor: {also}"));
            }
        }
    }
}

/// The `context_reset` we fill in: the frontend calls it when the context is usable.
///
/// **And also when it was rebuilt.** The ABI is explicit: "When context_reset is called, OpenGL
/// resources in the libretro implementation are guaranteed to be invalid" (`libretro.h`), and a
/// `context_reset` can arrive **without** `context_destroy` (when the context was lost from
/// outside, "the OpenGL context was lost and resources should just be recreated without any attempt
/// to free old resources"). In other words: this callback says two things at once: the new context
/// is up **and** what the live session keeps from the previous GPU is garbage.
///
/// That is why it does the same as [`context_lost`] with the old context, and more: it marks that
/// the session needs to be born again. It is the next `retro_run`'s `enable_gpu` that brings it
/// back: the GPU is rebuilt **on demand**, with the resolver the frontend left in [`GPU_OFFER`],
/// which is still valid.
unsafe extern "C" fn context_ready() {
    if let Ok(mut guard) = GPU.lock() {
        // The `glow::Context` we keep is the one for the context that was just rebuilt: the
        // functions it resolved point at the **old** context. Whoever draws with it needs to know
        // that before any `gl*` call; see [`zeebx::video::gpu::gpu_destroyed`].
        if let Some(gl_context) = guard.as_ref() {
            zeebx::video::gpu::gpu_destroyed(zeebx::video::gpu::gpu_address(gl_context));
        }
        *guard = None;
    }
    CONTEXT_READY.store(true, std::sync::atomic::Ordering::Relaxed);
    GPU_LOST.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The frontend announces that the context stopped being valid.
///
/// **The stored `glow::Context` becomes garbage here**: the GL functions it resolved no longer
/// exist. Discarding the context and going back to software is the safe answer: the game keeps
/// running on the processor, and the notice says why.
///
/// # The race, and what it costs
///
/// **What calls this is the frontend's video driver, and it may be another thread.** The core runs
/// [`retro_run`] on the thread the frontend uses for the game; this callback arrives from its video
/// thread: switching driver, turning threaded video on or off, recreating the window. The ABI
/// (`libretro.h`) **guarantees nothing** about this: it says what to do with the GL objects when
/// the notice arrives, not which thread it comes from nor that it waits for the frame to finish.
///
/// What the core can do here is cheap and is what is done: **mark the GPU as dead**, with an
/// `AtomicUsize` and no lock at all (`gpu_destroyed`), because blocking in this callback can block
/// the core: the frontend may be holding the emulator's frame, and a lock shared with `retro_run`
/// would close the cycle. From then on whoever draws sees the mark (`GpuState`): it stops sending
/// draws and reading the frame, and the session's `Drop` lets go of the GL names without calling
/// `delete_*` on a dead context.
///
/// **What cannot be fixed from here, and why:** the `gl*` call already in progress when the notice
/// arrives **finishes on the dead context**; there is no way to interrupt a driver call midway. And
/// there is no way to stop the two threads from using the same context without changing the
/// contract with the frontend: serializing drawing and `context_destroy` behind a lock requires the
/// drawing to happen on a thread the core controls, and it does not: `retro_run` is called by the
/// frontend, and the frontend decides whether video is on a separate thread. A lock around
/// `core()` here would, besides, really deadlock: `retro_video_refresh` delivers the frame with no
/// lock at all (see the note at the start of [`retro_run`]), and a frontend that waits for this
/// callback while the core waits for the frontend to consume the frame is a deadly embrace.
unsafe extern "C" fn context_lost() {
    if let Ok(mut guard) = GPU.lock() {
        if let Some(gl_context) = guard.as_ref() {
            zeebx::video::gpu::gpu_destroyed(zeebx::video::gpu::gpu_address(gl_context));
        }
        *guard = None;
    }
    CONTEXT_READY.store(false, std::sync::atomic::Ordering::Relaxed);
    GPU_LOST.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the frontend announced that the current GPU stopped being valid, or that a new GPU is
/// ready.
///
/// Both notices land here because the answer is the same: the live session draws on a GPU that is
/// no longer the current one. See [`context_lost`] and [`context_ready`].
static GPU_LOST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Asks whether the frontend accepts a null frame when nothing changed.
const ENV_GET_CAN_DUPE: u32 = 3;
const ENV_GET_SYSTEM_DIRECTORY: u32 = 9;
const ENV_SET_PIXEL_FORMAT: u32 = 10;
const ENV_SET_INPUT_DESCRIPTORS: u32 = 11;
const ENV_GET_LOG_INTERFACE: u32 = 27;
const ENV_GET_SAVE_DIRECTORY: u32 = 31;
const ENV_SET_CONTROLLER_INFO: u32 = 35;
const ENV_SET_VARIABLES: u32 = 16;
const ENV_GET_VARIABLE: u32 = 15;
/// `RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE`: the frontend announces that some option changed.
///
/// **Without this every core option is a reload-the-game option.** `GET_VARIABLE` returns the
/// current value, but asking for it on every frame, for every key, costs one call to the frontend
/// per key per frame. This is the cheap notice: it returns `true` a single time after the user
/// touched the menu, and only then is it worth rereading what can be applied on the fly.
const ENV_GET_VARIABLE_UPDATE: u32 = 17;
const ENV_GET_CORE_OPTIONS_VERSION: u32 = 52;
const ENV_SET_CORE_OPTIONS_V2: u32 = 67;
/// `RETRO_ENVIRONMENT_SET_AUDIO_BUFFER_STATUS_CALLBACK`: before each `retro_run`, the frontend
/// starts reporting how full its audio buffer is and whether an underrun is likely.
///
/// It is Libretro's own mechanism for automatic frameskip, not something we invented: the paragraph
/// in `libretro.h` itself says "if `underrun_likely`, the core should try to skip a frame".
const ENV_SET_AUDIO_BUFFER_STATUS_CALLBACK: u32 = 62;
/// `RETRO_ENVIRONMENT_GET_CURRENT_SOFTWARE_FRAMEBUFFER`: the frontend **lends** a buffer for the
/// core to draw the frame in.
///
/// Without it, the core builds the frame in its own vector and the frontend copies it: a 600 KB
/// copy per frame, which on a weak device is real work. With it, the core writes where the frame
/// will end up, and the borrowed pointer is what is handed to `retro_video_refresh`.
///
/// **The pointer is only valid within this call of `retro_run`** (`libretro.h` itself warns), and
/// that is why the request and the use stay in the same place, without keeping anything between
/// frames.
const ENV_GET_CURRENT_SOFTWARE_FRAMEBUFFER: u32 = 40 | 0x1_0000;

/// The buffer the frontend lends. The order and types are those of `libretro.h`.
#[repr(C)]
struct RetroFramebuffer {
    /// Filled in by the frontend; the core asks with null.
    data: *mut c_void,
    /// The core asks for the size it wants; the frontend may return a different one.
    width: u32,
    height: u32,
    /// Distance in bytes between the start of two lines, set by the frontend.
    pitch: usize,
    /// The pixel format, set by the frontend: **it may differ from the negotiated one**, and that
    /// is why it is checked before writing.
    format: u32,
}

/// `RETRO_ENVIRONMENT_SET_MINIMUM_AUDIO_LATENCY`: asks for more headroom in the frontend's audio
/// buffer.
///
/// The documentation of the callback above recommends this: without headroom, the underrun notice
/// arrives too late for the core to react in time. 96 ms (SAMPLE_RATE-independent, the frontend is
/// the one measuring) is six to eight frames at 60 Hz, the range `libretro.h` suggests.
const ENV_SET_MINIMUM_AUDIO_LATENCY: u32 = 63;

/// RetroPad device types and button identifiers.
const DEVICE_NONE: u32 = 0;
const DEVICE_JOYPAD: u32 = 1;
const DEVICE_ANALOG: u32 = 5;
const ID_B: u32 = 0;
const ID_Y: u32 = 1;
const ID_SELECT: u32 = 2;
const ID_START: u32 = 3;
const ID_UP: u32 = 4;
const ID_DOWN: u32 = 5;
const ID_LEFT: u32 = 6;
const ID_RIGHT: u32 = 7;
const ID_A: u32 = 8;
const ID_X: u32 = 9;
const ID_L: u32 = 10;
const ID_R: u32 = 11;
const ANALOG_LEFT: u32 = 0;
const ANALOG_AXIS_X: u32 = 0;
const ANALOG_AXIS_Y: u32 = 1;

/// RetroPad subclasses that identify the console's devices.
const DEVICE_ZPAD: u32 = ((1 + 1) << 8) | DEVICE_JOYPAD;
const DEVICE_BOOMERANG: u32 = ((2 + 1) << 8) | DEVICE_JOYPAD;

type EnvironmentFn = unsafe extern "C" fn(cmd: u32, data: *mut c_void) -> bool;
type KeyboardEventFn = unsafe extern "C" fn(down: bool, keycode: u32, character: u32, modifiers: u16);

#[repr(C)]
struct RetroMessage {
    msg: *const c_char,
    frames: u32,
}

#[repr(C)]
struct RetroVariable {
    key: *const c_char,
    value: *const c_char,
}

/// `retro_audio_buffer_status_callback_t`, the function type, not the registration struct.
type RetroAudioBufferStatusCallbackFn = unsafe extern "C" fn(bool, u32, bool);

#[repr(C)]
struct RetroAudioBufferStatusCallback {
    callback: Option<RetroAudioBufferStatusCallbackFn>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RetroCoreOptionValue {
    value: *const c_char,
    label: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RetroCoreOptionV2Category {
    key: *const c_char,
    desc: *const c_char,
    info: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RetroCoreOptionV2Definition {
    key: *const c_char,
    desc: *const c_char,
    desc_categorized: *const c_char,
    info: *const c_char,
    info_categorized: *const c_char,
    category_key: *const c_char,
    values: [RetroCoreOptionValue; 128],
    default_value: *const c_char,
}

#[repr(C)]
struct RetroCoreOptionsV2 {
    categories: *const RetroCoreOptionV2Category,
    definitions: *const RetroCoreOptionV2Definition,
}

unsafe impl Sync for RetroVariable {}
unsafe impl Sync for RetroCoreOptionsV2 {}
unsafe impl Sync for RetroCoreOptionV2Category {}
unsafe impl Sync for RetroCoreOptionV2Definition {}

#[repr(C)]
struct RetroKeyboardCallback {
    callback: Option<KeyboardEventFn>,
}
type VideoRefreshFn =
    unsafe extern "C" fn(data: *const c_void, width: u32, height: u32, pitch: usize);
type AudioSampleBatchFn = unsafe extern "C" fn(data: *const i16, frames: usize) -> usize;
type InputPollFn = unsafe extern "C" fn();
type InputStateFn = unsafe extern "C" fn(port: u32, device: u32, index: u32, id: u32) -> i16;

#[repr(C)]
pub struct RetroSystemInfo {
    library_name: *const c_char,
    library_version: *const c_char,
    valid_extensions: *const c_char,
    need_fullpath: bool,
    block_extract: bool,
}

#[repr(C)]
pub struct RetroGameGeometry {
    base_width: u32,
    base_height: u32,
    max_width: u32,
    max_height: u32,
    aspect_ratio: f32,
}

#[repr(C)]
pub struct RetroSystemTiming {
    fps: f64,
    sample_rate: f64,
}

#[repr(C)]
pub struct RetroSystemAvInfo {
    geometry: RetroGameGeometry,
    timing: RetroSystemTiming,
}

#[repr(C)]
pub struct RetroGameInfo {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

#[repr(C)]
struct RetroLogCallback {
    log: Option<unsafe extern "C" fn(level: u32, fmt: *const c_char, ...)>,
}

#[repr(C)]
struct RetroInputDescriptor {
    port: u32,
    device: u32,
    index: u32,
    id: u32,
    description: *const c_char,
}

#[repr(C)]
struct RetroControllerDescription {
    desc: *const c_char,
    id: u32,
}

// SAFETY: the descriptor tables are `static` and immutable; the pointers point to literals with a
// static lifetime. The ABI requires them to stay valid for the whole load.
unsafe impl Sync for RetroControllerDescription {}

#[repr(C)]
struct RetroControllerInfo {
    types: *const RetroControllerDescription,
    num_types: u32,
}

// SAFETY: same as above: the port list is immutable and points to the static descriptions.
unsafe impl Sync for RetroControllerInfo {}

/// What the frontend offers the core.
///
/// It is `Copy` on purpose: the pointers are taken out of the mutex **before** any call, so that
/// the core never holds a lock of its own while the frontend runs. A frontend that opens a dialog,
/// saves state or waits for another thread inside the callback would block the core, and that is
/// what happened with the full disk, when RetroArch opened the write warning in the middle of the
/// frame.
#[derive(Default, Clone, Copy)]
struct Frontend {
    environ: Option<EnvironmentFn>,
    video: Option<VideoRefreshFn>,
    audio_batch: Option<AudioSampleBatchFn>,
    input_poll: Option<InputPollFn>,
    input_state: Option<InputStateFn>,
}

/// State of the loaded game.
struct Core {
    session: Session,
    mixer: Mixer,
    /// Ports 1 and 2, as the guest sees them.
    ports: [Option<Device>; zeebx::input::PORTS],
    /// Frame buffer in the negotiated format, reused on every `retro_run`.
    frame: Vec<u8>,
    audio: Vec<i16>,
    /// Path to the content, for `retro_reset`.
    path: PathBuf,
    /// The profile's roots, to switch applets without losing saves or cache.
    storage: StoragePaths,
    /// The games found next to the content: the applet's ClassID and what to load.
    ///
    /// It is what makes it possible to serve the shell's launch request: the Z-Wheel asks for a
    /// class, and here we know which `.mod` or `.zip` answers for it.
    games: Vec<(u32, PathBuf)>,
    /// The Z-Wheel's content, to go back to it when a game it opened ends.
    z_wheel: Option<PathBuf>,
    /// Whether the current session was opened by the Z-Wheel: going back goes to it, as on the
    /// console.
    opened_via_z_wheel: bool,
    /// Whether the frontend delivers the RetroPad buttons in a bit mask.
    bitmasks: bool,
    /// Whether the frontend accepts a null frame when the screen did not change.
    accepts_dupe: bool,
    /// How many GPU frames the game had drawn on the previous frame. The engine's counter is
    /// cumulative, and the GPU may have drawn once at startup and never again: what decides is the
    /// difference between this frame and the previous one. See the comment in the block that builds
    /// the frame.
    gl_frames_before: u32,
    /// Signature of the last delivered frame.
    last_signature: Option<u64>,
    /// Virtual clock of the last call, so the audio follows the time that really passed.
    last_clock_ms: u32,
    /// Wall clock of the last audio delivery. A frame on the Vita takes longer than the
    /// virtual clock, and the speaker does not wait: the sample count uses this
    /// instant.
    #[cfg(target_os = "vita")]
    last_real_audio: std::time::Instant,
    /// Samples the frontend did not accept, left for the next call.
    pending_audio: Vec<i16>,
    /// Mixer buffer, reused on every frame.
    ///
    /// `render` allocated a new `Vec` per frame. On the Vita that fragments the heap, and the long
    /// track later cannot find a contiguous block.
    mix_buf: Vec<f32>,
    /// Whether it already warned that the frame left the console's size.
    warned_size: bool,
    /// MIDI synthesis policy configured in the core options.
    midi_backend: zeebx::audio::MidiBackend,
    /// Previous state of the RetroPad's Select, for the `AVK_CLR` shortcut.
    prev_select: bool,
    /// The previous pass's controller, per port, so that what changes becomes a **console key**.
    ///
    /// The applet reads the d-pad and button 1 as BREW keys (`0xe031`…`0xe064`), not by the
    /// controller's position: those are what the Z-Wheel navigates with. Without keeping the
    /// previous frame there is no way to know what changed, and the change is what becomes a key.
    prev_pad: [Pad; zeebx::input::PORTS],
    /// How many frames have been presented since the stop.
    ///
    /// The final screen stays in view for a moment before the frontend is dismissed: without that
    /// the content disappears on the same frame the game ends, and whoever is playing does not see
    /// the outcome.
    frames_after_stop: u32,
    /// Whether turning on hardware rendering was already attempted. Only once: a context that did
    /// not come will not come on the next frame. See [`enable_gpu`].
    gpu_enabled: bool,
    /// The frameskip policy chosen now; see [`Frameskip`].
    frameskip: Frameskip,
    /// The phase of the fixed skip and of the 30 FPS half presentation: how many frames have gone
    /// by since the last drawn one. It is the same counter as the desktop window's; see
    /// [`zeebx::speed::SkipCounter`].
    skip: SkipCounter,
    /// Whether `SET_AUDIO_BUFFER_STATUS_CALLBACK` was already requested from the frontend. Only
    /// once: asking again on every frame does not change the answer, and Libretro's own
    /// documentation asks for restraint with this call.
    frameskip_callback_requested: bool,
    /// Whether we already warned that `glReadPixels` made rasterization frameskip unsafe.
    frameskip_pixel_read_warned: bool,
    /// Speed cap chosen; see [`FpsLimit`].
    fps_limit: FpsLimit,
    /// This frame must repeat the previous image in the video callback.
    ///
    /// Separate from `skip_draw`: 2D games and games with `glReadPixels` may not save
    /// rasterization, but 30 FPS still needs to truly limit presentation.
    fps_limit_doubles: bool,
    /// Whether the outcome was already reported to the frontend.
    ///
    /// Without this the core would repeat the same line on every frame after the stop, and a log
    /// that grows forever hides precisely the instant the game stopped.
    stopped: bool,
}

fn frontend() -> &'static Mutex<Frontend> {
    static FRONTEND: OnceLock<Mutex<Frontend>> = OnceLock::new();
    FRONTEND.get_or_init(|| Mutex::new(Frontend::default()))
}

/// The game's state as seen by the ABI.
///
/// The engine is not `Send`: it carries `Rc`, JIT and rasterizer pointers, which are valid while
/// they stay on the same thread. The Libretro ABI guarantees that `retro_run`, `retro_reset` and
/// `retro_unload_game` are called on the thread that loaded the game, and that is the promise this
/// wrapper makes to the compiler.
struct CoreState(Core);

// SAFETY: see the comment above: the core is never moved to another thread.
unsafe impl Send for CoreState {}

/// Queue of the frontend's keys.
///
/// The callback does not run the guest: it **queues**, and `retro_run` hands the keys to the engine
/// on the core's normal thread. Running the emulator from inside the callback would be reentrancy
/// on top of the state `retro_run` is using.
fn key_events() -> &'static Mutex<std::collections::VecDeque<(u32, bool)>> {
    static KEY_EVENTS: OnceLock<Mutex<std::collections::VecDeque<(u32, bool)>>> = OnceLock::new();
    KEY_EVENTS.get_or_init(|| Mutex::new(std::collections::VecDeque::new()))
}

/// Translates a frontend key to the BREW virtual code, when there is one.
///
/// `Esc` and `P` are left out on purpose: they are the frontend's interface keys, not the game's.
fn avk_for_key(keycode: u32) -> Option<u32> {
    use zeebx::input::avk;
    Some(match keycode {
        RETROK_UP => avk::UP,
        RETROK_DOWN => avk::DOWN,
        RETROK_LEFT => avk::LEFT,
        RETROK_RIGHT => avk::RIGHT,
        RETROK_RETURN => avk::SELECT,
        RETROK_BACKSPACE | RETROK_ESCAPE => avk::CLR,
        RETROK_ASTERISK => avk::STAR,
        RETROK_HASH => avk::POUND,
        RETROK_0..=57 => avk::ZERO + (keycode - RETROK_0),
        _ => return None,
    })
}

/// The frontend's keyboard callback: it only queues.
unsafe extern "C" fn key_received(down: bool, keycode: u32, _character: u32, _modifiers: u16) {
    let Some(avk) = avk_for_key(keycode) else {
        return;
    };
    if let Ok(mut queue) = key_events().lock() {
        // A cap keeps a stuck key (or a frontend repeating nonstop) from growing without end.
        if queue.len() < 1024 {
            queue.push_back((avk, down));
        }
    }
}

fn core() -> &'static Mutex<Option<CoreState>> {
    static CORE: OnceLock<Mutex<Option<CoreState>>> = OnceLock::new();
    CORE.get_or_init(|| Mutex::new(None))
}

/// A copy of the frontend's callbacks, without holding the lock.
fn callbacks() -> Frontend {
    frontend().lock().map(|guard| *guard).unwrap_or_default()
}

/// Shows a notice to the player. It also goes to the log, because not every frontend shows
/// messages, and a notice nobody sees is good for nothing.
fn warning(text: &str) {
    let Ok(c_text) = CString::new(text) else {
        return;
    };
    let message = RetroMessage {
        // Three seconds at 60 Hz: time to read without getting in the player's way.
        msg: c_text.as_ptr(),
        frames: 180,
    };
    unsafe {
        environ(
            ENV_SET_MESSAGE,
            &message as *const RetroMessage as *mut c_void,
        );
    }
    log(&format!("Zeebx: {text}"));
}

unsafe fn environ(cmd: u32, data: *mut c_void) -> bool {
    let Some(callback) = callbacks().environ else {
        return false;
    };
    // SAFETY: the frontend promises the callback accepts the requested command. The core's lock has
    // already been released, so the frontend may do whatever it wants in here.
    unsafe { callback(cmd, data) }
}

/// An **informational** line in the frontend's log: what the core decided and where it fetched
/// things from.
///
/// **Everything was coming out as an error.** This shortcut wrote with level 3 (`RETRO_LOG_ERROR`)
/// and almost all of the core's reporting goes through it: font, sound bank, rasterizer, end of
/// game. In RetroArch's log that becomes `[libretro ERROR]` on an informational line, and whoever
/// reads the log to decide something (that is how the R36S session was read) starts hunting for a
/// defect that does not exist.
fn log(message: &str) {
    log_with_level(1, message);
}

/// Writes to the frontend's log with the `retro_log_level` level.
///
/// `0..=3` are `libretro.h`'s `DEBUG`, `INFO`, `WARN` and `ERROR`. **There is no `FATAL` in the
/// ABI**, and that is why [`zeebx::logging::Level::Fatal`] goes out as `ERROR`: inventing a number
/// outside the range would make the frontend discard the line, which is the worst outcome for the
/// message that says the emulator cannot continue.
fn log_with_level(level: u32, message: &str) {
    let Ok(text) = CString::new(message) else {
        return;
    };
    let mut callback = RetroLogCallback { log: None };
    let target = &mut callback as *mut RetroLogCallback as *mut c_void;
    if unsafe { environ(ENV_GET_LOG_INTERFACE, target) } {
        if let Some(write) = callback.log {
            // SAFETY: the frontend provided the callback and the format is a literal.
            unsafe { write(level, c"%s".as_ptr(), text.as_ptr()) };
        }
    }
}

/// Audio frames delivered, and the real-time count to measure the actual rate.
///
/// `av_info` declares 44100 frames per second. If what goes out of here is something else, the
/// frontend resamples (or its buffer runs dry) and the symptom is high-pitched, fast or choppy
/// sound, without anything inside the engine showing it. One line per second answers that on any
/// device.
static AUDIO_FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static AUDIO_PREVIOUS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static AUDIO_LAST_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static AUDIO_CLOCK: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Asks the frontend for the buffer the frame should be drawn in.
///
/// Returns `None` when it does not offer one, when the buffer is unusable (a format other than
/// RGB565, or a size other than requested) or when the pointer comes back null. **Every refusal
/// falls back to the usual path** (the core's own vector), because a frontend that does not lend a
/// buffer must not leave the core without an image.
///
/// See [`ENV_GET_CURRENT_SOFTWARE_FRAMEBUFFER`].
fn request_frontend_buffer(requested_width: u32, requested_height: u32) -> Option<(*mut u8, usize)> {
    let mut request = RetroFramebuffer {
        data: std::ptr::null_mut(),
        width: requested_width,
        height: requested_height,
        pitch: 0,
        format: PIXEL_FORMAT_RGB565,
    };
    let target = &mut request as *mut RetroFramebuffer as *mut c_void;
    if !unsafe { environ(ENV_GET_CURRENT_SOFTWARE_FRAMEBUFFER, target) } {
        return None;
    }
    // The frontend **may** return another format (`libretro.h` says so, for conversion). Writing
    // RGB565 into an XRGB8888 buffer would give a plausible and false image, so it is refused here.
    if request.data.is_null()
        || request.format != PIXEL_FORMAT_RGB565
        || request.width != requested_width
        || request.height != requested_height
    {
        return None;
    }
    Some((request.data.cast::<u8>(), request.pitch))
}

/// Writes the frame into the buffer the frontend lent, respecting its line pitch.
///
/// # Safety
///
/// The pointer and the pitch come from [`request_frontend_buffer`], which only returns them when
/// the frontend accepted the request, and `libretro.h` guarantees the buffer is valid until the end
/// of this `retro_run` call, which is where this runs.
fn write_frame(screen: &zeebx::video::display::Framebuffer, data: *mut u8, step: usize) {
    let (width, height) = (screen.width() as usize, screen.height() as usize);
    let step = step.max(width * 2);
    let bytes = step.saturating_mul(height);
    if bytes == 0 {
        return;
    }
    // SAFETY: the frontend promised a buffer with `step * height` usable bytes in this call.
    let dest = unsafe { std::slice::from_raw_parts_mut(data, bytes) };
    screen.write_rgb565_with_pitch(dest, step);
}

/// The `libretro.h` level that corresponds to the core's level.
fn libretro_level(level: zeebx::logging::Level) -> u32 {
    use zeebx::logging::Level;
    match level {
        Level::Debug => 0,
        Level::Info => 1,
        Level::Warning => 2,
        // `FATAL` does not exist in the ABI: its ceiling is `ERROR`.
        Level::Error | Level::Fatal => 3,
    }
}

/// Dumps to the frontend what the core logged since the last call.
///
/// **Here, and not inside the core.** The frontend's log is a variadic C callback, and calling it
/// from deep inside a drawing or loading function would mean crossing into frontend code in the
/// middle of a state of ours. The ring in [`zeebx::logging`] exists exactly for that: the core
/// keeps, and the libretro core delivers at a point where the ABI expects to be called.
///
/// The per-call cap keeps a debugged game, which writes thousands of lines per frame, from turning
/// the frontend's log into the emulator's bottleneck.
fn dump_log() {
    /// How many lines go out per frame. Three hundred is the whole ring, and anything more would
    /// only go out on the next frame, without delaying the game to serve the log.
    const PER_FRAME: usize = 64;

    for line in zeebx::logging::drain().into_iter().take(PER_FRAME) {
        log_with_level(
            libretro_level(line.level),
            &format!(
                "Zeebx [{}] {}: {}",
                line.level.tag(),
                line.target,
                line.text
            ),
        );
    }
    let dropped = zeebx::logging::dropped();
    if dropped > 0 {
        zeebx::logging::reset_dropped();
        log_with_level(
            2,
            &format!("Zeebx: {dropped} log line(s) dropped by the ring's cap"),
        );
    }
}

/// Applies the `zeebx_log` option to the core's logging.
///
/// The `ZEEBX_LOG` environment variable counts as the starting point (it is what serves whoever
/// debugs from the command line), and the frontend's option wins over it when present, because it
/// is the explicit choice of whoever has RetroArch open.
fn apply_log_level() {
    zeebx::logging::from_env();
    let Some(text) = (unsafe { read_option(c"zeebx_log") }) else {
        return;
    };
    match zeebx::logging::Threshold::from_text(&text) {
        Some(threshold) => threshold.apply(),
        None => log(&format!(
            "Zeebx: `{text}` is not a log level; continuing with the ZEEBX_LOG value or the default"
        )),
    }
}

/// Describes a port's device for the guest's `GetConnectedDevices`.
fn device_for_retro_device(device: u32) -> Option<Device> {
    match device {
        DEVICE_NONE => None,
        DEVICE_ZPAD => Some(Device::ZPad),
        DEVICE_BOOMERANG => Some(Device::Boomerang),
        _ => Some(Device::Controller),
    }
}

/// Whether the RetroPad's Select is pressed on the given port.
fn read_select(port: u32) -> bool {
    let front = callbacks();
    let (Some(poll), Some(state)) = (front.input_poll, front.input_state) else {
        return false;
    };
    // SAFETY: frontend callbacks, called on the `retro_run` thread.
    unsafe {
        poll();
        state(port, DEVICE_JOYPAD, 0, ID_SELECT) != 0
    }
}

/// The key of the option that mirrors the d-pad on this port's axes.
///
/// **One per port, not one for all.** The console has two ports and two players: whoever plays
/// with the stick on Z-Pad 1 does not force the owner of Z-Pad 2 to play with the d-pad turning
/// into an axis. RetroArch announces eight ports and the core only registers two; a port outside
/// the range has no option, and stays off.
fn mirror_key(port: u32) -> Option<&'static CStr> {
    match port {
        0 => Some(c"zeebx_dpad_to_analog_p1"),
        1 => Some(c"zeebx_dpad_to_analog_p2"),
        _ => None,
    }
}

/// What each RetroPad button becomes on the console, and what it is called on the mapping screen.
///
/// **A single table for both things, on purpose.** `read_pad` reads through it and
/// [`register_buttons`] labels through it. When they were two parallel lists, they diverged
/// silently: the label said "B = Button 1" and the reading delivered B as Button 2, so the
/// frontend's mapping screen taught people to press the wrong button (issue #41). With a single
/// table, that divergence is impossible to write.
///
/// **Each console button's name carries its position on the device, not the order of the
/// numbers:** `b1` is at the **bottom**, `b2` on the **left**, `b3` at the **top** and `b4` on the
/// **right** (checked against the official images). That is why Button 1 lands on the RetroPad's
/// `B`, which is the bottom one, and not on `A`, which is the right one. See
/// `docs/implementation/09-input.md`.
const RETROPAD_BUTTONS: [(u32, &str, &str); 12] = [
    (ID_UP, "up", "D-pad up"),
    (ID_DOWN, "down", "D-pad down"),
    (ID_LEFT, "left", "D-pad left"),
    (ID_RIGHT, "right", "D-pad right"),
    (ID_B, "b1", "Button 1 (bottom)"),
    (ID_Y, "b2", "Button 2 (left)"),
    (ID_X, "b3", "Button 3 (top)"),
    (ID_A, "b4", "Button 4 (right)"),
    (ID_L, "zl", "ZL"),
    (ID_R, "zr", "ZR"),
    (ID_START, "start", "Start"),
    (ID_SELECT, "back", "HOME/Back"),
];

/// Reads the RetroPad and builds the state the console sees.
///
/// With `bitmasks`, the twelve buttons come in a single word: one call to the frontend per frame
/// instead of twelve. The special `id` `ID_JOYPAD_MASK` returns the bits in the order of the
/// `RETRO_DEVICE_ID_JOYPAD_*`.
fn read_pad(port: u32, bitmasks: bool) -> Pad {
    let front = callbacks();
    let (Some(poll), Some(state)) = (front.input_poll, front.input_state) else {
        return Pad::default();
    };
    // SAFETY: the callbacks come from the frontend and are called on the `retro_run` thread.
    unsafe { poll() };
    let mask = match bitmasks {
        // SAFETY: a state query to the frontend itself.
        true => unsafe { state(port, DEVICE_JOYPAD, 0, ID_JOYPAD_MASK) as u32 },
        false => 0,
    };
    let button = |id: u32| -> bool {
        match bitmasks {
            true => mask & (1 << id) != 0,
            // SAFETY: a state query to the frontend itself.
            false => unsafe { state(port, DEVICE_JOYPAD, 0, id) != 0 },
        }
    };
    let mut pad = Pad::default();
    // Reading and labeling come from the **same** table: see [`RETROPAD_BUTTONS`].
    for (id, name, _) in RETROPAD_BUTTONS {
        if button(id) {
            if let Some(button_index) = Pad::button_by_name(name) {
                pad.press(button_index, true);
            }
        }
    }
    // The d-pad mirrored on the axes, when this port's option is on. **Before** the analog loop:
    // the mirror writes zero at rest (that is how the stick goes back to center when the direction
    // is released), and the last word has to belong to the real stick, which only writes when it
    // leaves the dead zone.
    if let Some(key_id) = mirror_key(port) {
        // SAFETY: a frontend option query, on the `retro_run` thread.
        if unsafe { read_option(key_id) }.as_deref() == Some("enabled") {
            pad.mirror_dpad_on_axes();
        }
    }
    // The RetroPad's two analog sticks become the console's four axes, in the range the guest
    // reads.
    set_retropad_axes(&mut pad, |index, id| {
        // SAFETY: a state query to the frontend itself.
        unsafe { state(port, DEVICE_ANALOG, index, id) as i32 }
    });
    pad
}

/// Puts the RetroPad's two analog sticks on the console's axes, with the resting dead zone.
///
/// **The dead zone is not about jitter: it is what makes the d-pad option work.** The RetroPad
/// delivers a stick resting at the center as zero, and writing that zero would erase, on every
/// frame, what the d-pad mirror had just put there: the `zeebx_dpad_to_analog_pN` option had no
/// effect inside the core, and only there (the standalone's `Player::pad` already had this rule,
/// which is why the measurement in the harness passed and RetroArch did not). Whoever is at the
/// center **does not write**; whoever is outside the dead zone beats the mirror.
///
/// The reading comes in as a parameter so the test can call this with no frontend at all.
fn set_retropad_axes(pad: &mut Pad, axis: impl Fn(u32, u32) -> i32) {
    let dead_zone =
        (zeebx::input::bindings::DEADZONE * zeebx::input::AXIS_RANGE as f32).round() as i32;
    for (index, value) in [
        (0usize, axis(ANALOG_LEFT, ANALOG_AXIS_X)),
        (1usize, axis(ANALOG_LEFT, ANALOG_AXIS_Y)),
        (2usize, axis(1, ANALOG_AXIS_X)),
        (3usize, axis(1, ANALOG_AXIS_Y)),
    ] {
        // The frontend's `-0x8000..=0x7fff` to the console stick's travel.
        let value = value / 256;
        if value.abs() < dead_zone {
            continue;
        }
        pad.set_axis(index, value);
    }
}

/// Registers the devices the user can choose on each port.
unsafe fn register_controllers() {
    static DESCRIPTIONS: [RetroControllerDescription; 5] = [
        RetroControllerDescription {
            desc: c"Disconnected".as_ptr(),
            id: DEVICE_NONE,
        },
        RetroControllerDescription {
            desc: c"Dragon".as_ptr(),
            id: DEVICE_JOYPAD,
        },
        RetroControllerDescription {
            desc: c"Z-Pad".as_ptr(),
            id: DEVICE_ZPAD,
        },
        RetroControllerDescription {
            desc: c"Boomerang".as_ptr(),
            id: DEVICE_BOOMERANG,
        },
        RetroControllerDescription {
            desc: c"USB Keyboard".as_ptr(),
            id: 3,
        },
    ];
    static PORTS: [RetroControllerInfo; 3] = [
        RetroControllerInfo {
            types: DESCRIPTIONS.as_ptr(),
            num_types: DESCRIPTIONS.len() as u32,
        },
        RetroControllerInfo {
            types: DESCRIPTIONS.as_ptr(),
            num_types: DESCRIPTIONS.len() as u32,
        },
        RetroControllerInfo {
            types: std::ptr::null(),
            num_types: 0,
        },
    ];
    // SAFETY: the tables have a static lifetime, as the ABI requires.
    unsafe {
        environ(ENV_SET_CONTROLLER_INFO, PORTS.as_ptr() as *mut c_void);
    }
}

/// Labels the buttons for the frontend's configuration screen.
unsafe fn register_buttons() {
    let mut descriptors: Vec<RetroInputDescriptor> = Vec::new();
    let labels = RETROPAD_BUTTONS.map(|(id, _, label)| (id, label));
    for port_index in 0..2u32 {
        for (id, text) in labels {
            descriptors.push(RetroInputDescriptor {
                port: port_index,
                device: DEVICE_JOYPAD,
                index: 0,
                id,
                description: text.as_ptr() as *const c_char,
            });
        }
        for (index, id, text) in [
            (ANALOG_LEFT, ANALOG_AXIS_X, "Stick X"),
            (ANALOG_LEFT, ANALOG_AXIS_Y, "Stick Y"),
        ] {
            descriptors.push(RetroInputDescriptor {
                port: port_index,
                device: DEVICE_ANALOG,
                index,
                id,
                description: text.as_ptr() as *const c_char,
            });
        }
    }
    descriptors.push(RetroInputDescriptor {
        port: 0,
        device: 0,
        index: 0,
        id: 0,
        description: std::ptr::null(),
    });
    // SAFETY: the list ends in a null `description` and the texts are static literals.
    unsafe {
        environ(
            ENV_SET_INPUT_DESCRIPTORS,
            descriptors.as_ptr() as *mut c_void,
        );
    }
}

fn dir(cmd: u32) -> Option<PathBuf> {
    let mut pointer: *const c_char = std::ptr::null();
    let target = &mut pointer as *mut *const c_char as *mut c_void;
    if !unsafe { environ(cmd, target) } || pointer.is_null() {
        return None;
    }
    // SAFETY: the frontend delivers a valid UTF-8 path during the call; we copy it here.
    let text = unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned();
    Some(PathBuf::from(text))
}

/// Registers configuration options in RetroArch through V2 (with categories) or the V0 fallback
/// (SET_VARIABLES).
unsafe fn register_core_options() {
    let mut version: u32 = 0;
    let has_v2 = unsafe {
        environ(
            ENV_GET_CORE_OPTIONS_VERSION,
            &mut version as *mut u32 as *mut c_void,
        )
    } && version >= 2;

    if has_v2 {
        // **A defect from the previous phase, fixed here.** Five video options already declared
        // `category_key: c"video"`, but this array only had the `"audio"` category; the video one
        // was never registered. The frontend does not lock up with a key that matches no
        // category, but the option ends up without the right grouping in the menu, and nobody
        // had noticed.
        static CATEGORIES: [RetroCoreOptionV2Category; 5] = [
            RetroCoreOptionV2Category {
                key: c"audio".as_ptr(),
                desc: c"Audio".as_ptr(),
                info: c"Zeebx sound synthesis and output settings".as_ptr(),
            },
            RetroCoreOptionV2Category {
                key: c"video".as_ptr(),
                desc: c"Video".as_ptr(),
                info: c"Zeebx rasterization and image settings".as_ptr(),
            },
            RetroCoreOptionV2Category {
                key: c"sistema".as_ptr(),
                desc: c"System".as_ptr(),
                info: c"Zeebx profiles and general settings".as_ptr(),
            },
            RetroCoreOptionV2Category {
                key: c"controles".as_ptr(),
                desc: c"Controls".as_ptr(),
                info: c"How each port's RetroPad becomes the console's controller".as_ptr(),
            },
            RetroCoreOptionV2Category {
                key: std::ptr::null(),
                desc: std::ptr::null(),
                info: std::ptr::null(),
            },
        ];

        let mut opt_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        opt_values[0] = RetroCoreOptionValue {
            value: c"auto".as_ptr(),
            label: c"Automatic (SoundFont if available, otherwise Table)".as_ptr(),
        };
        opt_values[1] = RetroCoreOptionValue {
            value: c"timbres".as_ptr(),
            label: c"Timbre table (fast / handhelds)".as_ptr(),
        };
        opt_values[2] = RetroCoreOptionValue {
            value: c"soundfont".as_ptr(),
            label: c"SoundFont (.sf2)".as_ptr(),
        };

        // The folder's banks, read now: a core option's list is fixed, and a free-form path does
        // not fit in it. See `banks_in_system_dir`.
        let mut bank_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        bank_values[0] = RetroCoreOptionValue {
            value: c"auto".as_ptr(),
            label: c"Automatic (the first .sf2 in the folder)".as_ptr(),
        };
        for (i, name) in banks_in_system_dir().into_iter().take(126).enumerate() {
            let name = leaked_c_text(&name);
            bank_values[i + 1] = RetroCoreOptionValue {
                value: name,
                label: name,
            };
        }

        let mut vol_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        // Eleven steps of ten. A list of a hundred and one items cannot be navigated with the
        // d-pad.
        const STEPS: [(&CStr, &CStr); 11] = [
            (c"100", c"100% (default)"),
            (c"90", c"90%"),
            (c"80", c"80%"),
            (c"70", c"70%"),
            (c"60", c"60%"),
            (c"50", c"50%"),
            (c"40", c"40%"),
            (c"30", c"30%"),
            (c"20", c"20%"),
            (c"10", c"10%"),
            (c"0", c"0% (mute)"),
        ];
        for (i, (option_value, option_label)) in STEPS.iter().enumerate() {
            vol_values[i] = RetroCoreOptionValue {
                value: option_value.as_ptr(),
                label: option_label.as_ptr(),
            };
        }

        let mut ras_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        ras_values[0] = RetroCoreOptionValue {
            value: c"auto".as_ptr(),
            label: c"Automatic (GPU when the frontend offers it)".as_ptr(),
        };
        ras_values[1] = RetroCoreOptionValue {
            value: c"software".as_ptr(),
            label: c"Processor (compatibility)".as_ptr(),
        };

        let mut on_off_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        on_off_values[0] = RetroCoreOptionValue {
            value: c"enabled".as_ptr(),
            label: c"On".as_ptr(),
        };
        on_off_values[1] = RetroCoreOptionValue {
            value: c"disabled".as_ptr(),
            label: c"Off".as_ptr(),
        };

        let mut scale_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        // **Both directions, and it matters to say which applies where.** Below 1x the drawing is
        // smaller and the presentation is what scales it up: only the processor rasterizer does
        // that, and it is where filling costs CPU. Above 1x it is supersampling, which only the
        // GPU does.
        const SCALES: [(&CStr, &CStr); 6] = [
            (c"1", c"1x — native, no supersampling (default)"),
            (c"0.5", c"0.5x — half (320×240), processor only"),
            (c"0.25", c"0.25x — a quarter (160×120), processor only"),
            (c"2", c"2x — draws at 1280×960 (GPU only)"),
            (c"3", c"3x — draws at 1920×1440 (GPU only)"),
            (c"4", c"4x — draws at 2560×1920 (GPU only)"),
        ];
        for (i, (option_value, option_label)) in SCALES.iter().enumerate() {
            scale_values[i] = RetroCoreOptionValue {
                value: option_value.as_ptr(),
                label: option_label.as_ptr(),
            };
        }

        let mut sample_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        const SAMPLES: [(&CStr, &CStr); 4] = [
            (c"1", c"Off"),
            (c"2", c"2x"),
            (c"4", c"4x"),
            (c"8", c"8x"),
        ];
        for (i, (option_value, option_label)) in SAMPLES.iter().enumerate() {
            sample_values[i] = RetroCoreOptionValue {
                value: option_value.as_ptr(),
                label: option_label.as_ptr(),
            };
        }

        let mut rate_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        rate_values[0] = RetroCoreOptionValue {
            value: c"44100".as_ptr(),
            label: c"44,100 Hz — with brightness (default)".as_ptr(),
        };
        rate_values[1] = RetroCoreOptionValue {
            value: c"22050".as_ptr(),
            label: c"22,050 Hz — half the cost and the memory".as_ptr(),
        };

        let mut voices_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        const VOICE_OPTS: [(&CStr, &CStr); 4] = [
            (c"128", c"128 (default)"),
            (c"96", c"96"),
            (c"64", c"64"),
            (c"48", c"48 — weak handhelds"),
        ];
        for (i, (option_value, option_label)) in VOICE_OPTS.iter().enumerate() {
            voices_values[i] = RetroCoreOptionValue {
                value: option_value.as_ptr(),
                label: option_label.as_ptr(),
            };
        }

        let mut cache_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        const CACHES: [(&CStr, &CStr); 5] = [
            (c"24", c"24 MiB (default)"),
            (c"48", c"48 MiB"),
            (c"16", c"16 MiB"),
            (c"8", c"8 MiB — weak handhelds"),
            (c"4", c"4 MiB"),
        ];
        for (i, (option_value, option_label)) in CACHES.iter().enumerate() {
            cache_values[i] = RetroCoreOptionValue {
                value: option_value.as_ptr(),
                label: option_label.as_ptr(),
            };
        }

        let mut profile_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        profile_values[0] = RetroCoreOptionValue {
            value: c"padrao".as_ptr(),
            label: c"Default".as_ptr(),
        };
        profile_values[1] = RetroCoreOptionValue {
            value: c"portatil".as_ptr(),
            label: c"Handheld (weak handheld device)".as_ptr(),
        };

        let mut fps_limit_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        fps_limit_values[0] = RetroCoreOptionValue {
            value: c"60".as_ptr(),
            label: c"60 FPS — the console's speed (default)".as_ptr(),
        };
        fps_limit_values[1] = RetroCoreOptionValue {
            value: c"30".as_ptr(),
            label: c"30 FPS — normal speed, half the frames".as_ptr(),
        };
        fps_limit_values[2] = RetroCoreOptionValue {
            value: c"desligado".as_ptr(),
            label: c"Off — boost/unlimited".as_ptr(),
        };

        let mut frameskip_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        const FRAMESKIP_OPTS: [(&CStr, &CStr); 8] = [
            (c"desligado", c"Off (default)"),
            (c"automatico", c"Automatic (by the audio buffer)"),
            (c"1", c"Fixed 1 — half the frames"),
            (c"2", c"Fixed 2 — a third of the frames"),
            (c"3", c"Fixed 3 — a quarter of the frames"),
            (c"4", c"Fixed 4 — a fifth of the frames"),
            (c"5", c"Fixed 5 — a sixth of the frames"),
            (c"6", c"Fixed 6 — a seventh of the frames"),
        ];
        for (i, (option_value, option_label)) in FRAMESKIP_OPTS.iter().enumerate() {
            frameskip_values[i] = RetroCoreOptionValue {
                value: option_value.as_ptr(),
                label: option_label.as_ptr(),
            };
        }

        // **Five levels, and "desligado" (off) apart from them.** The list is the same as the
        // core's log ([`zeebx::logging::Level`]) and the order goes from the most severe to the
        // chattiest, which is how a severity ceiling is chosen: the chosen level goes in, and
        // everything more severe does too.
        let mut discard_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        const DISCARD_OPTS: [(&CStr, &CStr); 2] = [
            (c"desligado", c"Off (default)"),
            (
                c"ligado",
                c"On — experimental, only helps on tiled GPUs (Mali)",
            ),
        ];
        for (i, (option_value, option_label)) in DISCARD_OPTS.iter().enumerate() {
            discard_values[i] = RetroCoreOptionValue {
                value: option_value.as_ptr(),
                label: option_label.as_ptr(),
            };
        }

        let mut log_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        const LOG_OPTS: [(&CStr, &CStr); 6] = [
            (c"desligado", c"Off"),
            (c"fatal", c"Fatal — only what prevents continuing"),
            (c"erro", c"Error — handled failures and above"),
            (c"aviso", c"Warning — what went off the expected path (default)"),
            (c"informacao", c"Info — what happened and is worth knowing"),
            (c"depuracao", c"Debug — the path of each decision (verbose)"),
        ];
        for (i, (option_value, option_label)) in LOG_OPTS.iter().enumerate() {
            log_values[i] = RetroCoreOptionValue {
                value: option_value.as_ptr(),
                label: option_label.as_ptr(),
            };
        }

        // The d-pad mirror on the axes. The list is the one from the request (issue #39): off
        // first, because off is the default, and it is the default because the idea was already
        // tried and undone twice (see `Pad::mirror_dpad_on_axes`).
        let mut mirror_values = [RetroCoreOptionValue {
            value: std::ptr::null(),
            label: std::ptr::null(),
        }; 128];
        const MIRROR_OPTS: [(&CStr, &CStr); 2] =
            [(c"disabled", c"Off"), (c"enabled", c"On")];
        for (i, (option_value, option_label)) in MIRROR_OPTS.iter().enumerate() {
            mirror_values[i] = RetroCoreOptionValue {
                value: option_value.as_ptr(),
                label: option_label.as_ptr(),
            };
        }

        // **One definition per port**, and there are two because the console has two
        // (`input::PORTS`). Each player turns on their own: whoever plays with the stick on Z-Pad 1
        // does not force the owner of Z-Pad 2 to play with the d-pad turning into an axis.
        let option_defs: [RetroCoreOptionV2Definition; 20] = [
            RetroCoreOptionV2Definition {
                key: c"zeebx_midi_backend".as_ptr(),
                desc: c"MIDI synthesizer (restart)".as_ptr(),
                desc_categorized: c"MIDI synthesizer (restart)".as_ptr(),
                info: c"MIDI playback engine: Auto uses a SoundFont if one is installed in the system folder; Timbre table starts instantly without heavy rendering (recommended for weak handhelds such as the H700). Reload the game to apply.".as_ptr(),
                info_categorized: c"Auto uses a SoundFont if present; Table starts instantly without the heavy SF2 load. Reload the game to apply.".as_ptr(),
                category_key: c"audio".as_ptr(),
                values: opt_values,
                default_value: c"auto".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_soundfont".as_ptr(),
                desc: c"SoundFont bank (restart)".as_ptr(),
                desc_categorized: c"SoundFont bank (restart)".as_ptr(),
                info: c"Which .sf2 plays the MIDI music. The list is the device's soundfonts folder, inside RetroArch's system folder (the log shows the path); put the console firmware's bank there, if you have it. Automatic uses the first one in alphabetical order. Reload the game to apply.".as_ptr(),
                info_categorized: c"Which .sf2 from the device's soundfonts folder plays the MIDI. Reload the game to apply.".as_ptr(),
                category_key: c"audio".as_ptr(),
                values: bank_values,
                default_value: c"auto".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_volume".as_ptr(),
                desc: c"Volume".as_ptr(),
                desc_categorized: c"Volume".as_ptr(),
                info: c"The console's master volume, from 0 to 100%. Applies right away, without reloading the game.".as_ptr(),
                info_categorized: c"The console's master volume. Applies right away.".as_ptr(),
                category_key: c"audio".as_ptr(),
                values: vol_values,
                default_value: c"100".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_perfil".as_ptr(),
                desc: c"Profile".as_ptr(),
                desc_categorized: c"Profile".as_ptr(),
                info: c"Handheld applies at once what a weak handheld device (RG40XX-H, muOS) needs: the timbre table instead of a SoundFont, reduced MIDI rate and voices, a smaller sound cache and, **when drawing happens on the processor**, 3D at 0.5x (320x240 scaled up to 640x480 at presentation), which measured 22% less real time, with a blockier image. With the GPU rasterizer the profile does not touch the resolution: there the separate option is in charge. While active, it ignores the individual options it covers (but not volume, fog or rasterizer, which stay on their own). The MIDI synthesizer changes when the content is reloaded; the rest applies without reloading.".as_ptr(),
                info_categorized: c"Handheld bundles the performance settings for weak handheld devices. Ignores the individual options it covers.".as_ptr(),
                category_key: c"sistema".as_ptr(),
                values: profile_values,
                default_value: c"padrao".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_soundfont_taxa".as_ptr(),
                desc: c"SoundFont rate".as_ptr(),
                desc_categorized: c"SoundFont rate".as_ptr(),
                info: c"The rate at which the MIDI music is synthesized from the sample bank. 44,100 Hz preserves the brightness of the .sf2 samples, which are recorded at that rate. 22,050 Hz costs half the time and memory, and is what suits a weak handheld. Applies from the next song on.".as_ptr(),
                info_categorized: c"44,100 Hz preserves the brightness; 22,050 Hz costs half. Applies from the next song on.".as_ptr(),
                category_key: c"audio".as_ptr(),
                values: rate_values,
                default_value: c"44100".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_midi_vozes".as_ptr(),
                desc: c"MIDI voices".as_ptr(),
                desc_categorized: c"Voices".as_ptr(),
                info: c"How many notes can sound at the same time in the sample bank. Fewer voices cost less processor and steal notes in dense passages, which sounds like notes disappearing. Applies from the next song on.".as_ptr(),
                info_categorized: c"Simultaneous notes in the bank. Fewer cost less and steal notes.".as_ptr(),
                category_key: c"audio".as_ptr(),
                values: voices_values,
                default_value: c"128".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_midi_efeitos".as_ptr(),
                desc: c"MIDI reverb and chorus".as_ptr(),
                desc_categorized: c"Reverb and chorus".as_ptr(),
                info: c"The reverb and chorus the MIDI music asks for, when it plays through the sample bank. How much effect each instrument gets comes from the music itself; when off, notes end dry. Applies from the next song on.".as_ptr(),
                info_categorized: c"The effects the MIDI music asks for. When off, notes end dry.".as_ptr(),
                category_key: c"audio".as_ptr(),
                values: on_off_values,
                default_value: c"enabled".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_cache_de_som_mb".as_ptr(),
                desc: c"Sound cache".as_ptr(),
                desc_categorized: c"Sound cache".as_ptr(),
                info: c"How much memory to keep for already decoded sound. Less memory makes the emulator forget music it played and synthesize it again when it comes back, which costs a pause; more memory avoids the pause and takes up RAM. Applies from the next eviction on.".as_ptr(),
                info_categorized: c"Memory for already decoded sound. Less costs pauses; more takes up RAM.".as_ptr(),
                category_key: c"audio".as_ptr(),
                values: cache_values,
                default_value: c"24".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_rasterizador".as_ptr(),
                desc: c"Rasterizer (restart)".as_ptr(),
                desc_categorized: c"Rasterizer (restart)".as_ptr(),
                info: c"What fills the 3D: Automatic uses the GPU when the frontend offers hardware rendering; Processor forces the software path. Use Processor when the image comes out wrong or black with a problematic video driver. Reload the game to apply.".as_ptr(),
                info_categorized: c"Automatic uses the GPU when there is hardware rendering; Processor forces software. Reload the game to apply.".as_ptr(),
                category_key: c"video".as_ptr(),
                values: ras_values,
                default_value: c"auto".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_resolucao_interna".as_ptr(),
                desc: c"3D internal resolution".as_ptr(),
                desc_categorized: c"Internal resolution".as_ptr(),
                info: c"The resolution the 3D is drawn at, per side. The frame delivered to the frontend stays 640x480, and shaders and aspect ratio do not change. Below 1x (0.5x, 0.25x) the drawing comes out smaller and is scaled up at presentation: it relieves the processor, and it is what suits a weak device; the image gets blockier. Above 1x it is supersampling: it smooths polygon edges, costs memory and fill, and only applies with the GPU rasterizer. **This option does not change the size of the image on screen**: the console delivers 640x480 and the frontend is what scales it; with integer scaling on in RetroArch (Integer Scale) it is only presented in multiples of 640x480, which on a 1080p screen gives 1280x960 with borders. To fill the screen, turn integer scaling off there. Applies right away.".as_ptr(),
                info_categorized: c"Below 1x relieves the processor; above 1x is supersampling and only applies on the GPU. The Handheld profile fixes this at 0.5x and overrides this option.".as_ptr(),
                category_key: c"video".as_ptr(),
                values: scale_values,
                default_value: c"1".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_antialias".as_ptr(),
                desc: c"Anti-aliasing".as_ptr(),
                desc_categorized: c"Anti-aliasing".as_ptr(),
                info: c"Samples per pixel in 3D filled by the GPU. Smooths polygon edges and costs fill. No effect on the processor rasterizer.".as_ptr(),
                info_categorized: c"Samples per pixel in GPU 3D. No effect on the processor.".as_ptr(),
                category_key: c"video".as_ptr(),
                values: sample_values,
                default_value: c"1".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_filtro_anisotropico".as_ptr(),
                desc: c"Anisotropic filtering".as_ptr(),
                desc_categorized: c"Anisotropic filtering".as_ptr(),
                info: c"Sharpness of textures seen at an angle, in 3D filled by the GPU. No effect on the processor rasterizer.".as_ptr(),
                info_categorized: c"Sharpness of textures seen at an angle. GPU only.".as_ptr(),
                category_key: c"video".as_ptr(),
                values: sample_values,
                default_value: c"1".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_neblina".as_ptr(),
                desc: c"Fog".as_ptr(),
                desc_categorized: c"Fog".as_ptr(),
                info: c"The fog the game asks for. Turning it off leaves distant scenery visible, which some prefer; it is the player's choice, not a fix. Applies right away.".as_ptr(),
                info_categorized: c"The fog the game asks for. Applies right away.".as_ptr(),
                category_key: c"video".as_ptr(),
                values: on_off_values,
                default_value: c"enabled".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_limite_fps".as_ptr(),
                desc: c"Speed limit".as_ptr(),
                desc_categorized: c"Speed limit".as_ptr(),
                info: c"Holds the game's logic against the real clock, as the console would. Use 60 FPS to keep light titles (RE4, Zeebo Extreme) from running too fast; 30 FPS keeps the normal logic speed and shows one of every two frames. Off unlocks boost, useful if you want to speed up games like Need for Speed. Applies right away.".as_ptr(),
                info_categorized: c"60 keeps games from running too fast; 30 keeps the logic and reduces frames; Off unlocks boost. Applies right away.".as_ptr(),
                category_key: c"sistema".as_ptr(),
                values: fps_limit_values,
                default_value: c"60".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_frameskip".as_ptr(),
                desc: c"Frame skipping".as_ptr(),
                desc_categorized: c"Frame skipping".as_ptr(),
                info: c"Skips the 3D drawing of some frames to relieve a weak processor, without changing the game's speed: the logic runs the same, only the drawing disappears for an instant. Fixed always skips the same proportion; Automatic only skips when the frontend warns that the audio is about to underrun. Applies right away.".as_ptr(),
                info_categorized: c"Skips drawing to relieve a weak processor, without changing the game's speed. Applies right away.".as_ptr(),
                category_key: c"video".as_ptr(),
                values: frameskip_values,
                default_value: c"desligado".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_descarte_de_tiles".as_ptr(),
                desc: c"Discard tiles (experimental)".as_ptr(),
                desc_categorized: c"Discard tiles (experimental)".as_ptr(),
                info: c"Tells the video driver that the frame's depth and stencil can be thrown away after it is presented. Pays off on tiled GPUs, like the handhelds' Mali, where it avoids writing those attachments back to memory; it does not measure on a desktop GPU. Off by default because a game that does not clear depth from one frame to the next relies on it; if garbage shows up in the image with this on, turn it off. Applies right away, and only has an effect with the GPU rasterizer.".as_ptr(),
                info_categorized: c"Only helps on tiled GPUs. Off by default because a game may rely on the previous frame's depth. Applies right away.".as_ptr(),
                category_key: c"video".as_ptr(),
                values: discard_values,
                default_value: c"desligado".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_log".as_ptr(),
                desc: c"Core log".as_ptr(),
                desc_categorized: c"Core log".as_ptr(),
                info: c"How much the core writes to RetroArch's log. The chosen level goes in and so does everything more severe: Warning shows only what went off the expected path, Info adds game startup, cache pruning and sound bank loading, and Debug shows the path of each decision (verbose, and slower). Applies right away, and the log goes to the file RetroArch is configured to use.".as_ptr(),
                info_categorized: c"How much the core writes to the log. The level goes in along with everything more severe. Applies right away.".as_ptr(),
                category_key: c"sistema".as_ptr(),
                values: log_values,
                default_value: c"aviso".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_dpad_to_analog_p1".as_ptr(),
                desc: c"D-pad on the stick axes (player 1)".as_ptr(),
                desc_categorized: c"D-pad on the axes (player 1)".as_ptr(),
                info: c"When On, player 1's d-pad also pushes the games' left stick: each direction writes the full travel to the axis, and releasing returns the axis to center. Meant for games that only listen to the axis and ignore the d-pad entirely. **Off by default** because a game that reads both channels moves two steps per press, and because one that reads changes reads the return to center as a step in the opposite direction; that was the defect that undid the two previous attempts. The buttons keep working: this option adds the axis, it does not swap the channel. It has no effect on the Boomerang port, whose axis comes from the motion sensor. **It is the inverse of RetroArch's Analog to Digital Type**, which reads the stick and presses the d-pad: here the d-pad is what is read, and the stick is what receives. The two together do not trip over each other: the real stick is read afterwards, and it is the one that counts. Applies right away.".as_ptr(),
                info_categorized: c"Player 1's d-pad also pushes the stick. For games that only read the axis. Off by default: a game that reads both channels moves two steps per press.".as_ptr(),
                category_key: c"controles".as_ptr(),
                values: mirror_values,
                default_value: c"disabled".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: c"zeebx_dpad_to_analog_p2".as_ptr(),
                desc: c"D-pad on the stick axes (player 2)".as_ptr(),
                desc_categorized: c"D-pad on the axes (player 2)".as_ptr(),
                info: c"The same as player 1, for the second port's controller. It is a separate option because there are two players and two controllers: turning it on for 1 does not force it on 2. Applies right away.".as_ptr(),
                info_categorized: c"The same as player 1, for the second port. Applies right away.".as_ptr(),
                category_key: c"controles".as_ptr(),
                values: mirror_values,
                default_value: c"disabled".as_ptr(),
            },
            RetroCoreOptionV2Definition {
                key: std::ptr::null(),
                desc: std::ptr::null(),
                desc_categorized: std::ptr::null(),
                info: std::ptr::null(),
                info_categorized: std::ptr::null(),
                category_key: std::ptr::null(),
                values: [RetroCoreOptionValue { value: std::ptr::null(), label: std::ptr::null() }; 128],
                default_value: std::ptr::null(),
            },
        ];

        let options_v2 = RetroCoreOptionsV2 {
            categories: CATEGORIES.as_ptr(),
            definitions: option_defs.as_ptr(),
        };

        unsafe {
            environ(
                ENV_SET_CORE_OPTIONS_V2,
                &options_v2 as *const _ as *mut c_void,
            );
        }
    } else {
        // Not `static` because the bank option carries the names read now. The frontend copies the
        // list during the call, as it does with the V2 definitions, which also live on the stack.
        //
        // A name with `|` or `;` would break this version's format, and is left out.
        let soundfonts: Vec<String> = banks_in_system_dir()
            .into_iter()
            .filter(|name| !name.contains(['|', ';']))
            .collect();
        let bank_value = leaked_c_text(&format!(
            "SoundFont bank (restart); {}",
            std::iter::once("auto".to_string()).chain(soundfonts).collect::<Vec<_>>().join("|")
        ));
        let variables: [RetroVariable; 20] = [
            RetroVariable {
                key: c"zeebx_midi_backend".as_ptr(),
                value: c"MIDI synthesizer (restart); auto|timbres|soundfont".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_soundfont".as_ptr(),
                value: bank_value,
            },
            RetroVariable {
                key: c"zeebx_volume".as_ptr(),
                value: c"Volume; 100|90|80|70|60|50|40|30|20|10|0".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_perfil".as_ptr(),
                value: c"Profile; padrao|portatil".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_soundfont_taxa".as_ptr(),
                value: c"SoundFont rate; 44100|22050".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_midi_vozes".as_ptr(),
                value: c"MIDI voices; 128|96|64|48".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_midi_efeitos".as_ptr(),
                value: c"MIDI reverb and chorus; enabled|disabled".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_cache_de_som_mb".as_ptr(),
                value: c"Sound cache (MiB); 24|48|16|8|4".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_rasterizador".as_ptr(),
                value: c"Rasterizer (restart); auto|software".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_resolucao_interna".as_ptr(),
                value: c"3D internal resolution; 1|0.5|0.25|2|3|4".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_antialias".as_ptr(),
                value: c"Anti-aliasing; 1|2|4|8".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_filtro_anisotropico".as_ptr(),
                value: c"Anisotropic filtering; 1|2|4|8".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_neblina".as_ptr(),
                value: c"Fog; enabled|disabled".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_limite_fps".as_ptr(),
                value: c"Speed limit; 60|30|desligado".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_frameskip".as_ptr(),
                value: c"Frame skipping; desligado|automatico|1|2|3|4|5|6".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_descarte_de_tiles".as_ptr(),
                value: c"Discard tiles (experimental); desligado|ligado".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_log".as_ptr(),
                value: c"Core log; desligado|fatal|erro|aviso|informacao|depuracao".as_ptr(),
            },
            // One per port: the console has two (`zeebx::input::PORTS`), and each player turns on
            // their own. See `mirror_key`, on the reading side.
            RetroVariable {
                key: c"zeebx_dpad_to_analog_p1".as_ptr(),
                value: c"D-pad on the stick axes (player 1); disabled|enabled".as_ptr(),
            },
            RetroVariable {
                key: c"zeebx_dpad_to_analog_p2".as_ptr(),
                value: c"D-pad on the stick axes (player 2); disabled|enabled".as_ptr(),
            },
            RetroVariable {
                key: std::ptr::null(),
                value: std::ptr::null(),
            },
        ];
        unsafe {
            environ(
                ENV_SET_VARIABLES,
                variables.as_ptr() as *mut c_void,
            );
        }
    }
}

/// The folder where the core looks for banks: `soundfonts`, inside the device, inside the system
/// folder. It is the same one the automatic search uses
/// ([`zeebx::audio::soundfont::default_dirs`]).
fn banks_dir(system: &Path) -> PathBuf {
    StoragePaths::for_frontend(system, Some(system))
        .device
        .join("soundfonts")
}

/// The names of the `.sf2` files in the banks folder, in order, for the `zeebx_soundfont` option.
/// Empty when the frontend does not give the system folder.
fn banks_in_system_dir() -> Vec<String> {
    let Some(system) = dir(ENV_GET_SYSTEM_DIRECTORY) else {
        return Vec::new();
    };
    zeebx::audio::soundfont::candidates_in(&[banks_dir(&system)])
        .into_iter()
        .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
        .collect()
}

/// A C string that lives until the end of the process.
///
/// The core options point at the texts; the fixed ones live in the binary, and the bank names read
/// from the folder need to live somewhere too. It is a few bytes, once per core load.
fn leaked_c_text(text: &str) -> *const c_char {
    let text = text.replace('\0', "");
    Box::leak(std::ffi::CString::new(text).unwrap_or_default().into_boxed_c_str()).as_ptr()
}

/// The bank chosen in the `zeebx_soundfont` option, inside the banks `dir`. `None` is
/// automatic.
fn bank_from_option(value: Option<&str>, dir: &Path) -> Option<PathBuf> {
    value
        .filter(|name| !name.is_empty() && *name != "auto")
        .map(|name| dir.join(name))
}

/// Queries the MIDI synthesis policy configured in the RetroArch frontend.
unsafe fn read_option_midi_backend() -> zeebx::audio::MidiBackend {
    let mut query = RetroVariable {
        key: c"zeebx_midi_backend".as_ptr(),
        value: std::ptr::null(),
    };
    // SAFETY: a call to the environ callback and a read of a valid C string delivered by the
    // frontend.
    let ok = unsafe {
        environ(ENV_GET_VARIABLE, &mut query as *mut _ as *mut c_void) && !query.value.is_null()
    };
    if ok {
        let val_str = unsafe { CStr::from_ptr(query.value) }.to_string_lossy();
        if let Ok(backend) = val_str.parse::<zeebx::audio::MidiBackend>() {
            return backend;
        }
    }
    zeebx::audio::MidiBackend::Auto
}

/// Reads a core option's value as text. `None` when the frontend does not have the key.
///
/// It exists so as not to repeat the same `unsafe` block for each option: the unsafe part is always
/// the same (building the query, calling the frontend, checking the pointer came back non-null) and
/// repeating it per key is how an `unsafe` stops being reviewed.
unsafe fn read_option(key_id: &CStr) -> Option<String> {
    let mut query = RetroVariable {
        key: key_id.as_ptr(),
        value: std::ptr::null(),
    };
    // SAFETY: a call to the environ callback and a read of a valid C string delivered by the
    // frontend.
    let ok = unsafe {
        environ(ENV_GET_VARIABLE, &mut query as *mut _ as *mut c_void)
            && !query.value.is_null()
    };
    if !ok {
        return None;
    }
    Some(unsafe { CStr::from_ptr(query.value) }.to_string_lossy().into_owned())
}

/// Reads the master volume chosen in the options, from 0.0 to 1.0.
///
/// Returns `None` when the key does not exist or is not a number: the frontend may be old, and a
/// corrupt value must not turn into silence without warning; the caller keeps what it had.
unsafe fn read_option_volume() -> Option<f32> {
    volume_from_text(&unsafe { read_option(c"zeebx_volume") }?)
}

/// Reads an integer from an option, clamped to the range the engine accepts.
fn number_from_text(text: &str, minimum: usize, maximum: usize) -> Option<usize> {
    let value: usize = text.trim().parse().ok()?;
    Some(value.clamp(minimum, maximum))
}

/// What `SET_AUDIO_BUFFER_STATUS_CALLBACK` reported last time.
///
/// Global because the frontend's callback has no way to return any context; it is just the
/// signature the C ABI allows. See [`ENV_SET_AUDIO_BUFFER_STATUS_CALLBACK`].
static AUDIO_UNDERRUN_LIKELY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `retro_audio_buffer_status_callback_t`: the frontend calls this **before** each `retro_run`,
/// saying whether its audio buffer is about to underrun.
///
/// It only keeps `underrun_likely`, and `libretro.h` itself says what to do with it: *"if true, the
/// core should try to skip a frame"*. The occupancy itself does not change the decision: the
/// frontend already did the math and decided that **now** is the time to skip.
unsafe extern "C" fn audio_buffer_status(active: bool, _occupancy: u32, underrun_likely: bool) {
    // With no audio in the frontend there is no buffer to protect. Keeping an old `true` in that
    // case would make Automatic skip drawing forever after the user turns audio off and on in the
    // menu.
    AUDIO_UNDERRUN_LIKELY.store(active && underrun_likely, std::sync::atomic::Ordering::Relaxed);
}

/// Asks the frontend to report on the audio buffer, and for more headroom in it so the report
/// arrives in time for the core to react.
///
/// **Only once**, not on every frame; see the `frameskip_callback_requested` field. Asking again
/// does not change the answer, and the documentation of `SET_MINIMUM_AUDIO_LATENCY` asks for
/// restraint.
fn request_audio_callback() -> bool {
    let cb = RetroAudioBufferStatusCallback {
        callback: Some(audio_buffer_status),
    };
    let accepted = unsafe {
        environ(
            ENV_SET_AUDIO_BUFFER_STATUS_CALLBACK,
            &cb as *const _ as *mut c_void,
        )
    };
    if accepted {
        log("Zeebx: automatic frameskip asked the frontend for the audio buffer report");
        // Six frames at 60 Hz: the range `libretro.h` itself recommends (six to eight) for the
        // underrun report to arrive with enough headroom for the core to react in time.
        let latency_ms: u32 = 96;
        unsafe {
            environ(
                ENV_SET_MINIMUM_AUDIO_LATENCY,
                &latency_ms as *const u32 as *mut c_void,
            );
        }
    } else {
        // **It degrades by itself, without breaking anything.** Without the callback,
        // `AUDIO_UNDERRUN_LIKELY` never changes from `false`, and Automatic mode behaves like Off:
        // it never skips. That is worse than working, but better than an old frontend keeping the
        // core from running.
        warning("Zeebx: the frontend does not accept the audio buffer report; automatic frameskip will not skip any frame");
    }
    accepted
}

/// Removes the callback and gives the frontend back the latency it already had.
///
/// This is not cosmetic cleanup: the callback pointer lives in the frontend. If it keeps pointing
/// into this `.so` after `retro_deinit`, a frontend that delivers audio late may call memory the
/// loader has already unloaded. And 96 ms only makes sense with Automatic on: leaving the requested
/// headroom after the user turns the mode off trades latency for nothing.
fn remove_audio_callback() {
    let empty = RetroAudioBufferStatusCallback { callback: None };
    unsafe {
        environ(
            ENV_SET_AUDIO_BUFFER_STATUS_CALLBACK,
            &empty as *const _ as *mut c_void,
        );
    }
    let fallback: u32 = 0;
    unsafe {
        environ(
            ENV_SET_MINIMUM_AUDIO_LATENCY,
            &fallback as *const u32 as *mut c_void,
        );
    }
    AUDIO_UNDERRUN_LIKELY.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the profile option's text asks for the Handheld profile.
///
/// Extracted because the check happens in two places (the session's birth, for the MIDI
/// synthesizer, and the hot reread, for the rest) and both had the same comparison written out
/// again. Two copies diverge over time; a function does not.
fn profile_is_handheld(text: Option<&str>) -> bool {
    text.is_some_and(|text| text.trim().eq_ignore_ascii_case("portatil"))
}

/// Reads an on/off switch. The value convention is `enabled`/`disabled`, the ecosystem's.
fn bool_from_text(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "enabled" | "ligado" | "on" | "true" | "1" => Some(true),
        "disabled" | "desligado" | "off" | "false" | "0" => Some(false),
        _ => None,
    }
}

/// Applies, all at once, **every** option that takes effect without recreating the session.
///
/// **All at once is the part that matters.** The frontend's notice (see [`options_changed`]) is
/// consumed on the first question: if each option were reread in its own `if`, the first would eat
/// the notice and the others would only change the next time the user fiddled with it, a defect
/// that shows up as "sometimes it doesn't take".
///
/// Each missing or corrupt option keeps what was already there, instead of going back to the
/// default: an old frontend that does not know the key must not undo the choice of whoever
/// configured it.
fn apply_hot_options(state: &mut Core) {
    // **The log first.** Everything the options below decide gets logged at the level the user
    // just chose, instead of the level being applied after the decisions.
    apply_log_level();
    // **The mode is read here; the decision to skip or not is per frame**, in `retro_run`. See
    // [`Frameskip`]: rereading the mode only when something changed is cheap, but the decision
    // itself (which frame to skip) needs a counter or the frontend's notice, and both apply on
    // every frame, not only when the user touches the menu.
    if let Some(limit) = unsafe { read_option(c"zeebx_limite_fps") }
        .as_deref()
        .and_then(FpsLimit::from_text)
    {
        if limit != state.fps_limit {
            // The 30 presentation rate must not inherit the previous limit's parity.
            state.skip = SkipCounter::default();
        }
        state.fps_limit = limit;
    }

    if let Some(mode) = unsafe { read_option(c"zeebx_frameskip") }
        .as_deref()
        .and_then(Frameskip::from_text)
    {
        let before = state.frameskip;
        if mode != before {
            // Changing the fixed ratio midway must not inherit the previous mode's phase: 1/2
            // turning into 1/7 on the fifth frame and skipping the very first one is a surprise
            // with no gain.
            state.skip = SkipCounter::default();
        }
        // Automatic's registration happens on the first `retro_run`, because
        // SET_MINIMUM_AUDIO_LATENCY is only valid inside that call. Here, which also runs in
        // retro_load_game, we only store the policy. Leaving Automatic can clean up right away.
        if mode != Frameskip::Auto && before == Frameskip::Auto && state.frameskip_callback_requested {
            remove_audio_callback();
            state.frameskip_callback_requested = false;
        }
        state.frameskip = mode;
    }
    if let Some(volume) = unsafe { read_option_volume() } {
        state.mixer.set_master(volume, false);
    }
    if let Some(fog) = unsafe { read_option(c"zeebx_neblina") }.as_deref().and_then(bool_from_text) {
        state.session.set_fog_allowed(fog);
    }
    if let Some(discard) =
        unsafe { read_option(c"zeebx_descarte_de_tiles") }.as_deref().and_then(bool_from_text)
    {
        state.session.set_tile_discard(discard);
    }
    // **The "Handheld" profile wins over the individual options it covers, when active.** Volume
    // and fog are left out on purpose: they are the player's taste, not processor or memory cost,
    // and the profile is about performance. The rasterizer is left out too: it only changes on
    // reload, and forcing the processor would take the GPU away from someone with a capable one;
    // the separate option remains the escape hatch for whoever needs it.
    let handheld_profile = profile_is_handheld(unsafe { read_option(c"zeebx_perfil") }.as_deref());

    // **One number, two mechanisms.** Below 1x the one that reduces is the processor rasterizer
    // (a smaller surface, scaled up at presentation); above 1x the one that enlarges is the GPU
    // one (supersampling). The value is handed to both, and each uses what concerns it.
    // **The Handheld profile brings the reduction built in.** On a weak device fill is what
    // dominates, and drawing at 320×240 measured 22% less real time (see
    // `docs/OPTIMIZING_V0.3.0.md`, section 28). Whoever dislikes the blockier image picks the
    // option by hand: the profile is about performance, and this is the performance item it was
    // missing.
    let scale_text = match handheld_profile {
        true => Some("0.5".to_string()),
        false => unsafe { read_option(c"zeebx_resolucao_interna") },
    };
    match scale_text.as_deref().map(str::trim) {
        Some("0.5") | Some("0,5") => {
            state.session.set_downscale(2);
            state.session.set_internal_resolution(1);
        }
        Some("0.25") | Some("0,25") => {
            state.session.set_downscale(4);
            state.session.set_internal_resolution(1);
        }
        other => {
            state.session.set_downscale(1);
            if let Some(scale) = other.and_then(|text| number_from_text(text, 1, 8)) {
                state.session.set_internal_resolution(scale);
            }
        }
    }

    // **Audio and memory apply to what comes next.** Music already synthesized does not change
    // rate and sound already cached does not shrink: these three apply from the next song and the
    // next eviction on, and that is why they do not ask for a restart; but that is also why the
    // effect is not immediate like the volume's, and the labels say so.
    let rate = if handheld_profile {
        Some(zeebx::audio::midi::RATE)
    } else {
        unsafe { read_option(c"zeebx_soundfont_taxa") }
            .as_deref()
            .and_then(|text| text.trim().parse::<u32>().ok())
    };
    if let Some(rate) = rate {
        zeebx::audio::soundfont::set_rate(rate);
    }
    let voices = if handheld_profile {
        Some(48)
    } else {
        unsafe { read_option(c"zeebx_midi_vozes") }
            .as_deref()
            .and_then(|text| number_from_text(text, 8, 256))
    };
    if let Some(voices) = voices {
        zeebx::audio::soundfont::set_voices(voices);
    }
    if let Some(effects) = unsafe { read_option(c"zeebx_midi_efeitos") }
        .as_deref()
        .and_then(bool_from_text)
    {
        zeebx::audio::soundfont::set_effects(effects);
    }
    let mib = if handheld_profile {
        Some(8)
    } else {
        unsafe { read_option(c"zeebx_cache_de_som_mb") }
            .as_deref()
            .and_then(|text| number_from_text(text, 1, 256))
    };
    if let Some(mib) = mib {
        zeebx::machine::set_sound_cache_cap(mib * 1024 * 1024);
    }

    // **Both enhancements go in the same call**, because the engine's API takes them together:
    // applying one alone would erase the other with its previous value.
    let (antialias, anisotropic) = if handheld_profile {
        (Some(1), Some(1))
    } else {
        (
            unsafe { read_option(c"zeebx_antialias") }
                .as_deref()
                .and_then(|text| number_from_text(text, 1, 16)),
            unsafe { read_option(c"zeebx_filtro_anisotropico") }
                .as_deref()
                .and_then(|text| number_from_text(text, 1, 16)),
        )
    };
    if antialias.is_some() || anisotropic.is_some() {
        state
            .session
            .set_enhancements(antialias.unwrap_or(1), anisotropic.unwrap_or(1));
    }
}

/// Converts the volume option's text (in percent) into the mixer's 0.0 to 1.0 factor.
///
/// It is separate from reading the frontend on purpose: the conversion is the part that can go
/// wrong (the value comes from a file the user edits by hand) and it is only testable if it does
/// not need a frontend callback to be called.
///
/// `None` for what is not a number, and **not** zero: a corrupt `.opt` must not turn into silence,
/// because silence looks like an emulator defect and not a configuration error.
fn volume_from_text(text: &str) -> Option<f32> {
    let percent: f32 = text.trim().parse().ok()?;
    if !percent.is_finite() {
        return None;
    }
    Some((percent / 100.0).clamp(0.0, 1.0))
}

/// Did the frontend touch any option since the last question?
///
/// See [`ENV_GET_VARIABLE_UPDATE`]. Asking this once per frame is cheap; rereading every key per
/// frame is not.
fn options_changed() -> bool {
    let mut changed = false;
    // SAFETY: the frontend writes a `bool` to the pointer handed over.
    unsafe {
        environ(
            ENV_GET_VARIABLE_UPDATE,
            &mut changed as *mut bool as *mut c_void,
        )
    };
    changed
}

/// `retro_api_version`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_api_version() -> u32 {
    API_VERSION
}

/// `retro_set_environment`: the frontend hands over the way back to the system.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_set_environment(callback: Option<EnvironmentFn>) {
    if let Ok(mut guard) = frontend().lock() {
        guard.environ = callback;
    }
    // SAFETY: from here on the environment is available to register what is needed.
    unsafe {
        register_controllers();
        register_buttons();
        register_core_options();
    }
}

/// `retro_set_video_refresh`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_set_video_refresh(callback: Option<VideoRefreshFn>) {
    if let Ok(mut guard) = frontend().lock() {
        guard.video = callback;
    }
}

/// `retro_set_audio_sample_batch`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_set_audio_sample_batch(callback: Option<AudioSampleBatchFn>) {
    if let Ok(mut guard) = frontend().lock() {
        guard.audio_batch = callback;
    }
}

/// `retro_set_input_poll`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_set_input_poll(callback: Option<InputPollFn>) {
    if let Ok(mut guard) = frontend().lock() {
        guard.input_poll = callback;
    }
}

/// `retro_set_input_state`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_set_input_state(callback: Option<InputStateFn>) {
    if let Ok(mut guard) = frontend().lock() {
        guard.input_state = callback;
    }
}

/// `retro_set_audio_sample`: the single-sample variant is not used; the core always delivers
/// batches.
#[unsafe(no_mangle)]
pub extern "C" fn retro_set_audio_sample(_callback: Option<unsafe extern "C" fn(i16, i16)>) {}

/// `retro_init`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_init() {}

/// Clears callbacks, the video context and the loaded content.
fn clear_frontend_state() {
    // Takes the Rust state out first, but only calls the frontend after releasing the mutex: the
    // frontend's callbacks may reenter the core.
    let had_audio = if let Ok(mut guard) = core().lock() {
        let had = guard
            .as_ref()
            .is_some_and(|CoreState(c)| c.frameskip_callback_requested);
        *guard = None;
        had
    } else {
        false
    };
    if had_audio {
        remove_audio_callback();
    }
    let keyboard = RetroKeyboardCallback { callback: None };
    unsafe {
        environ(
            ENV_SET_KEYBOARD_CALLBACK,
            &keyboard as *const RetroKeyboardCallback as *mut c_void,
        );
    }
    if let Ok(mut gpu) = GPU.lock() {
        *gpu = None;
    }
    if let Ok(mut offer) = GPU_OFFER.lock() {
        *offer = None;
    }
    CONTEXT_READY.store(false, std::sync::atomic::Ordering::Relaxed);
    GPU_LOST.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// `retro_deinit`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_deinit() {
    clear_frontend_state();
}

/// `retro_get_system_info`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_get_system_info(info: *mut RetroSystemInfo) {
    if info.is_null() {
        return;
    }
    // SAFETY: the frontend passed a valid pointer to fill in.
    unsafe {
        *info = RetroSystemInfo {
            library_name: c"Zeebx".as_ptr(),
            library_version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char,
            valid_extensions: c"mod|zip|7z".as_ptr(),
            need_fullpath: true,
            block_extract: true,
        };
    }
}

/// `retro_get_system_av_info`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_get_system_av_info(info: *mut RetroSystemAvInfo) {
    if info.is_null() {
        return;
    }
    // **A phone game is its phone's screen**, not the console's: Devil May Cry is 240×400, 3:5.
    // The frontend asks for this after `retro_load_game`, so the session already knows the game.
    // See `Session::display_screen`.
    let (width, height) = core()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|CoreState(state)| state.session.display_size()))
        .unwrap_or((640, 480));
    // SAFETY: the frontend passed a valid pointer to fill in.
    unsafe {
        *info = RetroSystemAvInfo {
            geometry: RetroGameGeometry {
                base_width: width,
                base_height: height,
                max_width: 640,
                max_height: 480,
                aspect_ratio: width as f32 / height as f32,
            },
            timing: RetroSystemTiming {
                fps: 60.0,
                sample_rate: SAMPLE_RATE as f64,
            },
        };
    }
}

/// `retro_load_game`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_load_game(game: *const RetroGameInfo) -> bool {
    if game.is_null() {
        return false;
    }
    // SAFETY: pointer validated and read only within the call.
    let path = unsafe { (*game).path };
    if path.is_null() {
        return false;
    }
    // SAFETY: UTF-8 path promised by the ABI.
    let path = unsafe { CStr::from_ptr(path) }
        .to_string_lossy()
        .into_owned();
    let Some(save_dir) = dir(ENV_GET_SAVE_DIRECTORY) else {
        log_with_level(3, "Zeebx: the frontend did not provide a save directory; refusing to load.");
        return false;
    };
    let system = dir(ENV_GET_SYSTEM_DIRECTORY);
    // The system folder keeps what belongs to the machine and what is disposable: the `fs:/` NAND
    // and the cache of extracted content. The title's saves stay in the saves directory, which is
    // what the frontend syncs. It is the same split PPSSPP makes between `flash0` and the memory
    // stick.
    let storage = StoragePaths::for_frontend(&save_dir, system.as_deref());
    if let Err(error) = storage.create_dirs() {
        log(&format!(
            "Zeebx: no permission on {} / {}: {error}",
            storage.saves.display(),
            storage.cache.display()
        ));
        return false;
    }
    log(&format!(
        "Zeebx: saves in {}, system in {}",
        storage.saves.display(),
        storage.cache.display()
    ));
    // Keyboard: the Z-Wheel navigates through `AVK_*`, which the RetroPad does not produce.
    static KEYBOARD: RetroKeyboardCallback = RetroKeyboardCallback {
        callback: Some(key_received),
    };
    unsafe {
        environ(
            ENV_SET_KEYBOARD_CALLBACK,
            &KEYBOARD as *const RetroKeyboardCallback as *mut c_void,
        );
    }
    // The video format is negotiated before running: without it there is no way to deliver a frame.
    let mut format = PIXEL_FORMAT_RGB565;
    let target = &mut format as *mut u32 as *mut c_void;
    if !unsafe { environ(ENV_SET_PIXEL_FORMAT, target) } {
        log_with_level(3, "Zeebx: the frontend does not accept RGB565.");
        return false;
    }
    // One call per frame instead of twelve, when the frontend delivers the mask.
    let bitmasks = unsafe { environ(ENV_GET_INPUT_BITMASKS, std::ptr::null_mut()) };
    log(&format!(
        "Zeebx: buttons through {}",
        match bitmasks {
            true => "bit mask",
            false => "individual queries",
        }
    ));
    // The repeated frame may go as null: it saves a 600 KB copy per frame and avoids keeping the
    // frontend busy with work that changes nothing on screen.
    let mut accepts_dupe = false;
    let dupe_target = &mut accepts_dupe as *mut bool as *mut c_void;
    let accepts_dupe = unsafe { environ(ENV_GET_CAN_DUPE, dupe_target) } && accepts_dupe;
    let ports = [Some(Device::Controller), None];
    let result = unsafe { load(&path, &storage, ports, PathBuf::from(&path)) };
    match result {
        Ok(mut create) => {
            create.bitmasks = bitmasks;
            create.accepts_dupe = accepts_dupe;
            create.last_clock_ms = create.session.clock_ms();
            if let Ok(mut guard) = core().lock() {
                *guard = Some(CoreState(create));
            }
            true
        }
        Err(error) => {
            log_with_level(3, &format!("Zeebx: could not open {path}: {error}"));
            false
        }
    }
}

/// Builds the game state. Separate from the export to keep the ABI's `unsafe` in a single place.
unsafe fn load(
    content_path: &str,
    storage: &StoragePaths,
    ports: [Option<Device>; zeebx::input::PORTS],
    path: PathBuf,
) -> Result<Core, StartError> {
    // **The order matters: the library and the font come before the session.**
    //
    // The font is read when the machine is built: it goes into the engine's `font`. Installing it
    // afterwards left the whole session without a font, and the game drew no text at all: that is
    // what made the Double Dragon screen stay white and empty, because what it draws there is the
    // message "Memory is insufficient. Please delete some files." on a white background.
    let (dir, games) = library(content_path);
    // **The local library's catalog, which is what the Z-Wheel reads to build the grid.** The
    // wheel does not read the ROM folder: it reads the profile's `tt_game_info`, and what links
    // one to the other is the `catalog.json` the interface writes (`library::sync_catalog`). The
    // core enumerated the games for the shell and **did not feed the catalog**; measured in the
    // windowless harness on 2026-09-22: without the catalog the wheel does not build the grid and
    // no key has anything to move; with it, the grid shows up drawn and confirming produces the
    // open request. It is the same path RetroArch uses on the device, and that is why it applies
    // here and not only in the scan.
    if !games.is_empty()
        && let Some(dir) = &dir
    {
        let _ = zeebx::library::sync_catalog(&zeebx::library::scan(dir));
    }
    let installed = installed_from_library(&games);
    #[cfg(test)]
    GAMES_SEEN.store(games.len() as u32, std::sync::atomic::Ordering::Relaxed);
    let source = prepare_system_font(storage, &games);
    match &source {
        Some(location) => log(&format!("Zeebx: system font in {}", location.display())),
        None => warning(&format!(
            "No tectoy.ttf in the collection ({} games): system text will not be drawn",
            games.len()
        )),
    }
    // **Where the sample bank should go, stated in the log.** The search is by directory and the
    // folder comes from the system root the frontend handed over: without this line, whoever
    // installs the core has no way to know the path, and "the bank does not work" is
    // indistinguishable from "the file is in the wrong place".
    zeebx::audio::soundfont::set_soundfont(bank_from_option(
        unsafe { read_option(c"zeebx_soundfont") }.as_deref(),
        &storage.device.join("soundfonts"),
    ));
    log(&zeebx::audio::soundfont::report(&storage.device));

    // **The profile comes before the options it replaces.** "Handheld" exists because ten loose
    // knobs are not what a handheld device needs: it is a single setting that bundles what the
    // investigation on weak ARM (RG40XX-H) measured as costing the most: a heavy synthesizer, a
    // sample bank at a high rate, many voices, a large cache, supersampling.
    let handheld_profile = profile_is_handheld(unsafe { read_option(c"zeebx_perfil") }.as_deref());
    let midi_backend = if handheld_profile {
        zeebx::audio::MidiBackend::Wavetable
    } else {
        unsafe { read_option_midi_backend() }
    };
    log(&format!("Zeebx: MIDI synthesizer selected: {:?}{}", midi_backend, if handheld_profile { " (Handheld profile)" } else { "" }));

    // **Asks the frontend for the GPU context, if it has one.** The frontend is the one that
    // accepts; we only use it later, when `context_reset` arrives. A refusal here changes nothing:
    // the software session is already born and it is what runs until proven otherwise.
    //
    // **Except when the user asked for software.** This is the only escape for a GL driver that
    // accepts the context and draws wrong, or does not draw: in that case the automatic fallback
    // does not trigger, because from the core's point of view nothing failed. Without the option,
    // the only remedy is to change the frontend or the device.
    let rasterizer = unsafe { read_option(c"zeebx_rasterizador") }.unwrap_or_default();
    if rasterizer.trim().eq_ignore_ascii_case("software") {
        log("Zeebx: rasterizer pinned to the processor by the core option; no GPU context will be requested");
    } else if cfg!(target_os = "vita") {
        // The GPU rasterizer asks for OpenGL ES 3 (`#version 300 es`, VAO, FBO). The Vita's
        // RetroArch draws with vitaGL, which does not deliver that profile: asking for the context
        // would be refused, or accepted by a driver that does not deliver. The processor's RGB565
        // is the path the frontend presents.
        log("Zeebx: on the PlayStation Vita drawing stays on the processor; the frontend there does not deliver OpenGL ES 3");
    } else {
        request_gpu_context();
    }

    let mut session = Session::start_software_with_storage_installed_policy(
        std::path::Path::new(content_path),
        ports,
        ZWheel::default(),
        storage,
        &installed,
        midi_backend,
    )?;
    let mixer = session.record_audio(SAMPLE_RATE);
    // **Native aspect ratio, always.** The core delivers the console's frame at 640×480 without
    // stretching: whoever tunes shaders needs a predictable source, and upscaling is the
    // frontend's job. The internal resolution stopped being fixed here because it became an
    // option; see [`apply_hot_options`], which runs right after the state exists.
    session.set_aspect_ratio(None);
    if !games.is_empty() {
        session.set_installed_applets(
            games
                .iter()
                .filter_map(|(class_id, content_path)| {
                    Some((*class_id, zeebx::library::module_id(content_path)?))
                }),
        );
        log(&format!(
            "Zeebx: {} game(s) installed from {}",
            games.len(),
            dir.as_deref().unwrap_or(std::path::Path::new(".")).display()
        ));
    }
    // **Serial capture, when requested.** It is the same instrument as the scan's
    // (`ZEEBX_ROM_SERIAL`) and `run`'s (`--serial`): created classes, opened databases, SQL,
    // widget properties and the widget tree of the first key. Without it, whoever is on the device
    // sees the game and does not see what the applet does inside, and it is with it that we
    // measured, on the Z-Wheel, that the core's path reacts to the key but asks `class_id = -1`
    // (0 rows) where the scan resolves the focus (`class_id = 17359702`, Alien Breaker) and
    // requests the open.
    // The per-class accessor census, the same as the scan's, and it is **opt-in** there for the
    // same reasons: it adds a section to the report, and the report goes into the baseline.
    if zeebx::config::env_var("ZEEBX_CORE_SELECTORS").is_ok() {
        session.enable_widget_census();
    }
    if let Ok(content_path) = std::env::var("ZEEBX_CORE_SERIAL") {
        match session.enable_serial(std::path::Path::new(&content_path)) {
            Ok(()) => log(&format!("Zeebx: serial capture in {content_path}")),
            Err(error) => warning(&format!("Zeebx: could not open the serial capture: {error}")),
        }
    }
    // The Z-Wheel is the shell: when it is the content, keeping the path is what allows going back
    // to it after a game ends, which is what the console does.
    let z_wheel = match session.class_id() == zeebx::session::Z_WHEEL {
        true => Some(path.clone()),
        false => None,
    };

    let mut core = Core {
        gpu_enabled: false,
        frameskip: Frameskip::Off,
        skip: SkipCounter::default(),
        frameskip_callback_requested: false,
        frameskip_pixel_read_warned: false,
        fps_limit: FpsLimit::Sixty,
        fps_limit_doubles: false,
        session,
        mixer,
        ports,
        frame: Vec::new(),
        audio: Vec::new(),
        path,
        storage: storage.clone(),
        games,
        z_wheel,
        opened_via_z_wheel: false,
        bitmasks: false,
        accepts_dupe: false,
        gl_frames_before: 0,
        last_signature: None,
        last_clock_ms: 0,
        #[cfg(target_os = "vita")]
        last_real_audio: std::time::Instant::now(),
        pending_audio: Vec::new(),
        mix_buf: Vec::new(),
        warned_size: false,
        midi_backend,
        prev_select: false,
        prev_pad: [Pad::default(); zeebx::input::PORTS],
        frames_after_stop: 0,
        stopped: false,
    };
    // The options apply from the first frame. It is the **same** function as the hot path on
    // purpose: two copies of the application diverge over time, and the one that runs less is the
    // one that goes wrong without anyone seeing.
    apply_log_level();
    apply_hot_options(&mut core);
    Ok(core)
}

/// The game library next to the content: `(dir, [(ClassID, what to load)])`.
///
/// It is what answers the shell's launch request: the Z-Wheel asks for a class and here we know
/// which file serves it. Without it the wheel opens empty, because it enumerates the installed ones
/// and finds none. Deduplicating by ClassID avoids listing twice whoever has the `.zip` **and** an
/// extracted copy.
fn library(content_path: &str) -> (Option<PathBuf>, Vec<(u32, PathBuf)>) {
    let dir = std::path::Path::new(content_path)
        .parent()
        .map(std::path::Path::to_path_buf);
    let mut games: Vec<(u32, PathBuf)> = Vec::new();
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for game in dir.as_deref().map(zeebx::library::scan).unwrap_or_default() {
        let Some(class_id) = game.clsid else {
            continue;
        };
        if seen.insert(class_id) {
            games.push((class_id, game.path));
        }
    }
    (dir, games)
}

/// Converts the discovered library into the format the shell enumerates at boot.
fn installed_from_library(games: &[(u32, PathBuf)]) -> Vec<(u32, String)> {
    games
        .iter()
        .filter_map(|(class_id, path)| {
            Some((*class_id, zeebx::library::module_id(path)?))
        })
        .collect()
}

/// Installs the system font at the device's root, if it is not there yet.
///
/// It comes from the cache when the Z-Wheel has already been extracted and, when it has not, from
/// the compressed package itself, extracting **only** the file, because materializing the whole
/// package is not worth it for 190 KB.
fn prepare_system_font(storage: &StoragePaths, games: &[(u32, PathBuf)]) -> Option<PathBuf> {
    zeebx::loader::archive::system_font_in(&storage.cache, &storage.device).or_else(|| {
        let candidates: Vec<&PathBuf> = games
            .iter()
            .filter(|(class_id, _)| *class_id == zeebx::session::Z_WHEEL)
            .map(|(_, path)| path)
            .collect();
        let package = candidates
            .iter()
            .find(|path| zeebx::loader::archive::is_packed(path))
            .or_else(|| candidates.first())?;
        zeebx::loader::archive::install_package_font(package, &storage.device)
    })
}

/// Switches the running applet within the same core.
///
/// It is what the console does when the Z-Wheel opens a game and when the game closes: the shell
/// stays the shell. The frontend does not take part: for it, `retro_run` just returned another
/// frame.
fn switch_to(state: &mut Core, path: &Path, opened_via_z_wheel: bool) -> Result<(), StartError> {
    // **If drawing already happens on the GPU, the new session is born there too**: the Z-Wheel
    // opening a game, the game going back to it. Without this the first switch would send drawing
    // back to the processor, and the symptom would be "hardware rendering works until the first
    // game".
    let installed = installed_from_library(&state.games);
    let mut session = match gpu() {
        Some(context) => Session::start_with_storage_installed_policy(
            path,
            state.ports,
            None,
            true,
            Some(context),
            ZWheel::default(),
            &state.storage,
            &installed,
            state.midi_backend,
        )?,
        None => Session::start_software_with_storage_installed_policy(
            path,
            state.ports,
            ZWheel::default(),
            &state.storage,
            &installed,
            state.midi_backend,
        )?,
    };
    let mixer = session.record_audio(SAMPLE_RATE);
    session.set_aspect_ratio(None);
    state.session = session;
    state.mixer = mixer;
    // The options apply from the first frame, and not only after the user touches the menu again.
    // It is the **same** function as the hot path: two copies of the application diverge over
    // time, and the one that runs less is the one that goes wrong without anyone seeing.
    apply_hot_options(state);
    state.path = path.to_path_buf();
    state.opened_via_z_wheel = opened_via_z_wheel;
    state.stopped = false;
    state.frames_after_stop = 0;
    state.last_signature = None;
    state.prev_select = false;
    state.last_clock_ms = state.session.clock_ms();
    #[cfg(target_os = "vita")]
    {
        state.last_real_audio = std::time::Instant::now();
    }
    Ok(())
}

/// `retro_run`: one virtual frame, one video frame and the corresponding audio.
#[unsafe(no_mangle)]
pub extern "C" fn retro_run() {
    // The buffers leave the state before the calls to the frontend: no core lock stays held while
    // the frontend runs, and that is what keeps a notice from it ("disk full, save anyway?") from
    // blocking the emulator.
    let (frame, audio, width, height, duplicate, borrowed, on_gpu, video_pitch) = {
        let Ok(mut guard) = core().lock() else {
            return;
        };
        let Some(CoreState(state)) = guard.as_mut() else {
            return;
        };

        // **Who is running now**, so the Z-Wheel cycle test can tell whether it is the wheel or
        // the game it opened. See [`CURRENT_CLASS`].
        // The two test instruments, and the annotation goes **on each line**: a `#[cfg(test)]` only
        // applies to the next item, and without it here CI's `cargo build` breaks with "cannot
        // find value CLOCK in this scope" while the local `cargo test` passes, which is exactly
        // what happened.
        #[cfg(test)]
        CURRENT_CLASS.store(state.session.class_id(), std::sync::atomic::Ordering::Relaxed);
        #[cfg(test)]
        CLOCK.store(state.session.clock_ms(), std::sync::atomic::Ordering::Relaxed);
        #[cfg(test)]
        INSTRUCTIONS.store(state.session.instructions(), std::sync::atomic::Ordering::Relaxed);
        // **The options that can be applied on the fly.** See [`options_changed`]: the frontend
        // announces once, and only then is it worth rereading, and the reread handles them all
        // together, because the notice is consumed on the first question (see
        // [`apply_hot_options`]). The MIDI synthesizer and the rasterizer do **not** go in here on
        // purpose: both are chosen when the session is born, and that is why their labels say
        // "(restart)".
        if options_changed() {
            apply_hot_options(state);
        }
        // The safe point to talk to the frontend: inside `retro_run`, where the ABI expects to be
        // called. See [`dump_log`].
        dump_log();
        // **The skip decision is per frame, and applies on every frame**, unlike the mode, which
        // only changes when the user touches the option. A `Fixed(n)` that only decided when the
        // option changes would skip (or not) forever from the first reading on, and not on every n
        // of n+1 frames. Inside retro_run, as SET_MINIMUM_AUDIO_LATENCY requires.
        if state.frameskip == Frameskip::Auto && !state.frameskip_callback_requested {
            state.frameskip_callback_requested = request_audio_callback();
        }
        if state.session.did_read_pixels() && !state.frameskip_pixel_read_warned {
            warning("Zeebx: rasterization frameskip was turned off for this game because it uses glReadPixels");
            state.frameskip_pixel_read_warned = true;
        }
        // `libretro.h` itself says what to do with the audio notice: it **is** Automatic's
        // decision, not a hint. And 30 FPS **does not slow down the logic**: the brake stays at
        // 1x, and only presentation duplicates one frame out of every two; see
        // [`FpsLimit::Thirty`].
        let pace = Pace::from_choices(state.fps_limit, state.frameskip);
        let surplus = AUDIO_UNDERRUN_LIKELY.load(std::sync::atomic::Ordering::Relaxed);
        let skip = state.skip.decide(&pace, surplus);
        state.fps_limit_doubles = skip.half;
        state
            .session
            .set_skip_draw(skip.any() && !state.session.did_read_pixels());
        // **The current GPU, and the current session.** The frontend's notice (context lost, or
        // context rebuilt) arrives in the middle of the frame and becomes a single mark
        // (`GPU_LOST`, see [`context_lost`]); what is done with it is this, **before**
        // [`enable_gpu`], because the live session keeps the previous GPU's rasterizer: going on
        // drawing through it would call GL functions that no longer exist.
        //
        // `gpu_enabled` is what says the session is on the GPU, and it is what gets reset: the new
        // session is born **on demand**, in `enable_gpu` right below, once the context is back.
        // With no new context, it goes back to the processor, which is the same path as an
        // activation failure.
        //
        // **The GPU also comes in on the first frame, and that is why it is here.** The GL context
        // only exists after the frontend calls `context_reset`, which happens after
        // `retro_load_game`: this is the first place where it can be ready. Recreating the session
        // costs a restart nobody sees: no frame has been delivered yet.
        let lost = GPU_LOST.swap(false, std::sync::atomic::Ordering::Relaxed);
        let was_on_gpu = lost && state.gpu_enabled;
        if lost {
            state.gpu_enabled = false;
        }
        enable_gpu(state);
        if was_on_gpu && !state.gpu_enabled {
            let before = state.path.clone();
            if let Err(error) = switch_to(state, &before, false) {
                warning(&format!(
                    "Zeebx: the GPU context was lost and it could not reopen even on the processor: {error}"
                ));
            } else {
                warning("Zeebx: the frontend switched the video context; continuing on the processor");
            }
        }
        // In GPU mode, the drawing target is the framebuffer the frontend indicates **on each
        // frame**: it may change.
        if let Some(get_framebuffer) = GPU_OFFER
            .lock()
            .ok()
            .and_then(|offer| offer.as_ref().and_then(|o| o.get_current_framebuffer))
            && gpu().is_some()
        {
            let fbo = unsafe { get_framebuffer() };
            state.session.draw_to_fbo(Some(fbo));
        }
        // Keys the frontend delivered since the last frame. The callback only queues; here they go
        // into the guest, on the core's normal thread.
        if let Ok(mut queue) = key_events().lock() {
            while let Some((avk, down)) = queue.pop_front() {
                state.session.set_key(avk, down);
            }
        }
        // The shell asked for another applet: the Z-Wheel chose a game, or the game asked to go
        // back. The request lives **inside** the engine (`Machine::pending_launch`), and the
        // desktop UI already serves it this way; here it switches session without the frontend
        // knowing.
        if let Some(class_id) = state.session.take_launch_request() {
            // A test instrument, and only that: the core's log goes out through the frontend's
            // callback, which is **variadic** and therefore cannot be implemented in a test in
            // stable Rust. Here the test observes what the frontend's interface would show as
            // text.
            #[cfg(test)]
            LAST_OPENED.store(class_id, std::sync::atomic::Ordering::Relaxed);
            let target = state
                .games
                .iter()
                .find(|(c, _)| *c == class_id)
                .map(|(_, path)| path.clone());
            match target {
                Some(path) => {
                    let is_z_wheel = class_id == zeebx::session::Z_WHEEL;
                    match switch_to(state, &path, !is_z_wheel) {
                        Ok(()) => log(&format!(
                            "Zeebx: the shell asked for {class_id:#010x}; opening {}",
                            path.display()
                        )),
                        Err(error) => log(&format!(
                            "Zeebx: the shell asked for {class_id:#010x} and it could not be opened: {error}"
                        )),
                    }
                }
                None => warning(&format!(
                    "The shell asked for {class_id:#010x}, which is not in the games folder"
                )),
            }
        }
        // Shortcut: the RetroPad's Select counts as `AVK_CLR`, which is the console's "back".
        // Without it the Z-Wheel gets stuck at startup for whoever has no keyboard mapped in the
        // frontend.
        let select = read_select(0);
        if select != state.prev_select {
            state.prev_select = select;
            state.session.set_key(zeebx::input::avk::CLR, select);
        }
        // Input first: the guest reads the controller inside the frame about to run.
        for port in 0..zeebx::input::PORTS {
            if state.ports[port].is_none() {
                continue;
            }
            let pad = read_pad(port as u32, state.bitmasks);
            state.session.set_port_pad(port, pad);
            // **The controller becomes a console key, as in the window.** `controller_key_events`
            // exists for that and has always been used by the desktop and the window; the core
            // **did not call it**, so whoever reads BREW keys got nothing from RetroArch. Measured
            // on 2026-09-22 with the Z-Wheel, 2600 frames and the button script: without this
            // translation, the wheel animates (223 distinct images) and asks for no open at all;
            // with it, the grid opens and the request goes out. It is the difference between "the
            // controller reaches the guest" and "the controller reaches the applet the way it
            // reads it".
            for (avk, pressed) in
                zeebx::input::controller_key_events(&state.prev_pad[port], &pad)
            {
                state.session.set_key(avk, pressed);
            }
            state.prev_pad[port] = pad;
        }
        // **The intermediate screens of an `Update` inside a callback go through as in the
        // window.** The machine keeps one screen per `IDISPLAY_Update` the guest calls inside the
        // callback (the Z-Wheel transition slides the screen in a synchronous loop, some two
        // hundred times), and the frontend is what shows them, one per frame, without advancing
        // the clock. The window consumed them; the core did **not**, and with the queue full
        // `advance_once` returned "presented" forever, without running the guest: no API
        // boundary, no key drained from the queue, and the applet stopped. Measured: the Z-Wheel
        // froze after confirming (virtual clock stuck at 37,012 ms) and only one key got through.
        if !state.session.show_intermediate_frame()
            && let Step::Stopped = state
                .session
                .run_frame(state.fps_limit.limit_speed())
        {
            if !state.stopped {
                state.stopped = true;
                let clock = state.session.clock_ms();
                let reason = state
                    .session
                    .stopped_reason()
                    .unwrap_or_else(|| "no reason reported".to_string());
                log(&format!("Zeebx: stopped at {clock} virtual ms: {reason}"));
                // The game's own log is what usually names the reason for the stop; the last lines
                // go to the frontend, which is where someone will look.
                let report = state.session.log();
                for line in report.iter().rev().take(6).rev() {
                    log(&format!("Zeebx:   {line}"));
                }
                let memory = state.session.memory();
                log(&format!(
                    "Zeebx:   heap {} bytes, {} objects",
                    memory.0, memory.1
                ));
                // On the console, quitting a game hands control back to the Z-Wheel, which is
                // another installed applet. Here the request is recorded, but **the ABI does not
                // let the core ask the frontend for other content**: either the core would load the
                // Z-Wheel on its own, or the frontend ends the content. See the plan, outcome
                // section.
                if let Some(class_id) = state.session.take_launch_request() {
                    log(&format!(
                        "Zeebx: the shell asked to open {class_id:#010x}; switching content inside the core does not exist yet"
                    ));
                }
                state.frames_after_stop = 0;
            }
        }
        // Before the frontend presents: it draws the FBO in the same context, and the engine's VAO
        // left bound broke that drawing on GLES. See `Rasterizer::hand_back_to_frontend`.
        if gpu().is_some() {
            state.session.hand_back_to_frontend();
        }
        // Video: the console's framebuffer, in the negotiated format — only the phone's area of it
        // for a phone game.
        let screen = state.session.display_screen();
        let (width, height) = (screen.width(), screen.height());
        // **Did the GPU draw on this frame?** A game that only draws 2D (Turma da Mônica and
        // Zenonia draw through `IDisplay`/`IBitmap` and never swap a GPU buffer) has nothing in the
        // FBO, and the frontend would present a black screen in both frontends.
        //
        // The engine's counter is **cumulative**, and the GPU may have drawn once at startup and
        // never again: what counts is the difference since the previous frame. When it did not
        // draw, what exists is the processor's frame, and that is what gets delivered.
        let gl_now = state.session.gpu_frames();
        let drew_on_gpu = gl_now != state.gl_frames_before;
        state.gl_frames_before = gl_now;
        // On the GPU path the frontend presents the FBO it got in the callback and ignores the
        // pixel pointer. Do not copy 600 KiB or compute a CPU signature in that case: besides being
        // useless, it competed with the Mali for the same weak CPU we want to leave to the guest.
        let on_gpu = gpu().is_some() && drew_on_gpu;
        // The console is 640×480 (a phone game, its phone's size), and that is the frame the
        // shader expects to receive. A different size is warned about once, instead of showing up
        // as a skewed image with no explanation.
        if !state.warned_size && (width, height) != state.session.display_size() {
            state.warned_size = true;
            log(&format!(
                "Zeebx: frame {width}x{height}, outside the console's 640x480"
            ));
        }
        let mut frame_buf = std::mem::take(&mut state.frame);
        // 30 FPS is a presentation cadence, not just a 3D optimization: on the hidden frame we
        // keep the previous bytes. If the frontend accepts dupes, we deliver a null pointer; if
        // not, we deliver the same bytes again; in both cases the image really is 30.
        // **The borrowed buffer, when the frontend offers it.** Only on the software path: with
        // drawing on the GPU the FBO is what presents, and there is no CPU frame to write.
        let borrowed = match on_gpu {
            true => None,
            false => request_frontend_buffer(width, height),
        };
        let video_pitch = borrowed
            .as_ref()
            .map(|(_, step)| *step)
            .unwrap_or(width as usize * 2);
        let duplicate = if on_gpu {
            false
        } else if state.fps_limit_doubles {
            state.accepts_dupe
        } else {
            match &borrowed {
                // Writes where the frame will end up: no intermediate vector and no copy.
                Some((data, step)) => write_frame(screen, *data, *step),
                None => {
                    screen.write_rgb565_into(&mut frame_buf);
                }
            }
            let signature = screen.signature();
            let same = state.accepts_dupe && state.last_signature == Some(signature);
            state.last_signature = Some(signature);
            same
        };
        // Audio: **the time comes from the virtual clock**, not from a fixed number. A game that
        // goes through two virtual frames between two calls needs to deliver twice the samples,
        // otherwise the sound lags behind the image and the frontend stutters trying to keep up.
        //
        // On the Vita the frame does not fit in that clock. The speaker consumes 44100 samples
        // per second of wall clock; delivering only the virtual amount leaves a gap, and the music
        // goes at the game's pace. The 150 ms cap fits in a player's short ring: a bigger
        // spike would be accepted by the ABI and discarded there, and the track would skip.
        let now_ms = state.session.clock_ms();
        let elapsed = u64::from(now_ms.wrapping_sub(state.last_clock_ms));
        state.last_clock_ms = now_ms;
        let virt = (elapsed.min(1000) * u64::from(SAMPLE_RATE) / 1000) as usize;
        // `cfg!` does not hide the field: the desktop core also compiles this file,
        // and `last_real_audio` only exists on the Vita.
        #[cfg(target_os = "vita")]
        let due = {
            let real = state.last_real_audio.elapsed().as_millis() as u64;
            state.last_real_audio = std::time::Instant::now();
            let real_n = (real.min(150) * u64::from(SAMPLE_RATE) / 1000) as usize;
            real_n.max(virt)
        };
        #[cfg(not(target_os = "vita"))]
        let due = virt;
        let mut sound = std::mem::take(&mut state.audio);
        sound.clear();
        // What the frontend did not accept last time goes first, so no piece disappears.
        sound.append(&mut state.pending_audio);
        let mut mix = std::mem::take(&mut state.mix_buf);
        state.mixer.render_into(due, &mut mix);
        for sample in mix.iter().copied() {
            sound.push((sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16);
        }
        state.mix_buf = mix;
        (
            frame_buf,
            sound,
            width,
            height,
            duplicate,
            borrowed,
            on_gpu,
            video_pitch,
        )
    };
    let front = callbacks();
    if let Some(video) = front.video {
        let (pointer, _) = match (on_gpu, duplicate) {
            // **In GPU mode the frame is already in the frontend's framebuffer**: delivering pixels
            // here would be a lie, and `libretro` has a sentinel to say exactly that.
            (true, _) => (HW_FRAME_BUFFER_VALID as *const c_void, ()),
            // A null frame says "repeat the previous one", which is what the ABI offers for a still
            // screen.
            (false, true) => (std::ptr::null(), ()),
            // **The borrowed pointer is what gets delivered**, not ours: `libretro.h` requires that
            // it be that one, with no offset.
            (false, false) => match &borrowed {
                Some((data, _)) => (*data as *const c_void, ()),
                None => (frame.as_ptr() as *const c_void, ()),
            },
        };
        // SAFETY: the buffer lives during the call; on a repeated frame the frontend reuses the
        // last one.
        unsafe {
            video(pointer, width, height, video_pitch);
        }
    }
    // The batch's return value is in **accepted** frames; whatever is left waits for the next call.
    let mut surplus = Vec::new();
    if let Some(batch) = front.audio_batch {
        let frames = audio.len() / 2;
        // SAFETY: the batch is interleaved stereo and the size is the number of frames.
        let accepted_frames = unsafe { batch(audio.as_ptr(), frames) }.min(frames);
        // **The last mile, measured.** How many frames the core delivers per real second, against
        // the 44100 it declares. See [`AUDIO_FRAMES`].
        {
            use std::sync::atomic::Ordering;
            let total = AUDIO_FRAMES.fetch_add(frames as u64, Ordering::Relaxed) + frames as u64;
            let start = *AUDIO_CLOCK.get_or_init(std::time::Instant::now);
            let now_ms = start.elapsed().as_millis() as u64;
            let last = AUDIO_LAST_MS.load(Ordering::Relaxed);
            if now_ms >= last + 1_000 {
                let before = AUDIO_PREVIOUS.swap(total, Ordering::Relaxed);
                AUDIO_LAST_MS.store(now_ms, Ordering::Relaxed);
                zeebx::logging!(
                    zeebx::logging::Level::Info,
                    "audio",
                    "audio: {} frames per real second ({:.0}% of 44100); the frontend accepted {}/{} on this frame",
                    (total - before) * 1_000 / (now_ms - last).max(1),
                    ((total - before) * 1_000) as f64 / (now_ms - last).max(1) as f64 / 441.0,
                    accepted_frames,
                    frames
                );
            }
        }
        if accepted_frames < frames {
            surplus = audio[accepted_frames * 2..].to_vec();
        }
    }
    // The buffers go back to the state, so the next call reuses the same allocation.
    let mut dismiss = false;
    if let Ok(mut guard) = core().lock() {
        if let Some(CoreState(state)) = guard.as_mut() {
            state.frame = frame;
            state.audio = audio;
            // The leftover must not grow without end; half a second is the cap.
            let limit = (SAMPLE_RATE as usize / 2) * 2;
            surplus.truncate(limit);
            state.pending_audio = surplus;
            if state.stopped {
                state.frames_after_stop += 1;
                // Two seconds of a still screen are enough to see the outcome and the log.
                if state.frames_after_stop == 120 {
                    // **Going back to the Z-Wheel.** On the console, closing a game hands control
                    // back to the shell, and the shell is another installed applet, so here we
                    // switch session, as the desktop UI already does. With no Z-Wheel available,
                    // the possible outcome is asking the frontend to end the content.
                    let rewind = state.z_wheel.clone().filter(|_| {
                        state.opened_via_z_wheel
                            || state.session.class_id() == zeebx::session::Z_WHEEL
                    });
                    match rewind {
                        Some(path) => match switch_to(state, &path, false) {
                            Ok(()) => log("Zeebx: game over; back to the Z-Wheel"),
                            Err(error) => {
                                log_with_level(2, &format!("Zeebx: could not go back to the Z-Wheel: {error}"));
                                dismiss = true;
                            }
                        },
                        None => dismiss = true,
                    }
                }
            }
        }
    }
    if dismiss {
        // Ending the content is the outcome the ABI offers: the frontend goes back to its menu,
        // instead of showing forever the last frame of a game that ended.
        log("Zeebx: game ended; asking the frontend to end the content");
        unsafe {
            environ(ENV_SHUTDOWN, std::ptr::null_mut());
        }
    }
}

/// `retro_unload_game`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_unload_game() {
    clear_frontend_state();
}

/// `retro_reset`: reloads the content from scratch, keeping saves and the NAND.
#[unsafe(no_mangle)]
pub extern "C" fn retro_reset() {
    let Ok(mut guard) = core().lock() else {
        return;
    };
    let Some(CoreState(old)) = guard.as_ref() else {
        return;
    };
    let path = old.path.clone();
    let ports = old.ports;
    let Some(save_dir) = dir(ENV_GET_SAVE_DIRECTORY) else {
        return;
    };
    let system = dir(ENV_GET_SYSTEM_DIRECTORY);
    let storage = StoragePaths::for_frontend(&save_dir, system.as_deref());
    let text = path.to_string_lossy().into_owned();
    // SAFETY: the same setup as loading, on the `retro_run` thread.
    match unsafe { load(&text, &storage, ports, path.clone()) } {
        Ok(create) => *guard = Some(CoreState(create)),
        Err(error) => log(&format!("Zeebx: reset failed: {error}")),
    }
}

/// `retro_set_controller_port_device`: changes the device the guest sees on the port.
#[unsafe(no_mangle)]
pub extern "C" fn retro_set_controller_port_device(port: u32, device: u32) {
    let Ok(mut guard) = core().lock() else {
        return;
    };
    let Some(CoreState(state)) = guard.as_mut() else {
        return;
    };
    let port_index = port as usize;
    if port_index >= zeebx::input::PORTS {
        return;
    }
    state.ports[port_index] = device_for_retro_device(device);
    state.session.set_ports(state.ports);
}

/// The last saved state, so `retro_serialize` delivers what `_size` measured.
///
/// The ABI calls the two in sequence (first the size, then the save), and the size varies with
/// what the game has in memory: textures, decoded images and GL buffers are most of it.
/// Recomputing at save time would give a state **different** from the one measured, and the
/// frontend would have allocated the buffer by the wrong number. That is why the measured size
/// keeps the bytes.
static MEASURED_STATE: std::sync::Mutex<Option<Vec<u8>>> = std::sync::Mutex::new(None);

/// `retro_serialize_size`: the size of the state, or zero when there is nothing to save.
///
/// **Zero only happens with no session or with a draw in progress**, and both are honest answers:
/// with no session there is no state, and in the middle of a started draw the state would promise
/// something that never existed. At any other moment the number is that of the real state.
#[unsafe(no_mangle)]
pub extern "C" fn retro_serialize_size() -> usize {
    let Ok(mut guard) = core().lock() else {
        return 0;
    };
    let Some(CoreState(state)) = guard.as_mut() else {
        return 0;
    };
    if let Err(reason) = state.session.can_save() {
        warning(&format!("Zeebx: cannot save right now: {reason}"));
        return 0;
    }
    let file = state.session.save_state();
    let size = file.len();
    if let Ok(mut measured) = MEASURED_STATE.lock() {
        *measured = Some(file);
    }
    size
}

/// `retro_serialize`: delivers the measured state.
///
/// Returns `false` (without writing anything) when it does not fit in the buffer the frontend
/// offered. That is the contract: the frontend allocates by the size it asked for, and lying about
/// it would corrupt its memory.
#[unsafe(no_mangle)]
pub extern "C" fn retro_serialize(data: *mut c_void, size: usize) -> bool {
    if data.is_null() {
        return false;
    }
    let Ok(guard) = MEASURED_STATE.lock() else {
        return false;
    };
    let Some(file) = guard.as_ref() else {
        return false;
    };
    if file.len() > size {
        warning(&format!(
            "Zeebx: the state has {} bytes and the frontend offered {size}",
            file.len()
        ));
        return false;
    }
    // SAFETY: the frontend guarantees `size` valid bytes in `data`, and the block above checked
    // that the state fits.
    unsafe {
        std::ptr::copy_nonoverlapping(file.as_ptr(), data as *mut u8, file.len());
    }
    true
}

/// `retro_unserialize`: puts the state back.
///
/// The refusal comes from the engine, with the reason: a missing section, a size that does not
/// match, a mismatched `crc32`. A half-applied state inside a running machine is worse than a
/// refused state, and that is why reading happens **before** any writing.
#[unsafe(no_mangle)]
pub extern "C" fn retro_unserialize(data: *const c_void, size: usize) -> bool {
    if data.is_null() || size == 0 {
        return false;
    }
    // SAFETY: the frontend guarantees `size` valid bytes in `data`.
    let file = unsafe { std::slice::from_raw_parts(data as *const u8, size) };
    let Ok(mut guard) = core().lock() else {
        return false;
    };
    let Some(CoreState(state)) = guard.as_mut() else {
        return false;
    };
    match state.session.restore_state(file) {
        Ok(()) => true,
        Err(error) => {
            warning(&format!("Zeebx: the save state was refused: {error}"));
            false
        }
    }
}

/// `retro_cheat_reset`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_cheat_reset() {}

/// `retro_cheat_set`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_cheat_set(_index: u32, _enabled: bool, _code: *const c_char) {}

/// `retro_load_game_special`: there are no subsystems.
#[unsafe(no_mangle)]
pub extern "C" fn retro_load_game_special(
    _game_type: u32,
    _info: *const RetroGameInfo,
    _num_info: usize,
) -> bool {
    false
}

/// `retro_get_region`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_get_region() -> u32 {
    0
}

/// `retro_get_memory_data`: saves are BREW files, not a contiguous region.
#[unsafe(no_mangle)]
pub extern "C" fn retro_get_memory_data(_id: u32) -> *mut c_void {
    std::ptr::null_mut()
}

/// `retro_get_memory_size`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_get_memory_size(_id: u32) -> usize {
    0
}


#[cfg(test)]
mod tests {
    use super::*;

    /// **The GPU struct's offsets have to match `libretro.h`.**
    ///
    /// The core fills in the struct, the frontend returns it filled in, and we read
    /// `get_current_framebuffer` and `get_proc_address` from it. One field more or less here would
    /// make those two be read from the wrong place, and the symptom would be "hardware rendering
    /// does not work", with nothing in the log to explain it. The numbers are the sum of the fields
    /// `include/libretro.h` declares, in its order.
    #[test]
    fn retropad_input_reaches_the_guest() {
        let Ok(rom_path) = zeebx::config::env_var("ZEEBX_TEST_ROM") else {
            eprintln!("no ZEEBX_TEST_ROM: nothing to test");
            return;
        };
        let dir = std::env::temp_dir().join(format!("zeebx-entrada-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = DIR.set(CString::new(dir.to_string_lossy().to_string()).unwrap());
        let path_c = CString::new(rom_path.clone()).unwrap();
        let info = RetroGameInfo {
            path: path_c.as_ptr(),
            data: std::ptr::null(),
            size: 0,
            meta: std::ptr::null(),
        };
        let mut reacted = None;
        let mut did_open: Option<(&str, i32, u32, u32)> = None;
        unsafe {
            retro_set_environment(Some(environment));
            retro_set_video_refresh(Some(video));
            retro_set_audio_sample_batch(Some(audio));
            retro_set_input_poll(Some(no_poll));
            retro_set_input_state(Some(entry));
            retro_init();
            // The port's device decides the pad mapping. `ZEEBX_TEST_DEVICE=zpad` uses the Z-Pad,
            // which is the console's controller; the Z-Wheel is a Zeebo app and reads it that way.
            let device_kind = match zeebx::config::env_var("ZEEBX_TEST_DEVICE").as_deref() {
                Ok("zpad") | Ok("ZPAD") => DEVICE_ZPAD,
                _ => DEVICE_JOYPAD,
            };
            retro_set_controller_port_device(0, device_kind);
            assert!(retro_load_game(&info), "the core refused {rom_path}");
            // How long to wait before sending input. The Z-Wheel takes longer than the games to
            // reach the interactive screen (the scan catches it with zero frames at six seconds),
            // and sending a button to a loading screen says nothing about the input path.
            let wait: u32 = zeebx::config::env_var("ZEEBX_TEST_WAIT")
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(180);
            for _ in 0..wait {
                retro_run();
            }
            for button in [ID_START, ID_A, ID_B, ID_SELECT] {
                if let Ok(mut seen) = SIGNATURES.lock() {
                    seen.clear();
                }
                BUTTON.store(button, Ordering::Relaxed);
                for _ in 0..40 {
                    retro_run();
                }
                BUTTON.store(u32::MAX, Ordering::Relaxed);
                for _ in 0..20 {
                    retro_run();
                }
                let distinct = SIGNATURES
                    .lock()
                    .map(|v| v.iter().collect::<std::collections::BTreeSet<_>>().len())
                    .unwrap_or(0);
                eprintln!("button {button}: {distinct} distinct image(s) in 60 frames");
                if distinct > 1 {
                    reacted = Some(button);
                    break;
                }
            }
            // **The stick**, which is how the Z-Wheel and the menus are navigated.
            for (which, value) in [("x", 0x7fff), ("x", -0x8000), ("y", 0x7fff), ("y", -0x8000)] {
                if let Ok(mut seen) = SIGNATURES.lock() {
                    seen.clear();
                }
                let target = match which {
                    "x" => &AXIS_X,
                    _ => &AXIS_Y,
                };
                target.store(value, Ordering::Relaxed);
                for _ in 0..40 {
                    retro_run();
                }
                target.store(0, Ordering::Relaxed);
                for _ in 0..20 {
                    retro_run();
                }
                let distinct = SIGNATURES
                    .lock()
                    .map(|v| v.iter().collect::<std::collections::BTreeSet<_>>().len())
                    .unwrap_or(0);
                eprintln!("stick {which}={value}: {distinct} distinct image(s)");
                if distinct > 1 {
                    reacted = Some(0x100 + value as u32);
                    break;
                }
            }
            // **Opening a game with the stick and the confirm button.** It is the whole item 8
            // cycle, with no frontend: the wheel is interactive since the wait, the stick navigates
            // it, and the open request shows up in `LAST_OPENED`; the core records it through a
            // test instrument, because its log goes out through the frontend's variadic callback.
            LAST_OPENED.store(0, Ordering::Relaxed);
            'attempts: for (direction, value) in [("x", 0x7fff), ("x", -0x8000), ("y", 0x7fff), ("y", -0x8000)] {
                // The four face buttons **and** Start: the Z-Wheel does not use the same one to
                // navigate and to confirm, and testing only the face ones is what left the cycle
                // without an answer.
                for button in [ID_A, ID_B, ID_Y, ID_X, ID_START] {
                    let target = match direction {
                        "x" => &AXIS_X,
                        _ => &AXIS_Y,
                    };
                    // **Push the stick and confirm without letting go.** The first version let go
                    // before pressing, and this combination (the stick held in one direction with
                    // the button pressed) is precisely how a carousel confirms the focused piece.
                    target.store(value, Ordering::Relaxed);
                    for _ in 0..30 {
                        retro_run();
                    }
                    BUTTON.store(button, Ordering::Relaxed);
                    for _ in 0..20 {
                        retro_run();
                    }
                    BUTTON.store(u32::MAX, Ordering::Relaxed);
                    target.store(0, Ordering::Relaxed);
                    // **After confirming, the wheel animates the transition** before asking for the
                    // open: twenty frames are not enough, and the request arrives during the
                    // animation.
                    for _ in 0..180 {
                        retro_run();
                    }
                    let class_id = LAST_OPENED.load(Ordering::Relaxed);
                    if class_id != 0 {
                        did_open = Some((direction, value, button, class_id));
                        break 'attempts;
                    }
                }
            }
            retro_unload_game();
            retro_deinit();
        }
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!(
            "cycle: {}",
            match did_open {
                Some((direction, value, button, class_id)) => format!(
                    "the shell asked for {class_id:#010x} (stick {direction}={value}, button {button})"
                ),
                None => "the wheel did not ask to open anything".to_string(),
            }
        );
        eprintln!(
            "input: {}",
            match reacted {
                Some(button) if button >= 0x100 => "arrived — the stick changed the image".to_string(),
                Some(button) => format!("arrived — button {button} changed the image"),
                None => "neither the four buttons nor the stick changed the image".to_string(),
            }
        );
    }

    /// The speed limit separates console, half frames and boost; it does not accept ambiguous text.
    #[test]
    fn the_fps_limit_text_becomes_the_right_mode() {
        assert_eq!(FpsLimit::from_text("60"), Some(FpsLimit::Sixty));
        assert_eq!(FpsLimit::from_text("30"), Some(FpsLimit::Thirty));
        assert_eq!(FpsLimit::from_text("desligado"), Some(FpsLimit::Off));
        assert_eq!(FpsLimit::from_text("120"), None);
        assert_eq!(FpsLimit::from_text("automatico"), None);
        assert_eq!(FpsLimit::from_text(""), None);
    }

    /// The frameskip option's text becomes the right mode, and garbage keeps the previous mode
    /// (`None`).
    #[test]
    fn the_frameskip_text_becomes_the_right_mode() {
        assert_eq!(Frameskip::from_text("desligado"), Some(Frameskip::Off));
        assert_eq!(Frameskip::from_text("automatico"), Some(Frameskip::Auto));
        assert_eq!(Frameskip::from_text("1"), Some(Frameskip::Fixed(1)));
        assert_eq!(Frameskip::from_text("6"), Some(Frameskip::Fixed(6)));
        assert_eq!(Frameskip::from_text("4294967295"), None);
        // Zero is not a valid fixed mode: skipping zero frames is the same as off, and a "0" coming
        // from outside looks more like a corrupt opt than a deliberate choice.
        assert_eq!(Frameskip::from_text("0"), None);
        assert_eq!(Frameskip::from_text("nao-existe"), None);
        assert_eq!(Frameskip::from_text(""), None);
    }

    /// The profile is only Handheld with the right text, and everything else (absence included) is
    /// Default.
    #[test]
    fn only_the_handheld_text_enables_the_profile() {
        assert!(profile_is_handheld(Some("portatil")));
        assert!(profile_is_handheld(Some(" PORTATIL ")));
        assert!(!profile_is_handheld(Some("padrao")));
        assert!(!profile_is_handheld(Some("portátil"))); // the declared value has no accent
        assert!(!profile_is_handheld(None));
        assert!(!profile_is_handheld(Some("")));
    }

    /// The options' switches and numbers accept what the frontend delivers, and reject garbage.
    ///
    /// What is checked here is the **rejection**: a value that cannot be understood has to become
    /// `None`, because the caller handles `None` by keeping what was already there. If it became
    /// some default, an old frontend (which does not know the key) would undo the choice of whoever
    /// configured it.
    #[test]
    fn option_values_are_read_and_garbage_is_rejected() {
        // The ecosystem's convention is enabled/disabled; the other spellings exist because a
        // hand-edited .opt follows no convention at all.
        assert_eq!(bool_from_text("enabled"), Some(true));
        assert_eq!(bool_from_text("disabled"), Some(false));
        assert_eq!(bool_from_text(" ON "), Some(true));
        assert_eq!(bool_from_text("false"), Some(false));
        assert_eq!(bool_from_text("talvez"), None);
        assert_eq!(bool_from_text(""), None);

        // The number is clamped to the range the engine accepts, not rejected: whoever asked for 99
        // wants the maximum.
        assert_eq!(number_from_text("1", 1, 8), Some(1));
        assert_eq!(number_from_text("4", 1, 8), Some(4));
        assert_eq!(number_from_text("99", 1, 8), Some(8));
        assert_eq!(number_from_text(" 2 ", 1, 8), Some(2));
        // Zero and negatives go up to the minimum; the engine does not draw at scale zero.
        assert_eq!(number_from_text("0", 1, 8), Some(1));
        assert_eq!(number_from_text("-3", 1, 8), None);
        assert_eq!(number_from_text("muito", 1, 8), None);
    }

    /// The core's log level goes through the option's tokens, and an invalid token does **not**
    /// silence the log: a typo turning the log off without warning is the worst outcome.
    #[test]
    fn the_log_level_option_becomes_the_core_setting() {
        use zeebx::logging::{Threshold, Level};

        assert_eq!(Threshold::from_text("desligado"), Some(Threshold::Off));
        assert_eq!(Threshold::from_text("aviso"), Some(Threshold::UpTo(Level::Warning)));
        assert_eq!(Threshold::from_text("informacao"), Some(Threshold::UpTo(Level::Info)));
        assert_eq!(Threshold::from_text("depuracao"), Some(Threshold::UpTo(Level::Debug)));
        assert_eq!(Threshold::from_text("fatal"), Some(Threshold::UpTo(Level::Fatal)));
        // Whatever is no token at all is rejected, and the caller is the one who handles it.
        assert_eq!(Threshold::from_text("banana"), None);

        // Each level has its number in `libretro.h`, and `FATAL` falls to the ABI's ceiling.
        assert_eq!(libretro_level(Level::Debug), 0);
        assert_eq!(libretro_level(Level::Info), 1);
        assert_eq!(libretro_level(Level::Warning), 2);
        assert_eq!(libretro_level(Level::Error), 3);
        assert_eq!(
            libretro_level(Level::Fatal),
            3,
            "the ABI has no FATAL: the ceiling is ERROR"
        );
    }

    /// The volume option's text becomes a mixer factor, and corrupt text does **not** become
    /// silence.
    #[test]
    fn the_volume_option_becomes_a_mixer_factor() {
        assert_eq!(volume_from_text("100"), Some(1.0));
        assert_eq!(volume_from_text("0"), Some(0.0));
        assert_eq!(volume_from_text("50"), Some(0.5));
        // The frontend may deliver it with surrounding spaces; the file is editable by hand.
        assert_eq!(volume_from_text(" 70 "), Some(0.7));
        // Out of range is clamped to the range, not rejected: 150% is a clear intention of "at the
        // maximum".
        assert_eq!(volume_from_text("150"), Some(1.0));
        assert_eq!(volume_from_text("-10"), Some(0.0));
        // What is not a number keeps what was already there, instead of muting the emulator.
        assert_eq!(volume_from_text("alto"), None);
        assert_eq!(volume_from_text(""), None);
        assert_eq!(volume_from_text("NaN"), None);
    }

    /// The bank option becomes the path inside the system's banks folder, and "auto" (or nothing,
    /// on an old frontend) is the automatic search.
    #[test]
    fn the_bank_option_points_to_the_banks_dir() {
        let system = Path::new("/retroarch/system");
        let dir = banks_dir(system);
        // The same folder the game load uses: the device's, plus `soundfonts`.
        assert_eq!(
            dir,
            StoragePaths::for_frontend(system, Some(system)).device.join("soundfonts")
        );
        assert!(dir.starts_with(system));
        assert_eq!(
            bank_from_option(Some("firmware.sf2"), &dir),
            Some(dir.join("firmware.sf2"))
        );
        assert_eq!(bank_from_option(Some("auto"), &dir), None);
        assert_eq!(bank_from_option(None, &dir), None);
    }

    #[test]
    fn the_gpu_struct_offsets_match_libretro_h() {
        use std::mem::{offset_of, size_of};
        assert_eq!(offset_of!(RetroHwRenderCallback, context_reset), 8);
        assert_eq!(offset_of!(RetroHwRenderCallback, get_current_framebuffer), 16);
        assert_eq!(offset_of!(RetroHwRenderCallback, get_proc_address), 24);
        assert_eq!(offset_of!(RetroHwRenderCallback, depth), 32);
        assert_eq!(offset_of!(RetroHwRenderCallback, version_major), 36);
        assert_eq!(offset_of!(RetroHwRenderCallback, version_minor), 40);
        assert_eq!(offset_of!(RetroHwRenderCallback, cache_context), 44);
        assert_eq!(offset_of!(RetroHwRenderCallback, context_destroy), 48);
        assert!(size_of::<RetroHwRenderCallback>() >= 56);
    }
    use std::ffi::CString;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// How many frames the frontend's video received.
    static FRAMES: AtomicU32 = AtomicU32::new(0);
    /// Which RetroPad button the test is holding. `u32::MAX` is none.
    static BUTTON: AtomicU32 = AtomicU32::new(u32::MAX);
    /// The folder the test frontend hands over as system and as saves.
    static DIR: OnceLock<CString> = OnceLock::new();
    /// The outside system, when `ZEEBX_CORE_SYSTEM` points to a real device tree.
    static SYSTEM: OnceLock<CString> = OnceLock::new();
    /// The signature of each delivered frame, in order.
    static SIGNATURES: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
    /// The last line pitch announced to the video callback.
    static VIDEO_PITCH: AtomicU32 = AtomicU32::new(0);

    /// The minimal environment the core needs, answering like a real frontend.
    ///
    /// What we do not have answers `false`: that is what RetroArch does with what it does not
    /// know, and that is how the core's refusal path gets exercised too.
    /// The buffer the fake frontend lends the core on `GET_CURRENT_SOFTWARE_FRAMEBUFFER`.
    ///
    /// Static because the pointer has to stay valid for the whole `retro_run` call in which it was
    /// handed over; `libretro.h` only guarantees that.
    static LENT_BUFFER: Mutex<Option<Vec<u8>>> = Mutex::new(None);

    /// The pitch the fake frontend uses: the width in bytes **plus some slack**.
    ///
    /// The slack is on purpose. With `pitch == width * 2` the core could write the whole frame at
    /// once and get it right by luck; with slack, whoever does not respect the pitch writes a
    /// skewed image, and that is what the proof needs to catch.
    const PITCH_SLACK: usize = 64;

    unsafe extern "C" fn environment(cmd: u32, data: *mut c_void) -> bool {
        match cmd {
            // **The frontend lends the buffer.** See [`ENV_GET_CURRENT_SOFTWARE_FRAMEBUFFER`].
            ENV_GET_CURRENT_SOFTWARE_FRAMEBUFFER => {
                if data.is_null() {
                    return false;
                }
                let request = data as *mut RetroFramebuffer;
                let (width, height) = unsafe { ((*request).width, (*request).height) };
                let step = width as usize * 2 + PITCH_SLACK;
                let Ok(mut guard) = LENT_BUFFER.lock() else {
                    return false;
                };
                *guard = Some(vec![0u8; step * height as usize]);
                let Some(buffer) = guard.as_mut() else {
                    return false;
                };
                unsafe {
                    (*request).data = buffer.as_mut_ptr() as *mut c_void;
                    (*request).pitch = step;
                    (*request).format = PIXEL_FORMAT_RGB565;
                }
                true
            }
            // Accepts RGB565 and refuses the rest: it is the format the console delivers, and
            // refusing the others makes the core follow the path it uses in RetroArch.
            ENV_SET_PIXEL_FORMAT => {
                !data.is_null() && unsafe { *(data as *const u32) } == PIXEL_FORMAT_RGB565
            }
            ENV_SET_VARIABLES | ENV_SET_CORE_OPTIONS_V2 | ENV_SET_INPUT_DESCRIPTORS | ENV_SET_CONTROLLER_INFO => {
                true
            }
            ENV_GET_CORE_OPTIONS_VERSION => {
                if !data.is_null() {
                    unsafe { *(data as *mut u32) = 2 };
                    true
                } else {
                    false
                }
            }
            ENV_GET_VARIABLE => {
                if !data.is_null() {
                    let var = data as *mut RetroVariable;
                    // Returns None to simulate the Auto default value
                    unsafe { (*var).value = std::ptr::null() };
                    true
                } else {
                    false
                }
            }
            ENV_GET_SYSTEM_DIRECTORY | ENV_GET_SAVE_DIRECTORY => {
                // The system may come from outside, through `ZEEBX_CORE_SYSTEM`: that is how the
                // test finds the real device, with the games installed, and manages to exercise the
                // Z-Wheel cycle end to end. Without the variable, each one gets the test's
                // temporary folder.
                let dir = if cmd == ENV_GET_SYSTEM_DIRECTORY {
                    SYSTEM.get().or_else(|| DIR.get())
                } else {
                    DIR.get()
                };
                let Some(dir) = dir else {
                    return false;
                };
                if data.is_null() {
                    return false;
                }
                // SAFETY: the core promises a `*const *const c_char` for writing.
                unsafe { *(data as *mut *const c_char) = dir.as_ptr() };
                true
            }
            _ => false,
        }
    }

    unsafe extern "C" fn video(data: *const c_void, width: u32, height: u32, step: usize) {
        FRAMES.fetch_add(1, Ordering::Relaxed);
        VIDEO_PITCH.store(step.min(u32::MAX as usize) as u32, Ordering::Relaxed);
        // A cheap frame signature: it changes when the image changes, which is what the test needs
        // to know to tell whether input reached the guest. A controller that does not get through
        // leaves the screen still, and so does a wrong button, and the two are told apart by
        // looking at the rest.
        let total = (step as u64) * u64::from(height);
        let bytes =
            unsafe { std::slice::from_raw_parts(data as *const u8, total.min(1 << 22) as usize) };
        let mut signature = 1469598103934665603u64;
        for &b in bytes.iter().step_by(97) {
            signature = (signature ^ u64::from(b)).wrapping_mul(1099511628211);
        }
        if let Ok(mut seen) = SIGNATURES.lock() {
            seen.push(signature);
        }
        let _ = (width, height);
    }

    /// How many stereo samples the core delivered to the frontend.
    static SAMPLES: AtomicU32 = AtomicU32::new(0);
    /// How many games the core found next to the content (see [`GAMES_SEEN`]).

    unsafe extern "C" fn audio(_data: *const i16, frames: usize) -> usize {
        // Counting here is what makes it possible to check **through the core's path** that audio
        // goes out: the engine has its own measure (peak, rms, jump), and the core has this one,
        // whether the batch reaches the frontend.
        SAMPLES.fetch_add(frames as u32, Ordering::Relaxed);
        frames
    }

    /// **Each button's position, pinned by a test.** It was what was missing in issue #41: the
    /// mapping screen's label and the reading table were two parallel lists, and they diverged
    /// silently: the label said "B = Button 1" and the reading delivered B as Button 2. Now there
    /// is a single table, and this test pins the device's physical numbering: whoever moves `b1`
    /// lands here, and not in the player's lap.
    #[test]
    fn the_action_buttons_follow_the_device_numbering() {
        // On the device: 1 at the bottom, 2 on the left, 3 at the top, 4 on the right. On the
        // RetroPad: `B` at the bottom, `Y` on the left, `X` at the top, `A` on the right; the hand
        // position is the same on both.
        let action = &RETROPAD_BUTTONS[4..8];
        assert_eq!(
            action.iter().map(|(_, name, _)| *name).collect::<Vec<_>>(),
            ["b1", "b2", "b3", "b4"]
        );
        assert_eq!(
            action.iter().map(|(id, _, _)| *id).collect::<Vec<_>>(),
            [ID_B, ID_Y, ID_X, ID_A]
        );

        // And every name in the table exists in the console's list: a typo here would leave the
        // button mute, with no error anywhere.
        for (_, name, _) in RETROPAD_BUTTONS {
            assert!(
                zeebx::input::BUTTON_NAMES.contains(&name),
                "{name} is not a console button"
            );
        }
    }

    /// **The core's whole path, in the order it happens.** The mirrored d-pad writes the axis and,
    /// right after, the analog loop runs, with the RetroPad delivering the stick **resting at the
    /// center**, which is the real case of someone playing with the d-pad. That is where the
    /// option did nothing: the resting zero erased the mirror on every frame, and only inside the
    /// core (the standalone already had the dead zone, which is why the measurement in the harness
    /// passed).
    #[test]
    fn a_centered_stick_does_not_clear_the_mirrored_dpad() {
        use zeebx::input::{AXIS_RANGE, DPAD, Pad};

        let stopped = |_: u32, _: u32| 0i32;
        let mut pad = Pad::default();
        pad.press(DPAD[0], true); // d-pad up
        pad.mirror_dpad_on_axes();
        set_retropad_axes(&mut pad, stopped);
        assert!(
            pad.axes[1] < 0,
            "the resting stick's zero erased the d-pad: Y axis = {}",
            pad.axes[1]
        );
        assert_eq!(pad.console_axis(1), 128 - AXIS_RANGE, "up is the low value");

        // Releasing the direction returns the axis to center, and the resting stick still does not
        // write.
        pad.press(DPAD[0], false);
        pad.mirror_dpad_on_axes();
        set_retropad_axes(&mut pad, stopped);
        assert_eq!(pad.axes[1], 0);

        // And the real stick, when it leaves the dead zone, beats the mirror.
        let pushed = |index: u32, id: u32| match (index, id) {
            (0, 0) => 0x4000,
            _ => 0,
        };
        pad.press(DPAD[0], true);
        pad.mirror_dpad_on_axes();
        set_retropad_axes(&mut pad, pushed);
        assert_eq!(pad.axes[0], 0x4000 / 256, "the real stick has the last word");
        assert_eq!(pad.axes[1], -AXIS_RANGE, "and the d-pad stays on the other axis");

        // Jitter inside the dead zone does not write: it is what keeps a worn stick quiet.
        let jitter = |_: u32, _: u32| 512i32; // 2 on the console's travel
        pad.press(DPAD[0], true);
        pad.mirror_dpad_on_axes();
        set_retropad_axes(&mut pad, jitter);
        assert_eq!(pad.axes[0], 0, "the jitter did not get past the dead zone");
    }

    /// The axis the test is pushing, in the RetroPad's range (`-0x8000..=0x7fff`).
    ///
    /// The Z-Wheel is navigated **with the stick**, so a test that only presses buttons does not
    /// move it, and the question "does input get through?" was left unanswered for it.
    static AXIS_X: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
    static AXIS_Y: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

    unsafe extern "C" fn entry(_port: u32, device: u32, _index: u32, id: u32) -> i16 {
        if device == DEVICE_ANALOG {
            // On the RetroPad, `0` is the left analog's X and `1` is the Y.
            let value = match id {
                0 => AXIS_X.load(Ordering::Relaxed),
                _ => AXIS_Y.load(Ordering::Relaxed),
            };
            return value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        }
        // The core asks for the state of each button, one by one.
        (BUTTON.load(Ordering::Relaxed) == id) as i16
    }

    extern "C" fn no_poll() {}

    /// **With no session there is nothing to save**, and the refusal writes nothing.
    ///
    /// Zero is still the right answer here, not because the core does not know how to save, but
    /// because no machine is built at all. A `serialize` that wrote half a machine would be worse
    /// than none: RetroArch would allow saving, and loading would return a game with the right
    /// memory and registers and the wrong object table, calling APIs with identifiers that no
    /// longer exist.
    #[test]
    fn without_a_session_there_is_nothing_to_save() {
        assert_eq!(
            retro_serialize_size(),
            0,
            "with no session, the size is zero"
        );
        let mut dest = [0u8; 16];
        assert!(
            !retro_serialize(dest.as_mut_ptr() as *mut c_void, dest.len()),
            "writing half a machine would be worse than refusing"
        );
        assert!(!retro_unserialize(
            dest.as_ptr() as *const c_void,
            dest.len()
        ));
        assert_eq!(dest, [0u8; 16], "the refusal must not have written anything");
    }

    /// **The core, exercised through its own ABI.**
    ///
    /// It is the test that was missing for the Z-Wheel cycle on the core's side: the engine is
    /// measured by the scan, and the core's loop (which switches session when the shell asks) had
    /// no automated proof at all. Here there is no window and no RetroArch, but the path is the
    /// same: `retro_init`, `retro_load_game`, frames and `retro_unload_game`.
    ///
    /// **What this test proves, and what it does not.**
    ///
    /// It proves: the core loads content through its own ABI, delivers frames and tears down
    /// cleanly, without RetroArch and without a window. With the Z-Wheel, it delivers 360 frames.
    ///
    /// **It does not prove the session switch.** Driving the controller through the RetroPad's six
    /// keys (30 frames per key), no distinct image shows up (measured: 1 per phase) and the shell
    /// asks for no open at all. Either the headless Z-Wheel does not reach the state where it
    /// accepts the choice, or input does not reach the guest through the core's path. Both
    /// hypotheses are open and this test does **not** choose between them: what closes the cycle
    /// is RetroArch, with a real controller, which is what item 8 of the plan asks for.
    ///
    /// **Without `ZEEBX_CORE_ROM` it does not run.** ROMs do not go into the repository tree, and a
    /// test that downloads content by itself is worse than a test that does not run.
    ///
    /// ```bash
    /// ZEEBX_CORE_ROM="roms/Z-Wheel.zip" cargo test -p zeebx-libretro -- --nocapture
    /// ```
    #[test]
    fn the_core_abi_runs_a_rom() {
        let Ok(rom_path) = std::env::var("ZEEBX_CORE_ROM") else {
            eprintln!("no ZEEBX_CORE_ROM: nothing to run");
            return;
        };
        let dir = std::env::temp_dir().join(format!("zeebx-core-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = DIR.set(CString::new(dir.to_string_lossy().to_string()).unwrap());
        if let Ok(outside) = zeebx::config::env_var("ZEEBX_CORE_SYSTEM") {
            let _ = SYSTEM.set(CString::new(outside).unwrap());
        }
        FRAMES.store(0, Ordering::Relaxed);

        let path_c = CString::new(rom_path.clone()).unwrap();
        let requested_frames = zeebx::config::env_var("ZEEBX_CORE_FRAMES")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(60u32);

        let mut did_open = LAST_OPENED.load(Ordering::Relaxed);
        let mut which = None;
        // Before driving the controller there can be no open request at all: if there is one, it
        // came from loading the content, and the reading in the loop below would be measuring
        // something else.
        assert_eq!(did_open, 0, "the shell asked to open before any key");
        assert!(which.is_none());
        if let Ok(mut seen) = SIGNATURES.lock() {
            seen.clear();
        }
        let info = RetroGameInfo {
            path: path_c.as_ptr(),
            data: std::ptr::null(),
            size: 0,
            meta: std::ptr::null(),
        };
        unsafe {
            retro_set_environment(Some(environment));
            retro_set_video_refresh(Some(video));
            retro_set_audio_sample_batch(Some(audio));
            retro_set_input_poll(Some(no_poll));
            retro_set_input_state(Some(entry));
            retro_init();
            retro_set_controller_port_device(0, DEVICE_JOYPAD);
            assert!(retro_load_game(&info), "the core refused {rom_path}");
            // **The pace, measured.** This loop is the only place where our speed brake acts on
            // its own: there is no frontend waiting for retrace or audio. What we want to know is
            // whether `run_frame`'s `sleep` delivers 1× (virtual time equal to real time) and **how
            // irregular it makes things**: a brake that gets the average right and shakes on every
            // frame is judder.
            //
            // See `Session::run_frame` and front 10 of `docs/OPTIMIZING_V0.3.0.md`.
            let real_before = std::time::Instant::now();
            let clock_before = CLOCK.load(Ordering::Relaxed);
            let mut per_frame = Vec::with_capacity(requested_frames as usize);
            for _ in 0..requested_frames {
                let t = std::time::Instant::now();
                retro_run();
                per_frame.push(t.elapsed());
            }
            // **The frame went into the frontend's buffer.** If the core had written to its own
            // vector, the borrowed buffer would stay zeroed; if it had ignored the pitch, the lines
            // would be shifted. Both things show up here.
            {
                let guard = LENT_BUFFER.lock().expect("the test's buffer");
                let buffer = guard.as_ref().expect("the frontend lent the buffer");
                let lit = buffer.iter().filter(|b| **b != 0).count();
                assert!(
                    lit > buffer.len() / 100,
                    "the borrowed buffer stayed blank ({lit} of {} byte(s)): the core did not draw into it",
                    buffer.len()
                );
                assert_eq!(
                    VIDEO_PITCH.load(Ordering::Relaxed) as usize,
                    640 * 2 + PITCH_SLACK,
                    "the core wrote with the borrowed pitch, but announced a different pitch to the frontend"
                );
            }
            let real = real_before.elapsed();
            let advanced = CLOCK.load(Ordering::Relaxed).wrapping_sub(clock_before);
            per_frame.sort();
            let median = per_frame.get(per_frame.len() / 2).copied().unwrap_or_default();
            let p95 = per_frame
                .get(per_frame.len() * 95 / 100)
                .copied()
                .unwrap_or_default();
            let worst = per_frame.last().copied().unwrap_or_default();
            eprintln!(
                "pace: {} frame(s) — virtual {advanced} ms in real {:.0} ms ({:.0}% of speed),                  per frame: median {:.2} ms, p95 {:.2} ms, worst {:.2} ms",
                requested_frames,
                real.as_secs_f64() * 1000.0,
                f64::from(advanced) / (real.as_secs_f64() * 1000.0) * 100.0,
                median.as_secs_f64() * 1000.0,
                p95.as_secs_f64() * 1000.0,
                worst.as_secs_f64() * 1000.0
            );
            // **Drives the Z-Wheel.** The question in this part is practical: with which button
            // does the player confirm the choice, and does the open request reach the core? Each
            // RetroPad button is held for twenty frames and released for ten, and the test stops at
            // the first one the shell accepts. Without the request, it says none worked, which is
            // also an answer.
            did_open = LAST_OPENED.load(Ordering::Relaxed);
            which = None;
            for button in [ID_A, ID_B, ID_X, ID_Y, ID_START, ID_SELECT] {
                let before = FRAMES.load(Ordering::Relaxed);
                BUTTON.store(button, Ordering::Relaxed);
                for _ in 0..20 {
                    retro_run();
                }
                BUTTON.store(u32::MAX, Ordering::Relaxed);
                for _ in 0..10 {
                    retro_run();
                }
                // Did the image change while the button was pressed? That is what separates "input
                // does not get through" from "it gets through, and the button is another one".
                let distinct = SIGNATURES
                    .lock()
                    .map(|v| {
                        let start = (before as usize).min(v.len());
                        v[start..].iter().collect::<std::collections::BTreeSet<_>>().len()
                    })
                    .unwrap_or(0);
                eprintln!("button {button}: {distinct} distinct image(s) in 30 frames");
                did_open = LAST_OPENED.load(Ordering::Relaxed);
                if did_open != 0 {
                    which = Some(button);
                    break;
                }
            }
            retro_unload_game();
            retro_deinit();
        }
        eprintln!(
            "open request: {did_open:#010x} ({})",
            match did_open {
                0 => "no button opened anything".to_string(),
                _ => format!("with button {which:?}"),
            }
        );

        let frames = FRAMES.load(Ordering::Relaxed);
        assert!(frames > 0, "no frame reached the frontend");
        eprintln!("frames delivered to the frontend: {frames}");
        eprintln!(
            "stereo samples delivered to the frontend: {}",
            SAMPLES.load(Ordering::Relaxed)
        );
        eprintln!(
            "games next to the content: {}",
            GAMES_SEEN.load(Ordering::Relaxed)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The RetroPad becomes a console key on the core's path, and the Z-Wheel answers.**
    ///
    /// It is the end-to-end proof of item 8 through the path RetroArch uses, without a window and
    /// without looking at pixels: the loop runs frames, the test presses the RetroPad's buttons,
    /// and what is observed is the **core's decision** (the class that is running, through
    /// [`CURRENT_CLASS`]) and the **shell's request**, through [`LAST_OPENED`], which comes from
    /// `Session::take_launch_request`.
    ///
    /// **Time is the first trap.** The Z-Wheel's game grid only shows up after ~37 s of virtual
    /// clock, more than two thousand passes; the test that existed ran 60 frames and concluded that
    /// no button opened anything. `ZEEBX_CORE_FRAMES` says how many frames to run before starting
    /// to press.
    ///
    /// **The second trap is the folder.** The game the wheel opens is whichever is in focus, and
    /// going back to the wheel only happens when the game **ends by itself**, so the folder must
    /// have, next to the Z-Wheel, a title that ends by itself. Measured in the scan: `Zeebo Clube`
    /// and `Zeebo App` are the two that do that.
    ///
    /// **Without `ZEEBX_CORE_ROM` it does not run**, for the same reason as the others: ROMs do not
    /// go into the tree.
    #[test]
    fn the_retropad_becomes_a_console_key_on_the_core_path() {
        let Ok(rom_path) = std::env::var("ZEEBX_CORE_ROM") else {
            eprintln!("no ZEEBX_CORE_ROM: nothing to go through");
            return;
        };
        // **The translation, proven in the core's crate.** Two pads built by hand: the function is
        // the same as the engine's, but what calls it here is the core, and a type or name error
        // would show up exactly at this point.
        {
            let before = zeebx::input::Pad::default();
            let mut now = zeebx::input::Pad::default();
            let index = zeebx::input::Pad::button_by_name("b1").expect("b1 exists");
            now.press(index, true);
            let key_events = zeebx::input::controller_key_events(&before, &now);
            eprintln!("translation of b1: {key_events:?}");
            assert_eq!(
                key_events,
                vec![(zeebx::input::avk::CONFIRM, true)],
                "the controller translation did not produce the confirm key"
            );
        }
        let dir = std::env::temp_dir().join(format!("zeebx-ciclo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = DIR.set(CString::new(dir.to_string_lossy().to_string()).unwrap());
        if let Ok(outside) = zeebx::config::env_var("ZEEBX_CORE_SYSTEM") {
            let _ = SYSTEM.set(CString::new(outside).unwrap());
        }
        FRAMES.store(0, Ordering::Relaxed);
        LAST_OPENED.store(0, Ordering::Relaxed);
        CURRENT_CLASS.store(0, Ordering::Relaxed);
        BUTTON.store(u32::MAX, Ordering::Relaxed);

        let path_c = CString::new(rom_path.clone()).unwrap();
        let frames_until_grid = zeebx::config::env_var("ZEEBX_CORE_FRAMES")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(2600u32);
        let info = RetroGameInfo {
            path: path_c.as_ptr(),
            data: std::ptr::null(),
            size: 0,
            meta: std::ptr::null(),
        };

        unsafe {
            retro_set_environment(Some(environment));
            retro_set_video_refresh(Some(video));
            retro_set_audio_sample_batch(Some(audio));
            retro_set_input_poll(Some(no_poll));
            retro_set_input_state(Some(entry));
            retro_init();
            retro_set_controller_port_device(0, DEVICE_JOYPAD);
            assert!(retro_load_game(&info), "the core refused {rom_path}");
            for _ in 0..frames_until_grid {
                retro_run();
            }
            // What runs at the start is the wheel. If it is not, the rest of the test would measure
            // something else.
            let who = CURRENT_CLASS.load(Ordering::Relaxed);
            assert_eq!(
                who,
                zeebx::session::Z_WHEEL,
                "at the start the Z-Wheel is what runs, not {who:#010x}"
            );
            eprintln!("running at the start: {who:#010x} (Z-Wheel)");

            // The doc's script, on the RetroPad buttons: confirm on "Play", go down to the covers,
            // move two covers to the right and confirm. `b1` is the console's confirm. **The tempo
            // matters.** In the scan's script the keys are two or three seconds apart: the wheel
            // has transitions armed at 400 ms and a pulse of its own, and pressing every 0.4 s
            // tramples the next screen. Here each step holds 8 frames and waits a second and a half
            // before the next one. **The script, through `ZEEBX_CORE_KEYS` when another one is
            // wanted.** Format `ms:id`, with the RetroPad `id` (`1` is `Y`, the console's confirm),
            // comma-separated: the same spirit as the scan's `ZEEBX_ROM_KEYS`, which is where the
            // cycle was proven first. Without the variable, the doc's script applies: confirm on
            // "Play", go down to the covers, move two covers to the right and confirm. It depends
            // on **where the grid puts the focus**, and that is why it does not suit every folder:
            // with two games, the focus is already on the first and the arrows take it away from
            // there.
            let script: Vec<(u32, u32)> = match zeebx::config::env_var("ZEEBX_CORE_KEYS") {
                Ok(text) => text
                    .split(',')
                    .filter_map(|part| {
                        let (when, id) = part.split_once(':')?;
                        Some((when.trim().parse().ok()?, id.trim().parse().ok()?))
                    })
                    .collect(),
                Err(_) => vec![
                    (0, ID_Y),
                    (0, ID_DOWN),
                    (0, ID_RIGHT),
                    (0, ID_RIGHT),
                    (0, ID_Y),
                    (0, ID_Y),
                    (0, ID_DOWN),
                    (0, ID_Y),
                ],
            };
            let distinct = |since: usize| -> usize {
                SIGNATURES
                    .lock()
                    .map(|v| {
                        let start = since.min(v.len());
                        v[start..].iter().collect::<std::collections::BTreeSet<_>>().len()
                    })
                    .unwrap_or(0)
            };
            // **The wheel animates before the key**, and that is what gives meaning to the
            // measurement afterwards: a screen that was already still would say nothing about the
            // key.
            let animating = distinct(FRAMES.load(Ordering::Relaxed) as usize - 100);
            eprintln!(
                "before the key: {animating} distinct image(s) in 100 frames, clock {} ms, \
                 {} instructions",
                CLOCK.load(Ordering::Relaxed),
                INSTRUCTIONS.load(Ordering::Relaxed)
            );
            assert!(
                animating > 3,
                "the Z-Wheel was not animating before the key ({animating} distinct images)"
            );
            let mut opened = 0u32;
            for (when, step) in script {
                // With an instant in the script, waits for the **clock** to reach it: each
                // `retro_run` advances ~26 ms, not the 16 of a frame at 60 Hz. See [`CLOCK`].
                if when > 0 {
                    for _ in 0..frames_until_grid {
                        if CLOCK.load(Ordering::Relaxed) >= when {
                            break;
                        }
                        retro_run();
                    }
                }
                let before = FRAMES.load(Ordering::Relaxed) as usize;
                BUTTON.store(step, Ordering::Relaxed);
                for _ in 0..8 {
                    retro_run();
                }
                BUTTON.store(u32::MAX, Ordering::Relaxed);
                for _ in 0..90 {
                    retro_run();
                }
                opened = LAST_OPENED.load(Ordering::Relaxed);
                eprintln!(
                    "step {step}: open {opened:#010x}, {} distinct signature(s)",
                    distinct(before)
                );
                if opened != 0 {
                    break;
                }
            }
            // **The request does not go out on the same frame as the key.** In the scan's script
            // the last confirmation is at 38 s and the request shows up almost two seconds later,
            // once the wheel has torn down its screens and armed the launch timer. Checking only
            // right after each key measures the screen, not the outcome.
            for _ in 0..900 {
                retro_run();
                opened = LAST_OPENED.load(Ordering::Relaxed);
                if opened != 0 {
                    break;
                }
            }
            // **The key was handled.** Without translating the controller into console keys, the
            // wheel ignored the RetroPad entirely and kept animating forever: measured, 13 or 14
            // distinct images in every 24 frames, on every step. With the translation, the first
            // confirmation is handled and the screen stops animating.
            let after = distinct(FRAMES.load(Ordering::Relaxed) as usize - 100);
            eprintln!(
                "after the key: {after} distinct image(s) in 100 frames, clock {} ms, \
                 {} instructions",
                CLOCK.load(Ordering::Relaxed),
                INSTRUCTIONS.load(Ordering::Relaxed)
            );
            assert!(
                after < animating,
                "the wheel ignored the key: {after} distinct images after, against {animating} before"
            );
            // **The open request arrives, and the core switches session.** Before the fix for the
            // intermediate screens this was `0x00000000` and the wheel stood still; now it is the
            // class of the game the grid had in focus, the same `0x0108E356` the scan asks for.
            assert_ne!(opened, 0, "the script never got to request opening any game");
            let mut running = CURRENT_CLASS.load(Ordering::Relaxed);
            for _ in 0..900 {
                retro_run();
                running = CURRENT_CLASS.load(Ordering::Relaxed);
                if running != 0 && running != zeebx::session::Z_WHEEL {
                    break;
                }
            }
            assert_eq!(
                running, opened,
                "the core did not open the game the shell asked for ({opened:#010x})"
            );
            // **Going back to the wheel** depends on the game ending by itself, and what the grid
            // has in focus is the folder's first title: with a game that quits by itself next to
            // the Z-Wheel (`Zeebo Clube`, measured in the scan), the core hands control back to the
            // shell, and that is what this loop waits for. Without such a title, it is reported and
            // not enforced. **The window has to be longer than the game.** Going back only happens
            // after the title ends, and the one the grid puts in focus runs ~62 s of virtual clock,
            // more than the 3600 frames (60 s) of the first version, which therefore reported
            // `false` in a case where going back **did happen** (the following keys reach the
            // wheel's handlers).
            let mut returned = false;
            for _ in 0..9000 {
                retro_run();
                if CURRENT_CLASS.load(Ordering::Relaxed) == zeebx::session::Z_WHEEL {
                    returned = true;
                    break;
                }
            }
            eprintln!("went back to the Z-Wheel after the game: {returned}");

            // **What this test still does NOT prove, and is measured instead of assumed:** the open
            // request. On the scan's path it goes out (`opening requested: 0x0108e356`, with the
            // same keys and the same machine), and here it does not, which means the difference is
            // in how the core drives the session, and not in the key, which is what this test
            // proves. The number goes out in the report for the next session to measure from it.
            eprintln!("open request on the core's path: {opened:#010x}");
            retro_unload_game();
            retro_deinit();
        }
        eprintln!(
            "frames delivered to the frontend: {}, {} instructions in {} virtual ms",
            FRAMES.load(Ordering::Relaxed),
            INSTRUCTIONS.load(Ordering::Relaxed),
            CLOCK.load(Ordering::Relaxed)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The save state crosses the ABI, and comes back the same.**
    ///
    /// It is the end-to-end proof of item 6, and it is done through the path RetroArch uses: ask
    /// for the size, save, dirty the state, load and check. The check that matters is the last one
    /// (**saving again after loading has to give the same file byte for byte**), because that is
    /// what a save state promises. Comparing fields would be weaker: what the player sees is the
    /// game continuing from the same point.
    ///
    /// **Without `ZEEBX_CORE_ROM` it does not run**, for the same reason as the test above.
    #[test]
    fn the_save_state_crosses_the_abi() {
        let Ok(rom_path) = std::env::var("ZEEBX_CORE_ROM") else {
            eprintln!("no ZEEBX_CORE_ROM: nothing to save");
            return;
        };
        let dir = std::env::temp_dir().join(format!("zeebx-estado-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = DIR.set(CString::new(dir.to_string_lossy().to_string()).unwrap());
        if let Ok(outside) = zeebx::config::env_var("ZEEBX_CORE_SYSTEM") {
            let _ = SYSTEM.set(CString::new(outside).unwrap());
        }
        FRAMES.store(0, Ordering::Relaxed);
        let path_c = CString::new(rom_path.clone()).unwrap();
        let requested_frames = zeebx::config::env_var("ZEEBX_CORE_FRAMES")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(60u32);

        let info = RetroGameInfo {
            path: path_c.as_ptr(),
            data: std::ptr::null(),
            size: 0,
            meta: std::ptr::null(),
        };
        unsafe {
            retro_set_environment(Some(environment));
            retro_set_video_refresh(Some(video));
            retro_set_audio_sample_batch(Some(audio));
            retro_init();
            retro_set_controller_port_device(0, DEVICE_JOYPAD);
            assert!(retro_load_game(&info), "the core refused {rom_path}");
            for _ in 0..requested_frames {
                retro_run();
            }

            // 1) The size, measured by the frontend.
            let state_size = retro_serialize_size();
            assert!(state_size > 0, "with a session built, the size has to be positive");
            eprintln!("save state: {state_size} bytes");
            let mut first = vec![0u8; state_size];
            assert!(
                retro_serialize(first.as_mut_ptr() as *mut c_void, first.len()),
                "the save was refused"
            );

            // 2) The game goes on: the state has to become **different**.
            for _ in 0..requested_frames {
                retro_run();
            }
            let size_after = retro_serialize_size();
            let mut second = vec![0u8; size_after];
            assert!(retro_serialize(
                second.as_mut_ptr() as *mut c_void,
                second.len()
            ));
            assert_ne!(
                first, second,
                "the state did not change after running more frames: the test would be measuring nothing"
            );

            // 3) Load the first one, and check that saving again gives the same file.
            assert!(
                retro_unserialize(first.as_ptr() as *const c_void, first.len()),
                "the load was refused"
            );
            let size_back = retro_serialize_size();
            let mut third = vec![0u8; size_back];
            assert!(retro_serialize(
                third.as_mut_ptr() as *mut c_void,
                third.len()
            ));
            if third != first {
                // **Where** the two diverge, not the whole comparison: an `assert_eq!` of two
                // seven-megabyte vectors writes seven megabytes to the output and says nothing. The
                // section name is in the file as text, so the first difference points at the
                // culprit.
                let position = third
                    .iter()
                    .zip(first.iter())
                    .position(|(a, b)| a != b)
                    .unwrap_or(third.len().min(first.len()));
                let start = position.saturating_sub(64);
                let text = String::from_utf8_lossy(&third[start..position + 8]);
                let text = text
                    .chars()
                    .filter(|c| c.is_ascii_graphic() || *c == ' ')
                    .collect::<String>();
                let before = String::from_utf8_lossy(&first[start..position + 8]);
                let before = before
                    .chars()
                    .filter(|c| c.is_ascii_graphic() || *c == ' ')
                    .collect::<String>();
                panic!(
                    "the loaded state is not the one that was saved: first difference at byte {position} \
                     of {} (saved {start}..{}: {before:?}; loaded: {text:?})",
                    third.len(),
                    position + 8
                );
            }

            // 4) A flipped byte is refused, and does **not** change the machine.
            let mut corrupted = first.clone();
            let middle = corrupted.len() / 2;
            corrupted[middle] ^= 0xff;
            assert!(
                !retro_unserialize(corrupted.as_ptr() as *const c_void, corrupted.len()),
                "a corrupted state was accepted"
            );
            let final_size = retro_serialize_size();
            let mut quarter = vec![0u8; final_size];
            assert!(retro_serialize(quarter.as_mut_ptr() as *mut c_void, quarter.len()));
            assert_eq!(
                quarter, first,
                "refusing a corrupted state touched the machine"
            );

            retro_unload_game();
            retro_deinit();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
