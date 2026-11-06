//! The settings for QML, and the interface language.
//!
//! **A key, not one property per option.** There are dozens of options, and each one would be a
//! property with a getter, a setter and a signal. QML reads and writes by key — the path in
//! `settings.json`, such as `graphics.smooth` —, and the `version` number changes on every write:
//! the bindings that read it are re-evaluated. Every write saves the file and applies the effect
//! right away, as in egui: the volume goes to the open game, the internal resolution rebuilds the
//! target on the GPU.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qrectf.h");
        type QRectF = cxx_qt_lib::QRectF;
    }

    extern "RustQt" {
        /// The interface language. The QML bindings read `version` through each file's `tr()`:
        /// switching the language changes it, and every text is re-evaluated right away, as in
        /// egui.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(i32, version)]
        type Language = super::LanguageRust;

        /// A text from the catalog, by key.
        #[qinvokable]
        fn text(self: &Language, key_id: &QString) -> QString;

        /// The language changed: every text is re-evaluated.
        #[qinvokable]
        fn reload(self: Pin<&mut Language>);
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(i32, version)]
        #[qproperty(i32, edited_port, cxx_name = "editedPort")]
        type SettingsModel = super::SettingsModelRust;

        /// The value of an option: `bool`, number, or text. For list choices, the index.
        #[qinvokable]
        fn value(self: &SettingsModel, key_id: &QString) -> QVariant;

        /// Starts editing the settings of the game of `key_id`; empty goes back to the global ones.
        /// See [`super::emu_core::Core::with_game`].
        #[qinvokable]
        #[cxx_name = "editGame"]
        fn edit_game(self: Pin<&mut SettingsModel>, key_id: &QString);

        /// Writes an option, saves the file and applies the effect.
        #[qinvokable]
        fn define(self: Pin<&mut SettingsModel>, key_id: &QString, value: &QVariant);

        /// The labels of a list option's choices, already translated, in index order.
        #[qinvokable]
        fn options(self: &SettingsModel, key_id: &QString) -> QStringList;

        #[qinvokable]
        #[cxx_name = "chooseRomsDir"]
        fn choose_roms_dir(self: Pin<&mut SettingsModel>);

        /// Points to the Z-Wheel: the `.zip` or the `.mod` with `dir` false, or the folder that
        /// holds it.
        /// The actual screenshots folder: the chosen one, or the default.
        #[qinvokable]
        #[cxx_name = "screenshotsDir"]
        fn screenshots_dir(self: &SettingsModel) -> QString;

        #[qinvokable]
        #[cxx_name = "chooseScreenshotsDir"]
        fn choose_screenshots_dir(self: Pin<&mut SettingsModel>);

        /// Goes back to the default screenshots folder.
        #[qinvokable]
        #[cxx_name = "useDefaultScreenshotsDir"]
        fn use_default_screenshots_dir(self: Pin<&mut SettingsModel>);

        #[qinvokable]
        #[cxx_name = "screenshotsInDefaultDir"]
        fn screenshots_in_default_dir(self: &SettingsModel) -> bool;

        /// The `.sf2` bank for MIDI music as text: the chosen path, or the automatic search
        /// phrase.
        #[qinvokable]
        #[cxx_name = "soundfont"]
        fn soundfont(self: &SettingsModel) -> QString;

        #[qinvokable]
        #[cxx_name = "chooseSoundfont"]
        fn choose_soundfont(self: Pin<&mut SettingsModel>);

        /// Goes back to the automatic search: the first `.sf2` in the device's `soundfonts`
        /// folder.
        #[qinvokable]
        #[cxx_name = "useAutoSoundfont"]
        fn use_auto_soundfont(self: Pin<&mut SettingsModel>);

        #[qinvokable]
        #[cxx_name = "autoSoundfont"]
        fn auto_soundfont(self: &SettingsModel) -> bool;

        /// The `file://` address of the screenshots folder, created if it does not exist yet: the
        /// open button has to work before the first screenshot.
        #[qinvokable]
        #[cxx_name = "screenshotsDirUrl"]
        fn screenshots_dir_url(self: &SettingsModel) -> QString;

        /// The `file://` address of the recorded sessions folder, created if it does not exist yet.
        #[qinvokable]
        #[cxx_name = "sessionsDirUrl"]
        fn sessions_dir_url(self: &SettingsModel) -> QString;

        #[qinvokable]
        #[cxx_name = "chooseZWheel"]
        fn choose_z_wheel(self: Pin<&mut SettingsModel>, dir: bool);

        /// Looks for the Z-Wheel without asking: in the ROMs folder, next to it, in `Downloads`.
        #[qinvokable]
        #[cxx_name = "detectZWheel"]
        fn detect_z_wheel(self: Pin<&mut SettingsModel>);

        /// What to say about the Z-Wheel pointed to: found with covers, found without, or not
        /// found.
        #[qinvokable]
        #[cxx_name = "zWheelStatus"]
        fn z_wheel_status(self: &SettingsModel) -> QString;

        #[qinvokable]
        #[cxx_name = "zWheelFound"]
        fn z_wheel_found(self: &SettingsModel) -> bool;

        /// The configured ROMs folder no longer exists.
        #[qinvokable]
        #[cxx_name = "romsDirMissing"]
        fn roms_dir_missing(self: &SettingsModel) -> bool;

        #[qinvokable]
        #[cxx_name = "checkForUpdates"]
        fn check_for_updates(self: Pin<&mut SettingsModel>);

        #[qinvokable]
        #[cxx_name = "checkingForUpdate"]
        fn checking_for_update(self: &SettingsModel) -> bool;

        /// What the new-version check answered, as text; empty before checking.
        #[qinvokable]
        #[cxx_name = "updateStatus"]
        fn update_status(self: &SettingsModel) -> QString;

        /// The new version's page, if there is one.
        #[qinvokable]
        #[cxx_name = "updatePage"]
        fn update_page(self: &SettingsModel) -> QString;

        /// This copy downloads and installs the new version by itself (AppImage, Windows
        /// installer, `.app`), instead of just opening the page.
        #[qinvokable]
        #[cxx_name = "selfUpdates"]
        fn self_updates(self: &SettingsModel) -> bool;

        /// Brings back, in the main window, the new-version notice: that is where the installation
        /// proceeds.
        #[qinvokable]
        #[cxx_name = "showUpdateNotice"]
        fn show_update_notice(self: &SettingsModel);

        #[qinvokable]
        #[cxx_name = "discordConnected"]
        fn discord_connected(self: &SettingsModel) -> bool;

        /// "Settings are stored in …", with the file's path.
        #[qinvokable]
        #[cxx_name = "settingsLocation"]
        fn settings_location(self: &SettingsModel) -> QString;

        #[qinvokable]
        #[cxx_name = "emulatorVersion"]
        fn emulator_version(self: &SettingsModel) -> QString;

        /// The repository's address, or the chat server's invite, for the About tab.
        #[qinvokable]
        fn address(self: &SettingsModel, which: &QString) -> QString;

        // ---- The controls tab, for the port being edited --------------------------------------

        /// Switches the port being edited. An open capture is abandoned: leaving it would map the
        /// next key to the previous port's button.
        #[qinvokable]
        #[cxx_name = "editPort"]
        fn edit_port(self: Pin<&mut SettingsModel>, port: i32);

        #[qinvokable]
        #[cxx_name = "portEnabled"]
        fn port_enabled(self: &SettingsModel) -> bool;

        #[qinvokable]
        #[cxx_name = "setPortEnabled"]
        fn set_port_enabled(self: Pin<&mut SettingsModel>, enabled: bool);

        /// What the console sees on the port: 0 Z-Pad, 1 controller, 2 Boomerang, 3 keyboard.
        #[qinvokable]
        #[cxx_name = "deviceKind"]
        fn device_kind(self: &SettingsModel) -> i32;

        #[qinvokable]
        #[cxx_name = "setDevice"]
        fn set_device(self: Pin<&mut SettingsModel>, device_kind: i32);

        /// The host controllers the port can read: "none" first, then the `gilrs` ones and the
        /// Wii Remotes.
        #[qinvokable]
        fn controls(self: &SettingsModel) -> QStringList;

        /// The index of the port's controller in `controls`.
        #[qinvokable]
        #[cxx_name = "currentController"]
        fn current_controller(self: &SettingsModel) -> i32;

        #[qinvokable]
        #[cxx_name = "chooseController"]
        fn choose_controller(self: Pin<&mut SettingsModel>, index: i32);

        #[qinvokable]
        #[cxx_name = "resetMapping"]
        fn reset_mapping(self: Pin<&mut SettingsModel>);

        /// The Zeebo buttons that can be mapped, by internal name.
        #[qinvokable]
        fn buttons(self: &SettingsModel) -> QStringList;

        /// Where a button comes from, as text: "Z, South", or "unassigned".
        #[qinvokable]
        #[cxx_name = "buttonSources"]
        fn button_sources(self: &SettingsModel, button: &QString) -> QString;

        /// The button waiting for a key or a button, or empty.
        #[qinvokable]
        fn capturing(self: &SettingsModel) -> QString;

        /// Starts waiting for a source for the button; again on the same one, gives up.
        #[qinvokable]
        fn capture(self: Pin<&mut SettingsModel>, button: &QString);

        /// What is bound to a shortcut (`screenshot`, `fast_forward`), by name.
        #[qinvokable]
        fn shortcut(self: &SettingsModel, which: &QString) -> QString;

        /// Starts waiting for a shortcut's key — or button, for fast-forward —; again, gives
        /// up.
        #[qinvokable]
        #[cxx_name = "captureShortcut"]
        fn capture_shortcut(self: Pin<&mut SettingsModel>, which: &QString);

        #[qinvokable]
        #[cxx_name = "capturingShortcut"]
        fn capturing_shortcut(self: &SettingsModel, which: &QString) -> bool;

        /// Resets a shortcut to the factory one.
        #[qinvokable]
        #[cxx_name = "resetShortcut"]
        fn reset_shortcut(self: Pin<&mut SettingsModel>, which: &QString);

        /// The factory shortcut, as displayed: the save state one changes with the system.
        #[qinvokable]
        #[cxx_name = "defaultShortcut"]
        fn default_shortcut(self: &SettingsModel, which: &QString) -> QString;

        /// The per-frame read of the shortcuts tab: a controller button for the open capture.
        #[qinvokable]
        #[cxx_name = "readControllerShortcut"]
        fn read_controller_shortcut(self: Pin<&mut SettingsModel>);

        /// Why the last captured key was rejected, if it was: `shortcut` says whether the capture
        /// was the screenshot's or a Zeebo button's, because each one shows the reason in its own
        /// place.
        #[qinvokable]
        fn rejection(self: &SettingsModel, shortcut: bool) -> QString;

        /// What to say when a capture is waiting and no controller sends anything: empty while
        /// that is not the case. See [`SettingsModel::capture_hint`].
        #[qinvokable]
        #[cxx_name = "captureHint"]
        fn capture_hint(self: &SettingsModel) -> QString;

        #[qinvokable]
        #[cxx_name = "clearButton"]
        fn clear_button(self: Pin<&mut SettingsModel>, button: &QString);

        /// A `Qt::Key` in the settings window: lights up the button in the art, and with a capture
        /// open becomes its source. Esc gives up the capture.
        /// `modifiers` are the event's `Qt::KeyboardModifiers`: they count when capturing the save
        /// state shortcuts, which accept combinations.
        #[qinvokable]
        fn key(self: Pin<&mut SettingsModel>, code: i32, modifiers: i32, pressed: bool);

        /// The per-frame read of the tab: the controller, the capture by controller button and the
        /// calibration in progress.
        #[qinvokable]
        #[cxx_name = "readController"]
        fn read_controller(self: Pin<&mut SettingsModel>);

        #[qinvokable]
        fn pressed(self: &SettingsModel, button: &QString) -> bool;

        /// The position of one of the port's axes, from -1 to 1, live.
        #[qinvokable]
        fn axis(self: &SettingsModel, index: i32) -> f64;

        /// The console's axes, by internal name.
        #[qinvokable]
        fn axes(self: &SettingsModel) -> QStringList;

        /// The sources an axis accepts: "none" first, then the controller's axes.
        #[qinvokable]
        #[cxx_name = "axisSources"]
        fn axis_sources(self: &SettingsModel) -> QStringList;

        #[qinvokable]
        #[cxx_name = "axisSource"]
        fn axis_source(self: &SettingsModel, axis: &QString) -> i32;

        #[qinvokable]
        #[cxx_name = "setAxisSource"]
        fn set_axis_source(self: Pin<&mut SettingsModel>, axis: &QString, index: i32);

        #[qinvokable]
        #[cxx_name = "axisInverted"]
        fn axis_inverted(self: &SettingsModel, axis: &QString) -> bool;

        #[qinvokable]
        #[cxx_name = "invertAxis"]
        fn invert_axis(self: Pin<&mut SettingsModel>, axis: &QString, inverted: bool);

        /// Width over height of the controller art, or 0 without art.
        #[qinvokable]
        #[cxx_name = "artAspectRatio"]
        fn art_aspect_ratio(self: &SettingsModel) -> f64;

        #[qinvokable]
        fn parts(self: &SettingsModel) -> QStringList;

        /// Where a button's silhouette sits in the art, from 0 to 1.
        #[qinvokable]
        #[cxx_name = "partBounds"]
        fn part_bounds(self: &SettingsModel, index: i32) -> QRectF;

        /// The button under the point, by silhouette, not by a box: a cross-shaped D-pad and a
        /// round button respond only where there is a button.
        #[qinvokable]
        #[cxx_name = "buttonAt"]
        fn button_at(self: &SettingsModel, u: f64, v: f64) -> QString;

        /// The port's motion sensor, as text.
        #[qinvokable]
        fn sensor(self: &SettingsModel) -> QString;

        #[qinvokable]
        #[cxx_name = "sensorLacksPermission"]
        fn sensor_lacks_permission(self: &SettingsModel) -> bool;

        #[qinvokable]
        #[cxx_name = "hasReading"]
        fn has_reading(self: &SettingsModel) -> bool;

        /// The acceleration, the steering angle and what is pressed, on one line.
        #[qinvokable]
        #[cxx_name = "motionLine"]
        fn motion_line(self: &SettingsModel) -> QString;

        /// How much the Boomerang image rotates, in degrees.
        #[qinvokable]
        fn rotation(self: &SettingsModel) -> f64;

        #[qinvokable]
        fn calibrating(self: &SettingsModel) -> bool;

        #[qinvokable]
        #[cxx_name = "calibrationRejected"]
        fn calibration_rejected(self: &SettingsModel) -> bool;

        #[qinvokable]
        fn calibrate(self: Pin<&mut SettingsModel>);

        #[qinvokable]
        #[cxx_name = "resetCalibration"]
        fn reset_calibration(self: Pin<&mut SettingsModel>);

        /// Asks for the system password and writes the udev rule that unlocks the sensors.
        #[qinvokable]
        #[cxx_name = "unlockSensors"]
        fn unlock_sensors(self: Pin<&mut SettingsModel>);

        /// 0 idle, 1 asking for the password, 2 done, 3 failed.
        #[qinvokable]
        #[cxx_name = "grantStatus"]
        fn grant_status(self: &SettingsModel) -> i32;

        /// What to say when the unlock failed, with the rule's file.
        #[qinvokable]
        #[cxx_name = "grantFailure"]
        fn grant_failure(self: &SettingsModel) -> QString;

        /// The udev rule, to be created by hand when the request fails.
        #[qinvokable]
        #[cxx_name = "udevRule"]
        fn udev_rule(self: &SettingsModel) -> QString;

        /// The ROMs folder or the Z-Wheel changed: the library rebuilds the list, scanning the disk
        /// again when `scan`.
        #[qsignal]
        #[cxx_name = "libraryChanged"]
        fn library_changed(self: Pin<&mut SettingsModel>, scan: bool);

        #[qsignal]
        #[cxx_name = "languageChanged"]
        fn language_changed(self: Pin<&mut SettingsModel>);

        /// The theme changed. The palette has already been applied; QML still has to reread the
        /// warning and error colors.
        #[qsignal]
        #[cxx_name = "themeChanged"]
        fn theme_changed(self: Pin<&mut SettingsModel>);

        /// The main window's mode changed: it applies right away, as in egui. The game window's
        /// applies on the next game.
        #[qsignal]
        #[cxx_name = "mainWindowChanged"]
        fn main_window_changed(self: Pin<&mut SettingsModel>);
    }
}

