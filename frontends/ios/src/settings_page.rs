//! The settings screen, described as data.
//!
//! What gets changed here is the **same** [`zeebx::ui::settings::Settings`] as desktop and
//! Android, saved in the same format, with labels from the same language catalog. What is
//! particular to this frontend is who draws: Swift receives a list of rows — a switch, a choice, a
//! slider, a row that does something — and paints them the way iOS paints settings. It knows no
//! option by name.
//!
//! The split is on purpose. The Swift side is only compiled on a Mac, and what decides which
//! options exist, which are greyed out and what a tap changes is the part worth testing — so it
//! is here, where `cargo test` reaches it on any host.
//!
//! What was left out was left out because it does not exist here: the window options make no
//! sense on a single screen, and everything that depends on the graphics card (internal
//! resolution, antialiasing, the wide aspect ratio) is an empty function in the software
//! rasterizer, which is the only one this frontend has.

use std::ops::RangeInclusive;
use std::path::PathBuf;

use serde_json::{Value, json};

use zeebx::input::bindings::{CONFIGURABLE, Source};
use zeebx::speed::rewind::RewindSettings;
use zeebx::speed::turbo::{self, TurboMode};
use zeebx::speed::{FastForward, FpsLimit, Frameskip, ShortcutMode};
use zeebx::ui::settings::{self, OnScreenControlsMode, Scaling, Settings};

use crate::{App, controller};

/// A shortcut the physical controller can trigger: the two session ones and player 1's turbo key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalShortcut {
    FastForward,
    /// Held rewind. The stored shortcut is still `voltar`; see [`Settings`] shortcuts.
    Rewind,
    Turbo,
}

impl PhysicalShortcut {
    const ALL: [Self; 3] = [Self::FastForward, Self::Rewind, Self::Turbo];

    fn id(self) -> &'static str {
        match self {
            Self::FastForward => "capture.fast_forward",
            Self::Rewind => "capture.rewind",
            Self::Turbo => "capture.turbo",
        }
    }

    fn sources(self, settings: &Settings) -> Vec<Source> {
        match self {
            Self::FastForward => settings.shortcuts.fast_forward.clone(),
            Self::Rewind => settings.shortcuts.rewind.clone(),
            Self::Turbo => settings
                .controls
                .player(0)
                .map(|player| player.sources(turbo::TURBO_BUTTON).to_vec())
                .unwrap_or_default(),
        }
    }

    fn sources_mut(self, settings: &mut Settings) -> &mut Vec<Source> {
        match self {
            Self::FastForward => &mut settings.shortcuts.fast_forward,
            Self::Rewind => &mut settings.shortcuts.rewind,
            Self::Turbo => settings
                .controls
                .player_mut(0)
                .buttons
                .entry(turbo::TURBO_BUTTON.to_string())
                .or_default(),
        }
    }
}

/// What the next physical button press becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    Shortcut(PhysicalShortcut),
    /// A Zeebo button, by its name in [`CONFIGURABLE`].
    Button(&'static str),
}

/// The sections, in the same order as desktop's tabs.
const TABS: [&str; 7] = ["general", "graphics", "speed", "audio", "controls", "debug", "about"];

/// The rows of a section, in the order they are drawn.
#[derive(Default)]
struct Rows(Vec<Value>);

impl Rows {
    fn section(&mut self, title: &str) {
        self.0.push(json!({ "kind": "section", "label": title }));
    }

    /// A line of text. `warn` is for the one that says why something below it does nothing.
    fn note(&mut self, text: &str, warn: bool) {
        self.0.push(json!({ "kind": "note", "label": text, "warn": warn }));
    }

    fn toggle(&mut self, id: &str, label: &str, hint: Option<&str>, on: bool, enabled: bool) {
        self.0.push(json!({
            "kind": "toggle", "id": id, "label": label, "hint": hint, "on": on, "enabled": enabled,
        }));
    }

    fn choice(
        &mut self,
        id: &str,
        label: &str,
        hint: Option<&str>,
        options: Vec<String>,
        selected: usize,
        enabled: bool,
    ) {
        self.0.push(json!({
            "kind": "choice", "id": id, "label": label, "hint": hint, "options": options,
            "selected": selected, "enabled": enabled,
        }));
    }

    /// `format` is how the value reads, with `{n}` where the number goes: Swift redraws it while
    /// the thumb drags, without asking for the page again.
    fn slider(
        &mut self,
        id: &str,
        label: &str,
        hint: Option<&str>,
        value: u8,
        range: RangeInclusive<u8>,
        format: &str,
    ) {
        self.0.push(json!({
            "kind": "slider", "id": id, "label": label, "hint": hint, "value": value,
            "min": range.start(), "max": range.end(), "format": format, "enabled": true,
        }));
    }

