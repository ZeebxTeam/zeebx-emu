//! A game open in a window: the session, and what the window needs to remember between two frames.
//!
//! It came out of the egui `App` so the Qt interface opens, runs and closes a game the same way —
//! see `docs/implementation/21-qt-migration.md`, phase 1. There is no toolkit here: the input
//! arrives ready (controllers per port, motion, keys already as AVK), and what is returned is what
//! the window should do next. Reading the keyboard, finding a game in the library and drawing stay
//! with it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::input::{self, Pad, PORTS};
use crate::session::{MAX_SLICE, Session, StartError, Z_WHEEL};
use crate::ui::save_slots;
use crate::ui::settings::{Scaling, Settings};
use crate::input::bindings::Controls;
use crate::speed::rewind::{RewindSettings, RewindController, Reading};
use crate::speed::turbo::PortTurbo;
use crate::speed::{FastForward, Toggle, ShortcutMode, Pace};

/// How often the report is rewritten. Two seconds is frequent enough to follow a run and rare
/// enough not to weigh.
const REPORT_INTERVAL: Duration = Duration::from_secs(2);

/// Where a game's serial capture is written, next to the report. `title` is the library's,
/// [`crate::library::title_for`].
pub fn serial_path(title: &str) -> PathBuf {
    let name = match title.is_empty() {
        true => "zeebx.serial.log".to_string(),
        false => format!("{title}.serial.log"),
    };
    crate::config::config_dir().join("relatorios").join(name)
}

/// The folder of the recorded sessions. See [`session_path`].
pub fn sessions_dir() -> PathBuf {
    crate::config::config_dir().join("session_logs")
}

/// Where the session of `title` starting at `start` is written: `yyyy-mm-dd_hh-mm-ss_name.log`.
///
/// One file per run, not one per game like [`Report`]: the problem this file exists to catch
/// shows up after half an hour, and the next run must not delete it.
pub fn session_path(title: &str, start: std::time::SystemTime) -> PathBuf {
    let mut name = String::new();
    for letter in title.chars().flat_map(char::to_lowercase) {
        match letter.is_alphanumeric() {
            true => name.push(letter),
            false if !name.is_empty() && !name.ends_with('-') => name.push('-'),
            false => {}
        }
    }
    let name = match name.trim_end_matches('-') {
        "" => "zeebx",
        name => name,
    };
    sessions_dir().join(format!(
        "{}_{name}.log",
        crate::logging::file_timestamp(start)
    ))
}

/// How often the recorded session logs the process health.
///
/// Ten seconds match the GPU report (300 frames): in a half-hour session that is 180 lines, enough
/// to see a curve rise without drowning the rest.
const HEALTH_INTERVAL: Duration = Duration::from_secs(10);

/// The session recording of a running game. See [`crate::logging::write_to`].
struct Journal {
    /// The recording's number in the log.
    number: u64,
    start: Instant,
    next_health: Instant,
    /// The game's stop has already been logged: it keeps applying on every pass, and one line is
    /// enough.
    stop_recorded: bool,
    /// The API calls at the previous health line, for the window's rate.
    calls_before: u64,
    health_before: Instant,
}

/// The whole process's memory in bytes: resident and committed (private).
///
/// It is the number that tells our leak — the emulator's, the driver's — from the game's leak,
/// which stays inside the guest heap and shows up in [`crate::brew::heap::Snapshot`]. Resident
/// alone is misleading, because the system shrinks it when memory runs short; committed only grows
/// if someone asked and did not give back.
fn process_memory() -> Option<(u64, u64)> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let field = |name: &str| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(name))
                .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
                .map(|kb| kb * 1024)
        };
        Some((field("VmRSS:")?, field("VmData:")?))
    }
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn K32GetProcessMemoryInfo(
                process: isize,
                counters: *mut ProcessMemoryCounters,
                size: u32,
            ) -> i32;
        }
        let mut counters = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            ..Default::default()
        };
        // SAFETY: the structure is Windows' `PROCESS_MEMORY_COUNTERS`, with `cb` filled in, and
        // the current process's pseudo-handle does not need to be closed.
        let ok = unsafe {
            K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb)
        };
        (ok != 0).then_some((
            counters.working_set_size as u64,
            counters.pagefile_usage as u64,
        ))
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
    {
        None
    }
}

/// The report of a run, written on its own to a fixed place.
///
/// Without this the only way to see the report of a game that **does not end** — and the Z-Wheel
/// does not end, it repeats the opening — is to open the log window and export by hand. And an
/// export dialog is something one forgets to confirm: there were three rounds of analyzing a stale
/// report because the file had never been rewritten. A predictable, always current path is worth
/// more than one the user chooses.
#[derive(Default)]
pub struct Report {
    recorded: Option<Instant>,
}

