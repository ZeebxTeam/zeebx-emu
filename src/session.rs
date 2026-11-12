//! A running game, from the file to the frames on screen.
//!
//! It exists to separate the BREW lifecycle — loading the module, creating the applet, delivering
//! `EVT_APP_START`, spinning the event loop — from whoever observes it. The command line runs that
//! loop up to a limit and prints the result; the interface drives it one piece per drawn frame.
//! What changes is who calls [`Session::step`], not what it does.

use std::path::Path;
use std::time::{Duration, Instant};

/// On desktop and in the native core the session recompiles the blocks. On `wasm32` the emitted
/// block does not run in the browser. On iOS the system refuses the executable page Dynarmic
/// allocates — the simulator is also `TARGET_OS_IPHONE`. On the Vita the host is a Cortex-A9 and
/// dynarmic does not emit ARMv7. In those three, the interpreter takes the same place.
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "vita")))]
use crate::cpu::dynarmic::DynarmicCpu as SessionCpu;
#[cfg(any(target_arch = "wasm32", target_os = "ios", target_os = "vita"))]
use crate::cpu::interpreter::Interpreter as SessionCpu;
use crate::input::Pad;
use crate::library;
use crate::loader;
use crate::loader::archive;
use crate::loader::modfile::ModImage;
use crate::machine::{AppletResult, Machine, Outcome};
use crate::storage::StoragePaths;
use crate::speed::{SkipCounter, Skip, Pace};
use crate::video::display::{Framebuffer, Rect};

/// Largest slice of real time the frontends may ask for in one pass.
///
/// Shared so that desktop, headless and Android do not reintroduce a 16 ms ceiling and reduce
/// fast games like Crash Nitro Kart to a fraction of their speed.
pub const MAX_SLICE: Duration = Duration::from_millis(100);

/// Instruction ceiling per slice between two API calls — keeps an infinite loop in the guest from
/// hanging the emulator.
const INSTRUCTION_BUDGET: u64 = 500_000_000;

/// Over how much real time to measure the speed shown in the interface.
///
/// Half a second is short enough for the reading to follow a screen change and long enough not to
/// jitter every frame.
const SPEED_WINDOW_MS: u64 = 500;

/// How many samples the graph keeps. At half a second each, that is one minute of history.
const HISTORY: usize = 120;

/// How many game frames one window pass may run to catch up with the wall clock.
/// See [`Session::step`].
const FRAMES_PER_PASS: u32 = 4;

/// The lag from which the game catches up frames: one 60 Hz frame.
const CATCH_UP_LAG_MS: u64 = 17;

/// The slice of real time an unthrottled pass — or one above 1x — may spend emulating.
///
/// **Without this ceiling the window collapses.** The budget the frontend passes is the real time
/// since the previous pass; a pass that spent all of it would overrun the vsync, the next one
/// would get twice as much, and within a few passes the window would be at the [`MAX_SLICE`]
/// ceiling, at 10 frames per second. Twelve milliseconds leave four for the window to draw in a
/// 60 Hz vsync. The number is the vsync arithmetic, **not** a measurement: how much the window
/// actually needs was not measured.
const UNTHROTTLED_SLICE: Duration = Duration::from_millis(12);

/// Milliseconds of a frame at 60 Hz: the step a Libretro frontend advances per call.
const FRAME_MS: u64 = 16;

/// Ceiling of inner passes per virtual frame, against a guest that neither advances nor presents.
const MAX_STEPS_PER_FRAME: u32 = 200_000;

/// The largest lag the game catches up. Beyond it the lag is forgiven, not chased: a pause, a
/// load or a dragged window stop the virtual clock while the real one runs, and catching up all of
/// it afterwards would make the game race.
const MAX_LAG_MS: u64 = 250;

/// Why a game could not start.
#[derive(Debug)]
pub enum StartError {
    /// The file could not be read.
    Unreadable(std::io::Error),
    /// Not a `.mod` we know how to read.
    NotAModule(String),
    /// The module did not fit in guest memory.
    NotLoadable(String),
    /// The emulator stopped before the game started.
    Stopped(Outcome),
    /// There is no `.mif` next to the module, so we do not know which applet to create.
    NoApplet,
    /// `CreateInstance` refused, with the BREW error.
    Refused(u32),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(err) => write!(f, "could not read the file: {err}"),
            Self::NotAModule(err) => write!(f, "not a valid module: {err}"),
            Self::NotLoadable(err) => write!(f, "could not load the module: {err}"),
            Self::Stopped(outcome) => write!(f, "stopped before starting ({outcome:?})"),
            Self::NoApplet => write!(f, "no .mif next to the module says which applet to create"),
            Self::Refused(code) => write!(f, "CreateInstance refused with error {code}"),
        }
    }
}

/// What a slice of execution produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The game presented at least one new frame.
    Presented,
    /// The slice ended without a new frame — normal, the game goes on.
    Running,
    /// The game is ahead of the wall clock and was held back.
    Ahead,
    /// The game ended or crashed. The reason is in [`Session::stopped`].
    Stopped,
}

pub struct Session {
    /// The same BREW scheduler used by the bench and the command line, with the core that
    /// recompiles the module's ARM blocks. Kingdom Hearts draws its intro in its own ARM
    /// rasterizer; that is why the session uses the JIT directly. On `wasm32` that place is the
    /// interpreter.
    machine: Machine<SessionCpu>,
    /// The applet created and **not yet** started, with its ClassID.
    ///
    /// `EVT_APP_START` is dispatched on the first pass of the loop, not here. Running it inside
    /// `start` made the game begin before there was a window and before there was sound output:
    /// the Z-Wheel plays `sounds_loading.wav` at startup, and it came out with an empty screen —
    /// or, after sound started being turned on only once the window was ready, it did not come out
    /// at all, because the game had already played it.
    running_game: Option<(u32, u32)>,
    /// The host sound output. A Libretro frontend does not have this piece: it receives the mixer.
    #[cfg(feature = "audio")]
    audio: Option<crate::audio::Output>,
    title: String,
    /// The identity of the file the frontend opened, when the profile has an overlay. It is the
    /// folder for the game's saves and save state slots. See [`Session::content`].
    content: Option<crate::storage::ContentId>,
    /// The ClassID of this session's applet.
    class_id: u32,
    /// The intermediate screen on display, when there is one.
    /// See [`Session::show_intermediate_frame`].
    intermediate: Option<Framebuffer>,
    /// A phone game's area of the screen, cropped and centred. See [`Session::display_screen`].
    phone_views: Option<PhoneViews>,
    /// Instant and virtual clock reading when execution started, which is the pair used to
    /// measure whether the game is ahead.
    started: Instant,
    clock_base: u64,
    /// The ratio [`Session::started`] and [`Session::clock_base`] were anchored with.
    /// `None` is unthrottled. Changing the ratio **re-anchors**: see [`Session::set_ratio`].
    ratio: Option<f32>,
    /// The draw skip of the current frame. See [`SessionSkip`].
    skip: SessionSkip,
    /// The speed the sound follows. See [`Session::sync_sound`].
    fast_forward_sound: FastForwardSound,
    /// The sound stopped by the pause. See [`Session::freeze_audio`].
    sound_frozen: bool,
    /// The watchdog for a game that falls behind. See [`SpeedWatchdog`].
    watch: SpeedWatchdog,
    /// The turbo: each port's raw controller and the buttons that pulse. See [`Session::pulse`].
    turbo: SessionTurbo,
    stopped: Option<Outcome>,
    /// Start of the measurement window: the real instant, the virtual clock, the instructions and
    /// the frames at that time. Everything the debug panel shows comes from the difference between
    /// two of these readings.
    window: Mark,
    /// The last closed sample.
    sample: Sample,
    /// The recent samples, for the graph. Newest at the end.
    history: std::collections::VecDeque<Sample>,
}

/// The draw skip as the session decides it: once per frame, at its start.
///
/// **At the start, not on every pass.** A frame can span two window passes — the slice ends before
/// `eglSwapBuffers` —, and deciding again in the middle of it would draw half the scene. That is
/// why the decision keeps the buffer swap number it was taken at, and only the next swap opens the
/// next one.
#[derive(Debug, Clone, Copy, Default)]
struct SessionSkip {
    counter: SkipCounter,
    /// The `gl_swaps` at which the decision in force was taken.
    decided_at: Option<u32>,
    /// The decision in force.
    current: Skip,
    /// How much real time the last skipped frame took, and the last drawn one.
    ///
    /// **Separate, because a skipped frame costs a fraction of a drawn one**, and that is what the
    /// auto prediction asks: "skipping this one, does a drawn one still fit in the slice?". With a
    /// single measure, that of the last drawn frame, the prediction never saw slack in a heavy game
    /// — measured on Rolima, unlimited stayed at 3.95x, below the 8.27x of 10x.
    skipped_duration: Duration,
    drawn_duration: Duration,
}

/// How many frames one pass may run at the given ratio.
///
/// At 1x it is the usual [`FRAMES_PER_PASS`], to catch up with the clock. Above, it grows in the
/// same proportion: at 10x, a pass needs ten frames just to keep up. Unthrottled, the slice is
/// what limits it.
fn frame_cap(ratio: Option<f32>) -> u32 {
    match ratio {
        Some(ratio) => (FRAMES_PER_PASS as f32 * ratio.max(1.0)).ceil() as u32,
        None => u32::MAX,
    }
}

/// The turbo as the session applies it. See [`crate::speed::turbo`].
#[derive(Debug, Clone, Copy)]
struct SessionTurbo {
    /// Each port's controller as the frontend delivered it, before the turbo.
    raw: [Pad; crate::input::PORTS],
    /// The raw state the last time the keys were checked: whatever changed since then, the
    /// frontend has already turned into a key.
    raw_seen: [Pad; crate::input::PORTS],
    /// The buttons that pulse on each port.
    pulsing: [u32; crate::input::PORTS],
    presses: u8,
    /// Whether the frontend turns the controller into BREW keys. See [`Session::turbo_on_keys`].
    on_keys: bool,
    /// The keys the session asserted over the frontend, per button bit, and the value of each.
    asserted: [u32; crate::input::PORTS],
    values: [u32; crate::input::PORTS],
}

impl Default for SessionTurbo {
    fn default() -> Self {
        Self {
            raw: Default::default(),
            raw_seen: Default::default(),
            pulsing: [0; crate::input::PORTS],
            presses: crate::speed::turbo::DEFAULT_PRESSES,
            on_keys: false,
            asserted: [0; crate::input::PORTS],
            values: [0; crate::input::PORTS],
        }
    }
}

/// The buttons that also reach the game as a BREW key, and each one's key. It is the same table as
/// [`crate::input::controller_key_events`], minus the d-pad, which never pulses.
const BUTTONS_WITH_KEY: [(&str, u32); 2] = [
    ("b1", crate::input::avk::CONFIRM),
    ("b2", crate::input::avk::CLR),
];

/// What the fast-forward sound needs to remember between two passes. See
/// [`Session::sync_sound`].
#[derive(Debug, Clone, Copy)]
struct FastForwardSound {
    /// The real instant and the virtual clock of the previous pass.
    last: Option<(Instant, u64)>,
    /// The speed reached, on average. `None` before the first unlimited measurement.
    media: Option<f64>,
    /// The ratio requested on the previous pass: changing the ratio restarts its average.
    requested: Option<Option<f32>>,
    /// The last ratio above 1x and the average it stopped at: it is where the next press with the
    /// same ratio starts.
    remembered: Option<(Option<f32>, f64)>,
    /// The last pace and the last muffle delivered to the mixer.
    sent: Option<(f32, bool)>,
    /// Silenced by the rewind. See [`Session::mute`].
    muted: bool,
    /// Whether the last pass's pace asked for silence — the fast-forward "no sound".
    muted_by_pace: bool,
}

impl Default for FastForwardSound {
    fn default() -> Self {
        Self {
            last: None,
            media: Some(1.0),
            requested: None,
            remembered: None,
            sent: None,
            muted: false,
            muted_by_pace: false,
        }
    }
}

/// What the session watches to say in the log that the game fell behind.
///
/// **It is the diagnosis for the frameskip that "froze" in Qt** (NFS on an EndeavourOS machine:
/// with any choice other than auto, image and sound stopped while the window kept responding). It
/// did not reproduce, and the suspicion is that the game runs below real time on that machine:
/// auto is the only choice that skips frames when the game lags. When the game advances less than
/// half of what the pace asks, the log line says with which choices, at what speed and how much
/// each frame cost — which tells "too slow" from "really stopped".
#[derive(Debug, Clone, Copy, Default)]
struct SpeedWatchdog {
    /// The start of the measurement window: the real instant and the virtual clock.
    start: Option<(Instant, u64)>,
    /// The last pass, to spot a gap — pause, rewind, dragged window — and start over.
    last: Option<Instant>,
    /// Passes in the window, and in how many of them the game was ahead.
    laps: u32,
    ahead_count: u32,
    /// Whether the slow warning has already gone out and is waiting for recovery.
    slow: bool,
}

/// The watchdog window, in real time.
const WATCHDOG_WINDOW: Duration = Duration::from_secs(1);

/// The time constant of the speed average the sound follows, in seconds. Half a second is the same
/// [`SPEED_WINDOW_MS`] the panel uses: short to follow a scene change, long so as not to jitter.
/// It was not tuned by ear.
const SOUND_AVERAGE_S: f64 = 0.5;

/// Above this, the time between two passes is a gap, not speed.
const SOUND_GAP_S: f64 = 0.25;

/// A reading of the counters at one instant.
#[derive(Debug, Clone, Copy)]
struct Mark {
    real: Instant,
    clock_ms: u64,
    instructions: u64,
    frames: u32,
}

/// What happened in a measurement window.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sample {
    /// Fraction of the console's speed, in percent.
    pub speed: u32,
    /// Frames presented per second of real time.
    pub fps: u32,
    /// Guest instructions executed per second of real time.
    ///
    /// Not in millions: on a waiting screen the game yields and barely executes, and rounding to
    /// millions would show zero exactly when the number matters.
    pub ips: u64,
}

