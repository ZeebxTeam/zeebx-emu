//! The user's preferences, stored on disk.
//!
//! Everything here is optional and has a reasonable default: a missing file, a truncated one or
//! one from a newer version of the emulator must let the program open, not stop it. That is why
//! every field is `#[serde(default)]` and reading never fails — at worst, the default comes back.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::input::bindings::Source;
use crate::speed::rewind::RewindSettings;
use crate::speed::{FastForward, Frameskip, FpsLimit, Pace};

const FILE_NAME: &str = "settings.json";

/// Re-exported to preserve the desktop API during the migration to neutral configuration.
pub use crate::config::config_dir;

/// How the console image fills the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scaling {
    /// Only integer multiples of the original size. Never blurs, but leaves a border.
    Integer,
    /// The largest size that fits in the window, keeping the aspect ratio. It is the default: in a
    /// maximized window integer pixels leave a wide frame for nothing.
    #[default]
    Fit,
    /// Fills the whole window, whatever it costs the aspect ratio.
    Stretch,
}

impl Scaling {
    pub const ALL: [Self; 3] = [Self::Integer, Self::Fit, Self::Stretch];

    /// The key of the text that describes this option.
    pub fn key(self) -> &'static str {
        match self {
            Self::Integer => "graphics.scaling.integer",
            Self::Fit => "graphics.scaling.fit",
            Self::Stretch => "graphics.scaling.stretch",
        }
    }
}

/// How a window opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    /// At the size the emulator picks.
    #[serde(rename = "janela")]
    Window,
    #[default]
    #[serde(rename = "maximizada")]
    Maximized,
    #[serde(rename = "tela_cheia")]
    Fullscreen,
}

impl WindowMode {
    pub const ALL: [Self; 3] = [Self::Window, Self::Maximized, Self::Fullscreen];

    pub fn key_id(self) -> &'static str {
        match self {
            Self::Window => "graphics.window.windowed",
            Self::Maximized => "graphics.window.maximized",
            Self::Fullscreen => "graphics.window.fullscreen",
        }
    }
}

/// What the debug panel shows while running.
///
/// Everything off by default: it is a tool for someone hunting a problem, and information about
/// the frame gets in the way of someone who just wants to play.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugView {
    /// Turns the panel on. Without it, nothing below shows up.
    pub overlay: bool,
    /// Speed relative to the console, and frames per second.
    pub speed: bool,
    /// Guest instructions per second, and the virtual clock.
    pub clock: bool,
    /// The game's heap and live objects.
    pub memory: bool,
    /// The chart with the recent history.
    pub timeline: bool,
    /// The box in the top right corner, over the game: frames per second, speed and where the
    /// game draws. It does not depend on the panel, which is a strip at the bottom and shrinks the
    /// game.
    #[serde(rename = "metricas")]
    pub metrics: bool,
    /// The separate window with the run's log.
    pub log: bool,
    /// Writes each run to its own file, with a timestamp on each line. See
    /// [`crate::ui::running_game::session_path`].
    #[serde(rename = "gravar_sessao")]
    pub record_session: bool,
    /// Up to which level the **core** logs: `warning` (the default), `info`, `debug`, `error`,
    /// `fatal` or `off`. See [`crate::logging`].
    ///
    /// It is text and not an `enum` on purpose: the same vocabulary applies in the headless
    /// `config.ini`, in the Libretro core's `zeebx_log` option and in the `ZEEBX_LOG` variable,
    /// and converting in one place ([`crate::logging::Threshold::from_text`]) avoids four tables
    /// that drift apart.
    #[serde(rename = "nivel_de_log")]
    pub log_level: String,
}

