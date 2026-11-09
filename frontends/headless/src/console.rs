//! The console running, with nothing around it.
//!
//! It is what the two modes — with a window and without — have in common: the session, input, the
//! clock and the Z-Wheel chain. The only thing that differs between them is where the frame goes,
//! and that is left out of here on purpose: the one with a window calls [`Console::step`] and
//! paints; the one without calls the same [`Console::step`] and writes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use zeebx::input::gamepads::Gamepads;
use zeebx::input::{self, Pad};
use zeebx::session::{MAX_SLICE, Session, Step};
use zeebx::ui::library::{self, Game};
use zeebx::ui::settings::Settings;

use crate::config::Headless;
use crate::host_input::{self, Keyboard};

/// Why the loop ended.
pub enum LoopEnd {
    /// Keep going.
    Continue,
    /// The game stopped — finished, exited on its own or broke —, and there is nowhere to go
    /// back to.

    Finished(String),
    /// It broke in a way that needs to be reported.
    Error(String),
}

pub struct Console {
    pub settings: Settings,
    pub options: Headless,
    session: Option<Session>,
    /// What the ROMs folder holds. It is what lets the Z-Wheel open a title by ClassID.
    games: Vec<Game>,
    /// Where the console goes back to when a game exits. `None` makes the emulator exit too.
    z_wheel: Option<PathBuf>,
    /// Whether what is running was opened by the Z-Wheel. See [`Console::step`].
    via_z_wheel: bool,
    /// The GL context the session uses to rasterize 3D. With a window it is the window's; without
    /// a window, the core's offscreen one.
    gl: Option<Arc<glow::Context>>,
    pub keyboard: Keyboard,
    pads: Gamepads,
    /// The state of each port in the previous frame, to know what changed.
    prior: [Pad; input::PORTS],
    /// The BREW keys already delivered, to send only the transitions.
    delivered: HashSet<u32>,
    last: Instant,
    /// Whether the last turn found the game ahead. Whoever spins the loop uses this to sleep an
    /// instant instead of coming right back: without a window, or with vsync off, nothing else
    /// holds the turn back and the loop would burn a whole core waiting for the clock.
    ahead: bool,
    /// When to stop, in milliseconds of the game clock. `None` runs until closed.
    clock_limit: Option<u32>,
}

impl Console {
    /// Builds the console. The session does not start yet: with a window, it needs the GL context
    /// that only exists after the window opens.
    pub fn create(settings: Settings, options: Headless) -> Self {
        let games = match &options.roms {
            Some(dir) => library::scan(dir),
            None => Vec::new(),
        };
        // **Only the Z-Wheel the file points to.** The core knows how to look for one on its own
        // — in the ROMs folder and, failing that, by name inside the home folder —, and that makes
        // sense in the interface, where it is a friendly guess for someone who has not configured
        // anything yet. In a binary without an interface it does not: it would be called by another
        // program and would open something nobody asked for, found by rummaging through the home
        // of whoever ran it. The `z_wheel_in` that remains here is not a search, it is resolution —
        // it turns the `.zip`, the `.mod` or the folder the person pointed to into the path that
        // gets opened.
        let z_wheel = options.z_wheel.as_deref().and_then(library::z_wheel_in);
        Self {
            clock_limit: options.seconds.map(|s| s.saturating_mul(1000)),
            settings,
            options,
            session: None,
            games,
            z_wheel,
            via_z_wheel: false,
            gl: None,
            keyboard: Keyboard::default(),
            pads: Gamepads::new(),
            prior: Default::default(),
            delivered: HashSet::new(),
            last: Instant::now(),
            ahead: false,
        }
    }

    /// Whether the game is ahead of the wall clock.
    pub fn ahead(&self) -> bool {
        self.ahead
    }

    /// The context in which 3D will be rasterized. Set before opening the game.
    pub fn use_context(&mut self, gl: Option<Arc<glow::Context>>) {
        self.gl = gl;
    }

    pub fn session_mut(&mut self) -> Option<&mut Session> {
        self.session.as_mut()
    }