impl Session {
    /// Loads the module, creates the applet and delivers `EVT_APP_START`.
    ///
    /// A `.zip` is extracted to the cache first: the game writes (Peteca has a `.sav`), and writing
    /// back into a package is not something you want to do.
    /// Like [`Session::start`], but with the device already configured before the game starts.
    ///
    /// **The order matters.** `start` runs `AEEMod_Load`, creates the applet and dispatches
    /// `EVT_APP_START` — all before returning. A game that enumerates `IHID` at startup, like the
    /// Z-Wheel, has already asked what is connected before any adjustment made afterwards: with
    /// the ports applied only on the pass, it saw a controller and no keyboard, however much the
    /// configuration said otherwise.
    pub fn start_with(
        path: &Path,
        ports: [Option<crate::input::bindings::Device>; crate::input::PORTS],
        serial: Option<&Path>,
        gpu: bool,
        context: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
    ) -> Result<Self, StartError> {
        Self::start_inner(path, Some(ports), serial, gpu, context, z_wheel, &[])
    }

    /// Like [`Session::start_with`], but installs the modules before the guest boots.
    ///
    /// The Z-Wheel enumerates the games during `EVT_APP_START`. Installing after the session
    /// already exists is too late: its list is born empty and the gesture can never pick a game.
    pub fn start_with_installed(
        path: &Path,
        ports: [Option<crate::input::bindings::Device>; crate::input::PORTS],
        serial: Option<&Path>,
        gpu: bool,
        context: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        installed: &[(u32, String)],
    ) -> Result<Self, StartError> {
        Self::start_inner(path, Some(ports), serial, gpu, context, z_wheel, installed)
    }

    /// Like [`Session::start_with`], but receives the persistent root explicitly.
    ///
    /// It is the boundary the Libretro frontend will use: package cache and `fs:/` stop depending
    /// on the desktop UI configuration.
    #[allow(dead_code)] // consumed by `frontends/libretro`, not yet created.
    pub fn start_with_storage(
        path: &Path,
        ports: [Option<crate::input::bindings::Device>; crate::input::PORTS],
        serial: Option<&Path>,
        gpu: bool,
        context: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
    ) -> Result<Self, StartError> {
        Self::start_inner_with_storage(
            path,
            Some(ports),
            serial,
            gpu,
            context,
            z_wheel,
            storage,
            &[],
        )
    }

    /// Like [`Session::start_with_storage`], with the applets installed before the boot.
    ///
    /// The Libretro core uses this entry point for the Z-Wheel: it enumerates the library on the
    /// first `EVT_APP_START`, so `set_installed_applets` after the return is not enough.
    #[allow(dead_code)]
    pub fn start_with_storage_installed(
        path: &Path,
        ports: [Option<crate::input::bindings::Device>; crate::input::PORTS],
        serial: Option<&Path>,
        gpu: bool,
        context: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
        installed: &[(u32, String)],
    ) -> Result<Self, StartError> {
        Self::start_inner_with_storage(
            path,
            Some(ports),
            serial,
            gpu,
            context,
            z_wheel,
            storage,
            installed,
        )
    }

    /// Starts the engine without a window, audio device or host graphics context.
    ///
    /// No caller until `frontends/libretro` exists; the dead code warning is silenced on
    /// purpose.
    ///
    /// This is the Libretro core's entry point: video, audio and input are provided by frontend
    /// callbacks, and storage already comes scoped in [`StoragePaths`].
    #[allow(dead_code)] // consumed by `frontends/libretro`, not yet created.
    pub fn start_software_with_storage(
        path: &Path,
        ports: [Option<crate::input::bindings::Device>; crate::input::PORTS],
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
    ) -> Result<Self, StartError> {
        Self::start_inner_with_storage(path, Some(ports), None, false, None, z_wheel, storage, &[])
    }

    /// Software variant of [`Session::start_software_with_storage`] with the known library.
    #[allow(dead_code)]
    pub fn start_software_with_storage_installed(
        path: &Path,
        ports: [Option<crate::input::bindings::Device>; crate::input::PORTS],
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
        installed: &[(u32, String)],
    ) -> Result<Self, StartError> {
        Self::start_software_with_storage_installed_policy(
            path,
            ports,
            z_wheel,
            storage,
            installed,
            crate::audio::MidiBackend::Auto,
        )
    }

    /// Software variant receiving the MIDI policy explicitly.
    #[allow(dead_code)]
    pub fn start_software_with_storage_installed_policy(
        path: &Path,
        ports: [Option<crate::input::bindings::Device>; crate::input::PORTS],
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
        installed: &[(u32, String)],
        midi_policy: crate::audio::MidiBackend,
    ) -> Result<Self, StartError> {
        Self::start_inner_with_storage_policy(
            path,
            Some(ports),
            None,
            false,
            None,
            z_wheel,
            storage,
            installed,
            midi_policy,
        )
    }

    /// Hardware-accelerated variant receiving the MIDI policy explicitly.
    #[allow(dead_code)]
    pub fn start_with_storage_installed_policy(
        path: &Path,
        ports: [Option<crate::input::bindings::Device>; crate::input::PORTS],
        serial: Option<&Path>,
        gpu: bool,
        context: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
        installed: &[(u32, String)],
        midi_policy: crate::audio::MidiBackend,
    ) -> Result<Self, StartError> {
        Self::start_inner_with_storage_policy(
            path,
            Some(ports),
            serial,
            gpu,
            context,
            z_wheel,
            storage,
            installed,
            midi_policy,
        )
    }

    /// The serial goes in **before the module is created**, not after the session exists.
    ///
    /// The applet constructor runs inside `CreateInstance`, in here: turning capture on only
    /// afterwards left out everything it does at birth — including `Zeeboids v 1.1.1402`, which
    /// showed up in the report and not in the capture. A capture with a hole at the start is worse
    /// than none, because nobody knows there is a hole.
    fn start_inner(
        path: &Path,
        ports: Option<[Option<crate::input::bindings::Device>; crate::input::PORTS]>,
        serial: Option<&Path>,
        gpu: bool,
        context: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        installed: &[(u32, String)],
    ) -> Result<Self, StartError> {
        let storage = StoragePaths::from_root(crate::config::config_dir());
        // Up to 0.4.0 the desktop wrote the save inside the extraction, in the cache. Copying it to
        // the overlay takes the save out of the pruning's reach; the source stays, and the overlay
        // is never overwritten. Failing here does not stop the game: the VFS still reads the old
        // copy when the overlay does not have it.
        if let Ok(id) = storage.content_id(path) {
            match storage.migrate_legacy_package_writes(path, &id) {
                Ok(bytes) if bytes > 0 => crate::logging!(
                    crate::logging::Level::Info,
                    "session",
                    "{bytes} bytes of old saves copied from the cache to {}",
                    storage.save_for(&id).display()
                ),
                Ok(_) => {}
                Err(error) => crate::logging!(
                    crate::logging::Level::Warning,
                    "session",
                    "could not copy the old saves from the cache: {error}"
                ),
            }
        }
        Self::start_inner_with_storage(path, ports, serial, gpu, context, z_wheel, &storage, installed)
    }

    fn start_inner_with_storage(
        path: &Path,
        ports: Option<[Option<crate::input::bindings::Device>; crate::input::PORTS]>,
        serial: Option<&Path>,
        gpu: bool,
        context: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
        installed: &[(u32, String)],
    ) -> Result<Self, StartError> {
        Self::start_inner_with_storage_policy(
            path,
            ports,
            serial,
            gpu,
            context,
            z_wheel,
            storage,
            installed,
            crate::audio::MidiBackend::Auto,
        )
    }

