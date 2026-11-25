//! The running game: one turn of the loop, and what the pause menu does to it.
//!
//! It is the same turn as Android's — the shortcuts first, then rewind, then the controller, the
//! turbo and the step — over the same `Settings`. What differs is where the frame goes: there is
//! no GL context here, so the frame is the software rasterizer's, converted to BGRA once per
//! change and painted by UIKit.

use std::path::Path;
use std::time::Instant;

use zeebx::input::bindings::Source;
use zeebx::input::{self, AXIS_RANGE, Pad};
use zeebx::session::{MAX_SLICE, Session, Step};
use zeebx::speed::rewind::Reading;
use zeebx::speed::turbo::TURBO_BUTTON;
use zeebx::ui::debug_panel;
use zeebx::ui::save_slots;

use crate::{AHEAD, App, FRAME, NO_SESSION, RUNNING, STOPPED, bgra_from_rgb565, c_text};

impl App {
    /// Opens a game, with the sound and graphics settings currently in effect.
    pub(crate) fn open(&mut self, path: &Path) -> bool {
        self.set_error("");
        // The bank is opened when the machine is born: chosen now, it applies to this game.
        zeebx::audio::soundfont::set_soundfont(self.settings.audio.soundfont.clone());
        zeebx::audio::soundfont::set_effects(self.settings.audio.midi_effects);
        // No GPU: on iOS the frame is the software rasterizer's, painted by UIKit. The core's GL
        // context asks for EGL, and there is none here.
        let started = Session::start_software_with_storage_installed(
            path,
            zeebx::DEFAULT_PORTS,
            self.settings.z_wheel,
            &self.storage,
            &self.installed,
        );
        let mut session = match started {
            Ok(session) => session,
            Err(error) => {
                let reason = error.to_string();
                self.set_error(&self.catalog.format("play.open_failed", &[("reason", &reason)]));
                return false;
            }
        };
        session.set_fog_allowed(self.settings.graphics.fog);
        // This frontend turns the controller into BREW keys (see [`App::step`]): the turbo has to
        // pulse the key too.
        session.turbo_on_keys(true);
        // A device with no audio output cannot keep the game from running: missing sound goes to
        // the log, not to the player's face.
        let audio = &self.settings.audio;
        if let Some(reason) = session.set_audio(audio.enabled, audio.volume) {
            eprintln!("no sound: {reason}");
        }
        // Without an identity there are no save states; the game still opens.
        self.state_id = self.storage.content_id(path).ok().map(|id| id.as_str().to_string());
        self.save_slots = match self.state_id.as_deref() {
            Some(id) => save_slots::list(&self.states_root, id),
            None => std::array::from_fn(|_| None),
        };
        self.session = Some(session);
        self.open_path = Some(path.to_path_buf());
        self.forget_session_state();
        self.publish();
        true
    }

    /// Closes the game. The return points, the turbo and the fast-forward toggle do not carry
    /// over to the next one.
    pub(crate) fn close(&mut self) {
        self.session = None;
        self.open_path = None;
        self.state_id = None;
        self.save_slots = std::array::from_fn(|_| None);
        self.forget_session_state();
        self.frame.clear();
        self.width = 0;
        self.height = 0;
    }

    /// Restarts the open game from scratch: closes and opens the same path, like a console's
    /// reset button. What the game wrote to disk stays, and so do the save states.
    pub(crate) fn restart(&mut self) -> bool {
        let Some(path) = self.open_path.clone() else {
            return false;
        };
        self.close();
        self.open(&path)
    }

    fn forget_session_state(&mut self) {
        self.menu = false;
        self.paused = false;
        self.previous_pad = Pad::default();
        self.keyboard = 0;
        self.overlay.release();
        self.rewind = Default::default();
        self.fast_forward.disable();
        self.fast_forwarding = false;
        self.turbo.disable();
        self.frame_key = None;
        self.indicator = Default::default();
        self.debug_line = Default::default();
        self.last = Instant::now();
    }

    /// The app is leaving the screen, or the controller went away with a button down: the
    /// release will never arrive. On the console, losing the device equals releasing everything.
    pub(crate) fn release_input(&mut self) {
        self.controller.clear();
        self.keyboard = 0;
        self.overlay.release();
        self.last = Instant::now();
    }