    /// Opens a game. One that was already running is dropped — it is what the console does.
    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        // The screen the Z-Wheel left keeps counting until the game draws its first one: on the
        // console the switch does not go through a black frame.
        // The pending frame of the previous session goes in before it is dropped: without this, a
        // game opened right after the Z-Wheel draws would inherit the screen from two frames back.
        if let Some(previous) = self.session.as_mut() {
            previous.materialize_gl_frame();
        }
        let previous_screen = self
            .session
            .as_ref()
            .filter(|previous| previous.class_id() == zeebx::session::Z_WHEEL)
            .map(|previous| previous.screen().to_rgb565_bytes());

        let ports = std::array::from_fn(|port| {
            self.settings
                .controls
                .player(port)
                .filter(|player| player.enabled)
                .map(|player| player.device_kind)
        });
        zeebx::audio::soundfont::set_soundfont(self.settings.audio.soundfont.clone());
        zeebx::audio::soundfont::set_effects(self.settings.audio.midi_effects);
        let g = &self.settings.graphics;
        let mut session = Session::start_with(
            path,
            ports,
            None,
            g.gpu_rasterizer,
            self.gl.clone(),
            self.settings.z_wheel,
        )
        .map_err(|error| error.to_string())?;

        session.set_internal_resolution(g.internal_resolution as usize);
        session.set_aspect_ratio(g.ratio.aspect(16.0 / 9.0));
        session.set_enhancements(g.antialias as usize, g.anisotropic as usize);
        session.set_fog_allowed(g.fog);
        if let Some(screen) = previous_screen.filter(|_| session.class_id() != zeebx::session::Z_WHEEL) {
            session.inherit_screen(&screen);
        }
        // What the Z-Wheel sees as installed. Without this its list comes up empty and there is
        // nothing to open.
        session.set_installed_applets(
            self.games
                .iter()
                .filter_map(|game| Some((game.clsid?, library::module_id(&game.path)?))),
        );
        // Turning sound on here is safe **because the game has not started yet**: `start` only
        // prepares, and `EVT_APP_START` goes out on the first turn of the loop.
        let audio = &self.settings.audio;
        if let Some(error) = session.set_audio(audio.enabled, audio.volume) {
            eprintln!("warning: no sound: {error}");
        }