impl Default for DebugView {
    fn default() -> Self {
        Self {
            overlay: false,
            speed: true,
            clock: true,
            memory: true,
            timeline: true,
            metrics: false,
            log: false,
            record_session: false,
            log_level: "aviso".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Graphics {
    /// How the main window opens.
    #[serde(rename = "janela")]
    pub window: WindowMode,
    /// How the game window opens. With the game in the main window, it is how that window stays
    /// while the game runs; once the game closes, it goes back to what it was.
    #[serde(rename = "janela_do_jogo")]
    pub game_window: WindowMode,
    /// The game in a window of its own, as up to 0.4.2, instead of drawn in the main one in place
    /// of the library. PCSX2's "Render to Separate Window". Applies to the next game opened: a
    /// running game does not switch windows.
    #[serde(rename = "jogo_separado")]
    pub separate_game_window: bool,
    /// Ask before closing the game — via Esc, the menu, the toolbar or the window's close button
    /// —, like PCSX2's "Confirm Shutdown". One Esc too many does not throw away what was not
    /// saved. Z-Wheel and a game that exits on its own close without asking.
    #[serde(rename = "confirmar_ao_fechar")]
    pub confirm_on_close: bool,
    /// The size the main window had when it was closed, in points.
    ///
    /// `None` is the factory default (960×720). It only applies in windowed mode: maximized or
    /// fullscreen, the screen size is not the user's choice and is not stored.
    #[serde(rename = "tamanho_da_janela")]
    pub window_size: Option<[f32; 2]>,
    /// The same, for the game window.
    #[serde(rename = "tamanho_da_janela_do_jogo")]
    pub game_window_size: Option<[f32; 2]>,
    pub scaling: Scaling,
    /// Interpolate when scaling up. Off, the console's pixel shows up as a block.
    pub smooth: bool,
    /// Keep the Zeebo screen's 4:3.
    pub keep_aspect: bool,
    /// The `speed_limit` from before the frame limit, **only for reading** an old file.
    ///
    /// Off became [`crate::speed::FpsLimit::Off`]; see
    /// [`Settings::load_from`]. It is never written: the new file says the same in
    /// [`Speed::fps_limit`].
    #[serde(rename = "speed_limit", skip_serializing)]
    pub legacy_limit: Option<bool>,
    /// Put the frame on screen through the window's GL, instead of sending it as an egui texture.
    ///
    /// Through the GPU the frame is uploaded as RGB565, the format it is already in, and the GPU
    /// does the scaling. The old path converts to ARGB and then to RGBA on every repaint. **It
    /// does not change how the game is drawn** — the 3D is still rasterized in software —, and
    /// that is why the performance difference is small; it exists to be the place where a GPU
    /// backend would go.
    ///
    /// A driver that rejects the shaders makes the emulator fall back to the old path on its own.
    pub gpu_present: bool,
    /// Fill the console's 3D on the GPU, instead of on the CPU.
    ///
    /// This **changes who rasterizes**, not just how the finished frame reaches the screen. The
    /// vertex stage stays on the CPU and is the same in both — matrices, texture matrix and
    /// lighting —; what goes to the GPU is the fill, which is where the measurement showed the
    /// time.
    ///
    /// Measured, API time: Z-Wheel from 4340 to 2805 ms (-35%) over thirteen virtual seconds, and
    /// Crash on the track from 3438 to 2793 ms (-19%) over thirty.
    ///
    /// The drawing is **not** identical to software: edge rules and rounding diverge by
    /// construction. Measured on Z-Wheel, 1% of the pixels differ by more than eight levels and
    /// 0.06% by more than thirty-two; seven of the nine surfaces come out byte-for-byte equal,
    /// because they do not go through the rasterizer. Without a reachable GPU the emulator carries
    /// on in software and says why.
    pub gpu_rasterizer: bool,
    /// The internal resolution of the 3D filled on the GPU, in multiples of 640×480 per side.
    ///
    /// Only has an effect with [`Graphics::gpu_rasterizer`]: the game still sees 640×480, and the
    /// large frame only reaches the window when nothing 2D was drawn over it — see
    /// [`crate::machine::Machine::gpu_frame`]. In other cases the image is the usual one.
    #[serde(rename = "resolucao_interna")]
    pub internal_resolution: u8,
    /// Antialiasing of the 3D on the GPU, in samples per pixel (1 is off).
    pub antialias: u8,
    /// Anisotropic filtering of the 3D textures on the GPU (1 is off).
    #[serde(rename = "anisotropico")]
    pub anisotropic: u8,
    /// **Experimental.** The aspect ratio the 3D on the GPU is rendered at.
    #[serde(rename = "proporcao")]
    pub ratio: AspectRatio,
    /// Let the game's fog apply.
    ///
    /// Applies to both rasterizers. On the console the fog usually hides what the draw distance
    /// did not reach, and here the whole scene arrives — whoever prefers to see far turns it off.
    /// It is **on** by default: the game asked for the fog, and in many it is the effect, not the
    /// patch.
    #[serde(rename = "neblina")]
    pub fog: bool,
}

/// The aspect ratio of the 3D rendered on the GPU. Outside native, the perspective scene gains
/// sides instead of being stretched; HUD and 2D stay at 4:3 in the center.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AspectRatio {
    #[default]
    #[serde(rename = "nativa")]
    Native,
    #[serde(rename = "larga16x9")]
    Wide16x9,
    #[serde(rename = "larga16x10")]
    Wide16x10,
    /// The game window's.
    #[serde(rename = "janela")]
    Window,
}

impl AspectRatio {
    pub const ALL: [Self; 4] = [Self::Native, Self::Wide16x9, Self::Wide16x10, Self::Window];

    pub fn key_id(self) -> &'static str {
        match self {
            Self::Native => "graphics.aspect.native",
            Self::Wide16x9 => "graphics.aspect.16x9",
            Self::Wide16x10 => "graphics.aspect.16x10",
            Self::Window => "graphics.aspect.window",
        }
    }

    /// Width over height, with the window's when that applies. `None` is native.
    pub fn aspect(self, window: f32) -> Option<f32> {
        match self {
            Self::Native => None,
            Self::Wide16x9 => Some(16.0 / 9.0),
            Self::Wide16x10 => Some(16.0 / 10.0),
            Self::Window => Some(window),
        }
    }
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            window: WindowMode::Maximized,
            game_window: WindowMode::Maximized,
            separate_game_window: false,
            confirm_on_close: true,
            window_size: None,
            game_window_size: None,
            scaling: Scaling::default(),
            smooth: false,
            keep_aspect: true,
            legacy_limit: None,
            gpu_present: true,
            // Off by default: it is a new rasterizer, and the game-by-game review is up to whoever
            // uses it. On the two titles measured it wins, but that is no license to change the
            // drawing of all the others without someone having looked at them.
            gpu_rasterizer: false,
            internal_resolution: 1,
            antialias: 1,
            anisotropic: 1,
            ratio: AspectRatio::Native,
            fog: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    pub enabled: bool,
    /// From 0 to 100.
    pub volume: u8,
    /// The `.sf2` bank MIDI plays with. `None` is the automatic search: the first `.sf2` in the
    /// device's `soundfonts` folder. See [`crate::audio::soundfont::first_bank`].
    ///
    /// It exists for whoever has the console firmware's bank, or another of their liking, and does
    /// not want to touch the device folder. A configuration saved before it opens with `None`.
    pub soundfont: Option<PathBuf>,
    /// The reverb and chorus the score asks for, when it plays through the `.sf2` bank. See
    /// [`crate::audio::soundfont::set_effects`].
    pub midi_effects: bool,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            enabled: true,
            volume: 80,
            soundfont: None,
            midi_effects: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Interface language. `None` means "the system's".
    pub language: Option<String>,
    /// The interface theme, by the file's key (`rubi`). `None` is the system palette, which was the
    /// only one until themes existed. See [`crate::ui::themes`].
    #[serde(rename = "tema")]
    pub theme: Option<String>,
    /// Where to look for games.
    pub roms_dir: Option<PathBuf>,
    /// The Z-Wheel package: opens from the top bar and lends its covers to the library.
    pub z_wheel_path: Option<PathBuf>,
    /// Where screenshots are written. `None` is the default folder, see
    /// [`crate::ui::screenshot::default_dir`].
    pub screenshots_dir: Option<PathBuf>,
    #[serde(rename = "atalhos")]
    pub shortcuts: Shortcuts,
    /// How the library shows the games.
    #[serde(rename = "biblioteca")]
    pub library: LibraryMode,
    #[serde(rename = "movimento")]
    pub motion: Motion,
    /// The main window's bars that are hidden via the View menu.
    #[serde(rename = "barras")]
    pub bars: Bars,
    #[serde(rename = "controles_na_tela")]
    pub on_screen_controls: OnScreenControls,
    pub graphics: Graphics,
    #[serde(rename = "velocidade")]
    pub speed: Speed,
    pub debug: DebugView,
    pub audio: Audio,
    pub controls: crate::input::bindings::Controls,
    pub z_wheel: ZWheel,
    pub discord: Discord,
    #[serde(rename = "atualizacoes")]
    pub updates: Updates,
    /// The version in which the startup notice was dismissed for good. Another version shows it again.
    #[serde(rename = "aviso_dispensado_na_versao")]
    pub notice_dismissed_in_version: Option<String>,
}

/// The game window's keys that can be changed, by the mapping's key name (`F9`).
///
/// Esc, P and F11 stay fixed in the window; this is where they go if they ever become changeable.
/// See `docs/implementation/22-screenshots.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shortcuts {
    pub screenshot: String,
    /// Fast-forward: a key or a controller button, from any connected controller. See
    /// [`crate::speed::FastForward`].
    ///
    /// **A list, and of `Source`**, unlike the screenshot: whoever plays with a controller needs a
    /// button, and the game's button mapping already knows how to store one. A button that is here
    /// does not reach the game — see [`Shortcuts::reserved`].
    #[serde(rename = "avancar")]
    pub fast_forward: Vec<Source>,
    /// Rewind, which is held: a key or a button, like [`Shortcuts::fast_forward`]. See
    /// [`crate::speed::rewind`].
    #[serde(rename = "voltar")]
    pub rewind: Vec<Source>,
    /// Restart the open game, right away. Keyboard only, like the screenshot; empty is no key.
    #[serde(rename = "reiniciar")]
    pub restart: String,
    /// The game window's save states: save to the chosen slot, load from it, switch slots and undo
    /// the last load. See [`crate::ui::save_slots`].
    ///
    /// **A [`KeyCombo`], and not a single key**, unlike the ones above: on the Mac the F keys are
    /// for brightness, volume and Mission Control, and the convention there uses ⌘. Keyboard only,
    /// like the screenshot: a controller button that saves or loads a state is easy to press by
    /// accident. Empty is no key.
    #[serde(rename = "salvar_estado")]
    pub save_state: String,
    #[serde(rename = "carregar_estado")]
    pub load_state: String,
    #[serde(rename = "proximo_slot")]
    pub next_slot: String,
    #[serde(rename = "slot_anterior")]
    pub previous_slot: String,
    #[serde(rename = "desfazer_carregar")]
    pub undo_load: String,
}

