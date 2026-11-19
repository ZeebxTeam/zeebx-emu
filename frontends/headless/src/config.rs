//! `config.ini` turned into the [`Settings`] the core already understands.
//!
//! **There is no second configuration model here.** Graphics, audio and controller mapping are
//! the same fields the desktop interface writes to `settings.json`; what changes is the file
//! format, because whoever uses this frontend edits by hand and INI is what gets edited by hand.
//! What is left — how the window opens, what to do with the frame when there is no window —
//! belongs to the frontend and lives in [`Headless`].
//!
//! Every key is optional and falls back to a default. A missing file counts as an empty file: the
//! emulator has to open with the factory options, not refuse.

use std::path::PathBuf;

use zeebx::input::bindings::{Device, AxisSource, Controls, Player, Source};
use zeebx::logging::Threshold;
use zeebx::ui::settings::{Audio, Graphics, WindowMode, AspectRatio, Scaling, Settings};
use zeebx::speed::{Frameskip, FpsLimit};

use crate::ini::{Ini, Value, unquoted};

/// What exists only in this frontend.
#[derive(Debug, Clone)]
pub struct Headless {
    /// Whether a window opens, and how.
    pub video: Video,
    /// What to do with the frame without a window. See [`Dump`].
    pub dump: Dump,
    /// Window title. A frontend that manages windows usually matches by title.
    pub title: String,
    /// The size the window opens with, when it is not fullscreen.
    ///
    /// It only applies at startup, on purpose: `config.ini` is edited by hand, and writing the
    /// size back on every resize would be fighting whoever wrote it (see `create`, which never
    /// replaces a file that already exists). Whoever wants the window to remember its size uses
    /// a desktop frontend, which stores it in `graphics.window_size` in `settings.json` — see
    /// issue #74.
    pub size: (u32, u32),
    /// Wait for the screen's vertical retrace to swap the frame. Off, the window runs free.
    pub vsync: bool,
    /// Where the games are. It is what lets the Z-Wheel open a title by ClassID.
    pub roms: Option<PathBuf>,
    /// The Z-Wheel package, where the console goes back to when a game exits.
    pub z_wheel: Option<PathBuf>,
    /// Seconds of game time before exiting on its own. `None` runs until it is closed.
    pub seconds: Option<u32>,
    /// Exit when the game exits, instead of going back to the Z-Wheel.
    pub exit_with_game: bool,
}

/// How the frame reaches whoever is watching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Video {
    /// A window with the game and nothing else.
    Window,
    /// The same window, filling the screen.
    Fullscreen,
    /// No window. The frame goes out through [`Dump`], and the outside frontend does the painting.
    Off,
}

/// Where the frames go when there is no window.
#[derive(Debug, Clone)]
pub struct Dump {
    pub format: Format,
    /// `None` is standard output; with a path, the file — which can be a FIFO, and that is how an
    /// outside frontend reads without going through disk.
    pub dest: Option<PathBuf>,
    /// Stop after this many frames. `0` means no limit.
    pub limit: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Two bytes per pixel, the way the console stores it. It is the cheapest: nothing is
    /// converted.
    Rgb565,
    /// Four bytes per pixel, in R, G, B, A order. It costs one conversion per frame and is what
    /// most image libraries accept without fuss.
    Rgba,
    /// One numbered `.png` per frame, in a folder. For checking, not for playing.
    Png,
}

impl Default for Headless {
    fn default() -> Self {
        Self {
            video: Video::Window,
            dump: Dump {
                format: Format::Rgb565,
                dest: None,
                limit: 0,
            },
            title: "Zeebx".to_string(),
            size: (1280, 960),
            vsync: true,
            roms: None,
            z_wheel: None,
            seconds: None,
            exit_with_game: false,
        }
    }
}

/// What came out of reading: the configuration and what there is to say about the file.
pub struct LoadedConfig {
    pub settings: Settings,
    pub headless: Headless,
    /// Where the configuration came from, or where it should be.
    pub origin: ConfigOrigin,
    /// Bad line, unknown key, value not of the expected type. Nothing here stops the emulator
    /// from running; everything here is something whoever edited the file wants to know.
    pub warnings: Vec<String>,
}

/// Whether there was a file, and which one.
pub enum ConfigOrigin {
    /// There was a file, and it was read. Which one was already said in the first warning.
    Read,
    /// There was none. The path is where one should be created.
    Missing(PathBuf),
}

/// Where the file is looked for when nobody points to one: next to the executable first, and
/// then in the system configuration folder.
///
/// The order is this on purpose. A portable copy — the folder the outside frontend ships, with
/// the binary and `config.ini` inside — has to win over what is installed on the machine,
/// otherwise two frontends on the same computer fight over the same file.
pub fn default_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("config.ini"));
        }
    }
    paths.push(zeebx::ui::settings::config_dir().join("config.ini"));
    paths
}