use std::collections::HashSet;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QList, QRectF, QString, QStringList, QVariant};

use zeebx::egui::Key;
use zeebx::input::bindings::{Device, AxisSource, CONFIGURABLE, MotionCalibration, Source};
use zeebx::input::gamepads;
use zeebx::input::sensors;
use zeebx::input::wiimote::Wiimotes;
use zeebx::input::{AXIS_RANGE, AXIS_NAMES, Pad};
use zeebx::ui::host_input::PortSensor;

use zeebx::library;
use zeebx::ui::updater::{self, Response};
use zeebx::ui::running_game;
use zeebx::ui::screenshot;
use zeebx::ui::settings::{
    self, Shortcuts, KeyCombo, Modifiers, LibraryMode, WindowMode, AspectRatio, Scaling,
    Settings, resolution_label, level_label,
};

use zeebx::speed::rewind::RewindSettings;
use zeebx::speed::turbo::{self, TurboMode};
use zeebx::speed::{FastForward, Frameskip, FpsLimit, ShortcutMode};

use super::emu_core;

#[derive(Default)]
pub struct LanguageRust {
    version: i32,
}

impl qobject::Language {
    pub fn text(&self, key_id: &QString) -> QString {
        let key_id = String::from(key_id);
        emu_core::with(|emu_core| QString::from(emu_core.catalog.get(&key_id)))
    }

    pub fn reload(mut self: Pin<&mut Self>) {
        let version = self.version().wrapping_add(1);
        self.as_mut().set_version(version);
    }
}

#[derive(Default)]
pub struct SettingsModelRust {
    version: i32,
    edited_port: i32,
    /// The button waiting for a key or a controller button.
    capturing: Option<String>,
    /// The shortcut waiting for a key. Separate from `capturing` because `read_controller` belongs
    /// to the port being edited, and a shortcut belongs to no port.
    capturing_shortcut: Option<Shortcut>,
    /// The last rejected key: whether it was the shortcut capture, and the reason already
    /// translated.
    rejection: Option<(bool, String)>,
    /// When the open capture started. See [`qobject::SettingsModel::capture_hint`].
    capture_since: Option<std::time::Instant>,
    /// The keys pressed in the settings window, to light up the art.
    key_events: HashSet<Key>,
    /// The calibration in progress: the port and the still readings gathered so far.
    calibrating: Option<(usize, Vec<[f32; 3]>)>,
    /// The key of the game whose settings this window edits; empty, the global ones. See
    /// [`qobject::SettingsModel::with`].
    game: String,
    /// The last calibration was rejected because the device was not face up.
    calibration_rejected: bool,
    /// The permission request for the sensors, which runs on a thread while the system's
    /// password window is open.
    release: Arc<Mutex<SensorGrant>>,
}

/// The permission request for the sensors.
#[derive(Debug, Clone, Default)]
enum SensorGrant {
    #[default]
    Stop,
    /// The system's password window is open.
    Requesting,
    Done,
    Failed(String),
}

/// How long a capture waits, with no controller signal at all, before the window suggests what to
/// try.
const HINT_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