impl Report {
    /// A new game opened: the next request writes right away.
    pub fn forget(&mut self) {
        self.recorded = None;
    }

    /// Writes the running game's report, at most once every two seconds.
    pub fn save(&mut self, running_game: &RunningGame) {
        let now = Instant::now();
        if self.recorded.is_some_and(|before| now - before < REPORT_INTERVAL) {
            return;
        }
        self.recorded = Some(now);
        let dest = running_game.report_path();
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&dest, running_game.session.log().join("\n") + "\n");
    }
}

/// What the window does after a pass. See [`RunningGame::output`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// The game goes on.
    Continue,
    /// The game exited on its own and the Z-Wheel comes back, as on the console.
    ReopenZWheel,
    /// The game exited on its own and there is nowhere to go back to: the window closes.
    Close,
}

/// What is needed, besides the path, to open a game. See [`RunningGame::open`].
pub struct Opening<'a> {
    /// The ones that apply to this game: its own, when it has them (see
    /// [`crate::ui::settings::GameConfig`]). The opener is the one that merges them, because it
    /// is the one that rereads them during the game.
    pub settings: &'a Settings,
    /// Where to write the game's serial, when the log is on.
    pub serial: Option<&'a Path>,
    /// The GL context the GPU rasterizer borrows, when the window has one.
    pub gl: Option<Arc<glow::Context>>,
    /// The library's games, by ClassID and by module identifier: it is the list the Z-Wheel shows
    /// and from which it asks to launch.
    pub installed: Vec<(u32, String)>,
}

pub struct RunningGame {
    session: Session,
    /// The previous frame's controller, per port. Only the press becomes a console key: holding
    /// does not repeat, which is how a tap behaves in a menu.
    previous_pad: [Pad; PORTS],
    /// The BREW keys already delivered, so only the difference becomes an event.
    delivered_keys: HashSet<u32>,
    /// The key events that arrived between two frames, in the order they arrived.
    pending_key_events: Vec<(u32, bool)>,
    /// When the game last ran, to know how much real time it has to catch up.
    last_step: Instant,
    /// Paused by the window. It only changes through [`RunningGame::toggle_pause`], which takes the
    /// sound along.
    paused: bool,
    /// Whether the game was opened by the Z-Wheel. When it exits on its own, the Z-Wheel comes
    /// back, as on the console; opened from the library, exiting closes it.
    pub opened_by_z_wheel: bool,
    /// The session recording to a file, when the option is on.
    journal: Option<Journal>,
    /// The fast-forward shortcut, which is held or toggled. See
    /// [`RunningGame::read_fast_forward`].
    fast_forward: Toggle,
    /// Fast-forward turned on from the menu, apart from the shortcut. See
    /// [`RunningGame::toggle_fast_forward_lock`].
    fast_forward_locked: bool,
    /// Whether fast-forward applied on the last pass.
    fast_forwarding: bool,
    /// Each port's turbo: the toggle mode state. See [`crate::speed::turbo`].
    turbos: [PortTurbo; PORTS],
    /// The return points and the rewind shortcut. See [`RunningGame::read_rewind`].
    rewind: RewindController,
    /// The game's key, from [`crate::library::game_key`].
    key_id: Option<String>,
    /// The state from before the last load, to undo it. See [`RunningGame::undo_load`].
    before_load: Option<Vec<u8>>,
}

/// What the turbo needs to know on a pass: each port's turbo key, the mapping the mode and each
/// player's default button come from, and the rate.
pub struct TickTurbo<'a> {
    pub pressed: [bool; PORTS],
    pub controls: &'a Controls,
    pub taps_per_second: u8,
}