impl Shortcuts {
    pub const DEFAULT_SCREENSHOT: &str = "F9";
    /// Holding F10 has been fast-forward since its first design, in PR #83; whoever got used to it
    /// keeps the habit.
    pub const DEFAULT_FAST_FORWARD: &str = "F10";
    /// Backspace, which would be the obvious choice, is already the game's `AVK_CLR`.
    pub const DEFAULT_REWIND: &str = "F8";
    /// R for "reset", which no button of the factory keyboard mapping uses.
    pub const DEFAULT_RESET: &str = "R";

    pub fn default_fast_forward() -> Vec<Source> {
        vec![Source::key(Self::DEFAULT_FAST_FORWARD)]
    }

    /// The save state ones follow PCSX2 on Windows and Linux — F1 saves, F3 loads, F2 switches
    /// slot — and the Mac convention on the Mac, where ⌘S and ⌘L are every program's save and
    /// open, and OpenEmu does the same. No game uses ⌘, so nothing collides with the buttons.
    pub const DEFAULT_SAVE_STATE: &str = match cfg!(target_os = "macos") {
        true => "Cmd+S",
        false => "F1",
    };
    pub const DEFAULT_LOAD_STATE: &str = match cfg!(target_os = "macos") {
        true => "Cmd+L",
        false => "F3",
    };
    pub const DEFAULT_NEXT_SLOT: &str = match cfg!(target_os = "macos") {
        true => "Cmd+CloseBracket",
        false => "F2",
    };
    pub const DEFAULT_PREVIOUS_SLOT: &str = match cfg!(target_os = "macos") {
        true => "Cmd+OpenBracket",
        false => "Shift+F2",
    };
    /// F4 sits next to the three; ⌘Z is every Mac program's undo.
    pub const DEFAULT_UNDO_LOAD: &str = match cfg!(target_os = "macos") {
        true => "Cmd+Z",
        false => "F4",
    };

    /// The sources that belong to shortcuts and therefore do not reach the game.
    ///
    /// A controller button bound to a shortcut and also to a Zeebo button would do both things at
    /// once: fast-forward and, say, jump. The shortcut wins, and the editor warns about the
    /// conflict.
    pub fn reserved(&self) -> impl Iterator<Item = &Source> {
        self.fast_forward.iter().chain(&self.rewind)
    }
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            screenshot: Self::DEFAULT_SCREENSHOT.to_string(),
            fast_forward: Self::default_fast_forward(),
            rewind: vec![Source::key(Self::DEFAULT_REWIND)],
            restart: Self::DEFAULT_RESET.to_string(),
            save_state: Self::DEFAULT_SAVE_STATE.to_string(),
            load_state: Self::DEFAULT_LOAD_STATE.to_string(),
            next_slot: Self::DEFAULT_NEXT_SLOT.to_string(),
            previous_slot: Self::DEFAULT_PREVIOUS_SLOT.to_string(),
            undo_load: Self::DEFAULT_UNDO_LOAD.to_string(),
        }
    }
}

/// The modifiers of a [`KeyCombo`].
///
/// **`cmd` is ⌘ on the Mac.** Elsewhere it is the system key (the Windows key, Super), which Qt
/// calls Meta in both cases — but on the Mac Qt swaps the names, and what it calls Control is ⌘.
/// Whoever reads the keyboard from a toolkit is the one who undoes the swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

impl Modifiers {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A key and the modifiers that go with it: `F1`, `Shift+F2`, `Cmd+S`.
///
/// Stored as text, with the `egui::Key` key name at the end and the modifiers in front, separated
/// by `+`. A lone key is written as it always was (`F9`), so the same text serves single-key
/// shortcuts. egui's names have no `+`: the plus key's is `Plus`.
///
/// **Matches only with the same modifiers.** F2 switches to the next slot and Shift+F2 to the
/// previous one: accepting extra modifiers would make Shift+F2 also move forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub key: egui::Key,
    pub modifiers: Modifiers,
}