/// A rebindable shortcut of the game window, by the name QML uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    /// Keyboard only: see `docs/implementation/22-screenshots.md`.
    Screenshot,
    /// Key or button of any controller: see `docs/implementation/24-speed.md`.
    FastForward,
    /// Like fast-forward.
    Rewind,
    /// Restart the open game: keyboard only, like the screenshot.
    Reset,
    /// The save state ones: keyboard only, with or without a modifier. See [`KeyCombo`].
    SaveState,
    LoadState,
    NextSlot,
    PreviousSlot,
    UndoLoad,
}

impl Shortcut {
    fn from_name(name: &QString) -> Option<Self> {
        match String::from(name).as_str() {
            "screenshot" => Some(Self::Screenshot),
            "avancar" => Some(Self::FastForward),
            "voltar" => Some(Self::Rewind),
            "reiniciar" => Some(Self::Reset),
            "salvar_estado" => Some(Self::SaveState),
            "carregar_estado" => Some(Self::LoadState),
            "proximo_slot" => Some(Self::NextSlot),
            "slot_anterior" => Some(Self::PreviousSlot),
            "desfazer_carregar" => Some(Self::UndoLoad),
            _ => None,
        }
    }

    /// The catalog key of the shortcut's name.
    fn key_id(self) -> &'static str {
        match self {
            Self::Screenshot => "controls.shortcut.screenshot",
            Self::FastForward => "controls.shortcut.fast_forward",
            Self::Rewind => "controls.shortcut.rewind",
            Self::Reset => "controls.shortcut.restart",
            Self::SaveState => "controls.shortcut.save_state",
            Self::LoadState => "controls.shortcut.load_state",
            Self::NextSlot => "controls.shortcut.next_slot",
            Self::PreviousSlot => "controls.shortcut.previous_slot",
            Self::UndoLoad => "controls.shortcut.undo_load",
        }
    }

    /// The stored text of a save state shortcut, which is a [`KeyCombo`]. `None` for the others.
    fn combo(self, shortcuts: &Shortcuts) -> Option<&String> {
        Some(match self {
            Self::SaveState => &shortcuts.save_state,
            Self::LoadState => &shortcuts.load_state,
            Self::NextSlot => &shortcuts.next_slot,
            Self::PreviousSlot => &shortcuts.previous_slot,
            Self::UndoLoad => &shortcuts.undo_load,
            Self::Screenshot | Self::FastForward | Self::Rewind | Self::Reset => return None,
        })
    }

    fn combo_mut(self, shortcuts: &mut Shortcuts) -> Option<&mut String> {
        Some(match self {
            Self::SaveState => &mut shortcuts.save_state,
            Self::LoadState => &mut shortcuts.load_state,
            Self::NextSlot => &mut shortcuts.next_slot,
            Self::PreviousSlot => &mut shortcuts.previous_slot,
            Self::UndoLoad => &mut shortcuts.undo_load,
            Self::Screenshot | Self::FastForward | Self::Rewind | Self::Reset => return None,
        })
    }

    /// The factory one, in the text that gets stored.
    fn fallback(self) -> &'static str {
        match self {
            Self::Screenshot => Shortcuts::DEFAULT_SCREENSHOT,
            Self::FastForward => Shortcuts::DEFAULT_FAST_FORWARD,
            Self::Rewind => Shortcuts::DEFAULT_REWIND,
            Self::Reset => Shortcuts::DEFAULT_RESET,
            Self::SaveState => Shortcuts::DEFAULT_SAVE_STATE,
            Self::LoadState => Shortcuts::DEFAULT_LOAD_STATE,
            Self::NextSlot => Shortcuts::DEFAULT_NEXT_SLOT,
            Self::PreviousSlot => Shortcuts::DEFAULT_PREVIOUS_SLOT,
            Self::UndoLoad => Shortcuts::DEFAULT_UNDO_LOAD,
        }
    }

    /// Whether the key already belongs to this shortcut.
    fn has_key(self, shortcuts: &Shortcuts, key: Key) -> bool {
        match self {
            Self::Screenshot => Key::from_name(&shortcuts.screenshot) == Some(key),
            Self::FastForward => shortcuts.fast_forward.iter().any(|source| is_key(source, key)),
            Self::Rewind => shortcuts.rewind.iter().any(|source| is_key(source, key)),
            Self::Reset => Key::from_name(&shortcuts.restart) == Some(key),
            // A combination only takes the key when it is the key alone: Shift+F2 leaves F2 free,
            // because in the game window the combination only matches with the same modifiers.
            _ => self
                .combo(shortcuts)
                .and_then(|text| KeyCombo::read(text))
                .is_some_and(|combo| {
                    combo.modifiers.is_empty() && combo.key == key
                }),
        }
    }

    /// The sources of a shortcut that accepts a key or a button. The screenshot and the restart
    /// are not such shortcuts.
    fn origins(self, shortcuts: &mut Shortcuts) -> Option<&mut Vec<Source>> {
        match self {
            Self::FastForward => Some(&mut shortcuts.fast_forward),
            Self::Rewind => Some(&mut shortcuts.rewind),
            _ => None,
        }
    }
}

const SHORTCUTS: [Shortcut; 9] = [
    Shortcut::Screenshot,
    Shortcut::FastForward,
    Shortcut::Rewind,
    Shortcut::Reset,
    Shortcut::SaveState,
    Shortcut::LoadState,
    Shortcut::NextSlot,
    Shortcut::PreviousSlot,
    Shortcut::UndoLoad,
];

/// The shortcut that reserves the source: fast-forward or rewind.
fn shortcut_for_source(shortcuts: &Shortcuts, origin: &Source) -> Shortcut {
    match shortcuts.rewind.contains(origin) {
        true => Shortcut::Rewind,
        false => Shortcut::FastForward,
    }
}

/// Whether the source is the key. By the key, never by the text: see `ui::host_input::key_pressed`.
fn is_key(source: &Source, key: Key) -> bool {
    matches!(source, Source::Key { name } if Key::from_name(name) == Some(key))
}

/// How many still readings the motion calibration gathers: half a second at 100 per second.
const CALIBRATION_SAMPLES: usize = 50;

/// The devices the port can be, in the order of QML's indices.
const DEVICES: [Device; 4] = [Device::ZPad, Device::Controller, Device::Boomerang, Device::Keyboard];

/// The antialias and anisotropic levels the graphics tab offers, as in egui.
const ANTIALIAS: [u8; 4] = [1, 2, 4, 8];
const ANISOTROPIC: [u8; 5] = [1, 2, 4, 8, 16];

/// What a write asks for besides saving.
#[derive(Default)]
struct Effect {
    library: Option<bool>,
    language: bool,
    main_window: bool,
    theme: bool,
}

/// The on/off options, by key.
fn boolean<'a>(settings: &'a mut Settings, key_id: &str) -> Option<&'a mut bool> {
    Some(match key_id {
        "z_wheel.fim_de_vida" => &mut settings.z_wheel.end_of_life,
        "z_wheel.transicoes_sempre" => &mut settings.z_wheel.always_transitions,
        "discord.ativo" => &mut settings.discord.active,
        "atualizacoes.ao_abrir" => &mut settings.updates.on_launch,
        "atualizacoes.pre_lancamentos" => &mut settings.updates.prereleases,
        "graphics.smooth" => &mut settings.graphics.smooth,
        "graphics.jogo_separado" => &mut settings.graphics.separate_game_window,
        "graphics.confirmar_ao_fechar" => &mut settings.graphics.confirm_on_close,
        "graphics.keep_aspect" => &mut settings.graphics.keep_aspect,
        "graphics.neblina" => &mut settings.graphics.fog,
        "graphics.gpu_rasterizer" => &mut settings.graphics.gpu_rasterizer,
        "audio.enabled" => &mut settings.audio.enabled,
        "audio.midi_effects" => &mut settings.audio.midi_effects,
        "debug.overlay" => &mut settings.debug.overlay,
        "debug.speed" => &mut settings.debug.speed,
        "debug.clock" => &mut settings.debug.clock,
        "debug.memory" => &mut settings.debug.memory,
        "debug.timeline" => &mut settings.debug.timeline,
        "debug.metricas" => &mut settings.debug.metrics,
        "debug.log" => &mut settings.debug.log,
        "debug.gravar_sessao" => &mut settings.debug.record_session,
        "movimento.aviso_de_calibracao" => &mut settings.motion.calibration_notice,
        "barras.ferramentas" => &mut settings.bars.tools,
        "barras.status" => &mut settings.bars.status,
        "speed.ff.mute" => &mut settings.speed.fast_forward.mute,
        "speed.rewind.on" => &mut settings.speed.rewind.enabled,
        _ => return None,
    })
}

fn index<T: PartialEq>(options: &[T], value: &T) -> i32 {
    options.iter().position(|option| option == value).unwrap_or(0) as i32
}

fn choice<T: Copy>(options: &[T], index: i32) -> Option<T> {
    usize::try_from(index).ok().and_then(|i| options.get(i).copied())
}