    fn start_inner_with_storage_policy(
        path: &Path,
        ports: Option<[Option<crate::input::bindings::Device>; crate::input::PORTS]>,
        serial: Option<&Path>,
        gpu: bool,
        context: Option<std::sync::Arc<glow::Context>>,
        z_wheel: crate::config::ZWheel,
        storage: &StoragePaths,
        installed: &[(u32, String)],
        midi_policy: crate::audio::MidiBackend,
    ) -> Result<Self, StartError> {
        // Path chosen in the frontend, before extracting: it is what identifies the content.
        let content = path;
        let extracted;
        let path = match path.extension().and_then(|e| e.to_str()) {
            // `.7z` extracts through the same path: the unpacker is what tells the formats apart.
            Some("zip" | "7z") => {
                extracted =
                    archive::extract_in(path, &storage.cache).map_err(StartError::Unreadable)?;
                extracted.as_path()
            }
            _ => path,
        };
        crate::logging!(
            crate::logging::Level::Debug,
            "session",
            "content {} resolved to {}",
            content.display(),
            path.display()
        );
        let bytes = std::fs::read(path).map_err(StartError::Unreadable)?;
        crate::logging!(
            crate::logging::Level::Debug,
            "session",
            "{} bytes read; parsing the module",
            bytes.len()
        );
        let image = ModImage::parse(bytes).map_err(|e| StartError::NotAModule(e.to_string()))?;
        let extensions = extensions_of(path);
        let module = loader::load_with(&image, &extensions)
            .map_err(|e| StartError::NotLoadable(e.to_string()))?;

        // The root of the game's file system is the directory the `.mod` is in: that is where
        // the console keeps the title's files.
        let root = path.parent().map(Path::to_path_buf).unwrap_or_default();
        // The hash is of the file chosen by the frontend (`.zip` or `.mod`), before any
        // extraction. It no longer names the save — the store is shared, per module folder —,
        // but it still finds the old overlay to migrate and the title to show.
        let content_id = match storage.overlay {
            true => {
                let id = storage
                    .content_id(content)
                    .map_err(|e| StartError::Unreadable(e))?;
                write_title(storage, &id, content);
                Some(id)
            }
            false => None,
        };
        // The name under which the VFS shows the module in `/mod/<dir>/`, and its folder in the
        // store.
        let module_dir = root.file_name().map(|n| n.to_string_lossy().into_owned());
        if let (Some(id), Some(dir_name)) = (&content_id, &module_dir) {
            write_module_title(storage, dir_name, content);
            match storage.migrate_to_shared_saves(id, dir_name) {
                Ok(bytes) if bytes > 0 => crate::logging!(
                    crate::logging::Level::Info,
                    "session",
                    "{bytes} bytes of old saves copied to {}",
                    storage.mod_saves().display()
                ),
                Ok(_) => {}
                Err(error) => crate::logging!(
                    crate::logging::Level::Warning,
                    "session",
                    "could not copy the old saves to the shared store: {error}"
                ),
            }
        }
        let save_root = Some(storage.mod_saves());
        let cpu = SessionCpu::new().map_err(|e| StartError::NotLoadable(e.to_string()))?;
        let mut machine = Machine::new_with_storage_policy(cpu, module, root, storage, save_root, midi_policy);
        // The list must exist before `run` and `create_applet`: the Z-Wheel enumerates it at boot.
        machine.set_installed_applets(installed.iter().cloned());
        // Before any drawing: see [`Machine::use_gpu`].
        let has_context = context.is_some();
        machine.use_gpu(gpu, context);
        machine.configure_z_wheel(z_wheel);
        // The screen the console opens the Z-Wheel with. See [`ZWHEEL_SPLASH`].
        if library::applet_clsid(path) == Some(Z_WHEEL) {
            if let Some(splash) = path
                .parent()
                .and_then(|dir| std::fs::read(dir.join(ZWHEEL_SPLASH)).ok())
            {
                machine.paint_screen_rgb565(&splash);
            }
        }
        if let Some(serial_path) = serial {
            if let Some(dir) = serial_path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(error) = machine.enable_serial(serial_path) {
                crate::logging!(
                    crate::logging::Level::Warning,
                    "session",
                    "the serial capture did not open: {error}"
                );
            }
        }
        if let Some(ports) = ports {
            machine.set_ports(ports);
        }

        let outcome = machine
            .run(INSTRUCTION_BUDGET)
            .map_err(|e| StartError::NotLoadable(e.to_string()))?;
        if !matches!(outcome, Outcome::Returned { code: 0 }) {
            return Err(StartError::Stopped(outcome));
        }

        let clsid = library::applet_clsid(path).ok_or(StartError::NoApplet)?;
        let created = machine
            .create_applet(clsid, INSTRUCTION_BUDGET)
            .map_err(|e| StartError::NotLoadable(e.to_string()))?;
        let applet = match created {
            AppletResult::Called { code: 0, applet } if applet != 0 => applet,
            AppletResult::Called { code, .. } => return Err(StartError::Refused(code)),
            AppletResult::Stopped(stop) => return Err(StartError::Stopped(stop)),
            AppletResult::NoModule => return Err(StartError::NoApplet),
        };
        let title = library::title_for(path);
        // **One INFO line per session, with what answers "what is running and how".** It is the
        // pair missing from the core's report: the title said the game and nothing said the
        // rasterizer.
        crate::logging!(
            crate::logging::Level::Info,
            "session",
            "opened {} (class {clsid:#010x}) with the {} and {} applet(s) installed",
            // The name of the **requested content**, not `title`: in a `.zip` the session title
            // comes from the cache folder, which carries size and date and tells nothing to whoever
            // reads the log.
            content
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| content.display().to_string()),
            match gpu {
                true => "GPU rasterizer",
                false => "CPU rasterizer",
            },
            installed.len()
        );
        if gpu && !has_context {
            crate::logging!(
                crate::logging::Level::Warning,
                "session",
                "GPU requested without a GL context: drawing stays on the CPU"
            );
        }
        let clock_base = u64::from(machine.clock_ms());
        let window = Mark {
            real: Instant::now(),
            clock_ms: clock_base,
            instructions: machine.instructions(),
            frames: machine.gl_swaps(),
        };
        Ok(Self {
            machine,
            running_game: Some((applet, clsid)),
            #[cfg(feature = "audio")]
            audio: None,
            title,
            content: content_id,
            class_id: clsid,
            intermediate: None,
            phone_views: None,
            started: Instant::now(),
            clock_base,
            ratio: Some(1.0),
            skip: SessionSkip::default(),
            fast_forward_sound: FastForwardSound::default(),
            sound_frozen: false,
            watch: SpeedWatchdog::default(),
            turbo: SessionTurbo::default(),
            stopped: None,
            window,
            sample: Sample::default(),
            history: std::collections::VecDeque::new(),
        })
    }

    /// Advances the game until it presents a frame, spends `budget` of real time, or stops.
    ///
    /// The real-time ceiling is what keeps the interface alive: one pass of the game loop can be a
    /// tiny slice of instructions, and handing control back regularly is what allows redrawing
    /// and serving the keyboard while the game runs.
    ///
    /// It is [`Session::advance`] with the usual pace: the console's speed, or unthrottled.
    pub fn step(&mut self, budget: Duration, speed_limit: bool) -> Step {
        let pace = match speed_limit {
            true => Pace::CONSOLE,
            false => Pace::UNTHROTTLED,
        };
        self.advance(budget, &pace)
    }

    /// [`Session::step`] with the whole pace: the ratio, the draw skip and the half-rate
    /// presentation. See [`crate::speed`].
    ///
    /// **One pass can run several frames, and only the last one goes to the screen.** It is what
    /// lets the game catch up with the clock when the window falls below the vsync, and what lets
    /// it run above 1x: at 3x, a pass of 16 real ms needs three game frames.
    pub fn advance(&mut self, budget: Duration, pace: &Pace) -> Step {
        let step = self.advance_without_sound(budget, pace);
        self.sync_sound(pace);
        self.watch(pace, step);
        step
    }

    /// Says in the log when the game advances less than half of what the pace asks, and when it
    /// recovers. See [`SpeedWatchdog`].
    fn watch(&mut self, pace: &Pace, step: Step) {
        let now = Instant::now();
        let clock = u64::from(self.machine.clock_ms());
        let watch = &mut self.watch;
        let hole = watch
            .last
            .is_none_or(|last| (now - last).as_secs_f64() > SOUND_GAP_S);
        watch.last = Some(now);
        // Unthrottled there is no target to fall behind; stopped, there is no game.
        let Some(target) = pace.ratio.filter(|_| step != Step::Stopped) else {
            watch.start = None;
            return;
        };
        if hole {
            *watch = SpeedWatchdog {
                start: Some((now, clock)),
                last: Some(now),
                slow: watch.slow,
                ..SpeedWatchdog::default()
            };
            return;
        }
        watch.laps += 1;
        if step == Step::Ahead {
            watch.ahead_count += 1;
        }
        let Some((start, start_clock)) = watch.start else {
            watch.start = Some((now, clock));
            return;
        };
        let elapsed = now - start;
        if elapsed < WATCHDOG_WINDOW {
            return;
        }
        let speed = clock.saturating_sub(start_clock) as f64 / elapsed.as_millis() as f64;
        let fraction = speed / f64::from(target);
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        if fraction < 0.5 && !watch.slow {
            watch.slow = true;
            crate::logging!(
                crate::logging::Level::Warning,
                "velocidade",
                "the game ran {:.0}% of the request in the last second ({speed:.2}x of {target}x): \
                 frameskip {:?}, half-rate presentation {}, drawn frame {:.1} ms, \
                 skipped {:.1} ms, {} pass(es), {} ahead",
                fraction * 100.0,
                pace.frameskip,
                pace.half_rate_present,
                ms(self.skip.drawn_duration),
                ms(self.skip.skipped_duration),
                watch.laps,
                watch.ahead_count
            );
        } else if fraction >= 0.9 && watch.slow {
            watch.slow = false;
            crate::logging!(
                crate::logging::Level::Info,
                "velocidade",
                "the game is back to {:.0}% of the request ({speed:.2}x), with frameskip {:?}",
                fraction * 100.0,
                pace.frameskip
            );
        }
        watch.start = Some((now, clock));
        watch.laps = 0;
        watch.ahead_count = 0;
    }

    /// The sound follows the speed the game **reached**, not the requested one.
    ///
    /// With fast-forward at 10x and the host reaching 4x, a pace of 10 would leave the sound two
    /// and a half times ahead of the game: the effects would end before its `Stop`, and the stream
    /// queue would run dry in pops. The measured speed, on a half-second average, keeps the two
    /// together; it swings from light frame to heavy frame, and the average is what keeps the pitch
    /// from wobbling.
    ///
    /// **The average starts from the requested ratio, not from 1x.** Starting from 1x, it took a
    /// second and a half to get near 3x: the game sped up at once — measured on Action Hero 3D,
    /// 2.8x already in the first 100 ms —, and the sound and the label climbed slowly behind it,
    /// and fast-forward felt gradual. Starting from the request, both jump together with the game,
    /// and the average only comes down when the host cannot keep up. From the second press on, with
    /// the same ratio, it starts from where it stopped last time: on the same Action Hero at 10x
    /// the host reaches 6.7x, and starting from 10 on each press made the pitch slide for a second
    /// and a half. Unlimited has no request: it starts from the last time, or from the first
    /// measurement.
    ///
    /// At 1x the sound stays at exactly 1: the measurement swings around it, and following the
    /// swing would put the music out of tune for someone who is not even fast-forwarding.
    fn sync_sound(&mut self, pace: &Pace) {
        let now = Instant::now();
        let clock = u64::from(self.machine.clock_ms());
        let sound = &mut self.fast_forward_sound;
        if sound.requested != Some(pace.ratio) {
            if let (Some(requested), Some(media)) = (sound.requested, sound.media)
                && requested != Some(1.0)
            {
                sound.remembered = Some((requested, media));
            }
            sound.requested = Some(pace.ratio);
            sound.media = match sound.remembered {
                Some((requested, media)) if requested == pace.ratio => Some(media),
                _ => pace.ratio.map(f64::from),
            };
            sound.last = None;
        }
        let target = match pace.ratio {
            Some(1.0) => 1.0,
            _ => {
                if let Some((before, clock_before)) = sound.last {
                    let dt = (now - before).as_secs_f64();
                    // A large gap — pause, dragged window — is not speed: it only restarts the
                    // measurement, without pulling the average down.
                    if dt > 0.0 && dt <= SOUND_GAP_S {
                        let instant = clock.saturating_sub(clock_before) as f64 / 1000.0 / dt;
                        let weight = dt / (SOUND_AVERAGE_S + dt);
                        sound.media = Some(match sound.media {
                            Some(media) => media + (instant - media) * weight,
                            None => instant,
                        });
                    }
                }
                sound.last = Some((now, clock));
                sound.media
                    .unwrap_or(1.0)
                    .clamp(0.25, f64::from(crate::speed::FastForward::MAX) + 2.0)
            }
        };
        sound.muted_by_pace = pace.mute_sound;
        let request = (target as f32, pace.mute_sound || sound.muted);
        // Half a percent of difference cannot be heard, and it spares the audio line's lock.
        let changed = sound.sent.is_none_or(|(sent, silenced)| {
            silenced != request.1 || (sent - request.0).abs() > sent * 0.005
        });
        if !changed {
            return;
        }
        sound.sent = Some(request);
        #[cfg(feature = "audio")]
        if let Some(audio) = &self.audio {
            audio.mixer().set_fast_forward(request.0, request.1);
        }
    }

    /// [`Session::advance`] without the sound tracking.
    fn advance_without_sound(&mut self, budget: Duration, pace: &Pace) -> Step {
        if self.stopped.is_some() {
            return Step::Stopped;
        }
        self.sample_speed();
        self.set_ratio(pace.ratio);
        let start = Instant::now();
        // Up to 1x the budget the frontend gave applies, as it always did: the pass only spends
        // all of it when it is behind. Above that — or unthrottled — it would always spend it.
        let slice = match pace.ratio {
            Some(ratio) if ratio <= 1.0 => budget,
            _ => budget.min(UNTHROTTLED_SLICE),
        };
        let deadline = start + slice;
        let cap = frame_cap(pace.ratio);
        let mut before = self.machine.gl_swaps();
        let mut frames = 0;
        let mut frame_start = start;
        let mut drew = false;
        self.decide_skip(pace, frames, cap, deadline, drew);
        loop {
            if pace.ratio.is_some() && self.ahead_ms() > 0 {
                return Step::Ahead;
            }
            match self.advance_once() {
                Some(step) => return step,
                None if self.machine.gl_swaps() != before => {
                    // **One frame per window pass was the game's ceiling.** With the window below
                    // 60 frames per second — emulation and interface together overrunning the
                    // vsync —, the game advanced in the same proportion, smooth and slow: Quake ran
                    // in slow motion without stuttering. When behind, it runs more frames in this
                    // pass, and only the last one goes to the screen.
                    //
                    // Unthrottled, the same applies until the slice ends. Before, it returned on
                    // the first frame, and the window's "unthrottled" was tied to the monitor's
                    // vsync.
                    frames += 1;
                    let now = Instant::now();
                    match self.skip.current.any() {
                        true => self.skip.skipped_duration = now - frame_start,
                        false => {
                            self.skip.drawn_duration = now - frame_start;
                            drew = true;
                        }
                    }
                    frame_start = now;
                    before = self.machine.gl_swaps();
                    let keep_going = frames < cap
                        && now < deadline
                        && match pace.ratio {
                            Some(_) => self.lag_ms() > CATCH_UP_LAG_MS,
                            None => true,
                        };
                    self.decide_skip(pace, frames, cap, deadline, drew);
                    if !keep_going {
                        return Step::Presented;
                    }
                }
                None if Instant::now() >= deadline => return Step::Running,
                None => {}
            }
        }
    }

    /// Decides whether the frame about to start is drawn. See [`SessionSkip`].
    ///
    /// Auto skips the **surplus** frame: the one that does not need to reach the screen.
    ///
    /// **One drawn frame per pass is enough.** After it, skipping the following ones leaves on
    /// screen what it drew, a few frames older, and that is what fast-forward shows. Before it, a
    /// frame is only surplus if, skipped, it still leaves time for a drawn one in the slice — and,
    /// when throttled, if the game is still behind after it and the pass has not hit the ceiling.
    ///
    /// Both halves were measured. Skipping everything that did not hit the ceiling took Rolima to
    /// 8.27x at 10x, but the pass's last frame came out skipped almost always, and the screen
    /// froze. Skipping only what left room for a drawn one kept the screen, and dropped the same
    /// 10x to 4.11x. Whatever error remains breaks nothing: a frame drawn needlessly costs what it
    /// always cost.
    fn decide_skip(
        &mut self,
        pace: &Pace,
        frames: u32,
        cap: u32,
        deadline: Instant,
        drew: bool,
    ) {
        let swaps = self.machine.gl_swaps();
        if self.skip.decided_at == Some(swaps) {
            return;
        }
        let fits_another =
            Instant::now() + self.skip.skipped_duration + self.skip.drawn_duration < deadline;
        let surplus = (drew || fits_another)
            && match pace.ratio {
                Some(_) => {
                    frames + 1 < cap
                        && self.lag_ms() > CATCH_UP_LAG_MS + FRAME_MS
                }
                None => true,
            };
        let skip = self.skip.counter.decide(pace, surplus);
        self.skip.decided_at = Some(swaps);
        self.skip.current = skip;
        // A game that reads the screen back never skips: see [`crate::speed::Frameskip`].
        self.machine
            .set_skip_draw(skip.any() && !self.machine.did_read_pixels());
    }

    /// Anchors the clock again when the ratio changes.
    ///
    /// **Without this, releasing fast-forward froze the game.** At 3x the virtual clock runs ahead
    /// of the real one; measured back at 1x from the same anchor, it appears ahead by everything
    /// it gained, and the throttle holds it in [`Step::Ahead`] until the world catches up — the
    /// same that already happened when turning the limit back on after a while without it. The new
    /// anchor starts at the current instant, and what has passed is charged to no one.
    fn set_ratio(&mut self, ratio: Option<f32>) {
        if self.ratio == ratio {
            return;
        }
        self.ratio = ratio;
        self.anchor();
    }

    /// The current real instant now corresponds to the current virtual clock.
    fn anchor(&mut self) {
        self.started = Instant::now();
        self.clock_base = u64::from(self.machine.clock_ms());
    }

    /// Advances **one virtual frame**, without consulting the wall clock.
    ///
    /// It is the unit a frontend repeats: game time advances by the virtual clock, and no decision
    /// depends on how fast the host executes. [`Session::step`] remains the window's path, which
    /// has to hand control back to the operating system from time to time.
    pub fn run_frame(&mut self, limit_speed: bool) -> Step {
        if self.stopped.is_some() {
            return Step::Stopped;
        }
        self.set_ratio(limit_speed.then_some(1.0));
        if limit_speed {
            // The desktop applies the same throttle by returning `Step::Ahead` to the window.
            // Libretro has no asynchronous pass that can receive "come back later": `retro_run`
            // has to return a frame in this call. Sleeping **before** advancing is the correct
            // translation of the same contract: the previous frame's audio plays while waiting,
            // and the next frame is only born when the real clock has caught up with the virtual
            // one.
            //
            // Fifty ms is a defensive ceiling against an anomalous jump of the virtual clock during
            // loading. In the normal case the lead is one period (16–17 ms); without the ceiling a
            // ROM that got seconds ahead would freeze the frontend in a single call.
            let wait = self.ahead_ms().min(50);
            if wait > 0 {
                std::thread::sleep(Duration::from_millis(wait));
            }
        }
        let start = u64::from(self.machine.clock_ms());
        for _ in 0..MAX_STEPS_PER_FRAME {
            match self.advance_once() {
                Some(step) => return step,
                None => {
                    if u64::from(self.machine.clock_ms()).saturating_sub(start) >= FRAME_MS {
                        return Step::Presented;
                    }
                }
            }
        }
        // A guest that neither advances the clock nor presents anything: handing control back is
        // better than hanging the frontend forever.
        Step::Running
    }

    /// How many milliseconds the game is behind the wall clock.
    ///
    /// A lag greater than [`MAX_LAG_MS`] is forgiven right here: the start of the measurement
    /// moves forward until only the maximum is left.
    fn lag_ms(&mut self) -> u64 {
        let lag = (-self.lead_ms()).max(0) as u64;
        if lag > MAX_LAG_MS {
            // The forgiveness moves with the real anchor, and at 3x each real millisecond is worth
            // three of the game's.
            let ratio = f64::from(self.ratio.unwrap_or(1.0));
            let forgiven = (lag - MAX_LAG_MS) as f64 / ratio;
            self.started += Duration::from_secs_f64(forgiven / 1000.0);
            return MAX_LAG_MS;
        }
        lag
    }

    /// How far the game is ahead (positive) or behind (negative) since the anchor, at the ratio
    /// in force. See [`crate::speed::lead_ms`].
    fn lead_ms(&self) -> i64 {
        crate::speed::lead_ms(
            u64::from(self.machine.clock_ms()).saturating_sub(self.clock_base),
            self.started.elapsed().as_millis() as u64,
            self.ratio,
        )
    }

    /// The game's start: `EVT_APP_START` delivered to the applet, **once**.
    ///
    /// `Some(Step::Stopped)` when it crashed; `None` when it went fine or had already happened.
    ///
    /// It is kept apart from the loop pass because the two point to different places: crashing on
    /// the initial event is a problem with what the applet does at birth, and crashing on the
    /// first pass is a problem with its frame loop. The ROM scan classifies by that difference, and
    /// mixing them made a game that dies in the loop show up as dead at start.
    fn part(&mut self) -> Option<Step> {
        let (applet, clsid) = self.running_game.take()?;
        let started = match self.machine.start_applet(applet, clsid, INSTRUCTION_BUDGET) {
            Ok(started) => started,
            Err(_) => {
                self.stopped = Some(Outcome::Exception { pc: 0 });
                return Some(Step::Stopped);
            }
        };
        if !matches!(started, Outcome::Returned { .. } | Outcome::Budget) {
            self.stopped = Some(started);
            return Some(Step::Stopped);
        }
        None
    }

    /// One pass of the event loop. `Some` when there is an outcome, `None` to continue.
    fn advance_once(&mut self) -> Option<Step> {
        let step = self.advance_once_and_draw();
        self.refresh_phone_views();
        step
    }

    fn advance_once_and_draw(&mut self) -> Option<Step> {
        self.pulse();
        // Intermediate screens from the previous pass not yet shown: the window shows them before
        // the game advances. See [`Session::show_intermediate_frame`].
        if self.machine.has_update_frames() {
            return Some(Step::Presented);
        }
        self.machine.begin_pass();
        let step = self.advance_once_inner();
        self.machine.end_pass();
        step
    }

    fn advance_once_inner(&mut self) -> Option<Step> {
        // The game's start is the first thing of this pass, not of `start`: that way it happens
        // with the window already on screen and the sound already on.
        if let Some(step) = self.part() {
            return Some(step);
        }

        let outcomes = match self.machine.advance(INSTRUCTION_BUDGET) {
            Ok(outcomes) => outcomes,
            Err(err) => {
                self.stopped = Some(Outcome::Exception { pc: 0 });
                let _ = err;
                return Some(Step::Stopped);
            }
        };
        if self.machine.deliver_signals(INSTRUCTION_BUDGET).is_err()
            || self.machine.deliver_callbacks(INSTRUCTION_BUDGET).is_err()
        {
            self.stopped = Some(Outcome::Exception { pc: 0 });
            return Some(Step::Stopped);
        }
        // A stretch's instruction ceiling is **not** the end of the game: it is the yield the frame
        // loop makes so it can deliver input and check the clock. Treating it as a bad outcome
        // stopped the Z-Wheel on the first pass — it repeats the opening while nobody touches it,
        // and each repetition spends budget.
        if let Some(bad) = outcomes
            .iter()
            .find(|outcome| !matches!(outcome, Outcome::Returned { .. } | Outcome::Budget))
        {
            self.stopped = Some(bad.clone());
            return Some(Step::Stopped);
        }
        // The applet asked to close: it receives `EVT_APP_STOP` and the session ends as a normal
        // exit, which is what the window reads to go back to the Z-Wheel.
        if self.machine.close_requested() {
            let _ = self.machine.stop_applet();
            self.stopped = Some(Outcome::Returned { code: 0 });
            return Some(Step::Stopped);
        }
        // With no timer armed and no pending work, nothing else is going to happen.
        if outcomes.is_empty() && self.machine.is_idle() {
            self.stopped = Some(Outcome::Returned { code: 0 });
            return Some(Step::Stopped);
        }
        None
    }

    /// How many milliseconds the game is ahead of the wall clock.
    ///
    /// The virtual clock skips idle time instead of spending it — it is what keeps the time the
    /// game *measures* honest —, and without this throttle the emulator finishes early: Crash ran
    /// twelve seconds of game in one and a half of real time.
    fn ahead_ms(&self) -> u64 {
        self.lead_ms().max(0) as u64
    }

    /// Closes the measurement window when it expires and stores the stretch's speed.
    ///
    /// The computation is over the last [`SPEED_WINDOW_MS`], not over the whole session. An
    /// average since the start takes minutes to react to a heavier screen: the number goes down
    /// slowly long after the drop happened, and looks like a continuous decline where the game is
    /// already running stable.
    fn sample_speed(&mut self) {
        let now = Mark {
            real: Instant::now(),
            clock_ms: u64::from(self.machine.clock_ms()),
            instructions: self.machine.instructions(),
            frames: self.machine.frames(),
        };
        let elapsed = (now.real - self.window.real).as_millis() as u64;
        if elapsed < SPEED_WINDOW_MS {
            return;
        }
        let per_second = |amount: u64| amount * 1000 / elapsed;
        self.sample = Sample {
            speed: (per_second(now.clock_ms.saturating_sub(self.window.clock_ms)) / 10).min(999)
                as u32,
            fps: per_second(u64::from(now.frames.saturating_sub(self.window.frames))) as u32,
            ips: per_second(now.instructions.saturating_sub(self.window.instructions)),
        };
        if self.history.len() >= HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(self.sample);
        self.window = now;
    }

    /// The last closed sample.
    pub fn sample(&self) -> Sample {
        self.sample
    }

    /// The recent samples, from oldest to newest.
    pub fn history(&self) -> impl ExactSizeIterator<Item = &Sample> {
        self.history.iter()
    }

    /// API calls served since the game opened.
    pub fn api_calls(&self) -> u64 {
        self.machine.api_calls()
    }

    /// Bytes of the guest heap already handed out, and how many of our objects are alive.
    pub fn memory(&self) -> (u32, usize) {
        (self.machine.heap_used(), self.machine.live_objects())
    }

    /// The snapshot of the game's heap: holes, largest block, free and used.
    ///
    /// `memory()` says **how much** is in use; this one says **how** what is left is divided — and
    /// that is what tells "the heap filled up" from "the heap fell to pieces".
    pub fn heap_snapshot(&self) -> crate::brew::heap::Snapshot {
        self.machine.heap_snapshot()
    }

    /// The allocations and checks the heap refused, with the requested size and who asked.
    pub fn heap_refusals(&self) -> (Vec<(u32, u32)>, Vec<(u32, u32)>) {
        (
            self.machine.refused_allocations(),
            self.machine.refused_availability_checks(),
        )
    }

    /// The game's clock, in milliseconds.
    pub fn clock_ms(&self) -> u32 {
        self.machine.clock_ms()
    }

    /// Turns on the accessor census per widget class: what each class receives, per selector.
    ///
    /// It is the same thing the scan turns on with `ZEEBX_ROM_SELECTORS`, and the core had no way
    /// to ask for it — `Machine` is private to the engine. It serves to answer, **on the device**,
    /// what the widget family does not distinguish: what each proprietary class expects.
    pub fn enable_widget_census(&mut self) {
        self.machine.enable_widget_census();
    }

    /// Turns on serial capture: where the engine's **instrumentation** writes.
    ///
    /// Created classes, opened databases, SQL, widget properties and the widget tree of the first
    /// key come out here, without mixing with the game's log. The scan and `run` have had this
    /// through `ZEEBX_ROM_SERIAL` and `--serial` since forever; the core did not, and it is the
    /// difference between being able to look at what the applet does **on the device** and only
    /// being able to guess.
    pub fn enable_serial(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.machine.enable_serial(path)
    }

    /// The execution log.
    ///
    /// It joins what the game wrote with what the emulator has to say about it. The emulator's
    /// part is the one that almost always exists: most games do not use `DBGPRINTF`, and an empty
    /// window helps nobody understand what is going on.
    ///
    /// Repetitions in the game's log come grouped, which is how the emulator stores them — a game
    /// that writes the same line every frame would fill the window without saying anything more.
    pub fn log(&self) -> Vec<String> {
        let mut lines = Vec::new();

        // The network comes first because it is what is being hunted: whoever opens the log window
        // after asking to sync wants to see the address the game requested, not scroll to the end.
        let urls = self.machine.web_requests();
        if !urls.is_empty() {
            lines.push("— addresses the game requested through IWeb —".to_string());
            lines.extend(urls.iter().map(|url| format!("  {url}")));
        }
        let ignored = self.machine.ignored_gl();
        if !ignored.is_empty() {
            lines.push("— GL served without doing anything —".to_string());
            lines.push(format!("  {}", ignored.join(" ")));
        }
        let delivered = self.machine.delivered();
        if !delivered.is_empty() {
            lines.push("— the bridge, and what it did with the response —".to_string());
            lines.extend(delivered.iter().map(|l| format!("  {l}")));
        }
        let plaintexts = self.machine.plaintexts();
        if !plaintexts.is_empty() {
            lines.push("— what the game encrypted, in plaintext —".to_string());
            for block in &plaintexts {
                let hex: String = block.iter().map(|b| format!("{b:02x}")).collect();
                lines.push(format!("  {} bytes  {hex}", block.len()));
                lines.push(format!("    {:?}", String::from_utf8_lossy(block)));
            }
        }
        let presses = self.machine.pad_log();
        if !presses.is_empty() {
            lines.push("— presses delivered to the game —".to_string());
            // The port goes into the log because, with two connected to the same keys, the same
            // press shows up twice — and without saying where it came from, that looks like a bug.
            lines.extend(presses.iter().map(|&(ms, port, name, down)| {
                let action = match down {
                    true => "press  ",
                    false => "release",
                };
                format!("  {ms:>7} ms  port {}  {action} {name}", port + 1)
            }));
        }
        let media = self.machine.media_log();
        if !media.is_empty() {
            lines.push("— what the game did with the sound —".to_string());
            lines.extend(media.iter().map(|(ms, object, call, times)| {
                let repeats = match times {
                    1 => String::new(),
                    n => format!("  ({n}x)"),
                };
                format!("  {ms:>7} ms  {object:#010x}  {call}{repeats}")
            }));
        }
        let classes = self.machine.unknown_classes();
        if !classes.is_empty() {
            lines.push("— classes the game requested that we do not have —".to_string());
            lines.extend(classes.iter().map(|id| format!("  {id:#010x}")));
        }
        let faults = self.machine.swallowed_faults();
        if !faults.is_empty() {
            lines.push("— invalid accesses the game carried on past —".to_string());
            lines.extend(faults.iter().map(|note| format!("  {note}")));
        }
        let apis = self.machine.missing_apis();
        if !apis.is_empty() {
            lines.push("— missing APIs —".to_string());
            lines.extend(apis.iter().map(|note| format!("  {note}")));
        }
        let files = self.machine.missing_files();
        if !files.is_empty() {
            lines.push("— files not found —".to_string());
            lines.extend(files.iter().map(|name| format!("  {name}")));
        }
        let assumptions = self.machine.assumptions();
        if !assumptions.is_empty() {
            lines.push("— APIs served on assumption —".to_string());
            lines.extend(assumptions.iter().map(|note| format!("  {note}")));
        }
        let pointers = self.machine.bad_pointers();
        if !pointers.is_empty() {
            lines.push("— refused pointers —".to_string());
            lines.extend(pointers.iter().map(|note| format!("  {note}")));
        }

        let game = self.machine.debug_output();
        if !game.is_empty() {
            lines.push("— game log —".to_string());
            lines.extend(game.iter().map(|(line, times)| match times {
                1 => format!("  {line}"),
                n => format!("  {line}   ({n}x)"),
            }));
        }
        let semihosting = self.machine.cpu().semihosting();
        if !semihosting.trim().is_empty() {
            lines.push("— semihosting log —".to_string());
            lines.extend(semihosting.lines().map(|line| format!("  {line}")));
        }
        lines
    }

    /// Mixes the sound into a `Mixer` without a sound card, for recording: the caller asks for
    /// the frames with `render` at the pace of the virtual clock.
    pub fn record_audio(&mut self, rate: u32) -> crate::audio::Mixer {
        let mixer = crate::audio::Mixer::silent(rate);
        self.machine.set_audio(Some(mixer.clone()));
        mixer
    }

    /// Turns sound on or off, with the volume in `0..=100`.
    ///
    /// A host without a sound card cannot keep the game from running: the reason is returned to
    /// whoever wants to show it, and the emulator carries on muted.
    #[cfg(feature = "audio")]
    pub fn set_audio(&mut self, enabled: bool, volume: u8) -> Option<String> {
        let level = f32::from(volume.min(100)) / 100.0;
        if !enabled {
            self.audio = None;
            self.machine.set_audio(None);
            return None;
        }
        if let Some(output) = &self.audio {
            output.mixer().set_master(level, false);
            return None;
        }
        match crate::audio::Output::open(level, false) {
            Ok(output) => {
                // The new output is born at 1x and audible: what was delivered to the previous one
                // no longer applies. The pause does: changing the volume with the game paused must
                // not release the music.
                self.fast_forward_sound.sent = None;
                output.mixer().freeze(self.sound_frozen);
                self.machine.set_audio(Some(output.mixer()));
                self.audio = Some(output);
                None
            }
            Err(err) => Some(err),
        }
    }

    /// The controller of one port, as the player holds it. The turbo, if any, goes on top: see
    /// [`Session::set_turbo`].
    pub fn set_port_pad(&mut self, port: usize, pad: Pad) {
        let Some(raw) = self.turbo.raw.get_mut(port) else {
            return;
        };
        *raw = pad;
        self.pulse_port(port, false);
    }

    /// The buttons that pulse on a port, as a [`Pad`] mask. Zero turns its turbo off.
    ///
    /// The frontend decides **which** buttons, through each player's mode
    /// ([`crate::speed::turbo::PortTurbo`]); the session decides **when** they are pressed, by the
    /// game's clock, before each pass of the event loop.
    pub fn set_turbo(&mut self, port: usize, pulsing: u32) {
        let Some(mask) = self.turbo.pulsing.get_mut(port) else {
            return;
        };
        if std::mem::replace(mask, pulsing) != pulsing {
            // Right away, not on the next loop pass: a turbo released in the "pressed" phase would
            // leave the button pressed until then, and [`Session::pulse`] does not even look at a
            // port without turbo.
            self.pulse_port(port, false);
        }
    }

    /// How many presses per second the turbo gives, on the game's clock.
    pub fn set_turbo_rate(&mut self, presses: u8) {
        self.turbo.presses = presses;
    }

    /// Says that the frontend turns the controller into BREW keys — confirm (`0xe064`) for button
    /// 1, `AVK_CLR` for 2 —, as the desktop window does and the Libretro core does not.
    ///
    /// **Only then does the session touch the keys.** The turbo pulses the controller state, and a
    /// game that reads button 1 through `EVT_KEY` would see no pulse at all. With the frontend
    /// delivering the key from the raw state, the session asserts on top the key of the buttons
    /// that pulse, and hands back to the raw state when they stop. Without the frontend delivering,
    /// there is nothing to fix — and asserting would invent a key the frontend never releases.
    pub fn turbo_on_keys(&mut self, enabled: bool) {
        self.turbo.on_keys = enabled;
    }

    /// Applies the turbo phase to all ports. Called before each pass of the event loop, which is
    /// the smallest unit in which the game's clock advances.
    fn pulse(&mut self) {
        for port in 0..crate::input::PORTS {
            if self.turbo.pulsing[port] != 0 || self.turbo.asserted[port] != 0 {
                self.pulse_port(port, true);
            }
        }
    }

    /// Delivers the port's controller with the turbo on top, and — with `key_events` — fixes the
    /// BREW keys of the buttons that pulse. Keys are only fixed inside the loop, after the frontend
    /// has delivered its own for this pass: see [`Session::turbo_on_keys`].
    fn pulse_port(&mut self, port: usize, key_events: bool) {
        let phase = crate::speed::turbo::pressed_phase(self.machine.clock_ms(), self.turbo.presses);
        let turbo = &mut self.turbo;
        let pulsing = turbo.pulsing[port];
        let raw = turbo.raw[port];
        let effective = crate::speed::turbo::modulate(raw, pulsing, phase);
        self.machine.set_port_pad(port, effective);
        if !key_events || !turbo.on_keys {
            return;
        }
        let seen = std::mem::replace(&mut turbo.raw_seen[port], raw);
        for (name, key) in BUTTONS_WITH_KEY {
            let Some(index) = Pad::button_by_name(name) else {
                continue;
            };
            let bit = 1u32 << index;
            let raw_pressed = raw.is_down(index);
            // What the game has now: what the session asserted, unless the frontend has delivered
            // a new key since then — the raw one, which takes over.
            let changed_in_frontend = seen.is_down(index) != raw_pressed;
            let current = match turbo.asserted[port] & bit != 0 && !changed_in_frontend {
                true => turbo.values[port] & bit != 0,
                false => raw_pressed,
            };
            let desired = match pulsing & bit != 0 {
                true => phase,
                false => raw_pressed,
            };
            if desired != current {
                self.machine.set_key(key, desired);
            }
            match pulsing & bit != 0 {
                true => {
                    turbo.asserted[port] |= bit;
                    match desired {
                        true => turbo.values[port] |= bit,
                        false => turbo.values[port] &= !bit,
                    }
                }
                false => turbo.asserted[port] &= !bit,
            }
        }
    }

    /// The motion calibrations the game started and finished. See
    /// [`crate::machine::Machine::calibration`].
    pub fn calibration(&self) -> (u32, u32) {
        self.machine.calibration()
    }

    /// The acceleration of a port with a Boomerang. See
    /// [`crate::machine::Machine::set_port_motion`].
    pub fn set_port_motion(&mut self, port: usize, acceleration: [f32; 3]) {
        self.machine.set_port_motion(port, acceleration);
    }

    /// A keyboard key, pressed or released.
    ///
    /// The console has a keyboard besides the two controllers, and BREW delivers it as an event to
    /// the application, not through `IHID` — see [`crate::input::EVT_KEY`]. The Z-Wheel depends on
    /// it: the opening form only moves with `AVK_0` or `AVK_CLR`, which no controller button
    /// produces.
    pub fn set_installed_applets(&mut self, classes: impl IntoIterator<Item = (u32, String)>) {
        self.machine.set_installed_applets(classes);
    }

    pub fn take_launch_request(&mut self) -> Option<u32> {
        self.machine.take_launch_request()
    }

    /// Delivers a widget event to the applet. See [`Machine::deliver_event_to_applet`].
    pub fn deliver_event_to_applet(&mut self, evt: u32, w: u16) -> Result<u32, crate::cpu::CpuError> {
        let result = self.machine.deliver_event_to_applet(evt, w);
        self.refresh_phone_views();
        result
    }

    pub fn set_key(&mut self, avk: u32, pressed: bool) {
        self.machine.set_key(avk, pressed);
    }

    /// Says which device the console sees on each port.
    pub fn set_ports(
        &mut self,
        ports: [Option<crate::input::bindings::Device>; crate::input::PORTS],
    ) {
        self.machine.set_ports(ports);
    }

    /// Saves the machine state, in the versioned format of [`crate::save_state`].
    pub fn save_state(&self) -> Vec<u8> {
        self.machine.save_state()
    }

    /// Puts back a state saved by [`Session::save_state`].
    pub fn restore_state(&mut self, file: &[u8]) -> Result<(), crate::save_state::Error> {
        self.machine.restore_state(file)?;

        // The save state keeps the console clock, not the host clocks. If we kept
        // started/clock_base from the instant before the Load, the limiter would compare the
        // restored clock with a timeline that no longer exists: going back ten minutes could look
        // ten minutes behind; moving to a newer state could look ahead and get stuck in
        // Step::Ahead. The new anchor starts exactly at the restored virtual instant.
        self.anchor();
        let now = self.started;
        let clock_ms = self.clock_base;
        self.window = Mark {
            real: now,
            clock_ms,
            instructions: self.machine.instructions(),
            frames: self.machine.frames(),
        };
        self.sample = Sample::default();
        self.history.clear();

        // These two pieces live in the Session, not in the Machine: both describe what the host
        // was showing after the saved point. The restored machine becomes the source of truth
        // again on the next frame.
        self.intermediate = None;
        self.stopped = None;
        self.refresh_phone_views();
        // The buffer swap counter came back along with the machine: the stored skip decision is
        // for a frame that, for it, has not happened yet.
        self.skip.decided_at = None;
        Ok(())
    }

    /// Whether it is possible to save now. See [`crate::machine::Machine::can_save`].
    pub fn can_save(&mut self) -> Result<(), String> {
        self.machine.can_save()
    }

    /// Whether a rewind point can be marked now: what [`Session::can_save`] asks, and no file open
    /// for writing. See [`crate::machine::Machine::has_writable_file`].
    pub fn can_mark_point(&mut self) -> Result<(), String> {
        self.machine.can_save()?;
        match self.machine.has_writable_file() {
            true => Err("there is a file open for writing".to_string()),
            false => Ok(()),
        }
    }

    /// Restarts the measurement against the wall clock: the real time that passed up to here is not
    /// for the game to chase. It is what the end of the rewind asks — the game stood still while it
    /// went back.
    pub fn restart_clock(&mut self) {
        self.anchor();
    }

    /// Silences the sound, apart from fast-forward: the rewind goes back points without the game
    /// advancing, and what the card plays in the meantime is the rest of the previous point.
    pub fn mute(&mut self, muted: bool) {
        if self.fast_forward_sound.muted == muted {
            return;
        }
        self.fast_forward_sound.muted = muted;
        let pace = self.fast_forward_sound.sent.map_or(1.0, |(pace, _)| pace);
        let silenced = muted || self.fast_forward_sound.muted_by_pace;
        self.fast_forward_sound.sent = Some((pace, silenced));
        #[cfg(feature = "audio")]
        if let Some(audio) = &self.audio {
            audio.mixer().set_fast_forward(pace, silenced);
        }
    }

    /// Stops the sound where it is, or releases it: it is the window's pause. See
    /// [`crate::audio::Mixer::freeze`].
    ///
    /// Different from [`Session::mute`]: muted, the sound runs without coming out; frozen, it waits
    /// for the game.
    pub fn freeze_audio(&mut self, frozen: bool) {
        if self.sound_frozen == frozen {
            return;
        }
        self.sound_frozen = frozen;
        #[cfg(feature = "audio")]
        if let Some(audio) = &self.audio {
            audio.mixer().freeze(frozen);
        }
    }

    /// The speed the game reaches, in multiples of the console's, by the same average the sound
    /// follows outside 1x; at 1x, or before the first unlimited measurement, that of
    /// [`Session::sample`].
    ///
    /// It is the number of the ▶▶ label. The sample measures half-second windows, and on a
    /// fast-forward press it showed 1x climbing up to the request. See [`Session::sync_sound`].
    pub fn reached_speed(&self) -> f32 {
        let media = match self.fast_forward_sound.requested {
            Some(Some(1.0)) | None => None,
            Some(_) => self.fast_forward_sound.media,
        };
        media.map_or(self.sample.speed as f32 / 100.0, |media| media as f32)
    }

    /// Signature of the screen contents, so the frontend can avoid resending a repeated frame.
    pub fn screen_signature(&self) -> u64 {
        self.screen().signature()
    }

    /// The identity of the opened file: the hash of the chosen `.zip`, `.7z` or `.mod`, before
    /// extraction. `None` only in a hand-built profile, without overlay.
    ///
    /// It exists for the save state slots ([`crate::ui::states`]): the session already computed
    /// the hash to find the saves folder, and computing it again would read the whole package
    /// once more for every game opened — and for every one the Z-Wheel launches.
    pub fn content(&self) -> Option<&crate::storage::ContentId> {
        self.content.as_ref()
    }

    /// The root of the game's file system: where the extraction lives.
    ///
    /// The frontend needs it to prune the cache without deleting the running game.
    pub fn content_root(&self) -> &Path {
        self.machine.file_root()
    }

    /// Brings to the CPU screen the frame `eglSwapBuffers` left pending.
    ///
    /// **Whoever reads the screen pixels must call this first.** 2D drawing calls it on its own,
    /// and the window that presents the GPU texture does not need the pixels. Whoever converts the
    /// screen to bytes — the frame dump, an outside frontend, the scan's analysis — does, or else
    /// it gets the previous frame. See [`crate::machine::Machine::present_gl`].
    pub fn materialize_gl_frame(&mut self) {
        self.machine.materialize_gl_frame();
        self.refresh_phone_views();
    }

    /// How many buffer swaps there were and how many of them had to bring the frame to the CPU.
    pub fn gl_frame_readbacks(&self) -> (u32, u32) {
        (
            self.machine.gl_swaps(),
            self.machine.gl_frame_materializations(),
        )
    }

    /// State calls sent to the GPU and how many the mirror spared.
    pub fn state_sent_and_spared(&self) -> (u64, u64) {
        self.machine.state_sent_and_spared()
    }

    /// The console screen, as it is now: 640×480.
    ///
    /// For a phone game it is the phone's area **centred** on a black console screen: the game
    /// draws from the corner, and a frontend that does not crop would otherwise show it squeezed
    /// into it. A frontend that crops calls [`Session::display_screen`] instead.
    pub fn screen(&self) -> &Framebuffer {
        match &self.phone_views {
            Some(views) => &views.centred,
            None => self.raw_screen(),
        }
    }

    /// What the game shows, at the size it uses: the whole 640×480 screen for a Zeebo game, only
    /// the phone's area for a phone game (see [`crate::machine::Machine::visible_area`]). The
    /// frontend scales it with the aspect ratio of its own size.
    pub fn display_screen(&self) -> &Framebuffer {
        match &self.phone_views {
            Some(views) => &views.crop,
            None => self.raw_screen(),
        }
    }

    /// The size of [`Session::display_screen`], known from the start: the game's class comes
    /// from its `.mif`, before the applet exists. It is what a frontend announces before the first
    /// frame — `libretro`'s `retro_get_system_av_info` is asked right after loading.
    pub fn display_size(&self) -> (u32, u32) {
        match crate::input::phone::phone_screen(self.class_id) {
            Some((width, height)) => (u32::from(width), u32::from(height)),
            None => (640, 480),
        }
    }

    /// The machine's screen, or the intermediate one on display, as the game drew it.
    fn raw_screen(&self) -> &Framebuffer {
        self.intermediate
            .as_ref()
            .unwrap_or_else(|| self.machine.screen())
    }

    /// Rebuilds the phone game's views, when the screen changed since the last time.
    ///
    /// Called wherever the screen may have changed — after each pass of the loop, an intermediate
    /// screen, a frame brought back from the GPU, a loaded state —, because [`Session::screen`]
    /// only reads. Two copies of at most 640×480 per frame, and only for a phone game.
    fn refresh_phone_views(&mut self) {
        let Some(area) = self.machine.visible_area() else {
            self.phone_views = None;
            return;
        };
        let source = self.raw_screen();
        let key = (source.generation(), source.writes(), self.intermediate.is_some());
        if self.phone_views.as_ref().is_some_and(|views| views.key == key) {
            return;
        }
        let (crop, centred) = phone_views(source, area);
        self.phone_views = Some(PhoneViews { key, crop, centred });
    }

    /// Puts the next intermediate screen on display, if there is one, and says whether it did.
    ///
    /// While there are any, whoever shows the session should display one per frame without
    /// advancing the game: it is what makes a synchronous animation, like the Z-Wheel transition,
    /// appear. See [`crate::machine::Machine::take_update_frame`].
    pub fn show_intermediate_frame(&mut self) -> bool {
        self.intermediate = self.machine.take_update_frame();
        self.refresh_phone_views();
        self.intermediate.is_some()
    }

    /// The stop was a **normal exit**, not a crash.
    ///
    /// `Outcome::Returned` is the applet that asked to close (`EVT_APP_STOP`) or that ended with
    /// nothing pending — the same outcome the window reads to go back to the Z-Wheel. The others
    /// are real failures: an API that does not exist here, a jump to an invalid address.
    ///
    /// It exists because [`stopped_reason`](Self::stopped_reason) returns only text, and whoever
    /// only has the text cannot choose between "back to the menu" and "crashed" — that is what
    /// made a game's clean exit show up as an error on Android.
    pub fn exited_normally(&self) -> bool {
        matches!(self.stopped, Some(Outcome::Returned { .. }))
    }

    /// The reason for the stop, if the game stopped, in text useful to whoever is looking at the
    /// screen.
    ///
    /// The outcome's `Debug` carries raw addresses, and "method not implemented at `0xf0014034`"
    /// says nothing to anyone — the name of the interface and the method does.
    pub fn stopped_reason(&self) -> Option<String> {
        Some(match self.stopped.as_ref()? {
            Outcome::Returned { .. } => "the game ended".to_string(),
            Outcome::Unimplemented { addr, caller, .. } => format!(
                "the game called {}, which does not exist here yet (from {caller:#010x})",
                crate::brew::aee::describe(*addr)
            ),
            // `lr` goes along: on a jump to address zero `pc` says nothing, and the caller is the
            // only clue to which pointer was empty.
            Outcome::Fault { addr, pc, lr } => {
                format!("invalid access to {addr:#010x}, at {pc:#010x} (from {lr:#010x})")
            }
            Outcome::Exception { pc } => format!("ARM core exception at {pc:#010x}"),
            Outcome::Budget => "the game exceeded the instruction budget".to_string(),
            Outcome::CallLimit { calls } => {
                format!(
                    "{calls} API calls without yielding — likely an infinite loop"
                )
            }
        })
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// The last frame of the GL rasterizer, when there is one. It serves to separate what the 3D
    /// drew from what reached the screen composited.
    pub fn frame_gl(&mut self) -> Option<Framebuffer> {
        self.machine.gl_frame()
    }

    /// Registers `r0`–`r11` and the likely return addresses on the stack, from the last fault.
    pub fn failure(&self) -> ([u32; 12], &[u32], Option<u32>) {
        let lr = match self.stopped {
            Some(Outcome::Fault { lr, .. }) => Some(lr),
            _ => None,
        };
        (self.machine.fault_regs(), self.machine.fault_stack(), lr)
    }

    /// The frame texture on the GPU. See [`Machine::frame_texture`].
    pub fn frame_texture(&self) -> Option<u32> {
        match self.intermediate {
            Some(_) => None,
            None => self.machine.frame_texture(),
        }
    }

    /// The 3D frame at the internal resolution, when it is the one on display. See
    /// [`crate::machine::Machine::gpu_frame`].
    pub fn gpu_frame(&self) -> Option<crate::video::rasterizer::GpuFrame> {
        match self.intermediate {
            Some(_) => None,
            None => self.machine.gpu_frame(),
        }
    }

    /// The 3D frame at the internal resolution, for recording without a window.
    pub fn large_frame(&mut self) -> Option<Framebuffer> {
        self.machine.large_frame()
    }

    /// The frame the window shows, in RGB with eight bits per channel: width, height and the bytes.
    ///
    /// **The large one only when it is the one on display**, by the same question as
    /// [`Session::gpu_frame`]. [`Session::large_frame`] alone reads the GPU even with a HUD drawn
    /// afterwards, and the screenshot would come out without the HUD. Otherwise the console screen
    /// applies, at 640×480. With the GPU, it is GL: the caller leaves the rasterizer's context
    /// current. See `docs/implementation/22-screenshots.md`.
    pub fn capture(&mut self) -> (u32, u32, Vec<u8>) {
        if self.gpu_frame().is_some()
            && let Some((width, height, rgba)) = self.machine.large_frame_rgba()
        {
            // Without the alpha: some games clear the background with zero alpha, and the
            // screenshot would come out transparent.
            let rgb = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            return (width as u32, height as u32, rgb);
        }
        // A phone game's screenshot is its area, not the console screen around it.
        let screen = self.display_screen();
        let rgb = screen
            .to_argb()
            .into_iter()
            .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, p as u8])
            .collect();
        (screen.width(), screen.height(), rgb)
    }

    /// Turns on real-time counting per API method. See [`Session::api_profile`].
    pub fn enable_api_profile(&mut self) {
        self.machine.enable_api_profile();
    }

    /// How much real time each API method cost, from most to least expensive, in ns.
    pub fn api_profile(&self) -> Vec<(String, u64)> {
        self.machine.api_profile()
    }

    /// How many ARM instructions the game has executed.
    pub fn instructions(&self) -> u64 {
        self.machine.instructions()
    }

    /// How many GL frames the game has presented.
    pub fn presented_frames(&self) -> u64 {
        u64::from(self.machine.gl_swaps())
    }

    /// How many frames the game drew on the GPU — **buffer swaps or `glClear`**.
    ///
    /// It differs from [`Session::presented_frames`], which counts only swaps: the Z-Wheel draws
    /// the stage in a pbuffer and never swaps buffers, and a swap counter would say it draws
    /// nothing. Whoever asks "did the GPU draw in this frame?" needs this one.
    pub fn gpu_frames(&self) -> u32 {
        self.machine.frames()
    }

    /// Antialiasing (samples per pixel) and anisotropic filtering of the 3D on the GPU; they apply
    /// right away.
    pub fn set_enhancements(&mut self, samples: usize, anisotropic: usize) {
        self.machine.set_enhancements(samples, anisotropic);
    }

    /// Whether the game's fog applies; it applies right away.
    pub fn set_fog_allowed(&mut self, allowed: bool) {
        self.machine.set_fog_allowed(allowed);
    }

    /// Whether the current frame should skip 3D drawing and screen clearing. See
    /// [`crate::machine::Machine::set_skip_draw`].
    pub fn set_skip_draw(&mut self, skip: bool) {
        self.machine.set_skip_draw(skip);
    }

    /// Whether the game has already used `glReadPixels` and, to be safe, disabled rasterization
    /// frameskip.
    pub fn did_read_pixels(&self) -> bool {
        self.machine.did_read_pixels()
    }

    /// Turns on logging of every call the game makes, in order. See
    /// [`crate::machine::Machine::set_tracing`].
    pub fn set_tracing(&mut self, on: bool) {
        self.machine.set_tracing(on);
    }

    /// Restricts tracing to the calls whose name contains `part`. See
    /// [`crate::machine::Machine::set_trace_filter`].
    pub fn set_trace_filter(&mut self, part: Option<String>) {
        self.machine.set_trace_filter(part);
    }

    /// The logged calls, in order. See [`crate::machine::Machine::trace`].
    pub fn trace(&self) -> &[String] {
        self.machine.trace()
    }

    /// Changes the 3D internal resolution; it applies from the next frame.
    pub fn set_internal_resolution(&mut self, scale: usize) {
        self.machine.set_internal_resolution(scale);
    }

    /// Tells the GPU rasterizer to discard depth and stencil after the frame.
    pub fn set_tile_discard(&mut self, discard: bool) {
        self.machine.set_tile_discard(discard);
    }

    /// Reduces the 3D internal resolution in the **CPU** rasterizer, drawing on a smaller surface
    /// and scaling up at presentation. See [`Rasterizer::set_downscale`].
    pub fn set_downscale(&mut self, downscale: usize) {
        self.machine.set_downscale(downscale);
    }

    /// The experimental 3D aspect ratio, width over height; `None` is the console's 4:3.
    /// Makes the drawing go out to the frontend's framebuffer, when it provides one.
    ///
    /// **It is what the core needs for `SET_HW_RENDER`**, and that is why it has a public path: the
    /// engine already knows how to draw into an outside framebuffer (`Machine::draw_to_fbo`,
    /// verified by the test `the_engine_draws_into_the_frontend_framebuffer`), but whoever has the
    /// framebuffer at hand is the core, every frame, through `get_current_framebuffer` of
    /// `retro_hw_render_callback`.
    ///
    /// `Some(0)` is the frontend's default framebuffer; `None` hands the drawing back to the
    /// engine's own framebuffer, which is the usual path.
    pub fn draw_to_fbo(&mut self, fbo: Option<u32>) {
        self.machine.draw_to_fbo(fbo);
    }

    /// Signals that the window painted on the borrowed GL context since the last step.
    ///
    /// Whoever shares the context with the engine calls this once per frame, before the game
    /// advances: what the window changed on the GPU stops being what the engine thinks is there.
    pub fn resume_context(&mut self) {
        self.machine.resume_context();
    }

    /// Closes the frame on the GPU side before the frontend presents its framebuffer.
    ///
    /// The pair of [`Session::draw_to_fbo`]: that one takes the context at the start of the frame,
    /// this one hands it back at the end, without the engine's objects bound.
    pub fn hand_back_to_frontend(&mut self) {
        self.machine.hand_back_to_frontend();
    }

    pub fn set_aspect_ratio(&mut self, aspect: Option<f32>) {
        self.machine.set_aspect_ratio(aspect);
    }

    /// The ClassID of the applet running in this session.
    pub fn class_id(&self) -> u32 {
        self.class_id
    }

    /// Starts with the screen the previous applet left, before this one's first drawing.
    ///
    /// The device framebuffer is not cleared on an applet switch. When opening a game, the Z-Wheel
    /// leaves "Aguarde enquanto o aplicativo é carregado" on screen (see [`ZWHEEL_SPLASH`]), and
    /// that is what is seen until the game draws its first frame.
    pub fn inherit_screen(&mut self, rgb565: &[u8]) {
        self.machine.paint_screen_rgb565(rgb565);
        self.refresh_phone_views();
    }

    /// Whether the applet exited on its own, and not through a failure.
    pub fn exited_on_its_own(&self) -> bool {
        matches!(self.stopped, Some(Outcome::Returned { .. }))
    }
}