    pub(crate) fn set_menu(&mut self, open: bool) {
        self.menu = open && self.session.is_some();
        if self.menu {
            // A finger still on a piece when the menu opens would hold its button for as long as
            // the menu is up.
            self.overlay.release();
        }
    }

    pub(crate) fn apply_graphics(&mut self) {
        let fog = self.settings.graphics.fog;
        if let Some(session) = &mut self.session {
            session.set_fog_allowed(fog);
        }
    }

    pub(crate) fn apply_audio(&mut self) {
        let audio = &self.settings.audio;
        if let Some(session) = &mut self.session
            && let Some(reason) = session.set_audio(audio.enabled, audio.volume)
        {
            eprintln!("no sound: {reason}");
        }
    }

    /// One turn of the loop: delivers the input, advances the game by the real time that has
    /// passed and publishes the frame.
    pub(crate) fn step(&mut self) -> i32 {
        let Some(session) = self.session.as_mut() else {
            return NO_SESSION;
        };

        // The session shortcuts: the on-screen piece, or the physical controller button bound to
        // them in settings. Rewind comes before everything: while rewinding, the game does not
        // advance and fast-forward is suspended. See `docs/implementation/24-speed.md`.
        let on_screen = self.overlay.shortcuts;
        let controller = self.controller;
        let held = |sources: &[Source]| {
            sources.iter().any(|source| match source {
                Source::Button { name } => controller.is_held(name),
                _ => false,
            })
        };
        let shortcuts = &self.settings.shortcuts;
        let speed = &self.settings.speed;
        let player = self.settings.controls.player(0);
        // `voltar` in the saved shortcuts is rewind, the same field the desktop window reads.
        let rewind_held = on_screen.rewind || held(&shortcuts.rewind);
        let fast_forward_held = on_screen.fast_forward || held(&shortcuts.fast_forward);
        let turbo_held =
            on_screen.turbo || player.is_some_and(|player| held(player.sources(TURBO_BUTTON)));
        // The shortcuts' physical sources do not reach the game: see `Shortcuts::reserved`.
        let reserved = |physical: &str| {
            shortcuts
                .reserved()
                .any(|source| matches!(source, Source::Button { name } if name == physical))
        };

        if !self.menu {
            let reading = self.rewind.read(session, rewind_held, self.paused, &speed.rewind);
            if reading == Reading::Released {
                self.last = Instant::now();
            }
        }

        // With the menu on screen, paused or rewinding, the game stays still — the clock
        // included, so it does not spend the whole budget at once when it resumes.
        let still = self.menu || self.paused || self.rewind.rewinding();
        // Sound stops too, except during rewind, which already silences it and swaps in each
        // point's sound. See `Session::freeze_audio`.
        session.freeze_audio(self.menu || self.paused);
        let mut result = RUNNING;
        if still {
            self.fast_forwarding = false;
            self.last = Instant::now();
        } else {
            // The budget is the real time since the previous turn, clamped to the ceiling: it is
            // how much the game needs to emulate to keep up with the wall clock. The ceiling
            // exists so a turn that took long — the app came back from the background — does not
            // become a jump of minutes.
            let now = Instant::now();
            let slice = now.saturating_duration_since(self.last).min(MAX_SLICE);
            self.last = now;

            let mut pad = controller.pad(player, reserved);
            pad.buttons |= self.keyboard | self.overlay.buttons;
            // The on-screen axis only counts where the physical stick is at rest: the two are not
            // added together, because half the travel of each is not the full travel of either.
            for (axis, touch) in pad.axes.iter_mut().zip(self.overlay.axes) {
                if *axis == 0 {
                    *axis = (touch * AXIS_RANGE as f32).round() as i32;
                }
            }
            session.set_port_pad(0, pad);
            for (avk, pressed) in input::controller_key_events(&self.previous_pad, &pad) {
                session.set_key(avk, pressed);
            }
            self.previous_pad = pad;
            // Player 1's turbo: which buttons pulse comes from here, and when, from the game
            // clock.
            let pulsing = match player {
                Some(player) => {
                    self.turbo.pulsing(player.turbo, &player.turbo_button, &pad, turbo_held)
                }
                None => 0,
            };
            session.set_turbo_rate(speed.turbo_per_second);
            session.set_turbo(0, pulsing);

            // With intermediate screens waiting, one goes to the screen and the game does not
            // advance on this turn.
            if session.show_intermediate_frame() {
                result = FRAME;
            } else {
                let fast_forwarding =
                    self.fast_forward.update(fast_forward_held, speed.fast_forward.mode);
                self.fast_forwarding = fast_forwarding;
                let step = session.advance(slice, &speed.pace_with(fast_forwarding));
                self.rewind.track(session);
                result = match step {
                    Step::Presented => FRAME,
                    Step::Ahead => AHEAD,
                    Step::Running => RUNNING,
                    Step::Stopped => STOPPED,
                };
            }
        }

        // **Exiting the game is not failing.** The applet that asks to close ends normally, and
        // goes back to the library without a word; the message is kept for what breaks — an API
        // that does not exist, a null pointer.
        if result == STOPPED || session.exited_on_its_own() {
            let failure = (result == STOPPED && !session.exited_normally())
                .then(|| session.stopped_reason().unwrap_or_default());
            self.close();
            // An error from earlier — a save state that failed — is not this exit's reason.
            match failure {
                Some(reason) => {
                    self.set_error(&self.catalog.format("play.failed", &[("reason", &reason)]));
                }
                None => self.set_error(""),
            }
            return STOPPED;
        }

        self.publish();
        self.write_texts();
        result
    }