impl RunningGame {
    /// Builds the game's session with what is in the settings.
    ///
    /// `previous` is the running game this one replaces: a game opened by the Z-Wheel starts with
    /// the screen it left (see [`Session::inherit_screen`]); the Z-Wheel itself, reopened, opens
    /// with its own. It comes as `&mut` because the frame the GPU left pending has to get into the
    /// CPU screen before being read (see [`Session::materialize_gl_frame`]) — and this runs with
    /// its GL context current, which is what both windows already guarantee when opening a game.
    pub fn open(
        path: &Path,
        opening: Opening<'_>,
        mut previous: Option<&mut RunningGame>,
    ) -> Result<Self, StartError> {
        let settings = opening.settings;
        // The recording starts before the session: what goes wrong during loading is among the
        // things one most wants to see. The previous one closes first, with its report, or else
        // the start of this game would land in the other's file.
        if let Some(previous) = previous.as_deref_mut() {
            previous.end_journal("the session ended: another game was opened");
        }
        let number = match settings.debug.record_session {
            true => start_journal(path, &opening),
            false => None,
        };
        // The bank is opened when the machine is born: chosen now, it applies to this game.
        crate::audio::soundfont::set_soundfont(settings.audio.soundfont.clone());
        crate::audio::soundfont::set_effects(settings.audio.midi_effects);
        let previous_screen = previous
            .map(RunningGame::session_mut)
            .filter(|session| session.class_id() == Z_WHEEL)
            .map(|session| {
                session.materialize_gl_frame();
                session.screen().to_rgb565_bytes()
            });
        let ports = std::array::from_fn(|port| {
            settings
                .controls
                .player(port)
                .filter(|player| player.enabled)
                .map(|player| player.device_kind)
        });
        let session = Session::start_with(
            path,
            ports,
            opening.serial,
            settings.graphics.gpu_rasterizer,
            opening.gl,
            settings.z_wheel,
        );
        let mut session = match session {
            Ok(session) => session,
            Err(error) => {
                if let Some(number) = number {
                    crate::logging::write(
                        crate::logging::Level::Error,
                        "sessao",
                        &format!("the game did not open: {error}"),
                    );
                    crate::logging::stop_recording(number, "session closed: the game did not open");
                }
                return Err(error);
            }
        };
        if number.is_some() {
            crate::logging::write(
                crate::logging::Level::Info,
                "sessao",
                &format!(
                    "opened: \"{}\", applet {:#010x}",
                    crate::library::strip_fingerprint(session.title()),
                    session.class_id()
                ),
            );
        }
        session.set_internal_resolution(settings.graphics.internal_resolution as usize);
        session.set_aspect_ratio(settings.graphics.ratio.aspect(16.0 / 9.0));
        session.set_enhancements(
            settings.graphics.antialias as usize,
            settings.graphics.anisotropic as usize,
        );
        session.set_fog_allowed(settings.graphics.fog);
        if let Some(screen) = previous_screen.filter(|_| session.class_id() != Z_WHEEL) {
            session.inherit_screen(&screen);
        }
        session.set_installed_applets(opening.installed);
        // The window turns the controller into BREW keys (see [`RunningGame::advance`]): the turbo
        // has to pulse the key too.
        session.turbo_on_keys(true);
        // Turning sound on here is safe **because the game has not started yet**: `start` only
        // prepares, and `EVT_APP_START` goes out on the first loop pass. Before, the game already
        // played inside `start`, and the sound came out with an empty screen. Without the `audio`
        // feature — the Libretro core, which hands the sound to its frontend — the session has no
        // sound output to turn on.
        #[cfg(feature = "audio")]
        if let Some(error) = session.set_audio(settings.audio.enabled, settings.audio.volume) {
            eprintln!("no sound: {error}");
        }
        Ok(Self {
            session,
            previous_pad: Default::default(),
            delivered_keys: HashSet::new(),
            pending_key_events: Vec::new(),
            last_step: Instant::now(),
            paused: false,
            opened_by_z_wheel: false,
            fast_forward: Toggle::default(),
            fast_forward_locked: false,
            fast_forwarding: false,
            turbos: Default::default(),
            rewind: RewindController::default(),
            key_id: crate::library::game_key(path),
            before_load: None,
            journal: number.map(|number| Journal {
                number,
                start: Instant::now(),
                next_health: Instant::now() + HEALTH_INTERVAL,
                stop_recorded: false,
                calls_before: 0,
                health_before: Instant::now(),
            }),
        })
    }

    /// Closes this running game's recording, with the emulator's report before the last line.
    ///
    /// The report goes at the end, not bit by bit, because it is cumulative: missing APIs, invalid
    /// accesses, the game's grouped log. The snapshot at closing is the one that has everything.
    fn end_journal(&mut self, reason: &str) {
        let Some(journal) = self.journal.take() else {
            return;
        };
        let report = self.session.log();
        if !report.is_empty() {
            crate::logging::write_block(
                journal.number,
                &format!(
                    "\n——— emulator report at the end of the session ———\n{}\n———\n\n",
                    report.join("\n")
                ),
            );
        }
        let minutes = journal.start.elapsed().as_secs_f64() / 60.0;
        crate::logging::stop_recording(
            journal.number,
            &format!("{reason}, after {minutes:.1} min"),
        );
    }