        eprintln!("running: {}", session.title());
        self.session = Some(session);
        self.prior = Default::default();
        self.delivered.clear();
        self.keyboard.release_all();
        self.last = Instant::now();
        Ok(())
    }

    /// Drops the session.
    ///
    /// It has to happen **before** the GL context dies: the rasterizer keeps textures and buffers
    /// created in it, and releasing them afterwards would be talking about objects that no longer
    /// exist.
    pub fn close(&mut self) {
        self.session = None;
    }

    /// One turn: reads input, moves the game forward, and handles what it asked for on exit.
    pub fn step(&mut self) -> LoopEnd {
        if self.session.is_none() {
            return LoopEnd::Finished("no game".to_string());
        }

        self.pads.poll();
        // The state of each enabled port, from its mapping. Built before borrowing the session:
        // it needs the settings, which live in the same `self`.
        let ports: Vec<(usize, Pad)> = self
            .settings
            .controls
            .enabled_players()
            .map(|(index, player)| {
                (
                    index,
                    host_input::port_pad(player, &self.keyboard, &self.pads, index),
                )
            })
            .collect();

        // The console sends BREW keys besides the controller state: it is with them that the
        // Z-Wheel navigates. Only transitions go, otherwise every loop turn would repeat the press.
        let mut current: HashSet<u32> = self
            .keyboard
            .pressed()
            .iter()
            .filter_map(|key| input::avk_for(*key))
            .collect();
        for (_, pad) in &ports {
            current.extend(
                input::controller_key_events(&Pad::default(), pad)
                    .into_iter()
                    .filter_map(|(avk, pressed)| pressed.then_some(avk)),
            );
        }
        let key_events = transitions(&mut self.delivered, current);
        for (index, pad) in &ports {
            self.prior[*index] = *pad;
        }

        let pace = self.settings.speed.pace();
        let session = self.session.as_mut().expect("checked above");
        for (index, pad) in ports {
            session.set_port_pad(index, pad);
        }
        for (avk, pressed) in key_events {
            session.set_key(avk, pressed);
        }

        // The budget is the real time that has passed since the previous turn, clamped to the
        // ceiling: it is how much the game needs to emulate to keep up with the wall clock.
        let now_instant = Instant::now();
        let slice = (now_instant - self.last).min(MAX_SLICE);
        self.last = now_instant;
        // An intermediate screen that is waiting goes to the display first, and the game does not
        // move on this turn.
        self.ahead = false;
        if !session.show_intermediate_frame() {
            // With a window, it painted in the same context since the previous turn, and the GPU
            // state mirror does not know: without telling it, the game would draw with the
            // viewport, scissor and blending it left. Without a window the context is our own and
            // this does nothing. See `Session::resume_context`.
            session.resume_context();
            match session.advance(slice, &pace) {
                Step::Stopped => return LoopEnd::Finished(session.stopped_reason().unwrap_or_default()),
                Step::Ahead => self.ahead = true,
                Step::Presented | Step::Running => {}
            }
        }

        if let Some(cap) = self.clock_limit {
            if session.clock_ms() >= cap {
                return LoopEnd::Finished("the requested time is up".to_string());
            }
        }

        // The Z-Wheel opens a game by exiting and asking for its class. It is the console that
        // makes the switch.
        if let Some(class_id) = session.take_launch_request() {
            let path = self
                .games
                .iter()
                .find(|game| game.clsid == Some(class_id))
                .map(|game| game.path.clone());
            return match path {
                Some(path) => match self.open(&path) {
                    Ok(()) => {
                        self.via_z_wheel = true;
                        LoopEnd::Continue
                    }
                    Err(error) => LoopEnd::Error(format!("{}: {error}", path.display())),
                },
                None => LoopEnd::Error(format!(
                    "the Z-Wheel asked for class {class_id:#010x}, which is not in the ROMs folder"
                )),
            };
        }

        if session.exited_on_its_own() {
            // A game that exits — through its menu, through `ISHELL_CloseApplet` — returns the
            // console to the home screen, and the Zeebo's home screen is the Z-Wheel. Without it,
            // or with `exit_with_game`, the emulator exits too: it is what an outside frontend
            // expects, since its own screen is what shows up again.
            let tick = session.class_id() == zeebx::session::Z_WHEEL || self.via_z_wheel;
            let z_wheel = self.z_wheel.clone();
            if let (false, true, Some(path)) = (self.options.exit_with_game, tick, z_wheel) {
                self.via_z_wheel = false;
                return match self.open(&path) {
                    Ok(()) => LoopEnd::Continue,
                    Err(error) => LoopEnd::Error(format!("{}: {error}", path.display())),
                };
            }
            return LoopEnd::Finished("the game exited".to_string());
        }

        LoopEnd::Continue
    }
}

/// What changed between two sets of keys: the ones released and the ones pressed.
fn transitions(prior: &mut HashSet<u32>, current: HashSet<u32>) -> Vec<(u32, bool)> {
    let mut events: Vec<_> = prior
        .difference(&current)
        .map(|&avk| (avk, false))
        .collect();
    events.extend(current.difference(prior).map(|&avk| (avk, true)));
    events.sort_unstable();
    *prior = current;
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the edges go: a key that stays pressed is not repeated, and one that was released goes
    /// out with `false`. Without this the Z-Wheel would move one step per loop turn.

    #[test]
    fn only_transitions_are_delivered() {
        let mut delivered = HashSet::new();
        assert_eq!(
            transitions(&mut delivered, HashSet::from([input::avk::UP])),
            vec![(input::avk::UP, true)]
        );
        assert!(transitions(&mut delivered, HashSet::from([input::avk::UP])).is_empty());
        assert_eq!(
            transitions(&mut delivered, HashSet::new()),
            vec![(input::avk::UP, false)]
        );
    }
}