/// A phone game's area, on its own and centred. See [`Session::display_screen`].
struct PhoneViews {
    /// The source screen's `(generation, writes, intermediate)` when these were made: the same key
    /// means nothing changed and the copies are skipped.
    key: (u64, u64, bool),
    crop: Framebuffer,
    centred: Framebuffer,
}

/// The phone's `area` of `source`: alone, and in the middle of a black screen of the same size as
/// `source`. What the game left outside the area does not show in either.
fn phone_views(source: &Framebuffer, area: Rect) -> (Framebuffer, Framebuffer) {
    let (width, height) = (i32::from(area.width), i32::from(area.height));
    let (x, y) = (i32::from(area.x), i32::from(area.y));
    let mut crop = Framebuffer::new(width as u32, height as u32);
    crop.blit(0, 0, width, height, source, x, y, None);
    let mut centred = Framebuffer::new(source.width(), source.height());
    let left = (source.width() as i32 - width) / 2;
    let top = (source.height() as i32 - height) / 2;
    centred.blit(left, top, width, height, source, x, y, None);
    (crop, centred)
}

/// The ClassID of the Z-Wheel, the console's home screen.
///
/// **Choosing a game in it is closing it.** The path is in the module: on confirm, it writes the
/// marker `ttgmrun.tmp` and `StringLastAppRan`, tears down the screens and, on the `0x81c38`
/// timer, exits. The console reopens it because it is the home screen, and at startup it sees the
/// marker (`0x81904`), deletes it and opens the saved game two seconds later. Whoever runs the
/// Z-Wheel has to reopen it when it exits on its own — without that the chosen game never opened.
pub const Z_WHEEL: u32 = 0x0107_0798;

