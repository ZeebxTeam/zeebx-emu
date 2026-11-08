//! Settings, for a thumb.
//!
//! What gets changed here is the **same** [`zeebx::ui::settings::Settings`] as desktop, saved in
//! the same format, and the labels come from the same language catalog. What is not the same is
//! the form: on desktop the tabs sit at the top and each option is a little box with the caption
//! beside it; here the sections sit in a rail on the left — the device is locked to landscape,
//! and in landscape width is what there is to spare — and each option is a full-width strip.
//!
//! What was left out was left out because it does not exist here: window options make no sense
//! on a single screen, and controller mapping, Discord and updates are not wired up in this
//! frontend yet. The "Controls" section here is the one for the controls drawn on screen.

use zeebx::input::bindings::Source;
use zeebx::ui::settings::{self, OnScreenControlsMode, AspectRatio, Scaling, Settings};
use zeebx::speed::turbo::{self, TurboMode};
use zeebx::speed::rewind::RewindSettings;
use zeebx::speed::{FastForward, Frameskip, FpsLimit, ShortcutMode};

use crate::theme::TARGET_SIZE;
use crate::{Emulator, View, system, widgets};

/// A shortcut the physical controller can trigger: the two session ones and player 1's turbo
/// key. The button name is desktop's — see `crate::host_input::PHYSICAL_BUTTONS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalShortcut {
    FastForward,
    Back,
    Turbo,
}

impl PhysicalShortcut {
    const ALL: [Self; 3] = [Self::FastForward, Self::Back, Self::Turbo];

    fn key_id(self) -> &'static str {
        match self {
            Self::FastForward => "controls.shortcut.fast_forward",
            Self::Back => "controls.shortcut.rewind",
            Self::Turbo => "button.turbo",
        }
    }

    fn origins(self, settings: &Settings) -> Vec<Source> {
        match self {
            Self::FastForward => settings.shortcuts.fast_forward.clone(),
            Self::Back => settings.shortcuts.back.clone(),
            Self::Turbo => settings
                .controls
                .player(0)
                .map(|player| player.sources(turbo::TURBO_BUTTON).to_vec())
                .unwrap_or_default(),
        }
    }

    fn origins_mut(self, settings: &mut Settings) -> &mut Vec<Source> {
        match self {
            Self::FastForward => &mut settings.shortcuts.fast_forward,
            Self::Back => &mut settings.shortcuts.back,
            Self::Turbo => settings
                .controls
                .player_mut(0)
                .buttons
                .entry(turbo::TURBO_BUTTON.to_string())
                .or_default(),
        }
    }
}

impl Emulator {
    /// A physical controller button went down: with a capture open, it becomes the shortcut.
    ///
    /// **Only the buttons are replaced**, the saved keys (desktop's F10) stay in the file: the
    /// same `settings.json` can move from the phone to the computer. The button that confirmed
    /// the capture also reaches egui as "Enter" in the same round, and would reopen the capture
    /// that just closed: `just_captured` makes the tab ignore this round's click.
    pub(crate) fn captured_button(&mut self, name: &'static str) {
        let Some(which) = self.capturing.take() else {
            return;
        };
        let origins = which.origins_mut(&mut self.settings);
        origins.retain(|source| source.is_key());
        origins.push(Source::button(name));
        self.just_captured = true;
        self.save();
    }
}

/// The sections, in the same order as desktop's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    General,
    Graphics,
    Speed,
    Audio,
    Controls,
    Debug,
    About,
}

impl Tab {
    const ALL: [Self; 7] = [
        Self::General,
        Self::Graphics,
        Self::Speed,
        Self::Audio,
        Self::Controls,
        Self::Debug,
        Self::About,
    ];

    /// The catalog key for the section's name.
    fn key_id(self) -> &'static str {
        match self {
            Self::General => "settings.tab.general",
            Self::Graphics => "settings.tab.graphics",
            Self::Speed => "settings.tab.speed",
            Self::Audio => "settings.tab.audio",
            Self::Controls => "settings.tab.controls",
            Self::Debug => "settings.tab.debug",
            Self::About => "settings.tab.about",
        }
    }

    /// A sign for the rail. An icon recognized from afar is worth more than a name read up close.
    fn sign(self) -> &'static str {
        match self {
            Self::General => "⛭",
            Self::Graphics => "🖵",
            Self::Speed => "»",
            Self::Audio => "🔊",
            Self::Controls => "🎮",
            Self::Debug => "⏱",
            Self::About => "ℹ",
        }
    }
}