/// Where to create a file that does not exist: the system configuration folder.
///
/// **Not next to the executable**, which is the first place searched. There the installed copy
/// usually has no write permission, and one that did would be a file created inside the
/// installation — which disappears on the next update and belongs to nobody. The configuration
/// folder belongs to whoever ran it, and it is where they will look later.
pub fn where_to_create() -> PathBuf {
    zeebx::ui::settings::config_dir().join("config.ini")
}

/// Reads the file. A path that does not exist returns the default, saying where one should be.
pub fn load(config_path: Option<&std::path::Path>) -> LoadedConfig {
    // A `--config=` pointing at something that does not exist does **not** fall back to the
    // default: the person named the file they want, and that is where a new one will be
    // created.
    let chosen = match config_path {
        Some(config_path) => Some(config_path.to_path_buf()),
        None => default_paths().into_iter().find(|c| c.is_file()),
    };
    let missing = |location: PathBuf, warnings: Vec<String>| LoadedConfig {
        settings: Settings::default(),
        headless: Headless::default(),
        origin: ConfigOrigin::Missing(location),
        warnings,
    };
    let Some(chosen) = chosen else {
        return missing(where_to_create(), Vec::new());
    };
    match std::fs::read_to_string(&chosen) {
        Ok(text) => {
            let mut loaded = from_text(&text);
            loaded.warnings.insert(0, format!("reading {}", chosen.display()));
            loaded.origin = ConfigOrigin::Read;
            loaded
        }
        // It exists and does not open: permissions, a directory with that name. Creating over it
        // would be worse.
        Err(error) if chosen.exists() => {
            let message = format!("could not read {}: {error}", chosen.display());
            missing(chosen, vec![message])
        }
        Err(_) => missing(chosen, Vec::new()),
    }
}

/// Writes a factory `config.ini`, with comments, at `path`.
///
/// **Never over one that exists**, not even when the file is there and just did not open: a
/// `config.ini` full of settings is the work of whoever wrote it, and replacing it with the
/// example because of a permission error would be erasing what is not ours.
pub fn create(config_path: &std::path::Path) -> Result<(), String> {
    if config_path.exists() {
        return Err(format!("{} already exists", config_path.display()));
    }
    let text = model();
    if let Some(dir) = config_path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    }
    std::fs::write(config_path, text)
        .map_err(|error| format!("could not write {}: {error}", config_path.display()))
}

/// The shipped example, **before** the ports are filled in. See [`model`].
const EXAMPLE: &str = include_str!("../config.ini");

/// The example's line that gives way to the port sections.
///
/// It is a comment on purpose: if the substitution ever fails to happen, what is left is still a
/// valid `config.ini`, with one odd line, instead of a broken file.
const MARKER: &str = "#<<portas>>";

/// The complete `config.ini`: what `--example` prints and what the first run writes.
///
/// **The ports are generated, not written by hand.** They are the part people most want to edit
/// — the buttons — and also the one with the most lines: thirteen per port, and two ports.
/// Writing them in the example would mean keeping them in line with [`Controls::default`]
/// forever, and an example that says `button:East` where the default is `key:X` is worse than
/// an example without the line: whoever reads it comes to believe what is there.
pub fn model() -> String {
    EXAMPLE.replace(MARKER, &port_sections())
}