/// The 640×480 RGB565 image the console shows when opening the Z-Wheel, in its directory.
///
/// At boot it is "Bem-Vindo ao Zeebo". **To open a game, the Z-Wheel swaps the file before
/// exiting**: if `gamestartrgb.sav` exists, it renames this one to `zeebosplash.rgb565.sav` and
/// `gamestartrgb.sav` to this name (`0x81db0`), and on reopening it undoes the swap right in the
/// first millisecond (`0x822bc`). Whoever reads the file between the two is the console, when
/// reopening it — and what shows up is "Aguarde enquanto o aplicativo é carregado", which stays on
/// screen for the two seconds until the game opens, because the reopened Z-Wheel draws nothing in
/// that time.
pub const ZWHEEL_SPLASH: &str = "zeebosplash.rgb565.raw";

/// Execution from the inside, for the ROM scan — see [`crate::scan`].
///
/// [`Session::log`] builds text for the window; the survey needs the raw lists and the outcome as
/// an enum, to classify and compare with what was already known about the game. Only the test
/// uses this, and that is why it is not part of the session's interface.
#[cfg(test)]
impl Session {
    /// The engine, for whoever needs to turn on a measurement before running.
    ///
    /// It exists for the per-method time profile: turning on the per-call stopwatch is done
    /// **before** the loop, and the scan needs to do it from outside the session.
    pub(crate) fn machine_mut(&mut self) -> &mut Machine<SessionCpu> {
        &mut self.machine
    }