impl qobject::SettingsModel {
    pub fn value(&self, key_id: &QString) -> QVariant {
        let key_id = String::from(key_id);
        if key_id == "jogo.personalizado" {
            let game = &self.rust().game;
            return QVariant::from(&emu_core::with(|emu_core| emu_core.custom(game)));
        }
        self.with(|emu_core| {
            let path = |path: &Option<PathBuf>| {
                QVariant::from(&QString::from(
                    &path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                ))
            };
            if let Some(value) = boolean(&mut emu_core.settings, &key_id) {
                return QVariant::from(&*value);
            }
            let graphics = &emu_core.settings.graphics;
            let number = match key_id.as_str() {
                "roms_dir" => return path(&emu_core.settings.roms_dir),
                "z_wheel_path" => return path(&emu_core.settings.z_wheel_path),
                "language" => {
                    let current = emu_core.catalog.current();
                    emu_core
                        .catalog
                        .languages()
                        .iter()
                        .position(|language| language.code == current)
                        .unwrap_or(0) as i32
                }
                "biblioteca" => index(
                    &[LibraryMode::Grid, LibraryMode::Slider],
                    &emu_core.settings.library,
                ),
                // Zero is the system palette; the themes come after, in the selector's order.
                "tema" => emu_core
                    .theme()
                    .and_then(|theme| emu_core.themes.iter().position(|t| t.key_id == theme.key_id))
                    .map_or(0, |position| position as i32 + 1),
                "graphics.janela" => index(&WindowMode::ALL, &graphics.window),
                "graphics.janela_do_jogo" => index(&WindowMode::ALL, &graphics.game_window),
                "graphics.scaling" => index(&Scaling::ALL, &graphics.scaling),
                "graphics.proporcao" => index(&AspectRatio::ALL, &graphics.ratio),
                "graphics.resolucao_interna" => i32::from(graphics.internal_resolution.clamp(1, 6)) - 1,
                "graphics.antialias" => index(&ANTIALIAS, &graphics.antialias),
                "graphics.anisotropico" => index(&ANISOTROPIC, &graphics.anisotropic),
                "audio.volume" => i32::from(emu_core.settings.audio.volume),
                "speed.fps_limit" => index(&FpsLimit::ALL, &emu_core.settings.speed.fps_limit),
                "speed.frameskip" => index(&Frameskip::all(), &emu_core.settings.speed.frameskip),
                "speed.ff.ratio" => index(&FastForward::choices(), &emu_core.settings.speed.fast_forward.ratio),
                "speed.rewind.interval" => {
                    index(&RewindSettings::INTERVALS_MS, &emu_core.settings.speed.rewind.interval_ms)
                }
                "speed.rewind.memory" => {
                    index(&RewindSettings::MEMORY_SIZES_MB, &emu_core.settings.speed.rewind.memory_mb)
                }
                "turbo.rate" => i32::from(
                    emu_core.settings.speed.turbo_per_second.clamp(turbo::MIN_PRESSES, turbo::MAX_PRESSES)
                        - turbo::MIN_PRESSES,
                ),
                // The turbo mode and button belong to the port being edited.
                "turbo.mode" | "turbo.button" => {
                    let player = emu_core.settings.controls.player(self.port());
                    match key_id.as_str() {
                        "turbo.mode" => {
                            index(&TurboMode::ALL, &player.map(|j| j.turbo).unwrap_or_default())
                        }
                        _ => {
                            let button = player.map(|j| j.turbo_button.as_str()).unwrap_or_default();
                            index(&turbo::PULSABLE, &button)
                        }
                    }
                }
                "speed.ff.mode" => index(&ShortcutMode::ALL, &emu_core.settings.speed.fast_forward.mode),
                _ => {
                    eprintln!("unknown setting: {key_id}");
                    return QVariant::default();
                }
            };
            QVariant::from(&number)
        })
    }

    pub fn define(mut self: Pin<&mut Self>, key_id: &QString, value: &QVariant) {
        let key_id = String::from(key_id);
        let enabled = value.value::<bool>().unwrap_or_default();
        let number = value.value::<i32>().unwrap_or_default();
        let port = self.port();
        // Turning the game's own on and off happens outside the swap: the swap is what decides
        // what gets edited.
        if key_id == "jogo.personalizado" {
            let game = self.rust().game.clone();
            emu_core::with(|emu_core| emu_core.customize(&game, enabled));
            self.as_mut().apply(Effect::default());
            return;
        }
        let effect = self.with(|emu_core| {
            let mut effect = Effect::default();
            if let Some(field) = boolean(&mut emu_core.settings, &key_id) {
                *field = enabled;
            }
            let graphics = &mut emu_core.settings.graphics;
            match key_id.as_str() {
                "language" => {
                    let code = usize::try_from(number)
                        .ok()
                        .and_then(|i| emu_core.catalog.languages().get(i))
                        .map(|language| language.code.clone());
                    if let Some(code) = code {
                        effect.language = emu_core.set_language(&code);
                        effect.library = Some(false);
                    }
                }
                "biblioteca" => {
                    let modes = [LibraryMode::Grid, LibraryMode::Slider];
                    emu_core.settings.library = choice(&modes, number).unwrap_or_default();
                }
                "tema" => {
                    emu_core.settings.theme = usize::try_from(number - 1)
                        .ok()
                        .and_then(|i| emu_core.themes.get(i))
                        .map(|theme| theme.key_id.clone());
                    effect.theme = true;
                }
                "graphics.janela" => {
                    graphics.window = choice(&WindowMode::ALL, number).unwrap_or_default();
                    effect.main_window = true;
                }
                "graphics.janela_do_jogo" => {
                    graphics.game_window = choice(&WindowMode::ALL, number).unwrap_or_default();
                }
                "graphics.scaling" => {
                    graphics.scaling = choice(&Scaling::ALL, number).unwrap_or_default();
                }
                // Applies right away to the open game: the target is rebuilt on the next frame.
                "graphics.proporcao" => {
                    graphics.ratio = choice(&AspectRatio::ALL, number).unwrap_or_default();
                    let aspect = graphics.ratio.aspect(16.0 / 9.0);
                    emu_core.with_session(|session| session.set_aspect_ratio(aspect));
                }
                "graphics.resolucao_interna" => {
                    graphics.internal_resolution = (number + 1).clamp(1, 6) as u8;
                    let factor = graphics.internal_resolution as usize;
                    emu_core.with_session(|session| session.set_internal_resolution(factor));
                }
                "graphics.antialias" | "graphics.anisotropico" => {
                    if key_id == "graphics.antialias" {
                        graphics.antialias = choice(&ANTIALIAS, number).unwrap_or(1);
                    } else {
                        graphics.anisotropic = choice(&ANISOTROPIC, number).unwrap_or(1);
                    }
                    let (samples, level) = (graphics.antialias as usize, graphics.anisotropic as usize);
                    emu_core.with_session(|session| session.set_enhancements(samples, level));
                }
                // Applies right away: the next draw already comes out with or without fog.
                "graphics.neblina" => {
                    let allowed = graphics.fog;
                    emu_core.with_session(|session| session.set_fog_allowed(allowed));
                }
                // The pace is read on every turn: it applies right away, without telling the
                // session.
                "speed.fps_limit" => {
                    emu_core.settings.speed.fps_limit =
                        choice(&FpsLimit::ALL, number).unwrap_or_default();
                }
                "speed.frameskip" => {
                    emu_core.settings.speed.frameskip =
                        choice(&Frameskip::all(), number).unwrap_or_default();
                }
                "speed.ff.ratio" => {
                    emu_core.settings.speed.fast_forward.ratio =
                        choice(&FastForward::choices(), number).unwrap_or(FastForward::default().ratio);
                }
                "speed.ff.mode" => {
                    emu_core.settings.speed.fast_forward.mode =
                        choice(&ShortcutMode::ALL, number).unwrap_or_default();
                }
                "speed.rewind.interval" => {
                    emu_core.settings.speed.rewind.interval_ms =
                        choice(&RewindSettings::INTERVALS_MS, number).unwrap_or(1000);
                }
                "speed.rewind.memory" => {
                    emu_core.settings.speed.rewind.memory_mb = choice(&RewindSettings::MEMORY_SIZES_MB, number)
                        .unwrap_or(RewindSettings::DEFAULT_MEMORY_MB);
                }
                "turbo.rate" => {
                    emu_core.settings.speed.turbo_per_second =
                        (number.max(0) as u8).saturating_add(turbo::MIN_PRESSES).min(turbo::MAX_PRESSES);
                }
                "turbo.mode" => {
                    emu_core.settings.controls.player_mut(port).turbo =
                        choice(&TurboMode::ALL, number).unwrap_or_default();
                }
                "turbo.button" => {
                    emu_core.settings.controls.player_mut(port).turbo_button =
                        choice(&turbo::PULSABLE, number).unwrap_or(turbo::DEFAULT_BUTTON).to_string();
                }
                // Changing the volume with the game open has to apply right away, not only on the
                // next opening.
                "audio.enabled" | "audio.volume" => {
                    if key_id == "audio.volume" {
                        emu_core.settings.audio.volume = number.clamp(0, 100) as u8;
                    }
                    let (enabled, volume) = (emu_core.settings.audio.enabled, emu_core.settings.audio.volume);
                    emu_core.with_session(|session| {
                        session.set_audio(enabled, volume);
                    });
                }
                _ => {}
            }
            emu_core.save_settings();
            effect
        });
        self.as_mut().apply(effect);
    }