    /// Converts the console's frame, when it changed since the last conversion.
    pub(crate) fn publish(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let screen = session.screen();
        let key = (screen.generation(), screen.writes());
        if self.frame_key == Some(key) && !self.frame.is_empty() {
            return;
        }
        self.frame_key = Some(key);
        self.width = screen.width();
        self.height = screen.height();
        self.frame.clear();
        bgra_from_rgb565(screen.pixels(), &mut self.frame);
        self.frame_serial = self.frame_serial.wrapping_add(1);
    }

    /// What is written over the game: the "◀◀", the "▶▶ 3x", the "Turbo" and the pause, while
    /// they are active — a toggle left on by mistake looks like a game bug — and the debug
    /// panel's line. The same texts as the desktop window.
    fn write_texts(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let catalog = &self.catalog;
        let decimal_comma = !catalog.current().starts_with("en");
        let rewind = self.rewind.indicator().map(|no_points| match no_points {
            true => format!("◀◀ {}", catalog.get("rewind.empty")),
            false => "◀◀".to_string(),
        });
        let fast_forward = self.fast_forwarding.then(|| {
            zeebx::speed::fast_forward_label(
                self.settings.speed.fast_forward.ratio(),
                session.reached_speed(),
                decimal_comma,
            )
        });
        let turbo = self.turbo.enabled().then(|| catalog.get("turbo.on_screen").to_string());
        let paused = (self.paused && !self.menu).then(|| catalog.get("play.paused").to_string());
        let indicator: Vec<String> =
            rewind.into_iter().chain(fast_forward).chain(turbo).chain(paused).collect();
        self.indicator = c_text(&indicator.join("  ·  "));

        let debug = &self.settings.debug;
        self.debug_line = match debug.overlay {
            true => {
                let texts = debug_panel::Texts::fresh(
                    catalog,
                    debug,
                    session.sample(),
                    session.memory(),
                    session.clock_ms(),
                );
                let line: Vec<String> =
                    [texts.speed, texts.clock, texts.memory].into_iter().flatten().collect();
                c_text(&line.join("  ·  "))
            }
            false => Default::default(),
        };
    }

    /// Writes the state to `slot`. On failure the reason is in the error, already in the
    /// player's language.
    pub(crate) fn save_state(&mut self, slot: usize) -> bool {
        let result = match (self.state_id.as_deref(), self.session.as_mut()) {
            (Some(id), Some(session)) => save_slots::save(&self.states_root, id, slot, session),
            _ => Err(self.catalog.get("play.state.unavailable").to_string()),
        };
        match result {
            Ok(saved) => {
                self.save_slots[slot] = Some(saved);
                true
            }
            Err(reason) => {
                self.set_error(&self.catalog.format("play.state.error", &[("reason", &reason)]));
                false
            }
        }
    }