    /// The recorded session's health line, and the game's stop when it happens.
    fn update_journal(&mut self) {
        let Some(journal) = self.journal.as_mut() else {
            return;
        };
        if !journal.stop_recorded {
            if let Some(reason) = self.session.stopped_reason() {
                journal.stop_recorded = true;
                crate::logging::write(
                    crate::logging::Level::Error,
                    "sessao",
                    &format!("the game stopped: {reason}"),
                );
            }
        }
        let now = Instant::now();
        if now < journal.next_health {
            return;
        }
        journal.next_health = now + HEALTH_INTERVAL;
        // Per real second, like the line's other rates. The total goes along because it was the
        // total, not the rate, that hit the old ceiling of 200 million (issue #70).
        let calls = self.session.api_calls();
        let window = (now - journal.health_before).as_secs_f64().max(0.001);
        let calls_per_second =
            calls.saturating_sub(journal.calls_before) as f64 / window;
        journal.calls_before = calls;
        journal.health_before = now;
        let sample = self.session.sample();
        let heap = self.session.heap_snapshot();
        let (_, objects) = self.session.memory();
        let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        let process = match process_memory() {
            Some((resident, committed)) => format!(
                "process {:.1} MB resident and {:.1} MB committed",
                mb(resident),
                mb(committed)
            ),
            None => "process memory unavailable on this system".to_string(),
        };
        crate::logging::write(
            crate::logging::Level::Info,
            "saude",
            &format!(
                "{} min of session, {:.1} s on the game clock; speed {}%, {} frames/s, \
                 {:.1} M instructions/s, {:.0} thousand API calls/s ({:.1} M in total); \
                 {process}; game heap {:.1} of {:.1} MB in {} blocks, \
                 largest hole {:.1} MB; {objects} live BREW objects",
                journal.start.elapsed().as_secs() / 60,
                f64::from(self.session.clock_ms()) / 1000.0,
                sample.speed,
                sample.fps,
                sample.ips as f64 / 1e6,
                calls_per_second / 1e3,
                calls as f64 / 1e6,
                mb(u64::from(heap.used)),
                mb(u64::from(heap.cap)),
                heap.live,
                mb(u64::from(heap.largest_hole)),
            ),
        );
    }