impl KeyCombo {
    /// Reads the stored text. `None` for empty text, for an unknown modifier or for a key egui
    /// does not know.
    pub fn read(text: &str) -> Option<Self> {
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let key = egui::Key::from_name(parts.pop()?)?;
        let mut modifiers = Modifiers::default();
        for part in parts {
            let field = match part {
                "Ctrl" => &mut modifiers.ctrl,
                "Alt" => &mut modifiers.alt,
                "Shift" => &mut modifiers.shift,
                "Cmd" => &mut modifiers.cmd,
                _ => return None,
            };
            *field = true;
        }
        Some(Self {
            key,
            modifiers,
        })
    }

    /// The text that is stored: the modifiers in Ctrl+Alt+Shift+Cmd order, and the key.
    pub fn name(&self) -> String {
        let m = &self.modifiers;
        let mut name = String::new();
        for (enabled, text) in [
            (m.ctrl, "Ctrl+"),
            (m.alt, "Alt+"),
            (m.shift, "Shift+"),
            (m.cmd, "Cmd+"),
        ] {
            if enabled {
                name.push_str(text);
            }
        }
        name.push_str(self.key.name());
        name
    }

    /// What is shown to the reader: on the Mac, its symbols (`⇧⌘S`); elsewhere, `Shift+F2`. The
    /// key goes by its symbol when it has one: `]`, not `CloseBracket`.
    pub fn label(&self) -> String {
        let m = &self.modifiers;
        let key = self.key.symbol_or_name();
        if cfg!(target_os = "macos") {
            let mut label = String::new();
            for (enabled, symbol) in [(m.ctrl, "⌃"), (m.alt, "⌥"), (m.shift, "⇧"), (m.cmd, "⌘")]
            {
                if enabled {
                    label.push_str(symbol);
                }
            }
            label.push_str(key);
            return label;
        }
        let system = match cfg!(target_os = "windows") {
            true => "Win+",
            false => "Super+",
        };
        let mut label = String::new();
        for (enabled, text) in [
            (m.ctrl, "Ctrl+"),
            (m.alt, "Alt+"),
            (m.shift, "Shift+"),
            (m.cmd, system),
        ] {
            if enabled {
                label.push_str(text);
            }
        }
        label.push_str(key);
        label
    }
}

/// The search for new versions. See [`crate::ui::updater`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Updates {
    /// Ask GitHub at launch.
    #[serde(rename = "ao_abrir")]
    pub on_launch: bool,
    /// Whether a release marked as pre-release counts as a new version. **On by default**: up to
    /// 0.4.1 every release came out that way, and with it off nobody would be told anything.
    #[serde(rename = "pre_lancamentos")]
    pub prereleases: bool,
}

impl Default for Updates {
    fn default() -> Self {
        Self { on_launch: true, prereleases: true }
    }
}

/// The Discord Rich Presence. See [`crate::ui::discord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Discord {
    #[serde(rename = "ativo")]
    pub active: bool,
    /// Where the covers are published, with `{clsid}` or `{chave}` in place of the game. Empty
    /// uses the images registered in the application itself.
    #[serde(rename = "capas_url")]
    pub covers_url: String,
}

impl Default for Discord {
    fn default() -> Self {
        Self {
            active: true,
            covers_url: String::new(),
        }
    }
}

/// The main window's bars, which PCSX2 lets you hide: the toolbar, below the menu, and the status
/// bar, at the bottom of the window. Both show by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bars {
    #[serde(rename = "ferramentas")]
    pub tools: bool,
    pub status: bool,
}

impl Default for Bars {
    fn default() -> Self {
        Self {
            tools: true,
            status: true,
        }
    }
}

/// The motion controller, outside each port's mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Motion {
    /// The notice in the corner of the game while it calibrates the Boomerang.
    #[serde(rename = "aviso_de_calibracao")]
    pub calibration_notice: bool,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            calibration_notice: true,
        }
    }
}

/// When the controls drawn on screen appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnScreenControlsMode {
    /// They show until a physical controller press arrives, and come back on the next screen
    /// touch. It is the default because it serves both ways of playing without asking anyone to
    /// choose.
    #[default]
    #[serde(rename = "automatico")]
    Auto,
    #[serde(rename = "sempre")]
    Always,
    #[serde(rename = "nunca")]
    Never,
}

impl OnScreenControlsMode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Always, Self::Never];

    pub fn key_id(self) -> &'static str {
        match self {
            Self::Auto => "touch.mode.auto",
            Self::Always => "touch.mode.always",
            Self::Never => "touch.mode.never",
        }
    }
}

/// The controls drawn on a phone screen. Only Android draws them; the other frontends load them
/// and write them back without touching. See [`crate::input::touch`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OnScreenControls {
    #[serde(rename = "modo")]
    pub mode: OnScreenControlsMode,
    /// The size of the pieces, in percent of the default.
    #[serde(rename = "escala")]
    pub scale: u8,
    /// In percent. Not opaque by default: the piece sits on top of the game.
    #[serde(rename = "opacidade")]
    pub opacity: u8,
    /// Where the user put each piece, by [`crate::input::touch::TouchControl::key_id`], as a
    /// **fraction** of the screen. A fraction, not a point, because the same file may go from a
    /// phone to a tablet. A piece that is not here stays in its default place, and "restore" is
    /// emptying the map.
    #[serde(rename = "posicoes")]
    pub positions: std::collections::BTreeMap<String, [f32; 2]>,
    /// The size of each piece, in percent, on top of [`Self::scale`]. Same key as the positions;
    /// a piece that is not here stays at 100.
    #[serde(rename = "tamanhos")]
    pub sizes: std::collections::BTreeMap<String, u8>,
    /// Show the shortcut pieces. The turbo and the rewind, besides that, only show with the
    /// function in use — see [`OnScreenControls::show`].
    #[serde(rename = "mostrar_avancar")]
    pub show_fast_forward: bool,
    #[serde(rename = "mostrar_voltar")]
    pub show_rewind: bool,
    #[serde(rename = "mostrar_turbo")]
    pub show_turbo: bool,
}