    /// A row that does something when tapped. `role` tells Swift whether the doing is ours
    /// (`call`), or something only it can do: open the layout editor (`editor`) or an address
    /// (`url`, with the address in `detail`).
    fn action(&mut self, id: &str, label: &str, detail: &str, role: &str, enabled: bool) {
        self.0.push(json!({
            "kind": "action", "id": id, "label": label, "detail": detail, "role": role,
            "enabled": enabled,
        }));
    }
}

fn position<T: PartialEq>(all: &[T], current: &T) -> usize {
    all.iter().position(|item| item == current).unwrap_or(0)
}

impl App {
    /// The `.sf2` banks the player can choose from.
    ///
    /// There is no file picker here: the app's folder is the one the Files app shows, and a bank
    /// dropped in it — or in `soundfonts` or `roms` inside it — is found by looking.
    pub(crate) fn soundfonts(&self) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = [
            self.documents.clone(),
            self.documents.join("soundfonts"),
            self.roms.clone(),
        ]
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("sf2"))
        })
        .collect();
        found.sort();
        found
    }

    /// **The app's folder moves.** iOS gives the container a new path when the app is updated or
    /// restored, and the bank's path was saved whole: after an update it would point into a
    /// folder that is gone, and the music would fall back to the timbre table without a word. A
    /// bank that no longer exists is looked for again by its file name.
    pub(crate) fn relocate_soundfont(&mut self) {
        let Some(saved) = self.settings.audio.soundfont.clone() else {
            return;
        };
        if saved.is_file() {
            return;
        }
        let moved = self
            .soundfonts()
            .into_iter()
            .find(|candidate| candidate.file_name() == saved.file_name());
        if let Some(moved) = moved {
            self.settings.audio.soundfont = Some(moved);
            self.save();
        }
    }

    /// The whole screen: the sections and the rows of `tab`, as JSON.
    pub(crate) fn settings_json(&self, tab: &str) -> String {
        let tab = match TABS.contains(&tab) {
            true => tab,
            false => TABS[0],
        };
        let catalog = &self.catalog;
        let tabs: Vec<Value> = TABS
            .iter()
            .map(|id| json!({ "id": id, "title": catalog.get(&format!("settings.tab.{id}")) }))
            .collect();
        let rows = match tab {
            "graphics" => self.tab_graphics(),
            "speed" => self.tab_speed(),
            "audio" => self.tab_audio(),
            "controls" => self.tab_controls(),
            "debug" => self.tab_debug(),
            "about" => self.tab_about(),
            _ => self.tab_general(),
        };
        json!({ "tab": tab, "tabs": tabs, "rows": rows.0 }).to_string()
    }

    fn tab_general(&self) -> Rows {
        let catalog = &self.catalog;
        let mut rows = Rows::default();
        rows.section(catalog.get("settings.roms_folder"));
        rows.note(catalog.get("ios.roms.where"), false);
        let count = catalog.format("library.count", &[("count", &self.games.len().to_string())]);
        rows.action("rescan", catalog.get("library.rescan"), &count, "call", self.scan.is_none());

        let languages = catalog.languages();
        let selected = languages
            .iter()
            .position(|language| language.code == catalog.current())
            .unwrap_or(0);
        rows.choice(
            "language",
            catalog.get("settings.language"),
            None,
            languages.iter().map(|language| language.name.clone()).collect(),
            selected,
            true,
        );
        rows
    }

    fn tab_graphics(&self) -> Rows {
        let catalog = &self.catalog;
        let graphics = &self.settings.graphics;
        let mut rows = Rows::default();
        rows.choice(
            "scaling",
            catalog.get("graphics.scaling"),
            Some(catalog.get("graphics.scaling.integer.hint")),
            Scaling::ALL.iter().map(|mode| catalog.get(mode.key()).to_string()).collect(),
            position(&Scaling::ALL, &graphics.scaling),
            true,
        );
        // 4:3 only has an effect on "Fill": in the other two modes the aspect ratio is already
        // kept, and showing the option there gave a switch that does nothing.
        if graphics.scaling == Scaling::Stretch {
            rows.toggle(
                "keep_aspect",
                catalog.get("graphics.keep_aspect"),
                None,
                graphics.keep_aspect,
                true,
            );
        }
        rows.toggle("smooth", catalog.get("graphics.smooth"), None, graphics.smooth, true);
        rows.section("3D");
        rows.toggle(
            "fog",
            catalog.get("graphics.fog"),
            Some(catalog.get("graphics.fog.hint")),
            graphics.fog,
            true,
        );
        rows
    }

    /// The game's pace against the wall clock. See `docs/implementation/24-speed.md`.
    fn tab_speed(&self) -> Rows {
        let catalog = &self.catalog;
        let speed = &self.settings.speed;
        let mut rows = Rows::default();
        rows.choice(
            "fps_limit",
            catalog.get("speed.fps_limit"),
            Some(catalog.get("speed.fps_limit.hint")),
            FpsLimit::ALL.iter().map(|limit| catalog.get(limit.key_id()).to_string()).collect(),
            position(&FpsLimit::ALL, &speed.fps_limit),
            true,
        );
        let skips = Frameskip::all();
        rows.choice(
            "frameskip",
            catalog.get("speed.frameskip"),
            Some(catalog.get("speed.frameskip.hint")),
            skips.iter().map(|skip| settings::frameskip_label(*skip, catalog)).collect(),
            position(&skips, &speed.frameskip.normalized()),
            true,
        );

        rows.section(catalog.get("speed.ff"));
        let ratios = FastForward::choices();
        rows.choice(
            "ff_ratio",
            catalog.get("speed.ff.ratio"),
            Some(catalog.get("ios.ff.hint")),
            ratios
                .iter()
                .map(|ratio| match ratio {
                    0 => catalog.get("speed.ff.unlimited").to_string(),
                    n => format!("{n}x"),
                })
                .collect(),
            position(&ratios, &speed.fast_forward.ratio),
            true,
        );
        rows.choice(
            "ff_mode",
            catalog.get("speed.ff.mode"),
            None,
            ShortcutMode::ALL.iter().map(|mode| catalog.get(mode.key_id()).to_string()).collect(),
            position(&ShortcutMode::ALL, &speed.fast_forward.mode),
            true,
        );
        rows.toggle(
            "ff_mute",
            catalog.get("speed.ff.mute"),
            Some(catalog.get("speed.ff.mute.hint")),
            speed.fast_forward.mute,
            true,
        );

        rows.section(catalog.get("speed.rewind"));
        rows.toggle(
            "rewind_on",
            catalog.get("speed.rewind.on"),
            Some(catalog.get("ios.rewind.hint")),
            speed.rewind.enabled,
            true,
        );
        // The rewind's rows only make sense with the rewind turned on.
        let enabled = speed.rewind.enabled;
        rows.choice(
            "rewind_interval",
            catalog.get("speed.rewind.interval"),
            None,
            RewindSettings::INTERVALS_MS
                .iter()
                .map(|&ms| match ms % 1000 {
                    0 => catalog.format("speed.rewind.every_s", &[("n", &(ms / 1000).to_string())]),
                    _ => catalog.format("speed.rewind.every_ms", &[("n", &ms.to_string())]),
                })
                .collect(),
            position(&RewindSettings::INTERVALS_MS, &speed.rewind.interval_ms),
            enabled,
        );
        rows.choice(
            "rewind_memory",
            catalog.get("speed.rewind.memory"),
            Some(catalog.get("speed.rewind.memory.hint")),
            RewindSettings::MEMORY_SIZES_MB.iter().map(|mb| format!("{mb} MB")).collect(),
            position(&RewindSettings::MEMORY_SIZES_MB, &speed.rewind.memory_mb),
            enabled,
        );
        rows
    }

    fn tab_audio(&self) -> Rows {
        let catalog = &self.catalog;
        let audio = &self.settings.audio;
        let mut rows = Rows::default();
        rows.toggle(
            "audio",
            catalog.get("audio.enabled"),
            Some(catalog.get("audio.hint")),
            audio.enabled,
            true,
        );
        rows.slider("volume", catalog.get("audio.volume"), None, audio.volume, 0..=100, "{n}%");

        // The bank is opened when the game opens: the hint says the change applies to the next
        // one.
        rows.section(catalog.get("audio.soundfont"));
        let banks = self.soundfonts();
        let mut options = vec![catalog.get("audio.soundfont.auto").to_string()];
        options.extend(banks.iter().map(|bank| {
            bank.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        }));
        let selected = audio
            .soundfont
            .as_ref()
            .and_then(|chosen| banks.iter().position(|bank| bank == chosen))
            .map_or(0, |index| index + 1);
        rows.choice(
            "soundfont",
            catalog.get("settings.browse"),
            Some(catalog.get("audio.soundfont.hint")),
            options,
            selected,
            true,
        );
        rows.note(catalog.get("ios.soundfont.where"), false);
        rows.toggle(
            "midi_effects",
            catalog.get("audio.midi_effects"),
            Some(catalog.get("audio.midi_effects.hint")),
            audio.midi_effects,
            true,
        );
        rows
    }

    /// The controls drawn on screen, the physical controller's buttons and its shortcuts.
    fn tab_controls(&self) -> Rows {
        let catalog = &self.catalog;
        let touch = &self.settings.on_screen_controls;
        let mut rows = Rows::default();
        rows.choice(
            "touch_mode",
            catalog.get("touch.mode"),
            Some(catalog.get("touch.mode.hint")),
            OnScreenControlsMode::ALL
                .iter()
                .map(|mode| catalog.get(mode.key_id()).to_string())
                .collect(),
            position(&OnScreenControlsMode::ALL, &touch.mode),
            true,
        );
        // Size and position also apply under "never": whoever turns them off to play with a
        // controller can leave everything arranged for the next time without one.
        rows.slider("touch_scale", catalog.get("touch.scale"), None, touch.scale, 50..=200, "{n}%");
        rows.slider(
            "touch_opacity",
            catalog.get("touch.opacity"),
            None,
            touch.opacity,
            15..=100,
            "{n}%",
        );
        let layout = match touch.positions.is_empty() && touch.sizes.is_empty() {
            true => catalog.get("touch.layout.default"),
            false => catalog.get("touch.layout.custom"),
        };
        rows.action("touch_editor", catalog.get("touch.layout"), layout, "editor", true);
        // The shortcut pieces, one switch each: turbo and rewind, on top of that, only show up
        // with the feature in use. See `OnScreenControls::show`.
        rows.toggle("touch_show_ff", catalog.get("touch.show.ff"), None, touch.show_fast_forward, true);
        rows.toggle(
            "touch_show_rewind",
            catalog.get("touch.show.rewind"),
            None,
            touch.show_rewind,
            true,
        );
        rows.toggle("touch_show_turbo", catalog.get("touch.show.turbo"), None, touch.show_turbo, true);

        // Player 1's turbo, the only one a phone has. See `zeebx::speed::turbo`.
        rows.section(catalog.get("turbo"));
        let player = self.settings.controls.player(0);
        let mode = player.map(|player| player.turbo).unwrap_or_default();
        rows.choice(
            "turbo_mode",
            catalog.get("turbo.mode"),
            Some(catalog.get("turbo.hint.touch")),
            TurboMode::ALL.iter().map(|mode| catalog.get(mode.key_id()).to_string()).collect(),
            position(&TurboMode::ALL, &mode),
            true,
        );
        let button = player.map_or(turbo::DEFAULT_BUTTON, |player| player.turbo_button.as_str());
        rows.choice(
            "turbo_button",
            catalog.get("turbo.button"),
            None,
            turbo::PULSABLE
                .iter()
                .map(|button| catalog.get(&format!("button.{button}")).to_string())
                .collect(),
            position(&turbo::PULSABLE, &button),
            true,
        );
        rows.slider(
            "turbo_rate",
            catalog.get("turbo.rate"),
            None,
            self.settings.speed.turbo_per_second,
            turbo::MIN_PRESSES..=turbo::MAX_PRESSES,
            catalog.get("turbo.rate.value"),
        );

        // The physical controller. Tapping a row waits for the controller's next button; tapping
        // it again gives up.
        rows.section(catalog.get("ios.gamepad.buttons"));
        match self.controller_connected {
            true => rows.note(catalog.get("ios.gamepad.hint"), false),
            false => rows.note(catalog.get("ios.gamepad.none"), true),
        }
        let waiting = catalog.get("touch.shortcut.waiting");
        let mut remapped = false;
        for zeebo in CONFIGURABLE {
            let bound = player.map(|player| controller::bound_to(player, zeebo)).unwrap_or_default();
            remapped |= !bound.is_empty();
            let detail = match (self.capturing == Some(Capture::Button(zeebo)), bound.is_empty()) {
                (true, _) => waiting.to_string(),
                (false, false) => join(bound),
                // Nothing bound, and the factory button may have been given to another: then the
                // row is honestly empty.
                (false, true) => {
                    let factory: Vec<&str> = controller::factory_for(zeebo)
                        .into_iter()
                        .filter(|physical| !self.is_remapped_away(physical))
                        .collect();
                    match factory.is_empty() {
                        true => catalog.get("controls.unbound").to_string(),
                        false => join(factory),
                    }
                }
            };
            rows.action(
                &format!("map.{zeebo}"),
                catalog.get(&format!("button.{zeebo}")),
                &detail,
                "call",
                true,
            );
        }
        rows.action("map_reset", catalog.get("ios.gamepad.reset"), "", "call", remapped);

        // Shortcuts through the physical controller: on a phone there is no F10.
        rows.section(catalog.get("settings.tab.shortcuts"));
        for which in PhysicalShortcut::ALL {
            let label = match which {
                PhysicalShortcut::FastForward => catalog.get("controls.shortcut.fast_forward"),
                PhysicalShortcut::Rewind => catalog.get("controls.shortcut.rewind"),
                PhysicalShortcut::Turbo => catalog.get("button.turbo"),
            };
            let detail = match self.capturing == Some(Capture::Shortcut(which)) {
                true => waiting.to_string(),
                false => {
                    let sources = which.sources(&self.settings);
                    let buttons: Vec<&str> = sources
                        .iter()
                        .filter_map(|source| match source {
                            Source::Button { name } => Some(name.as_str()),
                            _ => None,
                        })
                        .collect();
                    match buttons.is_empty() {
                        true => catalog.get("controls.unbound").to_string(),
                        false => join(buttons),
                    }
                }
            };
            rows.action(which.id(), label, &detail, "call", true);
        }
        rows.action("clear_shortcuts", catalog.get("touch.shortcut.clear"), "", "call", true);
        rows
    }

    /// Whether the player bound `physical` to some Zeebo button, taking it from its factory role.
    fn is_remapped_away(&self, physical: &str) -> bool {
        self.settings.controls.player(0).is_some_and(|player| {
            CONFIGURABLE
                .iter()
                .any(|zeebo| controller::bound_to(player, zeebo).contains(&physical))
        })
    }

    fn tab_debug(&self) -> Rows {
        let catalog = &self.catalog;
        let debug = &self.settings.debug;
        let mut rows = Rows::default();
        rows.toggle(
            "debug_overlay",
            catalog.get("debug.overlay"),
            Some(catalog.get("debug.hint")),
            debug.overlay,
            true,
        );
        // The panel's rows only make sense with the panel turned on.
        rows.section(catalog.get("debug.show"));
        rows.toggle("debug_speed", catalog.get("debug.speed"), None, debug.speed, debug.overlay);
        rows.toggle("debug_clock", catalog.get("debug.clock"), None, debug.clock, debug.overlay);
        rows.toggle("debug_memory", catalog.get("debug.memory"), None, debug.memory, debug.overlay);
        rows
    }

    fn tab_about(&self) -> Rows {
        let catalog = &self.catalog;
        let mut rows = Rows::default();
        rows.note(catalog.get("about.tagline"), false);
        rows.note(
            &catalog.format("about.version", &[("version", env!("CARGO_PKG_VERSION"))]),
            false,
        );
        rows.action("link.repository", catalog.get("about.repository"), zeebx::ui::REPOSITORY, "url", true);
        rows.action("link.discord", catalog.get("about.discord"), zeebx::ui::DISCORD, "url", true);
        rows
    }

    /// A row changed. `value` is the switch (0 or 1), the index of the choice, or the slider's
    /// number; a row that only does something ignores it.
    pub(crate) fn settings_set(&mut self, id: &str, value: i32) {
        let index = usize::try_from(value).unwrap_or(0);
        let on = value != 0;
        let number = value.clamp(0, 255) as u8;
        // Any tap gives up a capture that was waiting — including a tap on the waiting row
        // itself, which is how one cancels.
        let was_capturing = self.capturing.take();
        if id == "rescan" {
            self.start_scan();
            return;
        }
        // Looked up before the settings are borrowed for writing: the list comes from the disk.
        let bank = match (id, index) {
            ("soundfont", 1..) => match self.soundfonts().into_iter().nth(index - 1) {
                Some(bank) => Some(bank),
                None => return,
            },
            _ => None,
        };
        let settings = &mut self.settings;
        match id {
            "language" => {
                let Some(code) = self.catalog.languages().get(index).map(|l| l.code.clone())
                else {
                    return;
                };
                self.catalog.select(&code);
                settings.language = Some(code);
            }
            "scaling" => settings.graphics.scaling = pick(&Scaling::ALL, index, settings.graphics.scaling),
            "keep_aspect" => settings.graphics.keep_aspect = on,
            "smooth" => settings.graphics.smooth = on,
            "fog" => settings.graphics.fog = on,
            "fps_limit" => settings.speed.fps_limit = pick(&FpsLimit::ALL, index, settings.speed.fps_limit),
            "frameskip" => {
                settings.speed.frameskip = pick(&Frameskip::all(), index, settings.speed.frameskip);
            }
            "ff_ratio" => {
                let ratio = &mut settings.speed.fast_forward.ratio;
                *ratio = pick(&FastForward::choices(), index, *ratio);
            }
            "ff_mode" => {
                let mode = &mut settings.speed.fast_forward.mode;
                *mode = pick(&ShortcutMode::ALL, index, *mode);
            }
            "ff_mute" => settings.speed.fast_forward.mute = on,
            "rewind_on" => settings.speed.rewind.enabled = on,
            "rewind_interval" => {
                let interval = &mut settings.speed.rewind.interval_ms;
                *interval = pick(&RewindSettings::INTERVALS_MS, index, *interval);
            }
            "rewind_memory" => {
                let memory = &mut settings.speed.rewind.memory_mb;
                *memory = pick(&RewindSettings::MEMORY_SIZES_MB, index, *memory);
            }
            "audio" => settings.audio.enabled = on,
            "volume" => settings.audio.volume = number.min(100),
            "soundfont" => settings.audio.soundfont = bank,
            "midi_effects" => settings.audio.midi_effects = on,
            "touch_mode" => {
                let mode = &mut settings.on_screen_controls.mode;
                *mode = pick(&OnScreenControlsMode::ALL, index, *mode);
            }
            "touch_scale" => settings.on_screen_controls.scale = number.clamp(50, 200),
            "touch_opacity" => settings.on_screen_controls.opacity = number.clamp(15, 100),
            "touch_show_ff" => settings.on_screen_controls.show_fast_forward = on,
            "touch_show_rewind" => settings.on_screen_controls.show_rewind = on,
            "touch_show_turbo" => settings.on_screen_controls.show_turbo = on,
            "turbo_mode" => {
                let player = settings.controls.player_mut(0);
                player.turbo = pick(&TurboMode::ALL, index, player.turbo);
            }
            "turbo_button" => {
                if let Some(button) = turbo::PULSABLE.get(index) {
                    settings.controls.player_mut(0).turbo_button = button.to_string();
                }
            }
            "turbo_rate" => {
                settings.speed.turbo_per_second = number.clamp(turbo::MIN_PRESSES, turbo::MAX_PRESSES);
            }
            "map_reset" => {
                let player = settings.controls.player_mut(0);
                for zeebo in CONFIGURABLE {
                    if let Some(sources) = player.buttons.get_mut(zeebo) {
                        sources.retain(Source::is_key);
                    }
                }
            }
            "clear_shortcuts" => {
                for which in PhysicalShortcut::ALL {
                    which.sources_mut(settings).retain(Source::is_key);
                }
            }
            "debug_overlay" => settings.debug.overlay = on,
            "debug_speed" => settings.debug.speed = on,
            "debug_clock" => settings.debug.clock = on,
            "debug_memory" => settings.debug.memory = on,
            other => {
                // A row that waits for the controller. Nothing is stored until the button
                // arrives; the screen still has to redraw, to say it is waiting.
                let capture = PhysicalShortcut::ALL
                    .into_iter()
                    .find(|which| which.id() == other)
                    .map(Capture::Shortcut)
                    .or_else(|| {
                        let zeebo = other.strip_prefix("map.")?;
                        CONFIGURABLE.into_iter().find(|known| *known == zeebo).map(Capture::Button)
                    });
                self.capturing = capture.filter(|capture| was_capturing != Some(*capture));
                self.revision = self.revision.wrapping_add(1);
                return;
            }
        }
        self.changed();
        // What is changed now applies to the game already open, not only to the next one.
        match id {
            "fog" => self.apply_graphics(),
            "audio" | "volume" => self.apply_audio(),
            _ => {}
        }
    }

    /// A physical controller button went down: with a capture open, it becomes the binding.
    ///
    /// **Only the buttons are replaced**, the saved keys (desktop's F10, the keyboard map) stay
    /// in the file: the same `settings.json` can move from the phone to the computer.
    pub(crate) fn captured_button(&mut self, name: &'static str) {
        let Some(capture) = self.capturing.take() else {
            return;
        };
        let sources = match capture {
            Capture::Shortcut(which) => which.sources_mut(&mut self.settings),
            Capture::Button(zeebo) => self
                .settings
                .controls
                .player_mut(0)
                .buttons
                .entry(zeebo.to_string())
                .or_default(),
        };
        sources.retain(Source::is_key);
        sources.push(Source::button(name));
        self.changed();
    }

    /// Whether the next press is spoken for, and by whom: a shortcut does not take the D-pad.
    pub(crate) fn capture_accepts(&self, bit: usize) -> bool {
        match self.capturing {
            Some(Capture::Shortcut(_)) => bit < controller::SHORTCUT_CAPABLE,
            Some(Capture::Button(_)) => true,
            None => false,
        }
    }
}