    pub fn options(&self, key_id: &QString) -> QStringList {
        let key_id = String::from(key_id);
        let labels: Vec<String> = self.with(|emu_core| {
            let catalog = &emu_core.catalog;
            let translate = |key_ids: &[&str]| key_ids.iter().map(|c| catalog.get(c).to_string()).collect();
            let off = catalog.get("common.off");
            match key_id.as_str() {
                "language" => catalog.languages().iter().map(|l| l.name.clone()).collect(),
                "biblioteca" => {
                    translate(&["settings.library_view.grid", "settings.library_view.slider"])
                }
                "tema" => {
                    let language = catalog.current();
                    std::iter::once(catalog.get("settings.theme.system").to_string())
                        .chain(emu_core.themes.iter().map(|theme| theme.name(language).to_string()))
                        .collect()
                }
                "graphics.janela" | "graphics.janela_do_jogo" => {
                    WindowMode::ALL.iter().map(|m| catalog.get(m.key_id()).to_string()).collect()
                }
                "graphics.scaling" => {
                    Scaling::ALL.iter().map(|e| catalog.get(e.key()).to_string()).collect()
                }
                "graphics.proporcao" => {
                    AspectRatio::ALL.iter().map(|p| catalog.get(p.key_id()).to_string()).collect()
                }
                "graphics.resolucao_interna" => (1..=6u8).map(resolution_label).collect(),
                "speed.fps_limit" => {
                    FpsLimit::ALL.iter().map(|l| catalog.get(l.key_id()).to_string()).collect()
                }
                "speed.frameskip" => Frameskip::all()
                    .into_iter()
                    .map(|f| settings::frameskip_label(f, catalog))
                    .collect(),
                "speed.ff.ratio" => FastForward::choices()
                    .into_iter()
                    .map(|n| match n {
                        0 => catalog.get("speed.ff.unlimited").to_string(),
                        n => format!("{n}x"),
                    })
                    .collect(),
                "speed.ff.mode" => {
                    ShortcutMode::ALL.iter().map(|m| catalog.get(m.key_id()).to_string()).collect()
                }
                "turbo.mode" => {
                    TurboMode::ALL.iter().map(|m| catalog.get(m.key_id()).to_string()).collect()
                }
                "speed.rewind.interval" => RewindSettings::INTERVALS_MS
                    .iter()
                    .map(|&ms| match ms % 1000 {
                        0 => catalog.format("speed.rewind.every_s", &[("n", &(ms / 1000).to_string())]),
                        _ => catalog.format("speed.rewind.every_ms", &[("n", &ms.to_string())]),
                    })
                    .collect(),
                "speed.rewind.memory" => {
                    RewindSettings::MEMORY_SIZES_MB.iter().map(|mb| format!("{mb} MB")).collect()
                }
                "turbo.button" => turbo::PULSABLE
                    .iter()
                    .map(|b| catalog.get(&format!("button.{b}")).to_string())
                    .collect(),
                "turbo.rate" => (turbo::MIN_PRESSES..=turbo::MAX_PRESSES)
                    .map(|n| catalog.format("turbo.rate.value", &[("n", &n.to_string())]))
                    .collect(),
                "graphics.antialias" => {
                    ANTIALIAS.iter().map(|&n| level_label(n, "MSAA", off)).collect()
                }
                "graphics.anisotropico" => {
                    ANISOTROPIC.iter().map(|&n| level_label(n, "AF", off)).collect()
                }
                _ => Vec::new(),
            }
        });
        string_list(labels)
    }

    pub fn choose_roms_dir(mut self: Pin<&mut Self>) {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        self.with(|emu_core| {
            emu_core.settings.roms_dir = Some(dir);
            emu_core.save_settings();
        });
        self.as_mut().apply(Effect {
            library: Some(true),
            ..Effect::default()
        });
    }

    pub fn screenshots_dir(&self) -> QString {
        self.with(|emu_core| {
            let dir = screenshot::dir(emu_core.settings.screenshots_dir.as_deref());
            QString::from(&dir.display().to_string())
        })
    }

    pub fn choose_screenshots_dir(mut self: Pin<&mut Self>) {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        self.with(|emu_core| {
            emu_core.settings.screenshots_dir = Some(dir);
            emu_core.save_settings();
        });
        self.as_mut().apply(Effect::default());
    }

    pub fn use_default_screenshots_dir(mut self: Pin<&mut Self>) {
        self.with(|emu_core| {
            emu_core.settings.screenshots_dir = None;
            emu_core.save_settings();
        });
        self.as_mut().apply(Effect::default());
    }

    pub fn screenshots_in_default_dir(&self) -> bool {
        self.with(|emu_core| emu_core.settings.screenshots_dir.is_none())
    }

    pub fn soundfont(&self) -> QString {
        self.with(|emu_core| {
            QString::from(&match &emu_core.settings.audio.soundfont {
                Some(path) => path.display().to_string(),
                None => emu_core.catalog.get("audio.soundfont.auto").to_string(),
            })
        })
    }

    // The bank is opened when the machine is created: the change applies from the next game on,
    // and the screen's hint says so. There is nothing to apply to the open game.
    pub fn choose_soundfont(mut self: Pin<&mut Self>) {
        let Some(file) = rfd::FileDialog::new()
            .add_filter("SoundFont", &["sf2"])
            .pick_file()
        else {
            return;
        };
        self.with(|emu_core| {
            emu_core.settings.audio.soundfont = Some(file);
            emu_core.save_settings();
        });
        self.as_mut().apply(Effect::default());
    }

    pub fn use_auto_soundfont(mut self: Pin<&mut Self>) {
        self.with(|emu_core| {
            emu_core.settings.audio.soundfont = None;
            emu_core.save_settings();
        });
        self.as_mut().apply(Effect::default());
    }

    pub fn auto_soundfont(&self) -> bool {
        self.with(|emu_core| emu_core.settings.audio.soundfont.is_none())
    }

    pub fn screenshots_dir_url(&self) -> QString {
        self.with(|emu_core| {
            let dir = screenshot::dir(emu_core.settings.screenshots_dir.as_deref());
            if let Err(error) = std::fs::create_dir_all(&dir) {
                eprintln!("could not create {}: {error}", dir.display());
            }
            QString::from(&screenshot::url_of(&dir))
        })
    }

    pub fn sessions_dir_url(&self) -> QString {
        let dir = running_game::sessions_dir();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!("could not create {}: {error}", dir.display());
        }
        QString::from(&screenshot::url_of(&dir))
    }

    pub fn choose_z_wheel(mut self: Pin<&mut Self>, dir: bool) {
        let chosen = match dir {
            true => rfd::FileDialog::new().pick_folder(),
            false => rfd::FileDialog::new().add_filter("Z-Wheel", &["zip", "mod"]).pick_file(),
        };
        if let Some(path) = chosen {
            self.as_mut().point_to_z_wheel(path);
        }
    }

    pub fn detect_z_wheel(mut self: Pin<&mut Self>) {
        let found = self.with(|emu_core| {
            library::detect_z_wheel(emu_core.settings.roms_dir.as_deref(), &emu_core.games)
        });
        if let Some(path) = found {
            self.as_mut().point_to_z_wheel(path);
        }
    }

    pub fn z_wheel_status(&self) -> QString {
        self.with(|emu_core| {
            let catalog = &emu_core.catalog;
            QString::from(&match (&emu_core.z_wheel, &emu_core.zwheel_metadata) {
                (Some(_), Some(zwheel_metadata)) => catalog
                    .format("settings.z_wheel.found", &[("count", &zwheel_metadata.len().to_string())]),
                (Some(_), None) => catalog.get("settings.z_wheel.found_no_art").to_string(),
                (None, _) => catalog.get("settings.z_wheel.not_found").to_string(),
            })
        })
    }

    pub fn z_wheel_found(&self) -> bool {
        self.with(|emu_core| emu_core.z_wheel.is_some())
    }

    pub fn roms_dir_missing(&self) -> bool {
        self.with(|emu_core| emu_core.settings.roms_dir.as_ref().is_some_and(|p| !p.is_dir()))
    }

    pub fn check_for_updates(self: Pin<&mut Self>) {
        self.with(|emu_core| emu_core.check_for_updates());
    }

    pub fn checking_for_update(&self) -> bool {
        self.with(|emu_core| emu_core.checking_for_update())
    }

    pub fn update_status(&self) -> QString {
        self.with(|emu_core| {
            let catalog = &emu_core.catalog;
            let text = match (&emu_core.updater, emu_core.checking_for_update()) {
                (_, true) => catalog.get("settings.updates.checking").to_string(),
                (Some(Response::NewVersion(release)), _) => {
                    catalog.format("settings.updates.new", &[("version", &release.version)])
                }
                (Some(Response::UpToDate), _) => catalog.get("settings.updates.up_to_date").to_string(),
                (Some(Response::Failed(reason)), _) => {
                    catalog.format("settings.updates.failed", &[("reason", reason)])
                }
                (None, false) => String::new(),
            };
            QString::from(&text)
        })
    }

    pub fn update_page(&self) -> QString {
        self.with(|emu_core| match &emu_core.updater {
            Some(Response::NewVersion(release)) => QString::from(&release.page),
            _ => QString::default(),
        })
    }

    pub fn self_updates(&self) -> bool {
        updater::Installation::current().can_self_replace()
    }

    pub fn show_update_notice(&self) {
        self.with(|emu_core| emu_core.reopen_update_notice());
    }

    pub fn discord_connected(&self) -> bool {
        self.with(|emu_core| emu_core.presence.connected())
    }

    pub fn settings_location(&self) -> QString {
        self.with(|emu_core| {
            let path = settings::settings_path().display().to_string();
            QString::from(&emu_core.catalog.format("settings.saved_at", &[("path", &path)]))
        })
    }

    pub fn emulator_version(&self) -> QString {
        QString::from(updater::CURRENT_VERSION)
    }

    pub fn address(&self, which: &QString) -> QString {
        QString::from(match String::from(which).as_str() {
            "discord" => zeebx::ui::DISCORD,
            _ => zeebx::ui::REPOSITORY,
        })
    }

    fn point_to_z_wheel(mut self: Pin<&mut Self>, path: PathBuf) {
        self.with(|emu_core| {
            emu_core.settings.z_wheel_path = Some(path);
            emu_core.save_settings();
            emu_core.refresh_z_wheel();
        });
        self.as_mut().apply(Effect {
            library: Some(false),
            ..Effect::default()
        });
    }

    /// Tells QML what changed. Outside `emu_core::with`: the signals call into QML right away.
    fn apply(mut self: Pin<&mut Self>, effect: Effect) {
        let version = self.version().wrapping_add(1);
        self.as_mut().set_version(version);
        if effect.language {
            self.as_mut().language_changed();
        }
        if let Some(scan) = effect.library {
            self.as_mut().library_changed(scan);
        }
        if effect.main_window {
            self.as_mut().main_window_changed();
        }
        // Here, and not inside the write's `with`: see `theme::apply`.
        if effect.theme {
            super::theme::apply();
            self.as_mut().theme_changed();
        }
    }
}

/// The first port and the Zeebo button the key is mapped to, if it is. Every port counts, not only
/// the enabled ones: enabling a port later must not create the collision the capture rejected.
fn key_owner(settings: &Settings, key: Key) -> Option<(usize, &'static str)> {
    owner(settings, |source| is_key(source, key))
}

/// The Zeebo button, and the port, that already has a source satisfying `is_match`.
fn owner(settings: &Settings, is_match: impl Fn(&Source) -> bool) -> Option<(usize, &'static str)> {
    (0..zeebx::input::PORTS).find_map(|port| {
        let player = settings.controls.player(port)?;
        CONFIGURABLE
            .iter()
            .find(|button| player.sources(button).iter().any(&is_match))
            .map(|button| (port, *button))
    })
}