impl Default for OnScreenControls {
    fn default() -> Self {
        Self {
            mode: OnScreenControlsMode::default(),
            scale: 100,
            opacity: 55,
            positions: Default::default(),
            sizes: Default::default(),
            show_fast_forward: true,
            show_rewind: true,
            show_turbo: true,
        }
    }
}

impl OnScreenControls {
    /// Whether the piece shows on screen.
    ///
    /// **The turbo and the rewind only show with the function in use**: the rewind ships off, and
    /// someone who never turned it on does not get a button that does nothing. Fast-forward has no
    /// on switch, only the speed, and that is why it always shows — on a phone there is no F10, and
    /// without the piece it would stay hidden from anyone who does not search the settings. Each
    /// one hides by its own key.
    pub fn show(&self, touch_control: crate::input::touch::TouchControl, settings: &Settings) -> bool {
        use crate::input::touch::TouchControl;
        match touch_control {
            TouchControl::FastForward => self.show_fast_forward,
            TouchControl::Rewind => self.show_rewind && settings.speed.rewind.enabled,
            TouchControl::Turbo => {
                let mode = settings.controls.player(0).map(|player| player.turbo);
                self.show_turbo
                    && mode.is_some_and(|mode| mode != crate::speed::turbo::TurboMode::Off)
            }
            _ => true,
        }
    }
}

/// The library layout on the main screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryMode {
    /// Cards side by side, as many as fit in the width.
    #[default]
    #[serde(rename = "grade")]
    Grid,
    /// One game at a time, Z-Wheel style: the logo reel on top and the boxes passing by.
    Slider,
}

/// The game's pace: the frame limit and the draw skip. See [`crate::speed`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Speed {
    #[serde(rename = "limite_fps")]
    pub fps_limit: FpsLimit,
    pub frameskip: Frameskip,
    #[serde(rename = "avanco")]
    pub fast_forward: FastForward,
    /// How many presses per second the turbo gives, on the game's clock. A single one for all
    /// players: the mode and the button are what belong to each one. See [`crate::speed::turbo`].
    #[serde(rename = "turbo_por_segundo")]
    pub turbo_per_second: u8,
    pub rewind: RewindSettings,
}

impl Default for Speed {
    fn default() -> Self {
        Self {
            fps_limit: FpsLimit::default(),
            frameskip: Frameskip::default(),
            fast_forward: FastForward::default(),
            turbo_per_second: crate::speed::turbo::DEFAULT_PRESSES,
            rewind: RewindSettings::default(),
        }
    }
}

impl Speed {
    /// The pace of these choices, to hand to the session.
    pub fn pace(&self) -> Pace {
        Pace::from_choices(self.fps_limit, self.frameskip)
    }

    /// The current pace: that of the choices, or fast-forward's on top of it.
    pub fn pace_with(&self, fast_forwarding: bool) -> Pace {
        match fast_forwarding {
            true => self.pace().fast_forwarded(&self.fast_forward),
            false => self.pace(),
        }
    }
}

/// The settings of a single game, when it has its own: audio, graphics, speed and controls.
///
/// Stored in `<config>/jogos/<key_id>.json`, one per game; **the file existing is the "use custom
/// settings"**. Each section is a whole copy of the global one at the moment the option was turned
/// on, not just the difference: changing the global one afterwards does not touch the game, which
/// is what "custom" means. The key is that of [`crate::library::game_key`].
///
/// Of the graphics, what belongs to the window — mode and size of both, and the presentation path
/// — stays global, even being in the file: see [`GameConfig::about`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GameConfig {
    pub audio: Audio,
    pub graphics: Graphics,
    #[serde(rename = "velocidade")]
    pub speed: Speed,
    pub controls: crate::input::bindings::Controls,
}