/// The choice at `index`, or what was there when the index is from another list.
fn pick<T: Copy>(all: &[T], index: usize, current: T) -> T {
    all.get(index).copied().unwrap_or(current)
}

/// Physical buttons, as the player reads them.
fn join(names: Vec<&str>) -> String {
    names.into_iter().map(controller::label).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_app;

    fn rows(app: &App, tab: &str) -> Vec<Value> {
        let page: Value = serde_json::from_str(&app.settings_json(tab)).unwrap();
        assert_eq!(page["tab"], tab);
        page["rows"].as_array().unwrap().clone()
    }

    fn row(app: &App, tab: &str, id: &str) -> Value {
        rows(app, tab)
            .into_iter()
            .find(|row| row["id"] == id)
            .unwrap_or_else(|| panic!("no row {id} in {tab}"))
    }

    fn bit(name: &str) -> u32 {
        1 << controller::NAMES.iter().position(|known| *known == name).unwrap()
    }

    /// A press and a release of one physical button, the way Swift reports them.
    fn press(app: &mut App, name: &str) {
        let pointer: *mut App = app;
        crate::zeebx_ios_controller(pointer, 1, bit(name), 0.0, 0.0, 0.0, 0.0);
        crate::zeebx_ios_controller(pointer, 1, 0, 0.0, 0.0, 0.0, 0.0);
    }

    /// A text that is still a catalog key: `ios.gamepad.hint` on screen instead of the hint.
    fn looks_like_key(text: &str) -> bool {
        text.contains('.')
            && !text.contains(' ')
            && text.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._".contains(c))
    }

    #[test]
    fn every_row_reads_as_text_and_every_id_is_its_own() {
        let (root, mut app) = test_app("linhas");
        // The rows that only show under a condition, shown.
        app.settings.graphics.scaling = Scaling::Stretch;
        for code in ["en", "pt-BR", "es", "es-MX"] {
            app.catalog.select(code);
            for tab in TABS {
                let rows = rows(&app, tab);
                assert!(!rows.is_empty(), "{tab}");
                let mut ids = std::collections::HashSet::new();
                for row in &rows {
                    for field in ["label", "hint"] {
                        let text = row[field].as_str().unwrap_or_default();
                        assert!(!looks_like_key(text), "{code}/{tab}: {text}");
                    }
                    if let Some(id) = row["id"].as_str() {
                        assert!(ids.insert(id.to_string()), "{tab}: {id} twice");
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// What the screen shows after a tap is what the tap asked for: every switch, every choice
    /// and every slider goes through `settings_set` and comes back through `settings_json`.
    #[test]
    fn every_control_changes_what_it_shows() {
        let (root, mut app) = test_app("ida-e-volta");
        app.settings.graphics.scaling = Scaling::Stretch;
        for tab in TABS {
            for before in rows(&app, tab) {
                let Some(id) = before["id"].as_str() else {
                    continue;
                };
                // The language list changes every label and has its own test; the bank list is
                // empty in an empty sandbox; and leaving "Fill" takes a row off this same page.
                if ["language", "soundfont", "scaling"].contains(&id) {
                    continue;
                }
                match before["kind"].as_str().unwrap() {
                    "toggle" => {
                        let on = !before["on"].as_bool().unwrap();
                        app.settings_set(id, i32::from(on));
                        assert_eq!(row(&app, tab, id)["on"], on, "{id}");
                        app.settings_set(id, i32::from(!on));
                    }
                    "choice" => {
                        let was = before["selected"].as_u64().unwrap();
                        let last = before["options"].as_array().unwrap().len() as u64 - 1;
                        let other = if was == last { 0 } else { last };
                        app.settings_set(id, other as i32);
                        assert_eq!(row(&app, tab, id)["selected"], other, "{id}");
                        app.settings_set(id, was as i32);
                    }
                    "slider" => {
                        let was = before["value"].as_i64().unwrap();
                        let min = before["min"].as_i64().unwrap();
                        let max = before["max"].as_i64().unwrap();
                        let other = if was == min { max } else { min };
                        app.settings_set(id, other as i32);
                        assert_eq!(row(&app, tab, id)["value"], other, "{id}");
                        // Past the end of the track, it stops at the end.
                        app.settings_set(id, 1000);
                        assert_eq!(row(&app, tab, id)["value"], max, "{id}");
                        app.settings_set(id, was as i32);
                    }
                    _ => {}
                }
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_scaling_choice_brings_and_takes_the_aspect_switch() {
        let (root, mut app) = test_app("escala-da-tela");
        let has_aspect = |app: &App| rows(app, "graphics").iter().any(|row| row["id"] == "keep_aspect");
        let stretch = position(&Scaling::ALL, &Scaling::Stretch) as i32;
        app.settings_set("scaling", stretch);
        assert_eq!(app.settings.graphics.scaling, Scaling::Stretch);
        assert!(has_aspect(&app));
        app.settings_set("scaling", position(&Scaling::ALL, &Scaling::Fit) as i32);
        assert!(!has_aspect(&app));
        // An index from another list leaves what was there.
        app.settings_set("scaling", 99);
        assert_eq!(app.settings.graphics.scaling, Scaling::Fit);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_change_is_on_disk_in_the_format_the_other_frontends_read() {
        let (root, mut app) = test_app("disco");
        let before = app.revision;
        app.settings_set("touch_opacity", 80);
        app.settings_set("rewind_on", 1);
        assert_ne!(app.revision, before);
        let saved = Settings::load_from(&app.file);
        assert_eq!(saved.on_screen_controls.opacity, 80);
        assert!(saved.speed.rewind.enabled);
        // The stored names are the frozen Portuguese ones.
        let text = std::fs::read_to_string(&app.file).unwrap();
        assert!(text.contains("\"controles_na_tela\""));
        assert!(text.contains("\"opacidade\": 80"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_language_changes_the_labels_at_once() {
        let (root, mut app) = test_app("trocar-idioma");
        let languages = row(&app, "general", "language");
        let options = languages["options"].as_array().unwrap();
        let portuguese = options.iter().position(|name| name == "Português (Brasil)").unwrap();
        app.settings_set("language", portuguese as i32);
        assert_eq!(app.settings.language.as_deref(), Some("pt-BR"));
        assert_eq!(row(&app, "general", "language")["selected"], portuguese);
        assert_eq!(row(&app, "general", "rescan")["label"], "Procurar de novo");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_shortcut_takes_the_next_button_and_keeps_the_desktop_key() {
        let (root, mut app) = test_app("atalho");
        app.settings_set("capture.rewind", 0);
        let waiting = app.catalog.get("touch.shortcut.waiting").to_string();
        assert_eq!(row(&app, "controls", "capture.rewind")["detail"], waiting);

        // The D-pad is not a shortcut: the capture keeps waiting.
        press(&mut app, "DPadUp");
        assert_eq!(row(&app, "controls", "capture.rewind")["detail"], waiting);

        press(&mut app, "LeftThumb");
        assert_eq!(row(&app, "controls", "capture.rewind")["detail"], "L3");
        let rewind = &app.settings.shortcuts.rewind;
        assert!(rewind.contains(&Source::button("LeftThumb")));
        assert!(rewind.iter().any(Source::is_key), "the F8 of the desktop stays in the file");

        // With nothing waiting, a press binds nothing.
        press(&mut app, "South");
        assert_eq!(row(&app, "controls", "capture.rewind")["detail"], "L3");

        app.settings_set("clear_shortcuts", 0);
        assert_eq!(row(&app, "controls", "capture.rewind")["detail"], "—");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn tapping_the_waiting_row_again_gives_up() {
        let (root, mut app) = test_app("desistir");
        app.settings_set("capture.turbo", 0);
        assert!(app.capturing.is_some());
        app.settings_set("capture.turbo", 0);
        assert_eq!(app.capturing, None);
        press(&mut app, "South");
        assert_eq!(row(&app, "controls", "capture.turbo")["detail"], "—");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_remapped_button_shows_where_it_went_and_what_it_left() {
        let (root, mut app) = test_app("remapear");
        assert_eq!(row(&app, "controls", "map.b1")["detail"], "A");
        assert_eq!(row(&app, "controls", "map.back")["detail"], "Menu");
        assert_eq!(row(&app, "controls", "map_reset")["enabled"], false);

        app.settings_set("map.b1", 0);
        press(&mut app, "East");
        assert_eq!(row(&app, "controls", "map.b1")["detail"], "B");
        assert_eq!(row(&app, "controls", "map.b2")["detail"], "—", "B belonged to button 2");
        assert_eq!(row(&app, "controls", "map_reset")["enabled"], true);

        // The keyboard map of the same file is untouched: it is the one the desktop reads.
        let player = app.settings.controls.player(0).unwrap();
        assert!(player.sources("b1").contains(&Source::key("Z")));

        app.settings_set("map_reset", 0);
        assert_eq!(row(&app, "controls", "map.b1")["detail"], "A");
        assert_eq!(row(&app, "controls", "map.b2")["detail"], "B");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_controller_section_says_when_there_is_none() {
        let (root, mut app) = test_app("sem-controle");
        let none = app.catalog.get("ios.gamepad.none").to_string();
        let notes = |app: &App| -> Vec<String> {
            rows(app, "controls")
                .iter()
                .filter(|row| row["kind"] == "note")
                .map(|row| row["label"].as_str().unwrap().to_string())
                .collect()
        };
        assert!(notes(&app).contains(&none));
        press(&mut app, "South");
        assert!(!notes(&app).contains(&none));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_bank_is_found_again_after_the_app_folder_moves() {
        let (root, mut app) = test_app("banco");
        let bank = app.documents.join("gm.sf2");
        std::fs::write(&bank, b"RIFF").unwrap();
        let chosen = row(&app, "audio", "soundfont");
        assert_eq!(chosen["options"].as_array().unwrap().len(), 2);
        assert_eq!(chosen["selected"], 0);

        // The path saved before an update: same file name, a container that is gone.
        app.settings.audio.soundfont = Some(PathBuf::from("/var/gone/Documents/gm.sf2"));
        app.relocate_soundfont();
        assert_eq!(app.settings.audio.soundfont.as_deref(), Some(bank.as_path()));
        assert_eq!(row(&app, "audio", "soundfont")["selected"], 1);

        app.settings_set("soundfont", 0);
        assert_eq!(app.settings.audio.soundfont, None);
        app.settings_set("soundfont", 1);
        assert_eq!(app.settings.audio.soundfont.as_deref(), Some(bank.as_path()));
        let _ = std::fs::remove_dir_all(root);
    }
}