/// The game being edited.
impl qobject::SettingsModel {
    /// This window's `emu_core::with`: with the settings of the game being edited in place of the
    /// global ones, when there is a game and it has its own. See [`emu_core::Core::with_game`].
    fn with<T>(&self, f: impl FnOnce(&mut emu_core::Core) -> T) -> T {
        let game = &self.rust().game;
        emu_core::with(|emu_core| emu_core.with_game(game, f))
    }

    pub fn edit_game(mut self: Pin<&mut Self>, key_id: &QString) {
        self.as_mut().rust_mut().game = String::from(key_id);
        self.apply(Effect::default());
    }
}

/// The controls tab, about the port being edited.
impl qobject::SettingsModel {
    fn port(&self) -> usize {
        usize::try_from(*self.edited_port()).unwrap_or(0).min(zeebx::input::PORTS - 1)
    }

    /// The port's controller right now, with this window's keyboard.
    fn pad(&self, emu_core: &emu_core::Core) -> Pad {
        emu_core.host_input.pad(
            &emu_core.settings.controls,
            &emu_core.settings.shortcuts,
            self.port(),
            &self.rust().key_events,
        )
    }

    /// The port's controls changed: saves, and with a game open it applies right away.
    fn controls_changed(mut self: Pin<&mut Self>) {
        self.with(|emu_core| {
            emu_core.save_settings();
            emu_core.ports_changed();
        });
        let version = self.version().wrapping_add(1);
        self.as_mut().set_version(version);
    }

    pub fn edit_port(mut self: Pin<&mut Self>, port: i32) {
        self.as_mut().rust_mut().capturing = None;
        self.as_mut().set_edited_port(port);
        let version = self.version().wrapping_add(1);
        self.as_mut().set_version(version);
    }

    pub fn port_enabled(&self) -> bool {
        let port = self.port();
        self.with(|emu_core| emu_core.settings.controls.player(port).is_some_and(|j| j.enabled))
    }

    pub fn set_port_enabled(mut self: Pin<&mut Self>, enabled: bool) {
        let port = self.port();
        self.with(|emu_core| emu_core.settings.controls.player_mut(port).enabled = enabled);
        self.as_mut().controls_changed();
    }

    pub fn device_kind(&self) -> i32 {
        let port = self.port();
        self.with(|emu_core| {
            let device_kind = emu_core.settings.controls.player(port).map(|j| j.device_kind);
            device_kind.map_or(1, |device_kind| index(&DEVICES, &device_kind))
        })
    }

    pub fn set_device(mut self: Pin<&mut Self>, device_kind: i32) {
        let port = self.port();
        let Some(device_kind) = choice(&DEVICES, device_kind) else {
            return;
        };
        self.with(|emu_core| emu_core.settings.controls.player_mut(port).device_kind = device_kind);
        self.as_mut().controls_changed();
    }

    /// The host controllers the port can pick: the `gilrs` ones, the Wii Remotes and, last, the
    /// one configured on the port even if it is not connected right now. Without it, a
    /// disconnected controller showed up as "keyboard only", and whoever opened the list saw the
    /// wrong setting; egui shows the stored name the same way.
    fn controller_names(emu_core: &emu_core::Core, port: usize) -> Vec<String> {
        let mut names = emu_core.host_input.gamepads.names();
        names.extend((0..emu_core.host_input.wiimotes.count()).map(Wiimotes::name));
        let configured = emu_core.settings.controls.player(port).and_then(|j| j.device.clone());
        if let Some(configured) = configured.filter(|name| !names.contains(name)) {
            names.push(configured);
        }
        names
    }

    pub fn controls(&self) -> QStringList {
        let port = self.port();
        let names = self.with(|emu_core| {
            let mut names = vec![emu_core.catalog.get("controls.device.none").to_string()];
            names.extend(Self::controller_names(emu_core, port));
            names
        });
        string_list(names)
    }

    pub fn current_controller(&self) -> i32 {
        let port = self.port();
        self.with(|emu_core| {
            let Some(device) = emu_core.settings.controls.player(port).and_then(|j| j.device.clone()) else {
                return 0;
            };
            let names = Self::controller_names(emu_core, port);
            names.iter().position(|name| *name == device).map_or(0, |i| i as i32 + 1)
        })
    }

    pub fn choose_controller(mut self: Pin<&mut Self>, index: i32) {
        let port = self.port();
        self.with(|emu_core| {
            let names = Self::controller_names(emu_core, port);
            let device = usize::try_from(index - 1).ok().and_then(|i| names.get(i).cloned());
            emu_core.settings.controls.player_mut(port).switch_controller(device);
        });
        self.as_mut().controls_changed();
    }

    pub fn reset_mapping(mut self: Pin<&mut Self>) {
        let port = self.port();
        self.with(|emu_core| {
            let player = emu_core.settings.controls.player_mut(port);
            *player = zeebx::input::bindings::Player::default_for_controller(player.device.clone());
        });
        self.as_mut().controls_changed();
    }

    pub fn buttons(&self) -> QStringList {
        // The turbo key is mapped like a button, but it is not a Zeebo button: see
        // [`zeebx::speed::turbo::TURBO_BUTTON`].
        let buttons = CONFIGURABLE.iter().chain([&turbo::TURBO_BUTTON]);
        string_list(buttons.map(|b| b.to_string()).collect())
    }

    pub fn button_sources(&self, button: &QString) -> QString {
        let (port, button) = (self.port(), String::from(button));
        self.with(|emu_core| {
            let origins = emu_core
                .settings
                .controls
                .player(port)
                .map(|player| player.sources(&button).to_vec())
                .unwrap_or_default();
            QString::from(&match origins.is_empty() {
                true => emu_core.catalog.get("controls.unbound").to_string(),
                false => origins.iter().map(Source::label).collect::<Vec<_>>().join(", "),
            })
        })
    }

    pub fn capturing(&self) -> QString {
        QString::from(self.rust().capturing.as_deref().unwrap_or_default())
    }

    pub fn capture(mut self: Pin<&mut Self>, button: &QString) {
        let button = String::from(button);
        let mut rust = self.as_mut().rust_mut();
        rust.capturing_shortcut = None;
        rust.rejection = None;
        // Clicking the same button again gives up the capture.
        rust.capturing = match rust.capturing.as_deref() == Some(button.as_str()) {
            true => None,
            false => Some(button),
        };
        rust.capture_since = rust.capturing.is_some().then(std::time::Instant::now);
    }

    pub fn clear_button(mut self: Pin<&mut Self>, button: &QString) {
        let (port, button) = (self.port(), String::from(button));
        self.with(|emu_core| emu_core.settings.controls.player_mut(port).clear(&button));
        self.as_mut().controls_changed();
    }

    pub fn key(mut self: Pin<&mut Self>, code: i32, modifiers: i32, pressed: bool) {
        let Some(key) = super::bridge::qt_key(code) else {
            return;
        };
        let port = self.port();
        let mut rust = self.as_mut().rust_mut();
        match pressed {
            true => rust.key_events.insert(key),
            false => rust.key_events.remove(&key),
        };
        if !pressed {
            return;
        }
        if let Some(which) = rust.capturing_shortcut.take() {
            drop(rust);
            if key == Key::Escape {
                return;
            }
            match which.combo(&Shortcuts::default()) {
                Some(_) => {
                    let modifiers = super::bridge::qt_modifiers(modifiers);
                    self.as_mut().set_combo(
                        which,
                        KeyCombo {
                            key,
                            modifiers,
                        },
                    );
                }
                None => self.as_mut().set_shortcut(which, Source::key(key.name())),
            }
            return;
        }
        let Some(button) = rust.capturing.take() else {
            return;
        };
        if key == Key::Escape {
            return;
        }
        drop(rust);
        // A shortcut's key does not become a button: in the game window the shortcut wins, and
        // the button would look mapped without responding.
        let rejection = self.with(|emu_core| {
            if let Some(which) = SHORTCUTS
                .into_iter()
                .find(|which| which.has_key(&emu_core.settings.shortcuts, key))
            {
                let catalog = &emu_core.catalog;
                let text = catalog.format(
                    "controls.shortcut.is_shortcut",
                    &[("key", key.name()), ("shortcut", catalog.get(which.key_id()))],
                );
                return Some(text);
            }
            // The keyboard first: it is what is under the hand of whoever is configuring.
            emu_core
                .settings
                .controls
                .player_mut(port)
                .bind(&button, Source::key(key.name()));
            None
        });
        self.as_mut().rust_mut().rejection = rejection.map(|text| (false, text));
        self.as_mut().controls_changed();
    }

    pub fn shortcut(&self, which: &QString) -> QString {
        let Some(which) = Shortcut::from_name(which) else {
            return QString::default();
        };
        self.with(|emu_core| {
            let shortcuts = &emu_core.settings.shortcuts;
            if let Some(text) = which.combo(shortcuts) {
                return QString::from(&match KeyCombo::read(text) {
                    Some(combo) => combo.label(),
                    None => emu_core.catalog.get("controls.unbound").to_string(),
                });
            }
            let origins = match which {
                Shortcut::Screenshot => return QString::from(&shortcuts.screenshot),
                Shortcut::Reset => {
                    return QString::from(match shortcuts.restart.is_empty() {
                        true => emu_core.catalog.get("controls.unbound"),
                        false => shortcuts.restart.as_str(),
                    });
                }
                Shortcut::FastForward => &shortcuts.fast_forward,
                Shortcut::Rewind => &shortcuts.rewind,
                _ => return QString::default(),
            };
            QString::from(&match origins.is_empty() {
                true => emu_core.catalog.get("controls.unbound").to_string(),
                false => origins.iter().map(Source::label).collect::<Vec<_>>().join(", "),
            })
        })
    }