impl Emulator {
    pub(crate) fn settings_page(&mut self, ctx: &egui::Context) {
        // The rail on the left: "back" at the top and the sections below, each one a whole touch
        // target. In landscape it costs 150 points of width and gives the content the screen's
        // full height — much better than a row of tabs that scrolls sideways.
        egui::SidePanel::left("trilha")
            .exact_width(150.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(10.0);
                let back = ui.add_sized(
                    [ui.available_width(), TARGET_SIZE],
                    egui::Button::new(format!("◀  {}", self.tr("common.back"))),
                );
                if back.clicked() {
                    self.location = View::Library;
                }
                // **No focus, no arrow navigation.** egui moves focus in the arrow's direction,
                // but only from a focus that already exists -- from nothing, it is `Tab` that
                // grants it, and no controller produces `Tab`. Entering the screen without focus,
                // the D-pad moved nothing at all here. We give the first one, and from there egui
                // manages: arrows move, Enter presses.
                if ctx.memory(|m| m.focused()).is_none() {
                    back.request_focus();
                }
                ui.add_space(12.0);
                for tab in Tab::ALL {
                    let name = format!("{}  {}", tab.sign(), self.catalog.get(tab.key_id()));
                    let button = egui::Button::new(name)
                        .min_size(egui::vec2(ui.available_width(), TARGET_SIZE))
                        .selected(self.tab == tab);
                    if ui.add(button).clicked() {
                        self.tab = tab;
                        // Switching sections closes the open hint: it belonged to another screen.
                        self.hint = None;
                    }
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            let title = self.catalog.get(self.tab.key_id()).to_string();
            ui.add_space(10.0);
            ui.heading(title);
            ui.add_space(6.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                let changed = match self.tab {
                    Tab::General => self.tab_general(ui),
                    Tab::Graphics => self.tab_graphics(ui),
                    Tab::Speed => self.tab_speed(ui),
                    Tab::Audio => self.tab_audio(ui),
                    Tab::Controls => self.tab_controls(ui),
                    Tab::Debug => self.tab_debug(ui),
                    Tab::About => self.tab_about(ui),
                };
                if changed {
                    self.save();
                }
                ui.add_space(24.0);
            });
        });
    }

    /// Language and ROM folder.
    fn tab_general(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;

        widgets::section(ui, self.catalog.get("settings.roms_folder"));
        let dir = self.roms();
        let location = match dir.as_os_str().is_empty() {
            true => self.catalog.get("library.no_folder").to_string(),
            false => dir.display().to_string(),
        };
        if widgets::navigate(ui, self.catalog.get("settings.browse"), &location) {
            self.location = View::FolderSelector(match dir.is_dir() {
                true => dir,
                false => std::path::PathBuf::from("/sdcard"),
            });
        }
        let count = self
            .catalog
            .format("library.count", &[("count", &self.games.len().to_string())]);
        let can_scan = self.scan.is_none();
        if ui
            .add_enabled_ui(can_scan, |ui| {
                widgets::navigate(ui, self.catalog.get("library.rescan"), &count)
            })
            .inner
        {
            self.reload();
        }

        widgets::section(ui, self.catalog.get("settings.language"));
        // One language per strip, with the active one marked. There are two or three: a choice
        // list is more direct than a box that opens over the screen.
        let languages: Vec<(String, String)> = self
            .catalog
            .languages()
            .iter()
            .map(|language| (language.code.clone(), language.name.clone()))
            .collect();
        let current = self.catalog.current().to_string();
        let mut chosen = None;
        for (code, name) in &languages {
            let button = egui::Button::new(name)
                .min_size(egui::vec2(ui.available_width(), TARGET_SIZE))
                .selected(*code == current);
            if ui.add(button).clicked() && *code != current {
                chosen = Some(code.clone());
            }
        }
        if let Some(code) = chosen {
            self.catalog.select(&code);
            self.settings.language = Some(code);
            changed = true;
        }

        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(self.catalog.format(
                "settings.saved_at",
                &[("path", &self.file.display().to_string())],
            ))
            .small()
            .color(ui.visuals().weak_text_color()),
        );
        changed
    }

    /// The game's pace against the wall clock. See `docs/implementation/24-speed.md`.
    fn tab_speed(&mut self, ui: &mut egui::Ui) -> bool {
        let catalog = &self.catalog;
        let speed = &mut self.settings.speed;
        let hint = &mut self.hint;
        let mut changed = false;

        let limits: Vec<(FpsLimit, String)> = FpsLimit::ALL
            .iter()
            .map(|limit| (*limit, catalog.get(limit.key_id()).to_string()))
            .collect();
        changed |= widgets::segmented(
            ui,
            "fps_limit",
            catalog.get("speed.fps_limit"),
            Some(catalog.get("speed.fps_limit.hint")),
            hint,
            &mut speed.fps_limit,
            &limits,
        );
        let skips: Vec<(Frameskip, String)> = Frameskip::all()
            .into_iter()
            .map(|skip| (skip, settings::frameskip_label(skip, catalog)))
            .collect();
        changed |= widgets::segmented(
            ui,
            "frameskip",
            catalog.get("speed.frameskip"),
            Some(catalog.get("speed.frameskip.hint")),
            hint,
            &mut speed.frameskip,
            &skips,
        );

        widgets::section(ui, catalog.get("speed.ff"));
        let ratios: Vec<(u8, String)> = FastForward::choices()
            .into_iter()
            .map(|n| match n {
                0 => (n, catalog.get("speed.ff.unlimited").to_string()),
                n => (n, format!("{n}x")),
            })
            .collect();
        changed |= widgets::segmented(
            ui,
            "ff_ratio",
            catalog.get("speed.ff.ratio"),
            Some(catalog.get("speed.ff.hint")),
            hint,
            &mut speed.fast_forward.ratio,
            &ratios,
        );
        let modes: Vec<(ShortcutMode, String)> = ShortcutMode::ALL
            .iter()
            .map(|mode| (*mode, catalog.get(mode.key_id()).to_string()))
            .collect();
        changed |= widgets::segmented(
            ui,
            "ff_mode",
            catalog.get("speed.ff.mode"),
            None,
            hint,
            &mut speed.fast_forward.mode,
            &modes,
        );
        changed |= widgets::toggle(
            ui,
            "ff_mute",
            catalog.get("speed.ff.mute"),
            Some(catalog.get("speed.ff.mute.hint")),
            hint,
            &mut speed.fast_forward.mute,
        );

        widgets::section(ui, catalog.get("speed.rewind"));
        changed |= widgets::toggle(
            ui,
            "rewind_on",
            catalog.get("speed.rewind.on"),
            Some(catalog.get("speed.rewind.on.hint")),
            hint,
            &mut speed.rewind.enabled,
        );
        let enabled = speed.rewind.enabled;
        ui.add_enabled_ui(enabled, |ui| {
            let intervals: Vec<(u32, String)> = RewindSettings::INTERVALS_MS
                .iter()
                .map(|&ms| {
                    let text = match ms % 1000 {
                        0 => catalog.format("speed.rewind.every_s", &[("n", &(ms / 1000).to_string())]),
                        _ => catalog.format("speed.rewind.every_ms", &[("n", &ms.to_string())]),
                    };
                    (ms, text)
                })
                .collect();
            changed |= widgets::segmented(
                ui,
                "rewind_interval",
                catalog.get("speed.rewind.interval"),
                None,
                hint,
                &mut speed.rewind.interval_ms,
                &intervals,
            );
            let memory_sizes: Vec<(u32, String)> = RewindSettings::MEMORY_SIZES_MB
                .iter()
                .map(|&mb| (mb, format!("{mb} MB")))
                .collect();
            changed |= widgets::segmented(
                ui,
                "rewind_memory",
                catalog.get("speed.rewind.memory"),
                Some(catalog.get("speed.rewind.memory.hint")),
                hint,
                &mut speed.rewind.memory_mb,
                &memory_sizes,
            );
        });
        changed
    }

    /// What changes how the game is drawn.
    fn tab_graphics(&mut self, ui: &mut egui::Ui) -> bool {
        // The three fields are split out on purpose: the catalog is read, the graphics are
        // written and the open hint is swapped in the same block, and only by borrowing each one
        // on its own does that get past the borrow checker.
        let catalog = &self.catalog;
        let graphics = &mut self.settings.graphics;
        let hint = &mut self.hint;
        let mut changed = false;

        // No section title before a segmented control: its own strip already carries the name
        // and the `?`, and both together were the same word twice.
        let modes: Vec<(Scaling, String)> = Scaling::ALL
            .iter()
            .map(|mode| (*mode, catalog.get(mode.key()).to_string()))
            .collect();
        changed |= widgets::segmented(
            ui,
            "scaling",
            catalog.get("graphics.scaling"),
            Some(catalog.get("graphics.scaling.integer.hint")),
            hint,
            &mut graphics.scaling,
            &modes,
        );
        // 4:3 only has an effect on "Fill": in the other two modes the aspect ratio is already
        // kept, and showing the option there gave a button that does nothing. Worse, it was the
        // reason "Fill" looked broken — when on, it undoes the fill.
        if graphics.scaling == Scaling::Stretch {
            changed |= widgets::toggle(
                ui,
                "keep_aspect",
                catalog.get("graphics.keep_aspect"),
                None,
                hint,
                &mut graphics.keep_aspect,
            );
        }
        changed |= widgets::toggle(
            ui,
            "smooth",
            catalog.get("graphics.smooth"),
            None,
            hint,
            &mut graphics.smooth,
        );

        widgets::section(ui, "3D");
        // Fog works in both rasterizers; the rest of this section, only on the GPU.
        changed |= widgets::toggle(
            ui,
            "fog",
            catalog.get("graphics.fog"),
            Some(catalog.get("graphics.fog.hint")),
            hint,
            &mut graphics.fog,
        );
        changed |= widgets::toggle(
            ui,
            "gpu_rasterizer",
            catalog.get("graphics.gpu_rasterizer"),
            Some(catalog.get("graphics.gpu_rasterizer.hint")),
            hint,
            &mut graphics.gpu_rasterizer,
        );
        // **Everything from here down is a no-op in software.** The `Rasterizer`'s
        // `set_aspect_ratio`, `set_scale`, `set_antialias` and anisotropic filtering are
        // functions with no body when 3D runs on the CPU: only the GPU can do that. Graying them
        // out while it is off tells the truth — before, they moved and nothing happened.
        let with_gpu = graphics.gpu_rasterizer;
        // Graying out the options is not enough: a grayed-out control reads as broken, not as
        // "depends on that one there". The line says what it depends on.
        if !with_gpu {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new(format!(
                        "↑  {}",
                        catalog.format(
                            "graphics.gpu_rasterizer.required",
                            &[("name", catalog.get("graphics.gpu_rasterizer"))],
                        )
                    ))
                    .small()
                    .color(ui.visuals().warn_fg_color),
                );
            });
            ui.add_space(4.0);
        }
        ui.add_enabled_ui(with_gpu, |ui| {
        changed |= widgets::slider(
            ui,
            "resolucao",
            catalog.get("graphics.internal_resolution"),
            Some(catalog.get("graphics.internal_resolution.hint")),
            hint,
            &mut graphics.internal_resolution,
            1..=4,
            |value| format!("{value}×"),
        );
        changed |= widgets::slider(
            ui,
            "antialias",
            catalog.get("graphics.antialias"),
            Some(catalog.get("graphics.antialias.hint")),
            hint,
            &mut graphics.antialias,
            1..=4,
            |value| format!("{value}×"),
        );
        changed |= widgets::slider(
            ui,
            "anisotropico",
            catalog.get("graphics.anisotropic"),
            Some(catalog.get("graphics.anisotropic.hint")),
            hint,
            &mut graphics.anisotropic,
            1..=16,
            |value| format!("{value}×"),
        );

        let ratios: Vec<(AspectRatio, String)> = AspectRatio::ALL
            .iter()
            .map(|p| (*p, catalog.get(p.key_id()).to_string()))
            .collect();
        changed |= widgets::segmented(
            ui,
            "proporcao",
            catalog.get("graphics.aspect"),
            Some(catalog.get("graphics.aspect.hint")),
            hint,
            &mut graphics.ratio,
            &ratios,
        );
        });

        // What is changed now applies to the game already open, not only to the next one.
        if changed {
            let graphics = self.settings.graphics.clone();
            if let Some(session) = &mut self.session {
                session.set_internal_resolution(graphics.internal_resolution as usize);
                session.set_enhancements(graphics.antialias as usize, graphics.anisotropic as usize);
                session.set_fog_allowed(graphics.fog);
                session.set_aspect_ratio(graphics.ratio.aspect(16.0 / 9.0));
            }
        }
        changed
    }

    fn tab_audio(&mut self, ui: &mut egui::Ui) -> bool {
        let catalog = &self.catalog;
        let audio = &mut self.settings.audio;
        let hint = &mut self.hint;
        let mut changed = widgets::toggle(
            ui,
            "audio",
            catalog.get("audio.enabled"),
            Some(catalog.get("audio.hint")),
            hint,
            &mut audio.enabled,
        );
        changed |= widgets::slider(
            ui,
            "volume",
            catalog.get("audio.volume"),
            None,
            hint,
            &mut audio.volume,
            0..=100,
            |value| format!("{value}%"),
        );

        // The bank is opened when the game opens: the hint says the change applies to the next
        // one.
        widgets::section(ui, self.catalog.get("audio.soundfont"));
        ui.weak(self.catalog.get("audio.soundfont.hint"));
        let soundfont = self.settings.audio.soundfont.clone();
        let shown = match &soundfont {
            Some(soundfont_path) => soundfont_path.display().to_string(),
            None => self.catalog.get("audio.soundfont.auto").to_string(),
        };
        if widgets::navigate(ui, self.catalog.get("settings.browse"), &shown) {
            let start = soundfont
                .as_deref()
                .and_then(std::path::Path::parent)
                .filter(|dir| dir.is_dir())
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| std::path::PathBuf::from("/sdcard"));
            self.location = View::SoundfontSelector(start);
        }
        if soundfont.is_some()
            && ui
                .add_sized(
                    [ui.available_width(), 52.0],
                    egui::Button::new(self.catalog.get("audio.soundfont.clear")),
                )
                .clicked()
        {
            self.settings.audio.soundfont = None;
            changed = true;
        }
        changed |= widgets::toggle(
            ui,
            "efeitos_midi",
            self.catalog.get("audio.midi_effects"),
            Some(self.catalog.get("audio.midi_effects.hint")),
            &mut self.hint,
            &mut self.settings.audio.midi_effects,
        );

        if changed {
            let audio = self.settings.audio.clone();
            if let Some(session) = &mut self.session {
                if let Some(error) = session.set_audio(audio.enabled, audio.volume) {
                    log::error!("no sound: {error}");
                }
            }
        }
        changed
    }

    /// The controls drawn on screen: when they appear, their size, transparency and where they
    /// sit.
    fn tab_controls(&mut self, ui: &mut egui::Ui) -> bool {
        let catalog = &self.catalog;
        let touch = &mut self.settings.on_screen_controls;
        let hint = &mut self.hint;
        let modes: Vec<(OnScreenControlsMode, String)> = OnScreenControlsMode::ALL
            .iter()
            .map(|mode| (*mode, catalog.get(mode.key_id()).to_string()))
            .collect();
        let mut changed = widgets::segmented(
            ui,
            "touch_mode",
            catalog.get("touch.mode"),
            Some(catalog.get("touch.mode.hint")),
            hint,
            &mut touch.mode,
            &modes,
        );
        // Size and position also apply under "never": whoever turns them off to play with a
        // controller can leave everything arranged for the next time without one.
        changed |= widgets::slider(
            ui,
            "touch_scale",
            catalog.get("touch.scale"),
            None,
            hint,
            &mut touch.scale,
            50..=200,
            |v| format!("{v}%"),
        );
        changed |= widgets::slider(
            ui,
            "touch_opacity",
            catalog.get("touch.opacity"),
            None,
            hint,
            &mut touch.opacity,
            15..=100,
            |v| format!("{v}%"),
        );
        let layout = match touch.positions.is_empty() && touch.sizes.is_empty() {
            true => catalog.get("touch.layout.default"),
            false => catalog.get("touch.layout.custom"),
        };
        if widgets::navigate(ui, catalog.get("touch.layout"), layout) {
            self.location = View::TouchEditor;
        }
        // The shortcut pieces, one switch each: turbo and rewind, on top of that, only show up
        // with the feature in use. See `OnScreenControls::show`.
        for (key_id, label_key, value) in [
            ("touch_show_ff", "touch.show.ff", &mut touch.show_fast_forward),
            ("touch_show_rewind", "touch.show.rewind", &mut touch.show_rewind),
            ("touch_show_turbo", "touch.show.turbo", &mut touch.show_turbo),
        ] {
            changed |= widgets::toggle(ui, key_id, catalog.get(label_key), None, hint, value);
        }

        // Player 1's turbo, the only one Android has. See `zeebx::speed::turbo`.
        widgets::section(ui, catalog.get("turbo"));
        let player = self.settings.controls.player_mut(0);
        let modes: Vec<(TurboMode, String)> = TurboMode::ALL
            .iter()
            .map(|mode| (*mode, catalog.get(mode.key_id()).to_string()))
            .collect();
        changed |= widgets::segmented(
            ui,
            "turbo_mode",
            catalog.get("turbo.mode"),
            Some(catalog.get("turbo.hint.touch")),
            hint,
            &mut player.turbo,
            &modes,
        );
        let buttons: Vec<(&str, String)> = turbo::PULSABLE
            .iter()
            .map(|button| (*button, catalog.get(&format!("button.{button}")).to_string()))
            .collect();
        let mut button = turbo::PULSABLE
            .iter()
            .copied()
            .find(|b| *b == player.turbo_button)
            .unwrap_or(turbo::DEFAULT_BUTTON);
        if widgets::segmented(
            ui,
            "turbo_button",
            catalog.get("turbo.button"),
            None,
            hint,
            &mut button,
            &buttons,
        ) {
            player.turbo_button = button.to_string();
            changed = true;
        }
        changed |= widgets::slider(
            ui,
            "turbo_rate",
            catalog.get("turbo.rate"),
            None,
            hint,
            &mut self.settings.speed.turbo_per_second,
            turbo::MIN_PRESSES..=turbo::MAX_PRESSES,
            |n| catalog.format("turbo.rate.value", &[("n", &n.to_string())]),
        );

        // Shortcuts through the physical controller: on a phone there is no F10. Tapping a row
        // waits for the controller's next button; tapping again gives up. See
        // [`Emulator::captured_button`].
        widgets::section(ui, catalog.get("settings.tab.shortcuts"));
        let just_captured = std::mem::take(&mut self.just_captured);
        for which in PhysicalShortcut::ALL {
            let value = match self.capturing == Some(which) {
                true => catalog.get("touch.shortcut.waiting").to_string(),
                false => {
                    let origins = which.origins(&self.settings);
                    let buttons: Vec<String> = origins
                        .iter()
                        .filter(|source| !source.is_key())
                        .map(Source::label)
                        .collect();
                    match buttons.is_empty() {
                        true => catalog.get("controls.unbound").to_string(),
                        false => buttons.join(", "),
                    }
                }
            };
            if widgets::navigate(ui, catalog.get(which.key_id()), &value) && !just_captured {
                self.capturing = match self.capturing == Some(which) {
                    true => None,
                    false => Some(which),
                };
            }
        }
        if ui
            .add_sized([ui.available_width(), TARGET_SIZE], egui::Button::new(catalog.get("touch.shortcut.clear")))
            .clicked()
        {
            for which in PhysicalShortcut::ALL {
                which.origins_mut(&mut self.settings).retain(|source| source.is_key());
            }
            self.capturing = None;
            changed = true;
        }
        changed
    }

    fn tab_debug(&mut self, ui: &mut egui::Ui) -> bool {
        let catalog = &self.catalog;
        let debug = &mut self.settings.debug;
        let hint = &mut self.hint;
        let mut changed = widgets::toggle(
            ui,
            "overlay",
            catalog.get("debug.overlay"),
            Some(catalog.get("debug.hint")),
            hint,
            &mut debug.overlay,
        );
        // The panel's rows only make sense with the panel turned on.
        ui.add_enabled_ui(debug.overlay, |ui| {
            widgets::section(ui, catalog.get("debug.show"));
            for (key_id, label, value) in [
                ("speed", "debug.speed", &mut debug.speed),
                ("clock", "debug.clock", &mut debug.clock),
                ("memory", "debug.memory", &mut debug.memory),
                ("timeline", "debug.timeline", &mut debug.timeline),
            ] {
                changed |= widgets::toggle(ui, key_id, catalog.get(label), None, hint, value);
            }
        });
        changed
    }

    fn tab_about(&mut self, ui: &mut egui::Ui) -> bool {
        ui.add_space(6.0);
        ui.label(self.tr("about.tagline"));
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                self.catalog
                    .format("about.version", &[("version", env!("CARGO_PKG_VERSION"))]),
            )
            .small()
            .color(ui.visuals().weak_text_color()),
        );
        ui.add_space(16.0);
        // A link here opens nothing on its own: it is the VIEW `Intent` that sends the address to
        // the device's browser.
        for (key_id, address) in [
            ("about.repository", zeebx::ui::REPOSITORY),
            ("about.discord", zeebx::ui::DISCORD),
        ] {
            if widgets::navigate(ui, self.tr(key_id), address) {
                system::open_url(&self.app, address);
            }
        }
        false
    }
}