    /// Puts the state of `slot` into the game and takes it out of the menu, running.
    pub(crate) fn load_state(&mut self, slot: usize) -> bool {
        let result = match (self.state_id.as_deref(), self.session.as_mut()) {
            (Some(id), Some(session)) => save_slots::load(&self.states_root, id, slot, session),
            _ => Err(self.catalog.get("play.state.unavailable").to_string()),
        };
        if let Err(reason) = result {
            self.set_error(&self.catalog.format("play.state.error", &[("reason", &reason)]));
            return false;
        }
        // The rewind points belong to another timeline.
        self.rewind.forget_points();
        // The console state is back; the input, however, is what is pressed now, not what was
        // pressed at the saved instant. Clearing it keeps the tap that confirmed Load from
        // getting into the first restored frame.
        self.previous_pad = Pad::default();
        self.overlay.release();
        if let Some(session) = self.session.as_mut() {
            session.set_port_pad(0, Pad::default());
        }
        self.frame_key = None;
        self.menu = false;
        self.paused = false;
        self.last = Instant::now();
        self.publish();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_app;
    use std::time::Duration;

    /// Turns of the loop at the pace of a display link, for about `ms` of real time.
    fn run(app: &mut App, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            assert_ne!(app.step(), STOPPED, "{:?}", app.error);
            std::thread::sleep(Duration::from_millis(8));
        }
    }

    fn clock(app: &App) -> u32 {
        app.session.as_ref().unwrap().clock_ms()
    }

    /// The whole path a player walks, over a real game: open, play, open the menu, save, load,
    /// pause, restart, close. It is **environment-driven**, like the session's own tests: without
    /// `ZEEBX_TEST_ROM` there is no game to run, and the repository carries none.
    #[test]
    fn a_real_game_plays_pauses_and_comes_back_from_a_save_state() {
        let Ok(rom) = zeebx::config::env_var("ZEEBX_TEST_ROM") else {
            eprintln!("no ZEEBX_TEST_ROM: nothing to run");
            return;
        };
        let rom = std::path::PathBuf::from(rom);
        let (root, mut app) = test_app("jogo");
        std::fs::copy(&rom, app.roms.join(rom.file_name().unwrap())).unwrap();
        app.start_scan();
        assert!(app.finish_scan(true));
        assert_eq!(app.games.len(), 1);
        let path = app.games[0].path.clone();

        assert!(app.open(&path), "{:?}", app.error);
        run(&mut app, 1500);
        assert_eq!(app.frame.len(), (app.width * app.height * 4) as usize);
        assert!(app.width > 0 && clock(&app) > 0);
        assert!(app.state_id.is_some(), "a file that was read has an identity");

        // The menu stops the game clock; the turns keep coming and change nothing.
        app.set_menu(true);
        app.step();
        let stopped_at = clock(&app);
        run(&mut app, 200);
        assert_eq!(clock(&app), stopped_at);

        assert!(app.save_state(3), "{:?}", app.error);
        let slots: serde_json::Value = serde_json::from_str(&app.slots_json()).unwrap();
        assert_eq!(slots["available"], true);
        assert_eq!(slots["slots"][3]["used"], true);
        assert_eq!(slots["slots"][0]["used"], false);
        let (width, height) = app.thumbnail(3).expect("the slot has a picture");
        assert_eq!(app.image.len(), (width * height * 4) as usize);
        assert!(!app.load_state(0), "an empty slot loads nothing");
        assert!(app.menu, "and leaves the menu where it was");

        // Play on, then go back: the clock returns to the saved instant and the game runs.
        app.set_menu(false);
        run(&mut app, 600);
        assert!(clock(&app) > stopped_at);
        app.set_menu(true);
        assert!(app.load_state(3), "{:?}", app.error);
        assert!(!app.menu && !app.paused);
        assert!(clock(&app) <= stopped_at + 100, "{} against {stopped_at}", clock(&app));
        run(&mut app, 300);

        // Paused by choice: still, and it says so over the game.
        app.paused = true;
        app.step();
        let paused_at = clock(&app);
        run(&mut app, 200);
        assert_eq!(clock(&app), paused_at);
        assert_eq!(app.indicator.to_str().unwrap(), app.catalog.get("play.paused"));
        app.paused = false;

        // The debug line follows its switch.
        assert!(app.debug_line.is_empty());
        app.settings_set("debug_overlay", 1);
        app.step();
        assert!(!app.debug_line.is_empty());

        // Restart is the same game from zero, and the save state is still there.
        assert!(app.restart(), "{:?}", app.error);
        assert!(clock(&app) < paused_at);
        assert!(app.save_slots[3].is_some());
        run(&mut app, 300);

        app.close();
        assert_eq!(app.step(), NO_SESSION);
        assert!(app.frame.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