    pub fn capture_shortcut(mut self: Pin<&mut Self>, which: &QString) {
        let which = Shortcut::from_name(which);
        let mut rust = self.as_mut().rust_mut();
        // Clicking the same shortcut again gives up the capture.
        rust.capturing_shortcut = match rust.capturing_shortcut == which {
            true => None,
            false => which,
        };
        rust.capturing = None;
        rust.rejection = None;
        rust.capture_since = rust.capturing_shortcut.is_some().then(std::time::Instant::now);
    }

    pub fn capturing_shortcut(&self, which: &QString) -> bool {
        let which = Shortcut::from_name(which);
        which.is_some() && self.rust().capturing_shortcut == which
    }

    pub fn reset_shortcut(mut self: Pin<&mut Self>, which: &QString) {
        let Some(which) = Shortcut::from_name(which) else {
            return;
        };
        self.as_mut().rust_mut().capturing_shortcut = None;
        if which.combo(&Shortcuts::default()).is_some() {
            if let Some(fallback) = KeyCombo::read(which.fallback()) {
                self.as_mut().set_combo(which, fallback);
            }
            return;
        }
        self.as_mut().set_shortcut(which, Source::key(which.fallback()));
    }

    pub fn default_shortcut(&self, which: &QString) -> QString {
        let Some(which) = Shortcut::from_name(which) else {
            return QString::default();
        };
        QString::from(&match KeyCombo::read(which.fallback()) {
            Some(combo) => combo.label(),
            None => which.fallback().to_string(),
        })
    }

    /// A controller button for the open capture of a shortcut that accepts a button. The
    /// screenshot is keyboard only, and for it this does nothing.
    pub fn read_controller_shortcut(mut self: Pin<&mut Self>) {
        let Some(which @ (Shortcut::FastForward | Shortcut::Rewind)) = self.rust().capturing_shortcut else {
            return;
        };
        let button = self.with(|emu_core| {
            emu_core.host_input.gamepads.poll();
            emu_core.host_input.gamepads.first_button_from_any()
        });
        if let Some(button) = button {
            self.as_mut().rust_mut().capturing_shortcut = None;
            self.as_mut().set_shortcut(which, button);
        }
    }

    /// The hint for a capture that has been waiting for more than [`HINT_DELAY`] without any
    /// controller having sent anything since it started.
    ///
    /// **This is the case of the PS4 controller on the Mac (issue #59)**: the capture waited
    /// forever, saying nothing, and whoever held the controller had nowhere to start. A key closes
    /// the capture right away, so the long wait is always someone trying a controller.
    pub fn capture_hint(&self) -> QString {
        let rust = self.rust();
        let opened = rust.capturing.is_some()
            || matches!(rust.capturing_shortcut, Some(Shortcut::FastForward | Shortcut::Rewind));
        let Some(since) = rust.capture_since.filter(|_| opened) else {
            return QString::default();
        };
        if since.elapsed() < HINT_DELAY {
            return QString::default();
        }
        emu_core::with(|emu_core| {
            match emu_core.host_input.gamepads.last_event().is_some_and(|when| when >= since) {
                true => QString::default(),
                false => QString::from(emu_core.catalog.get("controls.no_signal")),
            }
        })
    }

    pub fn rejection(&self, shortcut: bool) -> QString {
        match &self.rust().rejection {
            Some((hint_shortcut, text)) if *hint_shortcut == shortcut => QString::from(text),
            _ => QString::default(),
        }
    }

    /// Changes a shortcut's source, or says why not: Esc, P and F11 belong to the game window, and
    /// another shortcut's key would end up with two owners. Reject, rather than swap the two,
    /// because the swap would change a shortcut without anyone seeing.
    ///
    /// **A Zeebo button on the same key** is a rejection for the screenshot, as it always was, and
    /// only a warning for fast-forward: there the shortcut's source stops reaching the game
    /// ([`Shortcuts::reserved`]), and whoever chose it knows what they did. The warning says which
    /// button lost it.
    fn set_shortcut(mut self: Pin<&mut Self>, which: Shortcut, origin: Source) {
        let rejection = self.with(|emu_core| {
            let catalog = &emu_core.catalog;
            let source_label = origin.label();
            let key = match &origin {
                Source::Key { name } => Key::from_name(name),
                _ => None,
            };
            if let Some(key) = key {
                if matches!(key, Key::Escape | Key::P | Key::F11) {
                    return Some(catalog.format("controls.shortcut.reserved", &[("key", key.name())]));
                }
                if let Some(other) = SHORTCUTS
                    .into_iter()
                    .find(|other| *other != which && other.has_key(&emu_core.settings.shortcuts, key))
                {
                    return Some(catalog.format(
                        "controls.shortcut.taken_shortcut",
                        &[("key", &source_label), ("shortcut", catalog.get(other.key_id()))],
                    ));
                }
            }
            let owner = owner(&emu_core.settings, |source| *source == origin)
                .or_else(|| key.and_then(|key| key_owner(&emu_core.settings, key)));
            let conflict = owner.map(|(port, button)| {
                let button = catalog.get(&format!("button.{button}")).to_string();
                let port = (port + 1).to_string();
                let key_id = match which {
                    Shortcut::FastForward | Shortcut::Rewind => "controls.shortcut.shared",
                    _ => "controls.shortcut.taken",
                };
                catalog.format(key_id, &[("key", &source_label), ("button", &button), ("port", &port)])
            });
            if !matches!(which, Shortcut::FastForward | Shortcut::Rewind) && conflict.is_some() {
                return conflict;
            }
            let shortcuts = &mut emu_core.settings.shortcuts;
            match which {
                Shortcut::Screenshot => shortcuts.screenshot = source_label.clone(),
                Shortcut::Reset => shortcuts.restart = source_label.clone(),
                Shortcut::FastForward | Shortcut::Rewind => {
                    if let Some(origins) = which.origins(shortcuts) {
                        *origins = vec![origin.clone()];
                    }
                }
                // The save state ones go through `set_combo`.
                _ => {}
            }
            emu_core.save_settings();
            conflict
        });
        self.as_mut().rust_mut().rejection = rejection.map(|text| (true, text));
        self.as_mut().apply(Effect::default());
    }

    /// The [`set_shortcut`](Self::set_shortcut) of the save state shortcuts, which accept
    /// combinations. The same rejections, looking at the modifiers: Esc, P and F11 alone and
    /// Alt+Enter belong to the game window; the same combination cannot serve two shortcuts; and
    /// the bare key of a Zeebo button is rejected, as for the screenshot. With a modifier it
    /// collides with no button: the game does not read modifiers.
    fn set_combo(mut self: Pin<&mut Self>, which: Shortcut, combo: KeyCombo) {
        let rejection = self.with(|emu_core| {
            let catalog = &emu_core.catalog;
            let label = combo.label();
            let alone = combo.modifiers.is_empty();
            let key = combo.key;
            let alt_enter = Modifiers {
                alt: true,
                ..Modifiers::default()
            };
            if (alone && matches!(key, Key::Escape | Key::P | Key::F11))
                || (key == Key::Enter && combo.modifiers == alt_enter)
            {
                return Some(catalog.format("controls.shortcut.reserved", &[("key", &label)]));
            }
            let shortcuts = &emu_core.settings.shortcuts;
            let other = SHORTCUTS
                .into_iter()
                .filter(|other| *other != which)
                .find(|other| match other.combo(shortcuts) {
                    Some(text) => KeyCombo::read(text) == Some(combo),
                    None => alone && other.has_key(shortcuts, key),
                });
            if let Some(other) = other {
                return Some(catalog.format(
                    "controls.shortcut.taken_shortcut",
                    &[("key", &label), ("shortcut", catalog.get(other.key_id()))],
                ));
            }
            if alone && let Some((port, button)) = key_owner(&emu_core.settings, key) {
                let button = catalog.get(&format!("button.{button}")).to_string();
                let port = (port + 1).to_string();
                return Some(catalog.format(
                    "controls.shortcut.taken",
                    &[("key", &label), ("button", &button), ("port", &port)],
                ));
            }
            if let Some(text) = which.combo_mut(&mut emu_core.settings.shortcuts) {
                *text = combo.name();
            }
            emu_core.save_settings();
            None
        });
        self.as_mut().rust_mut().rejection = rejection.map(|text| (true, text));
        self.as_mut().apply(Effect::default());
    }

    pub fn read_controller(mut self: Pin<&mut Self>) {
        let port = self.port();
        let capturing = self.rust().capturing.clone();
        let mut calibrating = self.as_mut().rust_mut().calibrating.take();
        let mut rejected = None;
        let mut button_rejection = None;
        let changed = self.with(|emu_core| {
            // **gilrs only updates the state when the event queue is drained.** Without this the
            // art stayed unlit no matter how much one pressed.
            emu_core.host_input.gamepads.poll();
            let controls = &emu_core.settings.controls;
            let mut changed = false;
            if let Some(button) = &capturing {
                let device = controls.player(port).and_then(|j| j.device.clone());
                // The chosen Wii Remote answers through its own buttons; the others, through gilrs.
                let origin = match emu_core.host_input.port_wiimote(controls, port) {
                    Some(wiimote) if device.is_some() => wiimote.first_source(),
                    _ => emu_core.host_input.gamepads.first_active(device.as_deref(), port),
                };
                // A shortcut's button does not become a Zeebo button, for the same reason as the
                // key: in the game the shortcut wins. See [`Shortcuts::reserved`].
                let reserved = origin.as_ref().is_some_and(|origin| {
                    emu_core.settings.shortcuts.reserved().any(|r| r == origin)
                });
                match origin {
                    Some(origin) if reserved => {
                        let catalog = &emu_core.catalog;
                        button_rejection = Some(catalog.format(
                            "controls.shortcut.is_shortcut",
                            &[
                                ("key", &origin.label()),
                                ("shortcut", catalog.get(shortcut_for_source(&emu_core.settings.shortcuts, &origin).key_id())),
                            ],
                        ));
                        changed = true;
                    }
                    Some(origin) => {
                        emu_core.settings.controls.player_mut(port).bind(button, origin);
                        changed = true;
                    }
                    None => {}
                }
            }
            // The calibration in progress gathers still readings and settles on the average.
            let raw = emu_core.host_input.raw_port_motion(&emu_core.settings.controls, port);
            if let (Some((from, samples)), Some(raw)) = (&mut calibrating, raw)
                && *from == port
            {
                samples.push(raw);
                if samples.len() >= CALIBRATION_SAMPLES {
                    let n = samples.len() as f32;
                    let media: [f32; 3] =
                        std::array::from_fn(|i| samples.iter().map(|a| a[i]).sum::<f32>() / n);
                    match MotionCalibration::from_rest(media) {
                        Some(calibration) => {
                            emu_core.settings.controls.player_mut(port).motion_calibration =
                                calibration;
                            rejected = Some(false);
                            changed = true;
                        }
                        None => rejected = Some(true),
                    }
                    calibrating = None;
                }
            }
            changed
        });
        let mut rust = self.as_mut().rust_mut();
        rust.calibrating = calibrating;
        if let Some(rejected) = rejected {
            rust.calibration_rejected = rejected;
        }
        if button_rejection.is_some() {
            rust.rejection = button_rejection.map(|text| (false, text));
        }
        if changed {
            rust.capturing = None;
            drop(rust);
            self.as_mut().controls_changed();
        }
    }