/// The `[port1]` and `[port2]` sections as the file writes them, from the defaults.
fn port_sections() -> String {
    let controls = Controls::default();
    (0..zeebx::input::PORTS)
        .map(|index| {
            let fallback = Player::default();
            let player = controls.player(index).unwrap_or(&fallback);
            port_section(index, player)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn port_section(index: usize, player: &Player) -> String {
    let mut lines = vec![
        format!("[port{}]", index + 1),
        format!("enabled = {}", yes_or_no(player.enabled)),
        "# What the console sees plugged in: gamepad (the Dragon), zpad, keyboard or boomerang.".to_string(),
        format!("device = {}", device_name(player.device_kind)),
        "# Which host controller feeds this port. `--controllers` lists the names the system".to_string(),
        "# gives them. Without this line, the port stays on the keyboard only; with it, the".to_string(),
        "# buttons below also get the controller's, and any line you change counts as written.".to_string(),
    ];
    match &player.device {
        Some(name) => lines.push(format!("controller = \"{name}\"")),
        None => lines.push("# controller = \"Xbox Wireless Controller\"".to_string()),
    }
    lines.push(
        "# The D-pad also pushes the left stick, for games that only listen to the axis.".to_string(),
    );
    lines.push(
        "# **Off by default**: games that read both channels move two squares per press.".to_string(),
    );
    lines.push(format!(
        "dpad_to_analog = {}",
        yes_or_no(player.dpad_to_axes)
    ));
    lines.push(String::new());

    // In the order the interface shows them, not the map's alphabetical order: `up, down, left,
    // right` in sequence are one D-pad, and `b1, b2, b3, b4` are the four buttons.
    for button in zeebx::input::bindings::CONFIGURABLE {
        let origins: Vec<String> = player.sources(button).iter().map(write_source).collect();
        lines.push(format!("{button} = {}", origins.join(", ")));
    }
    lines.push(String::new());

    // The axes stay commented out because **that is what the default is**: without a chosen
    // controller, the console has no analog axis at all, and writing them here would change the
    // behavior instead of describing it. As soon as the `controller` line exists, they come in on
    // their own.
    lines.push(
        "# The console's four analog axes, each one coming from an axis of the host controller."
            .to_string(),
    );
    lines.push(
        "# They only exist with a `controller` chosen, and then they take these values on their".to_string(),
    );
    lines.push("# own — the lines below are for changing them. `:inverted` flips the direction, and".to_string());
    lines.push("# an empty line turns the axis off.".to_string());
    // In the console's order — x, y, z, rz — not the map's alphabetical order: `x` and `y` are
    // one stick, `z` and `rz` are the other, and reading them out of pairs means nothing.
    let defaults = Player::default_axes();
    for axis in zeebx::input::AXIS_NAMES {
        let Some(origin) = defaults.get(axis) else {
            continue;
        };
        let suffix = match origin.invert {
            true => ":inverted",
            false => "",
        };
        lines.push(format!("# axis_{axis} = {}{suffix}", origin.name));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn write_source(source: &Source) -> String {
    match source {
        Source::Key { name } => format!("key:{name}"),
        Source::Button { name } => format!("button:{name}"),
        Source::Axis { name, positive } => {
            let sign = match positive {
                true => '+',
                false => '-',
            };
            format!("axis:{name}{sign}")
        }
    }
}

fn device_name(device_kind: Device) -> &'static str {
    match device_kind {
        Device::Controller => "gamepad",
        Device::ZPad => "zpad",
        Device::Keyboard => "keyboard",
        Device::Boomerang => "boomerang",
    }
}

fn yes_or_no(value: bool) -> &'static str {
    match value {
        true => "true",
        false => "false",
    }
}

/// The core logic, separated from the disk so it can be tested.
pub fn from_text(text: &str) -> LoadedConfig {
    let (mut ini, mut warnings) = Ini::read(text);
    let mut settings = Settings::default();
    let mut headless = Headless::default();

    // ---- [video] -----------------------------------------------------------------------
    if let Some(v) = ini.get("video", "mode") {
        match v.text.to_lowercase().as_str() {
            "window" => headless.video = Video::Window,
            "fullscreen" => headless.video = Video::Fullscreen,
            "none" => headless.video = Video::Off,
            other => warn(&mut warnings, &v, &format!(
                "`{other}` is not a video mode; use window, fullscreen or none"
            )),
        }
    }
    // `Settings` has its own way of saying this, and the session consults it: keeping them in
    // agreement prevents the window from opening fullscreen while the core thinks it did not.
    settings.graphics.game_window = match headless.video {
        Video::Fullscreen => WindowMode::Fullscreen,
        _ => WindowMode::Window,
    };
    settings.graphics.window = settings.graphics.game_window;

    if let Some(v) = ini.get("video", "scaling") {
        match v.text.to_lowercase().as_str() {
            "integer" => settings.graphics.scaling = Scaling::Integer,
            "fit" => settings.graphics.scaling = Scaling::Fit,
            "stretch" => settings.graphics.scaling = Scaling::Stretch,
            other => warn(&mut warnings, &v, &format!(
                "`{other}` is not a scaling mode; use integer, fit or stretch"
            )),
        }
    }
    boolean(&mut ini, "video", "smooth", &mut settings.graphics.smooth, &mut warnings);
    boolean(&mut ini, "video", "keep_aspect", &mut settings.graphics.keep_aspect, &mut warnings);
    // `speed_limit` is the name from before `fps_limit`, and is still accepted: an old
    // `config.ini` with it off keeps running unthrottled. When both appear, the new one wins.
    let mut limited = settings.speed.fps_limit.limit_speed();
    boolean(&mut ini, "video", "speed_limit", &mut limited, &mut warnings);
    if !limited {
        settings.speed.fps_limit = FpsLimit::Off;
    }
    if let Some(v) = ini.get("video", "fps_limit") {
        match FpsLimit::from_text(&v.text) {
            Some(limit) => settings.speed.fps_limit = limit,
            None => warn(&mut warnings, &v, "fps_limit expects 60, 30 or off"),
        }
    }
    if let Some(v) = ini.get("video", "frameskip") {
        match Frameskip::from_text(&v.text) {
            Some(skip) => settings.speed.frameskip = skip,
            None => warn(&mut warnings, &v, "frameskip expects off, auto or a number from 1 to 6"),
        }
    }
    boolean(&mut ini, "video", "vsync", &mut headless.vsync, &mut warnings);
    if let Some(v) = ini.get("video", "title") {
        headless.title = unquoted(&v.text).to_string();
    }
    let (mut width, mut height) = headless.size;
    integer(&mut ini, "video", "width", &mut width, &mut warnings);
    integer(&mut ini, "video", "height", &mut height, &mut warnings);
    headless.size = (width.max(160), height.max(120));

    // ---- [graphics] ---------------------------------------------------------------------
    // The core's `gpu_present` does not appear here: it chooses between putting the frame out
    // through the GPU or sending it as an egui texture, and there is no egui here. The raw window
    // always presents through the GPU, which is the cheap path — the frame is already in RGB565
    // and the GPU does the scaling.
    let g: &mut Graphics = &mut settings.graphics;
    boolean(&mut ini, "graphics", "gpu_rasterizer", &mut g.gpu_rasterizer, &mut warnings);
    boolean(&mut ini, "graphics", "fog", &mut g.fog, &mut warnings);
    read_u8(&mut ini, "graphics", "internal_resolution", &mut g.internal_resolution, 1, 8, &mut warnings);
    read_u8(&mut ini, "graphics", "antialias", &mut g.antialias, 1, 16, &mut warnings);
    read_u8(&mut ini, "graphics", "anisotropic", &mut g.anisotropic, 1, 16, &mut warnings);
    if let Some(v) = ini.get("graphics", "aspect") {
        match v.text.to_lowercase().as_str() {
            "native" => g.ratio = AspectRatio::Native,
            "16x9" => g.ratio = AspectRatio::Wide16x9,
            "16x10" => g.ratio = AspectRatio::Wide16x10,
            "window" => g.ratio = AspectRatio::Window,
            other => warn(&mut warnings, &v, &format!(
                "`{other}` is not an aspect; use native, 16x9, 16x10 or window"
            )),
        }
    }

    // ---- [audio] -----------------------------------------------------------------------
    let a: &mut Audio = &mut settings.audio;
    boolean(&mut ini, "audio", "enabled", &mut a.enabled, &mut warnings);
    read_u8(&mut ini, "audio", "volume", &mut a.volume, 0, 100, &mut warnings);
    boolean(&mut ini, "audio", "midi_effects", &mut a.midi_effects, &mut warnings);
    if let Some(v) = ini.get("audio", "soundfont") {
        let text = unquoted(&v.text);
        a.soundfont = match text.is_empty() || text == "-" {
            true => None,
            false => Some(PathBuf::from(text)),
        };
    }

    // ---- [dump] ---------------------------------------------------------------------
    if let Some(v) = ini.get("dump", "format") {
        match v.text.to_lowercase().as_str() {
            "rgb565" => headless.dump.format = Format::Rgb565,
            "rgba" => headless.dump.format = Format::Rgba,
            "png" => headless.dump.format = Format::Png,
            other => warn(&mut warnings, &v, &format!(
                "`{other}` is not a dump format; use rgb565, rgba or png"
            )),
        }
    }
    if let Some(v) = ini.get("dump", "destination") {
        let text = unquoted(&v.text);
        headless.dump.dest = match text.is_empty() || text == "-" {
            true => None,
            false => Some(PathBuf::from(text)),
        };
    }
    if let Some(v) = ini.get("dump", "frame_limit") {
        match v.text.parse::<u64>() {
            Ok(n) => headless.dump.limit = n,
            Err(_) => warn(&mut warnings, &v, "frame_limit expects a number"),
        }
    }

    // ---- [system] ---------------------------------------------------------------------
    if let Some(v) = ini.get("system", "roms") {
        let path = PathBuf::from(unquoted(&v.text));
        settings.roms_dir = Some(path.clone());
        headless.roms = Some(path);
    }
    if let Some(v) = ini.get("system", "z_wheel") {
        let path = PathBuf::from(unquoted(&v.text));
        settings.z_wheel_path = Some(path.clone());
        headless.z_wheel = Some(path);
    }
    if let Some(v) = ini.get("system", "language") {
        settings.language = Some(unquoted(&v.text).to_string());
    }
    if let Some(v) = ini.get("system", "seconds") {
        match v.text.parse::<u32>() {
            Ok(0) => headless.seconds = None,
            Ok(n) => headless.seconds = Some(n),
            Err(_) => warn(&mut warnings, &v, "seconds expects a number"),
        }
    }
    if let Some(v) = ini.get("system", "log") {
        let text = unquoted(&v.text);
        match Threshold::from_text(text) {
            Some(threshold) => {
                threshold.apply();
                // **The canonical name, not the text that came in.** `config.ini` speaks English
                // (`log = warn`) and the stored setting is in Portuguese (`aviso`): storing the
                // raw text left the same level with two forms, and the factory `config.ini` came
                // to describe a default that was not the emulator's. See [`Level::name`].
                settings.debug.log_level = match threshold {
                    Threshold::Off => "desligado".to_string(),
                    Threshold::UpTo(level) => level.name().to_string(),
                };
            }
            // A misspelled level cannot be accepted silently: whoever is debugging needs to know
            // the line did nothing, instead of concluding that the log is what is broken.
            None => warn(
                &mut warnings,
                &v,
                "`log` expects off, fatal, error, warn, info or debug",
            ),
        }
    }
    boolean(&mut ini, "system", "exit_with_game", &mut headless.exit_with_game, &mut warnings);
    boolean(&mut ini, "system", "z_wheel_end_of_life", &mut settings.z_wheel.end_of_life, &mut warnings);

    // ---- [port1], [port2] ------------------------------------------------------------
    //
    // A port the file does not mention stays as the default has it: the first with a controller
    // and the others free. Mentioning the section, even empty, turns the port on — that is what
    // someone means by writing `[port2]`.
    let mut controls = Controls::default();
    for (index, name) in port_names().into_iter().enumerate() {
        if !ini.has_section(&name) {
            continue;
        }
        let player = controls.player_mut(index);
        player.enabled = true;
        read_port(&mut ini, &name, player, &mut warnings);
    }
    for leftover in ini.sections_with("port") {
        if !port_names().contains(&leftover) {
            warnings.push(format!(
                "[{leftover}]: the console has {} ports, from port1 to port{}",
                zeebx::input::PORTS,
                zeebx::input::PORTS
            ));
        }
    }
    settings.controls = controls;

    warnings.extend(ini.leftovers());
    LoadedConfig {
        settings,
        headless,
        // The caller is the one who knows where the text came from; `load` fixes this right after.
        origin: ConfigOrigin::Missing(where_to_create()),
        warnings,
    }
}

fn port_names() -> Vec<String> {
    (1..=zeebx::input::PORTS).map(|n| format!("port{n}")).collect()
}

/// A port: which device the console sees, which host controller feeds it, and the mapping.
fn read_port(ini: &mut Ini, section: &str, player: &mut Player, warnings: &mut Vec<String>) {
    // The controller comes **before** everything, because choosing it swaps the whole mapping
    // for its typical one — the same thing the desktop controls screen does. While it only
    // brought the axes, the buttons stayed on the factory keyboard, and without a window there is
    // no keyboard: the stick moved and no button responded, which users reported as "the
    // controller is not recognized". Read first, the rest of the section adjusts on top of it.
    if let Some(v) = ini.get(section, "controller") {
        let source_name = unquoted(&v.text);
        player.switch_controller(match source_name.is_empty() {
            true => None,
            false => Some(source_name.to_string()),
        });
    }

    let mut enabled = player.enabled;
    boolean(ini, section, "enabled", &mut enabled, warnings);
    player.enabled = enabled;

    // The D-pad mirrored onto the axes: the same setting as the checkbox on the desktop controls
    // screen and the core's `zeebx_dpad_to_analog_pN`. Without this line the windowless frontend
    // would have no way to turn it on — `Controls` would arrive with the default and nobody
    // would know why.
    let mut mirror = player.dpad_to_axes;
    boolean(ini, section, "dpad_to_analog", &mut mirror, warnings);
    player.dpad_to_axes = mirror;

    if let Some(v) = ini.get(section, "device") {
        match v.text.to_lowercase().as_str() {
            "gamepad" | "dragon" => player.device_kind = Device::Controller,
            "zpad" | "z-pad" => player.device_kind = Device::ZPad,
            "keyboard" => player.device_kind = Device::Keyboard,
            "boomerang" => player.device_kind = Device::Boomerang,
            other => warn(warnings, &v, &format!(
                "`{other}` is not a device; use gamepad, zpad, keyboard or boomerang"
            )),
        }
    }
    // The buttons. One key per console button, with the sources separated by commas. Writing
    // the key **replaces** that button's default: whoever redefines `b1` wants what they wrote,
    // not what they wrote plus the factory `Z`.
    let factory = Player::default();
    for button in zeebx::input::bindings::CONFIGURABLE {
        let Some(v) = ini.get(section, button) else {
            continue;
        };
        let mut origins = Vec::new();
        if !unquoted(&v.text).trim().is_empty() {
            for chunk in v.text.split(',') {
                match origin(chunk.trim()) {
                    Some(source) => origins.push(source),
                    None => warn(warnings, &v, &format!(
                        "`{}` is not a source; use key:NAME, button:NAME or axis:NAME+",
                        chunk.trim()
                    )),
                }
            }
        }
        // The exception: with a controller chosen, a line equal to the factory keyboard does
        // **not** replace anything. It is the line the generated file writes for every button,
        // and every `config.ini` already shipped has it; counting as a replacement, it would
        // erase the controller button that `controller` just brought in. Whoever really wants
        // only the keyboard on a button with a controller chosen writes the keys in another order
        // or another set.
        if player.device.is_some() && origins == factory.sources(button) {
            continue;
        }
        player.clear(button);
        for source in origins {
            player.bind(button, source);
        }
    }

    // The console's analog axes. `axis_x = LeftStickX` or `axis_y = LeftStickY:inverted`.
    for axis in zeebx::input::AXIS_NAMES {
        let key_id = format!("axis_{axis}");
        let Some(v) = ini.get(section, &key_id) else {
            continue;
        };
        let text = unquoted(&v.text).trim().to_string();
        if text.is_empty() {
            player.axes.remove(axis);
            continue;
        }
        let (source_name, invert) = match text.split_once(':') {
            Some((source_name, mark)) => {
                let invert = matches!(mark.trim().to_lowercase().as_str(), "inverted");
                if !invert {
                    warn(warnings, &v, &format!("`{mark}` is not `inverted`"));
                }
                (source_name.trim(), invert)
            }
            None => (text.as_str(), false),
        };
        player.axes.insert(
            axis.to_string(),
            AxisSource {
                name: source_name.to_string(),
                invert,
            },
        );
    }
}

/// `key:Z`, `button:South`, `axis:LeftStickY-`.
///
/// The names are **the same as in `settings.json`** on purpose: whoever copied a mapping from
/// the interface to here does not need to translate anything, and the desktop controls screen
/// remains the easiest way to find out what a button is called.
fn origin(text: &str) -> Option<Source> {
    let (kind, input_name) = text.split_once(':')?;
    let input_name = input_name.trim();
    if input_name.is_empty() {
        return None;
    }
    match kind.trim().to_lowercase().as_str() {
        "key" => Some(Source::key(input_name)),
        "button" => Some(Source::button(input_name)),
        "axis" => {
            // The sign comes stuck at the end, which is how the interface shows it: `LeftStickY-`.
            let (input_name, positive) = match input_name.strip_suffix('+') {
                Some(input_name) => (input_name, true),
                None => (input_name.strip_suffix('-')?, false),
            };
            match input_name.trim().is_empty() {
                true => None,
                false => Some(Source::Axis {
                    name: input_name.trim().to_string(),
                    positive,
                }),
            }
        }
        _ => None,
    }
}

fn warn(warnings: &mut Vec<String>, value: &Value, text: &str) {
    warnings.push(format!("line {}: {text}", value.line));
}

/// `true`, `yes`, `1`, `on` — and their opposites.
fn boolean(ini: &mut Ini, section: &str, key_id: &str, dest: &mut bool, warnings: &mut Vec<String>) {
    let Some(v) = ini.get(section, key_id) else {
        return;
    };
    match v.text.to_lowercase().as_str() {
        "true" | "yes" | "1" | "on" => *dest = true,
        "false" | "no" | "0" | "off" => *dest = false,
        other => warn(warnings, &v, &format!("`{key_id}` expects true or false, not `{other}`")),
    }
}

fn integer(ini: &mut Ini, section: &str, key_id: &str, dest: &mut u32, warnings: &mut Vec<String>) {
    let Some(v) = ini.get(section, key_id) else {
        return;
    };
    match v.text.parse::<u32>() {
        Ok(n) => *dest = n,
        Err(_) => warn(warnings, &v, &format!("`{key_id}` expects a number")),
    }
}

/// A small number, held to a range. Outside it is the writer's mistake, and stating the limit is
/// worth more than silently accepting an `internal_resolution = 40` that would not fit the GPU.
fn read_u8(
    ini: &mut Ini,
    section: &str,
    key_id: &str,
    dest: &mut u8,
    minimum: u8,
    maximum: u8,
    warnings: &mut Vec<String>,
) {
    let Some(v) = ini.get(section, key_id) else {
        return;
    };
    match v.text.parse::<u8>() {
        Ok(n) if (minimum..=maximum).contains(&n) => *dest = n,
        Ok(n) => warn(warnings, &v, &format!("`{key_id}` = {n} is outside {minimum}..{maximum}")),
        Err(_) => warn(warnings, &v, &format!("`{key_id}` expects a number")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeebx::ui::settings::WindowMode;

    /// An empty file has to give exactly the factory options: it is what makes a missing
    /// `config.ini` the same thing as a `config.ini` with nothing inside.
    #[test]
    fn an_empty_file_is_the_default() {
        let loaded = from_text("");
        // The interface's default is to open maximized, and the one here is to open at the size
        // the file asks for — a maximized window would make `width` and `height` mean nothing.
        let expected = Settings {
            graphics: Graphics {
                window: WindowMode::Window,
                game_window: WindowMode::Window,
                ..Graphics::default()
            },
            ..Settings::default()
        };
        assert_eq!(loaded.settings, expected);
        assert_eq!(loaded.headless.video, Video::Window);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    #[test]
    fn the_keys_reach_the_core_settings() {
        let loaded = from_text(
            "[video]\n\
             mode = fullscreen\n\
             scaling = integer\n\
             smooth = true\n\
             speed_limit = false\n\
             frameskip = auto\n\
             [graphics]\n\
             gpu_rasterizer = true\n\
             internal_resolution = 2\n\
             aspect = 16x9\n\
             [audio]\n\
             volume = 42\n\
             soundfont = \"/bancos/firmware.sf2\"\n\
             midi_effects = false\n",
        );
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let g = &loaded.settings.graphics;
        assert_eq!(loaded.headless.video, Video::Fullscreen);
        assert_eq!(g.game_window, WindowMode::Fullscreen);
        assert_eq!(g.scaling, Scaling::Integer);
        assert!(g.smooth);
        assert_eq!(loaded.settings.speed.fps_limit, FpsLimit::Off);
        assert_eq!(loaded.settings.speed.frameskip, Frameskip::Auto);
        assert!(g.gpu_rasterizer);
        assert_eq!(g.internal_resolution, 2);
        assert_eq!(g.ratio, AspectRatio::Wide16x9);
        assert_eq!(loaded.settings.audio.volume, 42);
        assert_eq!(
            loaded.settings.audio.soundfont.as_deref(),
            Some(std::path::Path::new("/bancos/firmware.sf2"))
        );
        assert_eq!(from_text("[audio]\nsoundfont = -\n").settings.audio.soundfont, None);
        assert!(!loaded.settings.audio.midi_effects);
    }

    /// `fps_limit` beats the old `speed_limit`, and an odd value warns without changing anything.
    #[test]
    fn the_frame_limit_and_the_frameskip() {
        let loaded = from_text("[video]\nspeed_limit = false\nfps_limit = 30\nframeskip = 3\n");
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(loaded.settings.speed.fps_limit, FpsLimit::Thirty);
        assert_eq!(loaded.settings.speed.frameskip, Frameskip::Fixed(3));

        let loaded = from_text("[video]\nfps_limit = 120\nframeskip = 9\n");
        assert_eq!(loaded.warnings.len(), 2, "{:?}", loaded.warnings);
        assert_eq!(loaded.settings.speed.fps_limit, FpsLimit::Sixty);
        assert_eq!(loaded.settings.speed.frameskip, Frameskip::Off);
    }

    /// Mentioning the section turns the port on, and a port not mentioned stays as it ships.
    #[test]
    fn the_port_section_enables_the_port() {
        let loaded = from_text("[port2]\ndevice = keyboard\n");
        let ports = &loaded.settings.controls;
        assert!(ports.player(0).unwrap().enabled, "the first port is the factory one");
        let second = ports.player(1).unwrap();
        assert!(second.enabled);
        assert_eq!(second.device_kind, Device::Keyboard);
    }

    /// The D-pad on the axes comes from `config.ini` like any other port key — and a port that
    /// does not write it keeps the default, which is off.
    #[test]
    fn dpad_to_axes_comes_from_the_ini_and_defaults_to_off() {
        let loaded = from_text("[port1]\ndpad_to_analog = yes\n");
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert!(loaded.settings.controls.player(0).unwrap().dpad_to_axes);
        assert!(
            !loaded.settings.controls.player(1).unwrap().dpad_to_axes,
            "the second port did not write it, and the default is off"
        );

        // And the factory value the generator writes comes back as the emulator's default: it is
        // what the test comparing the full file with `from_text("")` enforces.
        let fallback = from_text("[port1]\ndpad_to_analog = false\n");
        assert!(!fallback.settings.controls.player(0).unwrap().dpad_to_axes);
    }

    /// Writing a button replaces its default, and only its default.
    #[test]
    fn the_written_button_replaces_the_default() {
        let loaded = from_text("[port1]\nb1 = key:Q, button:North\n");
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let port = loaded.settings.controls.player(0).unwrap();
        assert_eq!(
            port.sources("b1"),
            [Source::key("Q"), Source::button("North")]
        );
        // `b2` was not mentioned: it keeps what it ships with.
        assert!(!port.sources("b2").is_empty());
    }

    /// A button with an empty line has no source at all — that is how a button is turned off.
    #[test]
    fn an_empty_button_has_no_source() {
        let loaded = from_text("[port1]\nb2 =\n");
        assert!(loaded.settings.controls.player(0).unwrap().sources("b2").is_empty());
    }

    #[test]
    fn the_axes_come_out_with_the_requested_direction() {
        let loaded = from_text("[port1]\naxis_y = LeftStickY:inverted\naxis_x = RightStickX\n");
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let port = loaded.settings.controls.player(0).unwrap();
        assert_eq!(port.axes["y"], AxisSource { name: "LeftStickY".into(), invert: true });
        assert_eq!(port.axes["x"], AxisSource { name: "RightStickX".into(), invert: false });
    }

    /// The axis sign as a button source: it is what lets a stick drive the D-pad.
    #[test]
    fn an_axis_source_carries_its_direction() {
        let loaded = from_text("[port1]\nup = axis:LeftStickY-\n");
        assert_eq!(
            loaded.settings.controls.player(0).unwrap().sources("up"),
            [Source::Axis { name: "LeftStickY".into(), positive: false }]
        );
    }

    /// An out-of-range value, a meaningless source and an unknown key have to **show up**: in
    /// silence, whoever edited the file thinks they configured something and did not.
    #[test]
    fn what_is_wrong_is_reported() {
        let loaded = from_text(
            "[graphics]\n\
             internal_resolution = 40\n\
             [video]\n\
             mode = square\n\
             [port1]\n\
             b1 = Z\n\
             [audio]\n\
             volumee = 10\n",
        );
        let all = loaded.warnings.join("\n");
        assert!(all.contains("internal_resolution"), "{all}");
        assert!(all.contains("square"), "{all}");
        assert!(all.contains("is not a source"), "{all}");
        assert!(all.contains("volumee"), "{all}");
        // And none of this keeps the rest from applying.
        assert_eq!(loaded.settings.graphics.internal_resolution, 1);
    }

    /// A controller chosen by name gets the default axes along with it, otherwise the stick stays
    /// silent and whoever wrote only the name would have no way to guess more was missing.
    #[test]
    fn the_chosen_controller_gets_the_axes() {
        let loaded = from_text("[port1]\ncontroller = \"Pad #2\"\n");
        let port = loaded.settings.controls.player(0).unwrap();
        assert_eq!(port.device.as_deref(), Some("Pad #2"));
        assert_eq!(port.axes, Player::default_axes());
    }

    /// The player's path: take the generated file, uncomment `controller` and nothing else. The
    /// controller buttons have to respond — without a window there is no keyboard, and a `b1`
    /// with only `key:Z` leaves the controller silent except for the stick.
    #[test]
    fn the_controller_chosen_in_the_generated_file_drives_the_buttons() {
        let text = model().replacen(
            "# controller = \"Xbox Wireless Controller\"",
            "controller = \"Xbox Wireless Controller\"",
            1,
        );
        let loaded = from_text(&text);
        let port = loaded.settings.controls.player(0).unwrap();
        assert_eq!(port.device.as_deref(), Some("Xbox Wireless Controller"));
        for button in zeebx::input::bindings::CONFIGURABLE {
            assert!(
                port.sources(button).iter().any(|s| matches!(s, Source::Button { .. } | Source::Axis { .. })),
                "`{button}` has no source on the controller: {:?}",
                port.sources(button)
            );
        }
    }

    /// With a controller chosen, a line that is not the factory one still counts as written: the
    /// generated file's exception must not swallow a real mapping.
    #[test]
    fn with_a_controller_the_changed_line_replaces_the_default() {
        let loaded = from_text("[port1]\ncontroller = \"Pad\"\nb1 = button:East\nb2 = key:X, key:Space\n");
        let port = loaded.settings.controls.player(0).unwrap();
        assert_eq!(port.sources("b1"), [Source::button("East")]);
        assert_eq!(port.sources("b2"), [Source::key("X"), Source::key("Space")]);
        assert!(port.sources("up").contains(&Source::button("DPadUp")));
    }

    /// The example `--example` prints has to be a file the reader accepts without complaint. An
    /// example that warns teaches the wrong thing.
    #[test]
    fn the_shipped_example_is_valid() {
        let loaded = from_text(&model());
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    /// **Every line of the generated file says what would already apply without it.** That is
    /// what lets it ship with everything written: whoever edits sees the default and changes it,
    /// instead of guessing what to write. A default that changes in the code without changing
    /// here fails this test.
    #[test]
    fn the_full_file_describes_its_own_defaults() {
        assert_eq!(from_text(&model()).settings, from_text("").settings);
    }

    /// And the ports come out complete: both sections, with thirteen buttons each.
    #[test]
    fn the_ports_come_out_with_every_button() {
        let text = model();
        for port in 1..=zeebx::input::PORTS {
            assert!(text.contains(&format!("[port{port}]")), "port {port} is missing");
        }
        let loaded = from_text(&text);
        for index in 0..zeebx::input::PORTS {
            let player = loaded.settings.controls.player(index).unwrap();
            for button in zeebx::input::bindings::CONFIGURABLE {
                assert!(
                    !player.sources(button).is_empty(),
                    "port {index}: button {button} came out with no source"
                );
            }
        }
        // The second stays off: it shows up to be edited, not to be turned on.
        assert!(!loaded.settings.controls.player(1).unwrap().enabled);
    }
}

#[cfg(test)]
mod creation_tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zeebx-headless-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// What the first run writes has to read back without any warning, and give exactly the
    /// factory options. A created file that is born complaining would be worse than none.
    #[test]
    fn the_created_file_reads_back_without_warnings() {
        let path = dir("cria").join("config.ini");
        create(&path).unwrap();

        let loaded = load(Some(&path));
        assert!(matches!(loaded.origin, ConfigOrigin::Read));
        // The first warning is the "reading ..."; no other.
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
        assert_eq!(loaded.settings, from_text("").settings);
        // And it comes out straight: a file born crooked teaches crookedness to whoever edits it.
        let text = std::fs::read_to_string(&path).unwrap();
        for line in text.lines() {
            assert!(
                !line.starts_with(' ') && !line.starts_with('\t'),
                "indented line in the generated file: {line:?}"
            );
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// **Never over it.** A `config.ini` full of settings is the work of whoever wrote it.
    #[test]
    fn does_not_overwrite_what_exists() {
        let path = dir("existente").join("config.ini");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[audio]\nvolume = 7\n").unwrap();

        assert!(create(&path).is_err());
        assert_eq!(load(Some(&path)).settings.audio.volume, 7);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A `--config=` pointing at something that does not exist says it is missing at **that**
    /// path: that is where the file should be born, not in the system configuration folder.
    #[test]
    fn the_requested_config_is_where_the_file_is_created() {
        let path = dir("pedido").join("meu.ini");
        match load(Some(&path)).origin {
            ConfigOrigin::Missing(location) => assert_eq!(location, path),
            ConfigOrigin::Read => panic!("there should be no file"),
        }
    }
}