    /// Where this run's report is written on its own. See [`Report`].
    pub fn report_path(&self) -> PathBuf {
        let name = match self.session.title() {
            title if !title.is_empty() => format!("{title}.log"),
            _ => "zeebx.log".to_string(),
        };
        crate::config::config_dir().join("relatorios").join(name)
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// This game's key. See [`crate::library::game_key`].
    pub fn key_id(&self) -> Option<&str> {
        self.key_id.as_deref()
    }

    /// Forgets what the input had left: the previous controller, the delivered keys, the pause.
    ///
    /// It is what opening another game did to the window state before the running game existed,
    /// and it still applies to the running game that stays when opening the next one fails.
    pub fn forget_input(&mut self) {
        self.previous_pad = Default::default();
        self.delivered_keys.clear();
        self.pending_key_events.clear();
        self.set_paused(false);
        self.fast_forward.disable();
        self.fast_forward_locked = false;
        self.fast_forwarding = false;
        for turbo in &mut self.turbos {
            turbo.disable();
        }
        self.rewind.release();
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn toggle_pause(&mut self) {
        self.set_paused(!self.paused);
    }

    /// Pauses or releases the game, and the sound with it: without this the music kept playing
    /// with the image frozen. See [`Session::freeze_audio`].
    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.session.freeze_audio(paused);
    }

    /// Reads this pass's rewind shortcut. It comes **before** [`RunningGame::read_fast_forward`]
    /// and [`RunningGame::advance`]: while going back, the game does not advance and fast-forward
    /// is suspended. See [`RewindController::read`].
    ///
    /// **Going back a point is GL**, like advancing: the restored state rebuilds the textures on
    /// the GPU. The caller leaves the rasterizer's context current — Qt called this outside it,
    /// and the point's textures ended up in the Qt Quick context. And, as in
    /// [`RunningGame::advance`], the window may have painted on the same context since the
    /// previous step: the GPU state mirror is told first.
    pub fn read_rewind(&mut self, pressed: bool, settings_page: &RewindSettings) {
        if pressed && settings_page.enabled {
            self.session.resume_context();
        }
        let reading = self.rewind.read(&mut self.session, pressed, self.paused, settings_page);
        if reading == Reading::Released {
            self.reset_clock();
        }
    }

    /// What the window writes over the game while it goes back: `Some(true)` when there are no
    /// points left.
    pub fn rewind_indicator(&self) -> Option<bool> {
        self.rewind.indicator()
    }

    /// Forgets the return points: a loaded save state is another timeline.
    pub fn forget_points(&mut self) {
        self.rewind.forget_points();
    }

    /// Saves the state in the game's `slot`, under `root` (see [`crate::ui::save_slots`]).
    ///
    /// **With the rasterizer's context current**, like [`RunningGame::read_rewind`]: the thumbnail
    /// and the frame that goes into the state are read from the GPU texture.
    pub fn save_state(&mut self, root: &Path, slot: usize) -> Result<save_slots::Slot, String> {
        let id = self.content_id()?;
        save_slots::save(root, &id, slot, &mut self.session)
    }

    /// Loads the state from `slot`, first keeping the current one for [`RunningGame::undo_load`].
    ///
    /// With the rasterizer's context current: the restored state rebuilds the textures on the
    /// GPU.
    pub fn load_state(&mut self, root: &Path, slot: usize) -> Result<save_slots::Slot, String> {
        let id = self.content_id()?;
        // **The current one is kept first**, not after the slot is read: it is the accidental
        // load that costs dearly, and the undo is what gives it back. Without being able to save
        // now — a drawing in progress —, the load goes on, and there is nothing to undo: the
        // player asked for it.
        let before = self.session.can_save().ok().map(|()| {
            self.session.materialize_gl_frame();
            self.session.save_state()
        });
        let slot = save_slots::load(root, &id, slot, &mut self.session)?;
        self.before_load = before;
        self.after_restore();
        Ok(slot)
    }

    /// Goes back to the state from before the last load. Only once: undoing again does not redo.
    pub fn undo_load(&mut self) -> Result<(), String> {
        let before = self
            .before_load
            .take()
            .ok_or_else(|| "no load to undo".to_string())?;
        self.session.resume_context();
        if let Err(error) = self.session.restore_state(&before) {
            // The state is from the same session, and the engine would only refuse it because of
            // a bug of its own; it stays stored so a second attempt does not lose the progress for
            // good.
            self.before_load = Some(before);
            return Err(error.to_string());
        }
        self.after_restore();
        Ok(())
    }

    /// Whether there is a load to undo.
    pub fn can_undo_load(&self) -> bool {
        self.before_load.is_some()
    }

    fn content_id(&self) -> Result<String, String> {
        self.session
            .content()
            .map(|id| id.as_str().to_string())
            .ok_or_else(|| "this game's file was not identified".to_string())
    }

    /// What changes in the window when the console goes back to another instant.
    ///
    /// The rewind points belong to another timeline. So does the input the game already received:
    /// whatever is pressed now arrives again as a press on the first restored frame. And the real
    /// time spent loading is not for the game to catch up. The pause stays as it was: loading with
    /// the game stopped shows the loaded instant, stopped.
    fn after_restore(&mut self) {
        self.rewind.forget_points();
        self.previous_pad = Default::default();
        self.delivered_keys.clear();
        self.pending_key_events.clear();
        self.reset_clock();
    }

    /// Whether any player has toggle turbo on: the window says "Turbo" on screen.
    pub fn turbo_on(&self) -> bool {
        self.turbos.iter().any(PortTurbo::enabled)
    }

    /// Reads this pass's fast-forward shortcut and says whether it applies. Paused, it does not:
    /// the pause is a pause, and a toggle that is on waits for the game to come back.
    pub fn read_fast_forward(&mut self, pressed: bool, mode: ShortcutMode) -> bool {
        let enabled = self.fast_forward.update(pressed, mode) || self.fast_forward_locked;
        self.fast_forwarding = enabled && !self.paused && !self.rewind.rewinding();
        self.fast_forwarding
    }

    /// Turns fast-forward on or off from the menu, and says how it ended up.
    ///
    /// **A separate switch, not the shortcut's.** A menu item cannot be held: handing the shortcut
    /// a continuous "pressed" would turn it on in hold mode, but in toggle mode it would become a
    /// single press, and releasing would not turn it off. The lock applies on top of both modes,
    /// and the shortcut keeps working as always.
    pub fn toggle_fast_forward_lock(&mut self) -> bool {
        self.fast_forward_locked = !self.fast_forward_locked;
        self.fast_forward_locked
    }

    pub fn fast_forward_locked(&self) -> bool {
        self.fast_forward_locked
    }

    /// What the window writes over the game while it fast-forwards, or nothing. `decimal_comma` is
    /// the language's decimal separator. See [`crate::speed::fast_forward_label`].
    pub fn fast_forward_indicator(&self, fast_forward: &FastForward, decimal_comma: bool) -> Option<String> {
        self.fast_forwarding.then(|| {
            let reached = self.session.reached_speed();
            crate::speed::fast_forward_label(fast_forward.ratio(), reached, decimal_comma)
        })
    }

    /// Restarts the real-time count: what has passed up to here is not for the game to catch up.
    pub fn reset_clock(&mut self) {
        self.last_step = Instant::now();
    }

    /// A keyboard event: `keyboard` are the physical keys pressed now, already as AVK.
    ///
    /// **Every event counts, not only the state at the end of the frame.** A tap that starts and
    /// ends between two frames has to reach the game as a press and a release; looking only at the
    /// state on the next frame, it would vanish. The controllers come in with the previous frame's
    /// state, which is what is known of them until the next reading.
    pub fn keyboard_changed(&mut self, keyboard: impl IntoIterator<Item = u32>) {
        if self.paused {
            return;
        }
        let active = input::active_avks(keyboard, &self.previous_pad);
        let events = input::transitions(&mut self.delivered_keys, active);
        self.pending_key_events.extend(events);
    }

    /// One pass: hands the input to the game and makes it advance the real time that passed.
    ///
    /// Paused, it neither delivers nor advances — but [`RunningGame::output`] still applies,
    /// because a launch request made before the pause still needs to be served.
    pub fn advance(
        &mut self,
        pads: &[(usize, Pad)],
        motions: [[f32; 3]; PORTS],
        keyboard: impl IntoIterator<Item = u32>,
        pace: &Pace,
        turbo: &TickTurbo<'_>,
    ) {
        if self.paused || self.rewind.rewinding() {
            return;
        }
        self.previous_pad = Default::default();
        for &(port, pad) in pads {
            self.previous_pad[port] = pad;
        }
        let active = input::active_avks(keyboard, &self.previous_pad);
        let events = input::transitions(&mut self.delivered_keys, active);
        self.pending_key_events.extend(events);

        for &(port, pad) in pads {
            self.session.set_port_pad(port, pad);
        }
        // Which buttons pulse comes from here; when they are pressed, from the session, by the
        // game's clock. A port without a controller on this pass goes without turbo.
        self.session.set_turbo_rate(turbo.taps_per_second);
        for port in 0..PORTS {
            let pad = pads.iter().find(|(p, _)| *p == port).map(|(_, pad)| *pad);
            let player = turbo.controls.player(port);
            let pulsing = match (pad, player) {
                (Some(pad), Some(player)) => self.turbos[port].pulsing(
                    player.turbo,
                    &player.turbo_button,
                    &pad,
                    turbo.pressed[port],
                ),
                _ => 0,
            };
            self.session.set_turbo(port, pulsing);
        }
        for (port, motion) in motions.into_iter().enumerate() {
            self.session.set_port_motion(port, motion);
        }
        for (avk, pressed) in self.pending_key_events.drain(..) {
            self.session.set_key(avk, pressed);
        }
        // The budget is the real time that passed since the previous frame — and **not** a fixed
        // slice. A 16 ms slice became a speed ceiling: with the window synced to the monitor, it
        // was enough for emulation plus drawing to go past one vsync for the period to double to
        // 33 ms, and the game got 16 out of every 33, stuck at 50%. That is what Crash's selection
        // screen showed. The `MAX_SLICE` ceiling is only for the host that cannot keep up.
        //
        // With intermediate screens waiting, one goes to the screen and the game does not advance
        // on this frame.
        let now = Instant::now();
        let slice = (now - self.last_step).min(MAX_SLICE);
        self.last_step = now;
        if !self.session.show_intermediate_frame() {
            // `egui` painted on the same context since the previous step.
            self.session.resume_context();
            let _ = self.session.advance(slice, pace);
            self.rewind.track(&mut self.session);
        }
        self.update_journal();
    }

    /// The ClassID the game asked to launch, if it asked. Consumes the request.
    ///
    /// **Check this before [`RunningGame::output`].** The reopened Z-Wheel asks for the game and
    /// exits on the same pass; looking at the output first, the window reopened it again and the
    /// game never opened.
    pub fn launch_request(&mut self) -> Option<u32> {
        self.session.take_launch_request()
    }

    /// What to do after this pass.
    ///
    /// The Z-Wheel exits on its own to open the chosen game: reopening it is the console's role.
    /// The same when the game it opened closes — through its menu's `ISHELL_CloseApplet`, for
    /// example: the console goes back to the home screen. Opened from the library, a game that
    /// exits on its own closes its window. A failure is not an exit: the game stays on screen,
    /// with the reason.
    pub fn output(&self) -> Output {
        if !self.session.exited_on_its_own() {
            return Output::Continue;
        }
        match self.session.class_id() == Z_WHEEL || self.opened_by_z_wheel {
            true => Output::ReopenZWheel,
            false => Output::Close,
        }
    }
}

impl Drop for RunningGame {
    fn drop(&mut self) {
        self.end_journal("session closed normally");
    }
}

/// Opens the session file with the header. `None` when it could not: the recording is an
/// instrument, and failing to open the file is no reason not to open the game.
fn start_journal(path: &Path, opening: &Opening<'_>) -> Option<u64> {
    let settings = opening.settings;
    let start = std::time::SystemTime::now();
    let title = crate::library::strip_fingerprint(&crate::library::title_for(path));
    let dest = session_path(&title, start);
    let id = crate::library::module_id(path).unwrap_or_else(|| "?".to_string());
    let applet = match crate::library::applet_clsid(path) {
        Some(class_id) => format!("{class_id:#010x}"),
        None => "on the first line after opening".to_string(),
    };
    let graphics = &settings.graphics;
    let rasterizer = match (graphics.gpu_rasterizer, opening.gl.is_some()) {
        (true, true) => "GPU",
        (true, false) => "GPU requested, but no GL context: software",
        (false, _) => "software",
    };
    let header = format!(
        "Zeebx — session log\n\
         Game:          {title}\n\
         ID:            module {id}, applet {applet}\n\
         Package:       {}\n\
         Started:       {}\n\
         Emulator:      Zeebx {} ({}/{})\n\
         Video:         {rasterizer} rasterizer, internal resolution {}x, antialias {}x, \
         anisotropic {}x\n\
         Audio:         {}, volume {}\n\
         Log:           this file, INFO and up; the log window, {} and up\n\
         Health:        one `saude:` line every {} s, and `placa:` every 300 frames\n\
         Shutdown:      the last line says how the session ended. Without it, the process died\n\
         \x20              without warning: native crash, force-closed or power loss.\n\
         {}\n",
        path.display(),
        crate::logging::timestamp(start),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        graphics.internal_resolution,
        graphics.antialias,
        graphics.anisotropic,
        match settings.audio.enabled {
            true => "on",
            false => "off",
        },
        settings.audio.volume,
        settings.debug.log_level,
        HEALTH_INTERVAL.as_secs(),
        "=".repeat(96),
    );
    match crate::logging::write_to(&dest, &header) {
        Ok(number) => Some(number),
        Err(error) => {
            eprintln!("could not record the session to {}: {error}", dest.display());
            None
        }
    }
}

/// The height of the console screen, in pixels. The width comes from the aspect ratio: 640 at 4:3.
const SCREEN_HEIGHT: f32 = 480.0;

/// The size the frame is drawn at inside `area`, which is the window's free space.
///
/// `aspect` is the image's width over height: 4:3 at native, wider at the experimental 16:9.
/// Kept apart from the windows because it is the only part with a real rule, and the only one that
/// can be checked without opening one — and it is the same in egui and in Qt.
pub fn fit_viewport(area: [f32; 2], scale: Scaling, keep_aspect_ratio: bool, aspect: f32) -> [f32; 2] {
    let native = [SCREEN_HEIGHT * aspect, SCREEN_HEIGHT];
    if area[0] <= 0.0 || area[1] <= 0.0 {
        return native;
    }
    let times = |factor: f32| [native[0] * factor, native[1] * factor];
    let fits = (area[0] / native[0]).min(area[1] / native[1]);
    match (scale, keep_aspect_ratio) {
        (Scaling::Stretch, false) => area,
        (Scaling::Stretch, true) | (Scaling::Fit, _) => times(fits),
        // It never disappears: below one times the original size, it shrinks proportionally instead
        // of not fitting, because a small window must not hide the game.
        (Scaling::Integer, _) => match fits >= 1.0 {
            true => times(fits.floor()),
            false => times(fits),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saves, advances, loads and undoes in a real running game, and checks the game clock and the
    /// screen at each step. It is the path of the Qt window's save state shortcuts.
    fn save_run_load_and_undo(rom: &str, gl: Option<Arc<glow::Context>>) {
        let mut settings = Settings::default();
        settings.graphics.gpu_rasterizer = gl.is_some();
        settings.audio.enabled = false;
        let opening = Opening {
            settings: &settings,
            serial: None,
            gl,
            installed: Vec::new(),
        };
        let mut running_game =
            RunningGame::open(Path::new(rom), opening, None).expect("the game opened");
        let advance = |running_game: &mut RunningGame, ms: u32| {
            let target = running_game.session().clock_ms() + ms;
            while running_game.session().clock_ms() < target {
                running_game.session_mut().step(Duration::from_millis(50), false);
                assert_eq!(running_game.session().stopped_reason(), None, "the game stopped");
            }
        };
        let screen = |running_game: &mut RunningGame| {
            running_game.session_mut().materialize_gl_frame();
            running_game.session().screen().signature()
        };
        let root = std::env::temp_dir().join(format!(
            "zeebx-testes-partida-estado-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);

        advance(&mut running_game, 3000);
        assert!(
            running_game.undo_load().is_err(),
            "without a load there is nothing to undo"
        );
        running_game
            .save_state(&root, 4)
            .expect("the state was saved");
        let (saved_at, saved_screen) = (running_game.session().clock_ms(), screen(&mut running_game));

        advance(&mut running_game, 2000);
        let (clock_before, screen_before) = (running_game.session().clock_ms(), screen(&mut running_game));
        running_game
            .load_state(&root, 4)
            .expect("the state was loaded");
        assert_eq!(
            running_game.session().clock_ms(),
            saved_at,
            "the clock went back to the saved instant"
        );
        assert_eq!(
            screen(&mut running_game),
            saved_screen,
            "the screen went back to the saved instant's"
        );
        assert!(running_game.can_undo_load());

        running_game.undo_load().expect("the load was undone");
        assert_eq!(
            running_game.session().clock_ms(),
            clock_before,
            "the clock went back to before the load"
        );
        assert_eq!(
            screen(&mut running_game),
            screen_before,
            "the screen went back to before the load"
        );
        assert!(!running_game.can_undo_load(), "undo applies once");

        // And the game keeps advancing after all of it.
        advance(&mut running_game, 1000);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Only with `ZEEBX_TEST_ROM`. Opens the game with the `config_dir` profile: run it with
    /// `XDG_CONFIG_HOME` pointing at a disposable folder, so as not to touch the real saves.
    #[test]
    fn a_save_state_round_trips_in_a_running_game() {
        let Ok(rom) = crate::config::env_var("ZEEBX_TEST_ROM") else {
            eprintln!("no ZEEBX_TEST_ROM: nothing to save");
            return;
        };
        save_run_load_and_undo(&rom, None);
    }

    /// The same through the GPU: the state's thumbnail and screen come from the texture, and
    /// loading rebuilds the textures in the context.
    #[cfg(feature = "gpu")]
    #[test]
    fn a_save_state_round_trips_in_a_running_game_on_the_gpu() {
        let Ok(rom) = crate::config::env_var("ZEEBX_TEST_ROM") else {
            eprintln!("no ZEEBX_TEST_ROM: nothing to save");
            return;
        };
        let context = match crate::video::context::GlContext::create() {
            Ok(context) => context,
            Err(reason) => {
                eprintln!("no offscreen GPU: {reason}");
                return;
            }
        };
        save_run_load_and_undo(&rom, Some(context.gl.clone()));
    }

    #[test]
    fn integer_scaling_only_uses_exact_multiples() {
        // A 1500x1100 window fits the 640x480 screen twice, not two and a bit.
        let size = fit_viewport([1500.0, 1100.0], Scaling::Integer, true, 4.0 / 3.0);
        assert_eq!(size, [1280.0, 960.0]);
    }

    #[test]
    fn integer_scaling_shrinks_when_1x_does_not_fit() {
        // A window smaller than the screen must not hide the game, so there it shrinks.
        let size = fit_viewport([320.0, 240.0], Scaling::Integer, true, 4.0 / 3.0);
        assert_eq!(size, [320.0, 240.0]);
    }

    #[test]
    fn fit_to_window_keeps_the_aspect_ratio() {
        // Window too wide: border left on the sides, no stretching.
        let size = fit_viewport([1920.0, 480.0], Scaling::Fit, true, 4.0 / 3.0);
        assert_eq!(size, [640.0, 480.0]);
    }

    #[test]
    fn stretch_only_distorts_when_the_aspect_ratio_is_waived() {
        let area = [1000.0, 500.0];
        assert_eq!(fit_viewport(area, Scaling::Stretch, false, 4.0 / 3.0), area);
        // With the aspect ratio kept, "fill" becomes "fit".
        assert_eq!(
            fit_viewport(area, Scaling::Stretch, true, 4.0 / 3.0),
            fit_viewport(area, Scaling::Fit, true, 4.0 / 3.0)
        );
    }
}