impl GameConfig {
    /// The game's sections, copied from `settings`.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            audio: settings.audio.clone(),
            graphics: settings.graphics.clone(),
            speed: settings.speed.clone(),
            controls: settings.controls.clone(),
        }
    }

    /// The configuration that applies to the game: the `global` one, with the game's sections on
    /// top.
    pub fn about(&self, global: &Settings) -> Settings {
        let window = &global.graphics;
        Settings {
            audio: self.audio.clone(),
            graphics: Graphics {
                window: window.window,
                game_window: window.game_window,
                separate_game_window: window.separate_game_window,
                confirm_on_close: window.confirm_on_close,
                window_size: window.window_size,
                game_window_size: window.game_window_size,
                gpu_present: window.gpu_present,
                legacy_limit: None,
                ..self.graphics.clone()
            },
            speed: self.speed.clone(),
            controls: self.controls.clone(),
            ..global.clone()
        }
    }

    /// Those of the `key_id` game, if it has them. Unreadable counts as not having them: the game
    /// opens with the global one.
    pub fn load(key_id: &str) -> Option<Self> {
        Self::try_load_from(&game_settings_path(key_id))
    }

    pub fn try_load_from(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut config: Self = serde_json::from_str(&text).ok()?;
        config.controls.adopt();
        Some(config)
    }

    pub fn save(&self, key_id: &str) -> std::io::Result<()> {
        self.write_to(&game_settings_path(key_id))
    }

    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    /// Turns the game's settings off: it goes back to the global one.
    pub fn remove(key_id: &str) -> std::io::Result<()> {
        match std::fs::remove_file(game_settings_path(key_id)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

/// Where the settings of the `key_id` game live. See [`GameConfig`].
pub fn game_settings_path(key_id: &str) -> PathBuf {
    config_dir().join("jogos").join(format!("{key_id}.json"))
}

/// Re-exported by the UI so as not to break serialized preferences and desktop calls.
pub use crate::config::ZWheel;

impl Settings {
    /// Reads the preferences from `path`. A missing or unreadable file returns the default —
    /// opening with the factory options is always better than not opening.
    pub fn load_from(path: &Path) -> Self {
        let json_text = std::fs::read_to_string(path).ok();
        let mut settings: Self = json_text
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default();
        // A file from an earlier version does not carry what that version did not know, and the
        // default of a new field is not always empty.
        settings.controls.adopt();
        settings.adopt_legacy_limit();
        if let Some(json_text) = &json_text {
            settings.adopt_new_shortcuts(json_text, path);
        }
        settings
    }

    /// A new shortcut arrives without a key for anyone who already uses its default key on a
    /// Zeebo button.
    ///
    /// The R for restart and F1 to F4 for save state are not on any button of the default keyboard
    /// mapping, but someone who remapped may have put them on one: the shortcut, checked before
    /// the game, would steal the button, and a press on it would restart the game or load a state.
    /// It only applies to the file that did not know the shortcut yet — whoever chose it afterwards
    /// made a choice. A combination with a modifier (Shift+F2, ⌘S) does not collide with any
    /// button, and always arrives.
    ///
    /// **Each game's settings count too.** The shortcut is a single one, and a game with the key
    /// on a button would lose the button the same way. They live in `jogos/`, next to `path`.
    fn adopt_new_shortcuts(&mut self, text: &str, path: &Path) {
        let value = serde_json::from_str::<serde_json::Value>(text).ok();
        let shortcuts = &mut self.shortcuts;
        let fresh: Vec<&mut String> = [
            ("reiniciar", &mut shortcuts.restart),
            ("salvar_estado", &mut shortcuts.save_state),
            ("carregar_estado", &mut shortcuts.load_state),
            ("proximo_slot", &mut shortcuts.next_slot),
            ("slot_anterior", &mut shortcuts.previous_slot),
            ("desfazer_carregar", &mut shortcuts.undo_load),
        ]
        .into_iter()
        .filter(|(field, _)| {
            let knew = value
                .as_ref()
                .is_some_and(|value| value.pointer(&format!("/atalhos/{field}")).is_some());
            !knew
        })
        .map(|(_, shortcut)| shortcut)
        .filter(|shortcut| !shortcut.contains('+'))
        .collect();
        if fresh.is_empty() {
            return;
        }
        let mark_used = |controls: &crate::input::bindings::Controls, key: &str| {
            controls.players.iter().any(|player| {
                player
                    .buttons
                    .values()
                    .flatten()
                    .any(|source| matches!(source, Source::Key { name } if name == key))
            })
        };
        let games: Vec<GameConfig> = path
            .parent()
            .map(|dir| dir.join("jogos"))
            .and_then(|games_dir| std::fs::read_dir(games_dir).ok())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| GameConfig::try_load_from(&entry.path()))
            .collect();
        for shortcut in fresh {
            let key = shortcut.clone();
            if mark_used(&self.controls, &key) || games.iter().any(|game| mark_used(&game.controls, &key)) {
                shortcut.clear();
            }
        }
    }

    /// The `speed_limit` off of an old file becomes the frame limit off.
    ///
    /// On changes nothing: it was the default, and the new limit's default is the same throttle.
    /// The old field disappears on reading, and the next save already comes out without it.
    fn adopt_legacy_limit(&mut self) {
        if self.graphics.legacy_limit.take() == Some(false) {
            self.speed.fps_limit = FpsLimit::Off;
        }
    }

    pub fn load() -> Self {
        Self::load_from(&settings_path())
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&settings_path())
    }
}

/// The label of an internal resolution factor: the factor, the size and the market reference.
pub fn resolution_label(factor: u8) -> String {
    let (width, height) = (640 * u32::from(factor), 480 * u32::from(factor));
    let reference = match factor {
        1 => "native",
        2 => "~720p",
        3 => "~1080p",
        4 => "~1440p",
        _ => "~4K",
    };
    format!("{factor}x · {width}×{height} · {reference}")
}

/// The label of a draw skip choice: "Off", "Auto", or how many frames are drawn in fixed mode —
/// "1 of 3" tells the player more than the `n` of the formula.
pub fn frameskip_label(frameskip: Frameskip, catalog: &crate::ui::i18n::Catalog) -> String {
    match frameskip {
        Frameskip::Off => catalog.get("common.off").to_string(),
        Frameskip::Auto => catalog.get("speed.frameskip.auto").to_string(),
        Frameskip::Fixed(n) => format!(
            "{} 1/{}",
            catalog.get("speed.frameskip.fixed"),
            u32::from(n) + 1
        ),
    }
}

/// The label of an antialiasing or anisotropic level: 1 is off.
pub fn level_label(level: u8, abbrev: &str, off: &str) -> String {
    match level {
        0 | 1 => off.to_string(),
        n => format!("{n}x {abbrev}"),
    }
}

pub fn settings_path() -> PathBuf {
    config_dir().join(FILE_NAME)
}