    pub(crate) fn machine(&self) -> &Machine<SessionCpu> {
        &self.machine
    }

    pub(crate) fn stopped(&self) -> Option<&Outcome> {
        self.stopped.as_ref()
    }

    /// The start, without the loop pass that comes along in [`Session::step`].
    pub(crate) fn running_game(&mut self) -> Option<Step> {
        self.part()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// **The phone's area comes out on its own, and centred for whoever does not crop.**
    ///
    /// A phone game draws at the corner of the 640×480 screen. A frontend that knows about the
    /// area shows [`Session::display_screen`], just the area; one that does not keeps calling
    /// [`Session::screen`] and gets the area in the middle of a black console screen — never the
    /// game squeezed into a corner.
    #[test]
    fn the_phone_area_is_cropped_and_centred() {
        let mut source = Framebuffer::new(640, 480);
        source.set_pixel_native(0, 0, 0x1111);
        source.set_pixel_native(239, 399, 0x2222);
        source.set_pixel_native(240, 0, 0x3333); // outside the area
        let area = Rect { x: 0, y: 0, width: 240, height: 400 };

        let (crop, centred) = phone_views(&source, area);

        assert_eq!((crop.width(), crop.height()), (240, 400));
        assert_eq!(crop.get_pixel(0, 0), 0x1111);
        assert_eq!(crop.get_pixel(239, 399), 0x2222);

        assert_eq!((centred.width(), centred.height()), (640, 480));
        // (640 - 240) / 2 = 200, (480 - 400) / 2 = 40.
        assert_eq!(centred.get_pixel(200, 40), 0x1111);
        assert_eq!(centred.get_pixel(439, 439), 0x2222);
        assert_eq!(centred.get_pixel(0, 0), 0, "outside the area is black");
        assert!(
            (0..640).all(|x| centred.get_pixel(x, 0) != 0x3333),
            "what the game left outside its area does not show"
        );
    }

    /// A session without a game, with minimal memory: enough to save and restore state.
    pub(crate) fn minimal_session_for_save_state() -> Session {
        let mut mem = crate::cpu::mem::GuestMemory::new();
        let mut code = 0xe12f_ff1eu32.to_le_bytes().to_vec(); // bx lr
        code.resize(0x1000, 0);
        mem.map("code", 0, code, true).unwrap();
        mem.map_zeroed("data", 0x1000, 0x1000).unwrap();
        mem.map_zeroed("stack", loader::STACK_BASE, loader::STACK_SIZE)
            .unwrap();
        let modulo = loader::LoadedModule {
            mem,
            entry: 0,
            shell: loader::OBJECT_BASE,
            helpers: 0,
            out_module: loader::OBJECT_BASE,
            extensions: Vec::new(),
        };
        let mut cpu = SessionCpu::new().unwrap();
        crate::cpu::CpuBackend::reset(&mut cpu, &modulo.mem).unwrap();
        let machine = Machine::new(
            cpu,
            modulo,
            std::env::temp_dir().join("zeebx-session-state-clock"),
        );
        let now = Instant::now();
        let clock_ms = u64::from(machine.clock_ms());
        let window = Mark {
            real: now,
            clock_ms,
            instructions: machine.instructions(),
            frames: machine.frames(),
        };
        Session {
            machine,
            running_game: None,
            #[cfg(feature = "audio")]
            audio: None,
            title: "teste".to_string(),
            content: None,
            class_id: 0,
            intermediate: None,
            phone_views: None,
            started: now,
            clock_base: clock_ms,
            ratio: Some(1.0),
            skip: SessionSkip::default(),
            fast_forward_sound: FastForwardSound::default(),
            sound_frozen: false,
            watch: SpeedWatchdog::default(),
            turbo: SessionTurbo::default(),
            stopped: None,
            window,
            sample: Sample::default(),
            history: std::collections::VecDeque::new(),
        }
    }

    #[test]
    fn restoring_state_reanchors_the_session_wall_clock() {
        let mut session = minimal_session_for_save_state();
        let state = session.save_state();

        // Simulates a session that kept going long after the saved point. These pieces belong to
        // the host and do not go into the ZBXS file; if they survive the Load, the limiter
        // compares two different timelines and may speed up or hold back the game wrongly.
        session.started = Instant::now() - Duration::from_secs(30);
        session.clock_base = 123_456;
        session.window.real = Instant::now() - Duration::from_secs(5);
        session.window.clock_ms = 987_654;
        session.sample = Sample {
            speed: 321,
            fps: 99,
            ips: 123,
        };
        session.history.push_back(session.sample);
        session.intermediate = Some(Framebuffer::new(2, 2));
        session.stopped = Some(Outcome::Budget);

        session.restore_state(&state).unwrap();

        assert_eq!(session.clock_base, u64::from(session.machine.clock_ms()));
        assert_eq!(session.ahead_ms(), 0, "the restored state must not be born ahead");
        assert!(session.lag_ms() < 100, "the restored state was born artificially behind");
        assert!(session.history.is_empty());
        assert_eq!(session.sample.speed, 0);
        assert_eq!(session.sample.fps, 0);
        assert_eq!(session.sample.ips, 0);
        assert!(session.intermediate.is_none());
        assert!(session.stopped.is_none());
    }

    /// Changing the ratio restarts the measurement: releasing fast-forward must not leave the game
    /// "ahead" by everything it gained, nor turning it on charge a lag that did not exist.
    #[test]
    fn changing_the_ratio_reanchors_the_clock() {
        let mut session = minimal_session_for_save_state();
        // Thirty real seconds without the game advancing: at 1x, that is lag.
        session.started = Instant::now() - Duration::from_secs(30);
        assert!(session.lead_ms() < -29_000);

        session.set_ratio(Some(3.0));
        assert_eq!(session.ratio, Some(3.0));
        assert_eq!(session.ahead_ms(), 0);
        assert!(session.lag_ms() < 100);

        // Unthrottled there is no target: neither ahead nor behind.
        session.set_ratio(None);
        session.clock_base = 0;
        session.started = Instant::now() - Duration::from_secs(30);
        assert_eq!(session.lead_ms(), 0);

        // The same ratio does not re-anchor: the measurement continues from where it was.
        session.set_ratio(Some(1.0));
        let before = session.started;
        session.set_ratio(Some(1.0));
        assert_eq!(session.started, before);
    }

    /// The turbo goes on top of the raw controller, in the game clock's phase, and goes away when
    /// the mask is zeroed — without touching what the player holds.
    #[test]
    fn turbo_pulses_over_the_raw_controller() {
        let mut session = minimal_session_for_save_state();
        let b1 = Pad::button_by_name("b1").unwrap();
        let up = Pad::button_by_name("up").unwrap();
        let mut raw = Pad::default();
        raw.press(up, true);

        session.set_turbo(0, 1 << b1);
        session.set_port_pad(0, raw);
        // The new machine's clock is at the start of a period: the phase is "pressed".
        assert!(crate::speed::turbo::pressed_phase(session.machine.clock_ms(), 10));
        let seen = session.machine.port_pad(0);
        assert!(seen.is_down(b1), "the pulsing button is pressed in this phase");
        assert!(seen.is_down(up), "what the player holds stays");

        session.set_turbo(0, 0);
        let seen = session.machine.port_pad(0);
        assert!(!seen.is_down(b1), "without turbo, back to raw");
        assert!(seen.is_down(up));
    }

    /// The game sees HOME as `Back` and `Start` together, and releasing it releases both. See
    /// [`Pad::with_home_on_both`].
    #[test]
    fn the_game_sees_home_as_back_and_start() {
        let mut session = minimal_session_for_save_state();
        let mut home = Pad::default();
        home.press(crate::input::HOME, true);
        session.set_port_pad(0, home);
        let seen = session.machine.port_pad(0);
        assert!(seen.is_down(crate::input::HOME) && seen.is_down(crate::input::START));

        session.set_port_pad(0, Pad::default());
        let seen = session.machine.port_pad(0);
        assert!(!seen.is_down(crate::input::HOME) && !seen.is_down(crate::input::START));
    }

    /// Fast-forward starts at the requested ratio: the average of the sound and the label does not
    /// climb from 1x little by little. Without a request — unlimited —, it waits for the first
    /// measurement.
    #[test]
    fn fast_forward_starts_at_the_requested_ratio() {
        let mut session = minimal_session_for_save_state();
        assert_eq!(session.reached_speed(), 0.0, "at 1x the sample applies, still empty");

        let three = Pace { ratio: Some(3.0), ..Pace::CONSOLE };
        session.sync_sound(&three);
        assert_eq!(session.reached_speed(), 3.0);
        assert_eq!(session.fast_forward_sound.sent, Some((3.0, false)));

        // Going back to 1x resets the average, and the next request starts from it again.
        session.sync_sound(&Pace::CONSOLE);
        assert_eq!(session.fast_forward_sound.sent, Some((1.0, false)));
        let ten = Pace { ratio: Some(10.0), ..Pace::CONSOLE };
        session.sync_sound(&ten);
        assert_eq!(session.reached_speed(), 10.0);

        // The next press with the same ratio starts from where it stopped.
        session.fast_forward_sound.media = Some(6.5);
        session.sync_sound(&Pace::CONSOLE);
        session.sync_sound(&ten);
        assert_eq!(session.reached_speed(), 6.5);
        session.sync_sound(&three);
        assert_eq!(session.reached_speed(), 3.0, "the memory belongs to its own ratio");

        session.sync_sound(&Pace::UNTHROTTLED);
        assert_eq!(session.fast_forward_sound.media, None, "unlimited has nowhere to start from");
    }

    /// One second without the clock advancing, at 1x, is "slow"; a gap between passes — the pause,
    /// the rewind — restarts the measurement without flagging anything; and unthrottled there is
    /// no target.
    #[test]
    fn the_watchdog_flags_a_stalled_game_not_a_pause() {
        let mut session = minimal_session_for_save_state();
        let clock = u64::from(session.machine.clock_ms());
        let now = Instant::now();

        session.watch = SpeedWatchdog {
            start: Some((now - Duration::from_millis(1100), clock)),
            last: Some(now - Duration::from_millis(16)),
            ..SpeedWatchdog::default()
        };
        session.watch(&Pace::CONSOLE, Step::Running);
        assert!(session.watch.slow, "one second stalled at 1x was not flagged");
        assert_eq!(session.watch.laps, 0, "the window did not restart");

        // The last pass was one second ago: it is a gap, not slowness.
        session.watch = SpeedWatchdog {
            start: Some((now - Duration::from_millis(1100), clock)),
            last: Some(now - Duration::from_secs(1)),
            ..SpeedWatchdog::default()
        };
        session.watch(&Pace::CONSOLE, Step::Running);
        assert!(!session.watch.slow, "the pause was flagged");

        session.watch = SpeedWatchdog {
            start: Some((now - Duration::from_millis(1100), clock)),
            last: Some(now - Duration::from_millis(16)),
            ..SpeedWatchdog::default()
        };
        session.watch(&Pace::UNTHROTTLED, Step::Running);
        assert!(!session.watch.slow && session.watch.start.is_none(), "unthrottled has no target");
    }

    #[test]
    fn the_frame_cap_grows_with_the_ratio() {
        assert_eq!(frame_cap(Some(1.0)), FRAMES_PER_PASS);
        // Below 1x it does not shrink: catching up on lag still needs the same frames.
        assert_eq!(frame_cap(Some(0.5)), FRAMES_PER_PASS);
        assert_eq!(frame_cap(Some(3.0)), 3 * FRAMES_PER_PASS);
        assert_eq!(frame_cap(Some(2.5)), 10);
        assert_eq!(frame_cap(None), u32::MAX);
    }

/// **The engine draws into the framebuffer the frontend provides, and the test proves it.**
///
/// It is the missing piece for item 5: in `libretro`, the frontend is who presents the frame, and
/// the core draws into the framebuffer it points to. Here the test creates its **own** framebuffer,
/// tells the engine to draw into it and reads the pixels **from it** — not from the engine's
/// internal framebuffer. If the drawing were not going there, the outside framebuffer would keep
/// the undefined content it was born with, and the read would return black.
///
/// The comparison is against the software rasterizer at the same virtual instant, as in the
/// sibling test [`both_rasterizers_draw_the_same_frame`].
#[cfg(feature = "gpu")]
#[test]
fn the_engine_draws_into_the_frontend_framebuffer() {
    use glow::HasContext;
    use std::time::Duration;

    let Ok(rom) = crate::config::env_var("ZEEBX_TEST_ROM") else {
        eprintln!("no ZEEBX_TEST_ROM: nothing to compare");
        return;
    };
    let ms: u64 = crate::config::env_var("ZEEBX_TEST_MS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(3000);
    let context = match crate::video::context::GlContext::create() {
        Ok(context) => context,
        Err(reason) => {
            eprintln!("no offscreen GPU: {reason}");
            return;
        }
    };
    let gl = context.gl.clone();

    // The outside framebuffer, with the color texture: it plays the part of what the frontend
    // would provide.
    let (fbo, texture) = unsafe {
        let fbo = gl.create_framebuffer().expect("outside framebuffer");
        let texture = gl.create_texture().expect("outside texture");
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            640,
            480,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        // It is born with undefined content: painting it green guarantees that any non-black pixel
        // afterwards is real drawing, and not allocation leftovers.
        gl.clear_color(0.0, 1.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        (fbo, texture)
    };

    let mut session = Session::start_with(
        &std::path::PathBuf::from(&rom),
        crate::DEFAULT_PORTS,
        None,
        true,
        Some(gl.clone()),
        Default::default(),
    )
    .expect("the GPU session opened");
    session.machine_mut().draw_to_fbo(Some(fbo.0.get()));

    let base = session.clock_ms();
    // **The fence.** Halfway through, the outside framebuffer is painted green again. Without this
    // the test does not catch the defect that existed: the frontend's target was respected only
    // at the **creation** of the destination, and every following frame bound the internal
    // framebuffer again — the outside one kept what the first frame left, and a "something
    // changed" check passed with a black screen in the frontend. With the fence, the green only
    // disappears if the **new** frames are going there.
    let fence = ms / 2;
    let mut fenced = false;
    loop {
        let elapsed = session.clock_ms().saturating_sub(base);
        if elapsed >= ms as u32 {
            break;
        }
        if !fenced && elapsed >= fence as u32 {
            fenced = true;
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.clear_color(0.0, 1.0, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
        }
        match session.step(Duration::ZERO, false) {
            Step::Stopped => break,
            Step::Presented => {
                while session.show_intermediate_frame() {}
            }
            Step::Running | Step::Ahead => {}
        }
    }

    // Reads from the **outside** framebuffer: it is what proves the drawing went there.
    let mut pixels = vec![0u8; 640 * 480 * 4];
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.read_pixels(
            0,
            0,
            640,
            480,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    }
    let greens = pixels
        .chunks_exact(4)
        .filter(|p| p[0] == 0 && p[1] == 255 && p[2] == 0 && p[3] == 255)
        .count();
    let total = 640 * 480;
    eprintln!(
        "outside framebuffer: {} of {total} pixel(s) still in their initial color ({:.1}%)",
        greens,
        greens as f64 * 100.0 / total as f64
    );
    assert!(
        greens < total,
        "nothing was drawn into the frontend's framebuffer: it stayed as it was born"
    );
    // And what matters most: the frames **after the fence** also went there.
    assert!(
        greens < total / 10,
        "the frontend's framebuffer kept the fence color: the new frames did not go there"
    );

    unsafe {
        gl.delete_framebuffer(fbo);
        gl.delete_texture(texture);
    }
}


/// **Saving and loading in the GPU rasterizer gives the same frame.**
///
/// It is the missing proof for item 6: the `GpuState` save path was wired up and untested. The
/// test follows the sequence RetroArch performs, and goes beyond "the fields come back": it
/// **requires the drawing to stay the same**.
///
/// The path is this: run a stretch, **save**; run one more stretch, and keep the frame (it is what
/// the game does after the save); **load** the state and run the same stretch again. The two
/// frames have to be identical — same matrix, same textures, same frame. Without that the save
/// state "comes back" and the scene comes out different, which is the most expensive way to fail
/// to discover.
#[cfg(feature = "gpu")]
#[test]
fn the_gpu_state_continues_the_same_drawing() {
    use glow::HasContext;
    use std::time::Duration;

    let Ok(rom) = crate::config::env_var("ZEEBX_TEST_ROM") else {
        eprintln!("no ZEEBX_TEST_ROM: nothing to compare");
        return;
    };
    let ms: u64 = crate::config::env_var("ZEEBX_TEST_MS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(2000);
    let context = match crate::video::context::GlContext::create() {
        Ok(context) => context,
        Err(reason) => {
            eprintln!("no offscreen GPU: {reason}");
            return;
        }
    };
    let gl = context.gl.clone();
    let (fbo, texture) = unsafe {
        let fbo = gl.create_framebuffer().expect("outside framebuffer");
        let texture = gl.create_texture().expect("outside texture");
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            640,
            480,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        (fbo, texture)
    };

    let mut session = Session::start_with(
        &std::path::PathBuf::from(&rom),
        crate::DEFAULT_PORTS,
        None,
        true,
        Some(gl.clone()),
        Default::default(),
    )
    .expect("the GPU session opened");
    session.machine_mut().draw_to_fbo(Some(fbo.0.get()));

    // Runs `ms` virtual and reads the frame from the outside framebuffer.
    let run_and_read = |session: &mut Session, ms: u64| -> Vec<u8> {
        let base = session.clock_ms();
        while session.clock_ms().saturating_sub(base) < ms as u32 {
            match session.step(Duration::ZERO, false) {
                Step::Stopped => break,
                Step::Presented => {
                    while session.show_intermediate_frame() {}
                }
                Step::Running | Step::Ahead => {}
            }
        }
        let mut pixels = vec![0u8; 640 * 480 * 4];
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.read_pixels(
                0,
                0,
                640,
                480,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        pixels
    };

    // 1) A stretch, and the save point.
    let _ = run_and_read(&mut session, ms);
    assert!(session.can_save().is_ok(), "the engine refused to save at the end of the frame");
    let state = session.save_state();
    assert!(state.len() > 1024, "the state came out too small: {}", state.len());

    // 2) One more stretch, and the frame the game shows after the save.
    let expected = run_and_read(&mut session, ms);

    // 3) Back to the save point, and the same stretch again.
    session.restore_state(&state).expect("loaded");
    let after = run_and_read(&mut session, ms);

    let equal = expected.iter().zip(after.iter()).filter(|(a, b)| a == b).count();
    let total = expected.len();
    eprintln!(
        "the frame after the save and the frame after loading: {equal} of {total} byte(s) equal \
         ({:.2}%)",
        equal as f64 * 100.0 / total as f64
    );
    assert_eq!(
        equal, total,
        "the drawing did not stay the same after loading the state"
    );

    unsafe {
        gl.delete_framebuffer(fbo);
        gl.delete_texture(texture);
    }
}

/// **The same game through both rasterizers, at the same virtual instant.**
///
/// It is the check that was missing for hardware rendering. With the window closed, both paths
/// run the same content for the same virtual time, and the frames are compared byte by byte.
/// Without this, turning on hardware rendering would be swapping a measured path for a path nobody
/// looked at — and that is why `SET_HW_RENDER` stays undone in the core until this check exists.
///
/// **Without `ZEEBX_TEST_ROM` it does not run**, and **without a GPU neither**: on a terminal
/// without EGL the offscreen path answers that it does not exist, and that is the normal case, not
/// a test failure.
#[cfg(feature = "gpu")]
#[test]
fn both_rasterizers_draw_the_same_frame() {
    use std::time::Duration;

    let Ok(rom) = crate::config::env_var("ZEEBX_TEST_ROM") else {
        eprintln!("no ZEEBX_TEST_ROM: nothing to compare");
        return;
    };
    let origin = std::path::PathBuf::from(&rom);
    let ms: u64 = crate::config::env_var("ZEEBX_TEST_MS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(3000);

    // The context must stay alive while the session runs: the GL functions live in it.
    let context = match crate::video::context::GlContext::create() {
        Ok(context) => context,
        Err(reason) => {
            eprintln!("no offscreen GPU: {reason}");
            return;
        }
    };

    let screen_of = |gpu: bool, context: Option<std::sync::Arc<glow::Context>>| {
        let mut session = Session::start_with(
            &origin,
            crate::DEFAULT_PORTS,
            None,
            gpu,
            context,
            Default::default(),
        )
        .ok()?;
        let base = session.clock_ms();
        while session.clock_ms().saturating_sub(base) < ms as u32 {
            match session.step(Duration::ZERO, false) {
                Step::Stopped => break,
                Step::Presented => {
                    while session.show_intermediate_frame() {}
                }
                Step::Running | Step::Ahead => {}
            }
        }
        // The OpenGL frame may be pending — see [`Session::materialize_gl_frame`]. Without this,
        // the GPU path would deliver the previous screen, and the comparison down below would be
        // between two stale screens: it would pass without comparing any image.
        session.materialize_gl_frame();
        let (swaps, readbacks) = session.gl_frame_readbacks();
        let (sent_count, spared) = session.state_sent_and_spared();
        eprintln!(
            "  {}: {swaps} buffer swap(s), {readbacks} frame readback(s) to the CPU, \
{sent_count} state(s) sent and {spared} spared by the mirror",
            match gpu {
                true => "GPU",
                false => "CPU",
            }
        );
        let screen = session.screen();
        let (width, height) = (screen.width(), screen.height());
        let mut bytes = Vec::new();
        screen.write_rgb565_into(&mut bytes);
        Some((width, height, bytes))
    };

    let software = screen_of(false, None).expect("the software session opened");
    let gpu = screen_of(true, Some(context.gl.clone())).expect("the GPU session opened");

    assert_eq!(
        (software.0, software.1),
        (gpu.0, gpu.1),
        "the two paths draw at different sizes"
    );
    assert_eq!(software.2.len(), gpu.2.len());

    // RGB565 pixel comparison: how many differ and by how much. A rasterizer on the GPU and another
    // on the CPU do not give the **same** frame — the difference is rounding and order of
    // operations. What is required here is that they draw the same image, not that they be
    // identical.
    let mut differing = 0usize;
    let mut coarse = 0usize;
    let mut sum = 0u64;
    let mut worst = 0u16;
    for (a, b) in software.2.chunks_exact(2).zip(gpu.2.chunks_exact(2)) {
        let (a, b) = (
            u16::from_le_bytes([a[0], a[1]]),
            u16::from_le_bytes([b[0], b[1]]),
        );
        let (r1, g1, b1) = (a >> 11 & 0x1f, a >> 5 & 0x3f, a & 0x1f);
        let (r2, g2, b2) = (b >> 11 & 0x1f, b >> 5 & 0x3f, b & 0x1f);
        let d = (r1.abs_diff(r2) as u64)
            + (g1.abs_diff(g2) as u64)
            + (b1.abs_diff(b2) as u64);
        if d > 0 {
            differing += 1;
        }
        // One channel step in 565 is the **rounding** of the two converters, not a different
        // image: the software rasterizer truncates (`>> 3`), and `glReadPixels` in
        // `UNSIGNED_SHORT_5_6_5` rounds. Measured on Crash: 98.33% of the pixels differ by 1 or 2
        // steps, with worst 2 and average 1.93 — same image. What the count below keeps is the
        // difference that is **not** rounding.
        if d > 2 {
            coarse += 1;
        }
        sum += d;
        worst = worst.max(d as u16);
    }
    let total = software.2.len() / 2;
    // **Guard against an empty pass.** If both screens are blank, the comparison below passes
    // without having compared any image — that is what happened when the GPU frame started being
    // deferred and this test did not materialize before reading.
    let lit = software
        .2
        .chunks_exact(2)
        .filter(|p| p[0] != 0 || p[1] != 0)
        .count();
    assert!(
        lit > total / 100,
        "the frame came out blank ({lit} of {total} pixel(s) lit): there is no image to compare"
    );
    let percent = differing as f64 * 100.0 / total as f64;
    let coarse_pct = coarse as f64 * 100.0 / total as f64;
    let media = sum as f64 / total as f64;
    eprintln!(
        "software x GPU: {differing} of {total} pixel(s) differ ({percent:.2}%), \
         average difference {media:.3} per pixel, worst {worst}, above 2 steps: {coarse_pct:.2}%"
    );
    // **What this test guards is the image, not the rounding.** A different image is off by much
    // more than two channel steps; the rounding of the two converters is off by one or two.
    // Without this separation the test was red over an average of 1.9 steps — and a test nobody
    // can make green stops guarding anything.
    assert!(
        coarse_pct < 1.0,
        "the two rasterizers draw different images: {coarse_pct:.2}% of the pixels differ \
         by more than 2 channel steps (average {media:.3}, worst {worst})"
    );
    assert!(media <= 2.0, "average difference of {media:.3} steps per pixel");
}


    #[test]
    fn a_missing_file_says_it_could_not_be_read() {
        let err = Session::start_inner(
            &std::env::temp_dir().join("zeebx-nao-existe.mod"),
            None,
            None,
            false,
            None,
            Default::default(),
            &[],
        );
        assert!(matches!(err, Err(StartError::Unreadable(_))));
    }

    #[test]
    fn a_file_that_is_not_a_module_is_rejected_with_a_reason() {
        // What matters is not at which stage the garbage is stopped — the `.mod` header is loose,
        // and the one that refuses ends up being the loader —, but that the refusal comes with a
        // readable reason, because that is what the interface shows.
        let path = std::env::temp_dir().join("zeebx-teste-lixo.mod");
        std::fs::write(&path, b"isto nao e um modulo").unwrap();
        let Err(err) = Session::start_inner(&path, None, None, false, None, Default::default(), &[])
        else {
            panic!("a garbage file could not turn into a session");
        };
        assert!(!err.to_string().is_empty());
        let _ = std::fs::remove_file(&path);
    }
}

/// Stores the name of the chosen file next to the overlay, for the save list.
///
/// The overlay folder is named by the hash, which tells nothing to someone about to delete a
/// save, and a loose `.mod` does not even have an extraction in the cache to take a label from.
/// It does not overwrite: the first name sticks, and it is one less file to write on every open.
fn write_title(storage: &StoragePaths, id: &crate::storage::ContentId, content: &Path) {
    let dest = storage.metadata.join(format!("{}.titulo", id.as_str()));
    if dest.exists() {
        return;
    }
    let Some(title) = content.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
        return;
    };
    let _ = std::fs::create_dir_all(&storage.metadata);
    let _ = std::fs::write(dest, title);
}

/// The title of a folder in the save store, so the save list shows the game and not the number.
///
/// The first content to open the folder gives the name; another version of the same game does not
/// change it.
fn write_module_title(storage: &StoragePaths, dir: &str, content: &Path) {
    let dest = storage.metadata.join(crate::storage::module_title_file(dir));
    if dest.exists() {
        return;
    }
    let Some(title) = content.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
        return;
    };
    let _ = std::fs::create_dir_all(&storage.metadata);
    let _ = std::fs::write(dest, title);
}

/// Reads the extension modules that come with a `.mod` and leaves them ready for the loader.
///
/// A `.mod` that does not open is silently ignored: the extension is an extra of the package, and
/// refusing the whole game because a secondary module is corrupted would be trading a game that
/// partly runs for one that does not run.
pub fn extensions_of(mod_path: &std::path::Path) -> Vec<loader::ExtensionImage> {
    crate::library::extensions(mod_path)
        .into_iter()
        .filter_map(|(path, classes)| {
            let bytes = std::fs::read(path).ok()?;
            let image = ModImage::parse(bytes).ok()?;
            Some(loader::ExtensionImage { image, classes })
        })
        .collect()
}