    pub fn pressed(&self, button: &QString) -> bool {
        let button = String::from(button);
        self.with(|emu_core| {
            let pad = self.pad(emu_core);
            Pad::button_by_name(&button).is_some_and(|i| pad.is_down(i))
        })
    }

    pub fn axis(&self, index: i32) -> f64 {
        self.with(|emu_core| {
            let pad = self.pad(emu_core);
            let value = usize::try_from(index).ok().and_then(|i| pad.axes.get(i).copied());
            f64::from(value.unwrap_or(0)) / f64::from(AXIS_RANGE)
        })
    }

    pub fn axes(&self) -> QStringList {
        string_list(AXIS_NAMES.iter().map(|e| e.to_string()).collect())
    }

    pub fn axis_sources(&self) -> QStringList {
        let is_empty = self.with(|emu_core| emu_core.catalog.get("controls.axis.none").to_string());
        string_list(std::iter::once(is_empty).chain(gamepads::axis_names().map(String::from)).collect())
    }

    pub fn axis_source(&self, axis: &QString) -> i32 {
        let (port, axis) = (self.port(), String::from(axis));
        self.with(|emu_core| {
            let origin = emu_core.settings.controls.player(port).and_then(|j| j.axes.get(&axis).cloned());
            origin
                .and_then(|origin| gamepads::axis_names().position(|name| name == origin.name))
                .map_or(0, |i| i as i32 + 1)
        })
    }

    pub fn set_axis_source(mut self: Pin<&mut Self>, axis: &QString, index: i32) {
        let (port, axis) = (self.port(), String::from(axis));
        self.with(|emu_core| {
            let player = emu_core.settings.controls.player_mut(port);
            let axis_name = usize::try_from(index - 1).ok().and_then(|i| gamepads::axis_names().nth(i));
            match axis_name {
                Some(axis_name) => {
                    let inverted = player.axes.get(&axis).is_some_and(|s| s.invert);
                    player.axes.insert(
                        axis,
                        AxisSource {
                            name: axis_name.to_string(),
                            invert: inverted,
                        },
                    );
                }
                None => {
                    player.axes.remove(&axis);
                }
            }
        });
        self.as_mut().controls_changed();
    }

    pub fn axis_inverted(&self, axis: &QString) -> bool {
        let (port, axis) = (self.port(), String::from(axis));
        self.with(|emu_core| {
            let player = emu_core.settings.controls.player(port);
            player.and_then(|j| j.axes.get(&axis)).is_some_and(|s| s.invert)
        })
    }

    pub fn invert_axis(mut self: Pin<&mut Self>, axis: &QString, inverted: bool) {
        let (port, axis) = (self.port(), String::from(axis));
        self.with(|emu_core| {
            if let Some(origin) = emu_core.settings.controls.player_mut(port).axes.get_mut(&axis) {
                origin.invert = inverted;
            }
        });
        self.as_mut().controls_changed();
    }

    pub fn art_aspect_ratio(&self) -> f64 {
        self.with(|emu_core| {
            emu_core.art.as_ref().map_or(0.0, |art| art.width as f64 / art.height as f64)
        })
    }

    pub fn parts(&self) -> QStringList {
        let names = self.with(|emu_core| {
            let parts = emu_core.art.as_ref().map(|art| art.parts()).unwrap_or_default();
            parts.iter().map(|part| part.button.clone()).collect()
        });
        string_list(names)
    }

    pub fn part_bounds(&self, index: i32) -> QRectF {
        self.with(|emu_core| {
            let Some(art) = &emu_core.art else {
                return QRectF::default();
            };
            let Some(part) = usize::try_from(index).ok().and_then(|i| art.parts().get(i)) else {
                return QRectF::default();
            };
            let [u0, v0, u1, v1] = part.bounds(art).map(f64::from);
            QRectF::new(u0, v0, u1 - u0, v1 - v0)
        })
    }

    pub fn button_at(&self, u: f64, v: f64) -> QString {
        self.with(|emu_core| {
            let button = emu_core.art.as_ref().and_then(|art| art.hit(u as f32, v as f32));
            QString::from(button.unwrap_or_default())
        })
    }

    pub fn sensor(&self) -> QString {
        let port = self.port();
        self.with(|emu_core| {
            let sensor = emu_core.host_input.port_sensor(&emu_core.settings.controls, port);
            QString::from(&sensor.describe(&emu_core.catalog))
        })
    }

    pub fn sensor_lacks_permission(&self) -> bool {
        let port = self.port();
        self.with(|emu_core| {
            let sensor = emu_core.host_input.port_sensor(&emu_core.settings.controls, port);
            matches!(sensor, PortSensor::Controller(s) if s.permission_denied)
        })
    }

    pub fn has_reading(&self) -> bool {
        let port = self.port();
        self.with(|emu_core| {
            emu_core.host_input.raw_port_motion(&emu_core.settings.controls, port).is_some()
        })
    }

    pub fn motion_line(&self) -> QString {
        let port = self.port();
        self.with(|emu_core| {
            let [x, y, z] = emu_core.host_input.port_motion(&emu_core.settings.controls, port);
            let pad = self.pad(emu_core);
            let names: Vec<&str> = ["up", "down", "left", "right", "b1", "b2", "back"]
                .into_iter()
                .filter(|name| Pad::button_by_name(name).is_some_and(|i| pad.is_down(i)))
                .collect();
            let wheel_angle = x.atan2(y).to_degrees();
            QString::from(&format!(
                "x {x:+.2}  y {y:+.2}  z {z:+.2} g   ⟲ {wheel_angle:+.0}°  {}",
                names.join(" ")
            ))
        })
    }

    pub fn rotation(&self) -> f64 {
        let port = self.port();
        self.with(|emu_core| {
            // The steering angle, the same one Crash Nitro Kart computes: gravity in the plane of
            // the face. Lying flat, gravity leaves that plane and the angle turns into noise, so
            // the image only rotates when gravity is actually there.
            let [x, y, _] = emu_core.host_input.port_motion(&emu_core.settings.controls, port);
            let in_plane = (x * x + y * y).sqrt();
            let rotation = x.atan2(y) * ((in_plane - 0.3) / 0.4).clamp(0.0, 1.0);
            f64::from(rotation).to_degrees()
        })
    }

    pub fn calibrating(&self) -> bool {
        self.rust().calibrating.is_some()
    }

    pub fn calibration_rejected(&self) -> bool {
        self.rust().calibration_rejected
    }

    pub fn calibrate(mut self: Pin<&mut Self>) {
        let port = self.port();
        self.as_mut().rust_mut().calibrating = Some((port, Vec::new()));
    }

    pub fn reset_calibration(mut self: Pin<&mut Self>) {
        let port = self.port();
        self.as_mut().rust_mut().calibrating = None;
        self.with(|emu_core| {
            emu_core.settings.controls.player_mut(port).motion_calibration = Default::default();
        });
        self.as_mut().controls_changed();
    }

    pub fn unlock_sensors(mut self: Pin<&mut Self>) {
        let release = Arc::clone(&self.as_mut().rust_mut().release);
        if let Ok(mut state) = release.lock() {
            *state = SensorGrant::Requesting;
        }
        let _ = std::thread::Builder::new()
            .name("libera-sensores".into())
            .spawn(move || {
                let result = match sensors::unlock_sensors() {
                    Ok(()) => SensorGrant::Done,
                    Err(error) => SensorGrant::Failed(error),
                };
                if let Ok(mut state) = release.lock() {
                    *state = result;
                }
            });
    }

    pub fn grant_status(&self) -> i32 {
        match self.rust().release.lock().map(|e| e.clone()) {
            Ok(SensorGrant::Requesting) => 1,
            Ok(SensorGrant::Done) => 2,
            Ok(SensorGrant::Failed(_)) => 3,
            _ => 0,
        }
    }

    pub fn grant_failure(&self) -> QString {
        let error = match self.rust().release.lock().map(|e| e.clone()) {
            Ok(SensorGrant::Failed(error)) => error,
            _ => return QString::default(),
        };
        self.with(|emu_core| {
            QString::from(
                &emu_core
                    .catalog
                    .get("controls.boomerang.unlock_failed")
                    .replace("{erro}", &error)
                    .replace("{arquivo}", sensors::RULE_FILE),
            )
        })
    }

    pub fn udev_rule(&self) -> QString {
        QString::from(sensors::UDEV_RULE)
    }
}

fn string_list(texts: Vec<String>) -> QStringList {
    let mut list = QList::<QString>::default();
    for text in texts {
        list.append(QString::from(&text));
    }
    QStringList::from(&list)
}