/// Where to look for loose languages: next to the executable and in the configuration folder.
///
/// The first serves a portable copy of the emulator; the second, whoever wants to add a language
/// without touching the installation.
pub fn language_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("lang"));
        }
    }
    dirs.push(config_dir().join("lang"));
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_round_trip_through_disk_preserves_everything() {
        let dir = std::env::temp_dir().join("zeebx-testes-settings");
        let path = dir.join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);

        let settings = Settings {
            language: Some("pt-BR".into()),
            theme: Some("esmeralda".into()),
            roms_dir: Some(PathBuf::from("/jogos/zeebo")),
            z_wheel_path: Some(PathBuf::from("/jogos/Z-Wheel.zip")),
            screenshots_dir: Some(PathBuf::from("/imagens/zeebo")),
            shortcuts: Shortcuts {
                screenshot: "F12".into(),
                fast_forward: vec![Source::key("F7"), Source::button("LeftThumb")],
                rewind: vec![Source::button("RightThumb")],
                restart: "Home".into(),
                save_state: "Ctrl+S".into(),
                load_state: "F6".into(),
                next_slot: "Alt+Shift+CloseBracket".into(),
                previous_slot: String::new(),
                undo_load: "Cmd+Z".into(),
            },
            library: LibraryMode::Slider,
            motion: Motion {
                calibration_notice: false,
            },
            bars: Bars {
                tools: false,
                status: true,
            },
            on_screen_controls: OnScreenControls {
                mode: OnScreenControlsMode::Never,
                scale: 130,
                opacity: 80,
                positions: [("botoes".to_string(), [0.75, 0.5])].into(),
                sizes: [("dpad".to_string(), 140)].into(),
                show_fast_forward: false,
                show_rewind: true,
                show_turbo: false,
            },
            graphics: Graphics {
                scaling: Scaling::Fit,
                ..Graphics::default()
            },
            speed: Speed {
                fps_limit: FpsLimit::Thirty,
                frameskip: Frameskip::Fixed(2),
                fast_forward: FastForward {
                    ratio: 0,
                    mode: crate::speed::ShortcutMode::Toggle,
                    mute: true,
                },
                turbo_per_second: 15,
                rewind: RewindSettings {
                    enabled: true,
                    interval_ms: 2000,
                    memory_mb: 512,
                },
            },
            debug: DebugView {
                overlay: true,
                metrics: true,
                ..DebugView::default()
            },
            audio: Audio {
                volume: 42,
                ..Audio::default()
            },
            controls: crate::input::bindings::Controls::default(),
            z_wheel: ZWheel {
                end_of_life: false,
                ..ZWheel::default()
            },
            discord: Discord {
                active: false,
                covers_url: "https://exemplo/{chave}.png".into(),
            },
            updates: Updates { on_launch: false, prereleases: false },
            notice_dismissed_in_version: Some("0.1.0".into()),
        };
        settings.save_to(&path).unwrap();

        assert_eq!(Settings::load_from(&path), settings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whoever has a `settings.json` from before the window size gets `None` in both new fields,
    /// and what was stored comes back from disk unchanged.
    #[test]
    fn the_window_size_round_trips_and_is_missing_in_old_versions() {
        let dir = std::env::temp_dir().join("zeebx-testes-janela");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let old: Settings =
            serde_json::from_str(r#"{ "graphics": { "janela": "maximizada" } }"#).unwrap();
        assert_eq!(old.graphics.window_size, None);
        assert_eq!(old.graphics.game_window_size, None);

        let mut create = Settings::default();
        create.graphics.window_size = Some([1280.0, 800.0]);
        create.graphics.game_window_size = Some([640.0, 512.0]);
        create.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), create);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whoever has a `settings.json` from before the screenshot gets F9 and the default folder,
    /// without losing the rest.
    #[test]
    fn a_file_from_before_shortcuts_gets_f9() {
        let loaded: Settings = serde_json::from_str(r#"{ "roms_dir": "/jogos" }"#).unwrap();
        assert_eq!(loaded.shortcuts.screenshot, "F9");
        assert_eq!(loaded.shortcuts.fast_forward, [Source::key("F10")]);
        assert_eq!(loaded.screenshots_dir, None);
        assert_eq!(loaded.roms_dir, Some(PathBuf::from("/jogos")));
        // A file that already had F9 changed, from before fast-forward, gets F10 too.
        let changed: Settings =
            serde_json::from_str(r#"{ "atalhos": { "screenshot": "F12" } }"#).unwrap();
        assert_eq!(changed.shortcuts.screenshot, "F12");
        assert_eq!(changed.shortcuts.fast_forward, [Source::key("F10")]);
    }

    /// The `speed_limit` off from before the frame limit stays off, and the old field is not
    /// written again.
    #[test]
    fn the_legacy_speed_limit_becomes_the_frame_limit() {
        let dir = std::env::temp_dir().join("zeebx-testes-speed-limit");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        std::fs::write(&path, r#"{"graphics": {"speed_limit": false, "smooth": true}}"#).unwrap();
        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.speed.fps_limit, FpsLimit::Off);
        assert!(loaded.graphics.smooth);
        assert_eq!(loaded.graphics.legacy_limit, None);
        loaded.save_to(&path).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("speed_limit"));

        std::fs::write(&path, r#"{"graphics": {"speed_limit": true}}"#).unwrap();
        assert_eq!(Settings::load_from(&path).speed.fps_limit, FpsLimit::Sixty);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The R for restart arrives for whoever does not use it, and does not steal the button from
    /// whoever already mapped it. Whoever already knew the shortcut keeps what they chose.
    #[test]
    fn the_reset_shortcut_does_not_steal_a_button() {
        let dir = std::env::temp_dir().join(format!("zeebx-testes-reiniciar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        std::fs::write(&path, r#"{ "roms_dir": "/jogos" }"#).unwrap();
        assert_eq!(Settings::load_from(&path).shortcuts.restart, "R");

        let mut with_r = Settings::default();
        with_r.controls.player_mut(0).bind("b4", Source::key("R"));
        let mut value = serde_json::to_value(&with_r).unwrap();
        value["atalhos"].as_object_mut().unwrap().remove("reiniciar");
        std::fs::write(&path, value.to_string()).unwrap();
        assert_eq!(Settings::load_from(&path).shortcuts.restart, "", "b4's R became restart");

        // Saved with the shortcut, it applies even with R on a button: it was a choice.
        with_r.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path).shortcuts.restart, "R");

        // R on a button of a single game, with the general settings without it, also leaves the
        // shortcut without a key.
        let mut game = GameConfig::from_settings(&Settings::default());
        game.controls.player_mut(0).bind("b4", Source::key("R"));
        game.write_to(&dir.join("jogos").join("01234567.json")).unwrap();
        std::fs::write(&path, r#"{ "roms_dir": "/jogos" }"#).unwrap();
        assert_eq!(Settings::load_from(&path).shortcuts.restart, "", "the game's R became restart");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The save state F1 follows the R rule: it arrives for whoever does not use it, and does not
    /// steal the button from whoever put it on one. A combination with a modifier always arrives.
    #[test]
    fn the_save_state_shortcuts_do_not_steal_a_button() {
        let dir = std::env::temp_dir().join(format!("zeebx-testes-estado-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let mut with_f1 = Settings::default();
        with_f1.controls.player_mut(0).bind("b1", Source::key("F1"));
        with_f1.controls.player_mut(0).bind("b2", Source::key("F2"));
        let mut value = serde_json::to_value(&with_f1).unwrap();
        for field in [
            "salvar_estado",
            "carregar_estado",
            "proximo_slot",
            "slot_anterior",
            "desfazer_carregar",
        ] {
            value["atalhos"].as_object_mut().unwrap().remove(field);
        }
        std::fs::write(&path, value.to_string()).unwrap();
        let loaded = Settings::load_from(&path).shortcuts;
        assert_eq!(loaded.load_state, Shortcuts::DEFAULT_LOAD_STATE);
        assert_eq!(loaded.previous_slot, Shortcuts::DEFAULT_PREVIOUS_SLOT);
        if cfg!(target_os = "macos") {
            assert_eq!(loaded.save_state, "Cmd+S");
        } else {
            assert_eq!(loaded.save_state, "", "b1's F1 became save state");
            assert_eq!(loaded.next_slot, "", "b2's F2 became slot switch");
        }

        // Saved with the shortcut, it applies even with the key on a button: it was a choice.
        with_f1.save_to(&path).unwrap();
        assert_eq!(
            Settings::load_from(&path).shortcuts.save_state,
            Shortcuts::DEFAULT_SAVE_STATE
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whoever comes from 0.4.x did not have the option, and starts playing in the main window: it
    /// is the redesign everyone sees. The separate window is still one click away, in Graphics.
    #[test]
    fn an_old_file_plays_in_the_main_window() {
        let old: Settings =
            serde_json::from_str(r#"{ "graphics": { "janela_do_jogo": "tela_cheia" } }"#).unwrap();
        assert!(!old.graphics.separate_game_window);
        assert_eq!(old.graphics.game_window, WindowMode::Fullscreen);
    }

    #[test]
    fn the_combination_round_trips_through_text() {
        let shift_f2 = KeyCombo::read("Shift+F2").unwrap();
        assert_eq!(shift_f2.key, egui::Key::F2);
        assert!(shift_f2.modifiers.shift && !shift_f2.modifiers.cmd);
        assert_eq!(shift_f2.name(), "Shift+F2");

        let key_only = KeyCombo::read("F9").unwrap();
        assert!(key_only.modifiers.is_empty());
        assert_eq!(key_only.name(), "F9");

        // The order of whoever wrote it by hand does not matter; the stored one is always the
        // same.
        assert_eq!(
            KeyCombo::read("Cmd+Shift+]").unwrap().name(),
            "Shift+Cmd+CloseBracket"
        );
        assert_eq!(KeyCombo::read("Ctrl+Plus").unwrap().key, egui::Key::Plus);

        assert_eq!(KeyCombo::read(""), None);
        assert_eq!(KeyCombo::read("Hyper+F1"), None);
        assert_eq!(KeyCombo::read("Shift+"), None);

        for fallback in [
            Shortcuts::DEFAULT_SAVE_STATE,
            Shortcuts::DEFAULT_LOAD_STATE,
            Shortcuts::DEFAULT_NEXT_SLOT,
            Shortcuts::DEFAULT_PREVIOUS_SLOT,
            Shortcuts::DEFAULT_UNDO_LOAD,
        ] {
            assert_eq!(
                KeyCombo::read(fallback).map(|c| c.name()).as_deref(),
                Some(fallback)
            );
        }
    }

    #[test]
    fn a_missing_or_corrupt_file_returns_the_default() {
        // Opening with the factory options is always better than not opening.
        let missing = std::env::temp_dir().join("zeebx-nao-existe-mesmo/settings.json");
        assert_eq!(Settings::load_from(&missing), Settings::default());

        let dir = std::env::temp_dir().join("zeebx-testes-estragado");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ isto não é json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_controller_saved_before_axes_gets_the_axes_on_load() {
        // This is the path the emulator really uses: a file written by a version that did not
        // know the axes would leave the sticks dead, and the default of a new field is not always
        // empty.
        let dir = std::env::temp_dir().join("zeebx-testes-eixos");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"controls": {"players": [{"device": "PS4 Controller", "buttons": {}}]}}"#,
        )
        .unwrap();

        let loaded = Settings::load_from(&path);
        let player = loaded.controls.player(0).unwrap();
        assert_eq!(player.axes, crate::input::bindings::Player::default_axes());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_field_missing_from_the_file_uses_the_default() {
        // A file written by an earlier version does not have the new fields, and it must keep
        // working.
        let dir = std::env::temp_dir().join("zeebx-testes-parcial");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"language": "pt-BR"}"#).unwrap();

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.language.as_deref(), Some("pt-BR"));
        assert_eq!(loaded.graphics, Graphics::default());
        assert_eq!(loaded.audio, Audio::default());
        assert_eq!(loaded.controls, crate::input::bindings::Controls::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The custom game takes its four sections, and the rest — including the window, which lives
    /// in the graphics — stays the global one's.
    #[test]
    fn per_game_settings_replace_only_the_game_sections() {
        let mut for_game = Settings::default();
        for_game.audio.volume = 20;
        for_game.graphics.internal_resolution = 3;
        for_game.graphics.window = WindowMode::Fullscreen;
        for_game.speed.fps_limit = FpsLimit::Thirty;
        for_game.controls.players[0].enabled = false;
        let config = GameConfig::from_settings(&for_game);

        let mut global = Settings::default();
        global.language = Some("es".into());
        global.graphics.window_size = Some([800.0, 600.0]);
        let applies = config.about(&global);

        assert_eq!(applies.audio.volume, 20);
        assert_eq!(applies.graphics.internal_resolution, 3);
        assert_eq!(applies.speed.fps_limit, FpsLimit::Thirty);
        assert!(!applies.controls.players[0].enabled);
        assert_eq!(applies.language.as_deref(), Some("es"), "the language is the global one's");
        assert_eq!(applies.graphics.window, global.graphics.window, "the window mode is global");
        assert_eq!(applies.graphics.window_size, Some([800.0, 600.0]));
        // And the game's come back whole from an already built configuration.
        assert_eq!(GameConfig::from_settings(&applies).audio, config.audio);
    }

    /// The game's file goes to disk and back; without a file, or unreadable, the game has none of
    /// its own.
    #[test]
    fn per_game_settings_round_trip_and_without_a_file_there_are_none() {
        let dir = std::env::temp_dir().join("zeebx-testes-config-do-jogo");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("jogos").join("01234567.json");
        assert_eq!(GameConfig::try_load_from(&path), None);

        let mut config = GameConfig::default();
        config.audio.volume = 55;
        config.speed.frameskip = Frameskip::Fixed(1);
        config.write_to(&path).unwrap();
        assert_eq!(GameConfig::try_load_from(&path), Some(config));

        std::fs::write(&path, "{ isto não é json").unwrap();
        assert_eq!(GameConfig::try_load_from(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
