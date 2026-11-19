//! Zeebx on iOS: the same session, a different screen.
//!
//! The emulation is done by [`zeebx::session::Session`]. This file is the border with UIKit: Swift
//! asks for a scan, a step and a frame, and hands back touches and the controller. The window
//! lives in the Xcode project, because it is the one that owns the UIKit lifecycle.
//!
//! **Swift draws, and decides nothing.** Which options the settings have, what a finger presses,
//! what a controller button is bound to, when the game is paused: all of that is on this side,
//! over the same `Settings`, the same language catalog and the same [`zeebx::input::touch`] the
//! Android frontend uses. The reason is practical before it is tidy — the Swift half only compiles
//! on a Mac, and this half is what `cargo test` reaches anywhere. The modules are one per subject:
//! [`game`] is the turn of the loop, [`overlay`] and [`editor`] the controls drawn on screen,
//! [`controller`] the physical one, and [`settings_page`] the settings screen as data.
//!
//! **There is no Dynarmic on this target.** The JIT code allocator asks for an executable page,
//! and iOS — the simulator included — refuses that to a third-party app. The core already has the
//! interpreter for the case where the JIT cannot run; here it is the one.
//!
//! Calls come from a single thread. The audio mixer is what crosses threads, and that is already
//! the `cpal` contract on desktop; the library scan runs on a thread of its own and only hands
//! back a finished list.

mod controller;
mod editor;
mod game;
mod overlay;
mod settings_page;

use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use serde_json::json;

use zeebx::input::touch::{self, Element, TouchControl};
use zeebx::input::{self, Pad};
use zeebx::library;
use zeebx::session::Session;
use zeebx::speed::rewind::RewindController;
use zeebx::speed::turbo::PortTurbo;
use zeebx::storage::StoragePaths;
use zeebx::ui::i18n::Catalog;
use zeebx::ui::running_game::fit_viewport;
use zeebx::ui::save_slots;
use zeebx::ui::settings::Settings;

use controller::Controller;
use overlay::Overlay;

/// The same numbers as `include/zeebx_ios.h`. The tests below check what can be checked from
/// this side; the header is what Swift includes.
const NO_SESSION: i32 = -1;
const FRAME: i32 = 0;
const RUNNING: i32 = 1;
const AHEAD: i32 = 2;
const STOPPED: i32 = 3;

/// What a finished scan hands back: the games, and the applets the Z-Wheel can launch.
type Scan = (Vec<library::Game>, Vec<(u32, String)>);

/// What Swift needs to know after each turn to put the game on screen. `ZeebxView` in the header.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ViewState {
    /// Changes when the frame's pixels change: the image is only rebuilt then.
    frame_serial: u32,
    /// The size the frame is drawn at, in points, centered on the screen.
    frame_width: f32,
    frame_height: f32,
    /// Whether enlarging interpolates.
    smooth: i32,
    /// Changes when anything about the on-screen controls would be drawn differently.
    overlay_revision: u32,
    /// From 0 to 100.
    overlay_opacity: i32,
}

/// A piece of the on-screen controls, placed and ready to draw. `ZeebxElement` in the header.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ElementOut {
    /// The position in [`TouchControl::ALL`].
    kind: i32,
    center_x: f32,
    center_y: f32,
    half_width: f32,
    half_height: f32,
    /// Round, or a pill.
    round: i32,
    /// Whether it is pressed. On the D-pad, one bit per arm: up, down, left, right.
    pressed: u32,
    /// Where the stick's knob is, from -1 to 1.
    knob_x: f32,
    knob_y: f32,
    /// The editor's selection.
    highlighted: i32,
}

/// What the on-screen controls looked like the last time Swift asked: when it differs, the
/// revision moves and Swift redraws.
#[derive(Debug, Default, Clone, PartialEq)]
struct OverlayLook {
    visible: bool,
    opacity: u8,
    buttons: u32,
    shortcuts: [bool; 3],
    axes: [u32; 4],
    elements: u32,
    chosen: Option<&'static str>,
}

/// The app, on this side of the border.
///
/// Public because the `extern "C"` signature names it. The fields stay private: Swift only sees
/// the pointer, and there is no constructor outside the functions in here.
pub struct App {
    /// The folder the Files app shows as the app's own.
    documents: PathBuf,
    roms: PathBuf,
    /// Where settings are read and written. It is a file in the same format as desktop's.
    file: PathBuf,
    settings: Settings,
    /// The interface texts, from the same catalog desktop uses. The built-in languages are
    /// embedded in the binary.
    catalog: Catalog,
    /// Cache, shared system and persistent saves handed explicitly to the core.
    storage: StoragePaths,
    /// Save states live here, apart from the ROMs: swapping the collection does not erase where
    /// each game was.
    states_root: PathBuf,
    /// Moves on every change the settings screen would draw differently.
    revision: u32,

    games: Vec<library::Game>,
    /// Applets known to the library, for the Z-Wheel boot.
    installed: Vec<(u32, String)>,
    /// Result of a background scan. While it exists, the old list stays usable.
    scan: Option<Receiver<Scan>>,
    /// The last image handed to Swift — a cover or a save state thumbnail — in BGRA.
    image: Vec<u8>,

    session: Option<Session>,
    /// Where the open game came from: it is what "Restart" opens again.
    open_path: Option<PathBuf>,
    /// BLAKE3 identity of the file that opened the session, the same rule as the core's storage.
    state_id: Option<String>,
    save_slots: [Option<save_slots::Slot>; save_slots::SLOTS],
    last: Instant,
    /// The previous turn's controller: only a change becomes a BREW key.
    previous_pad: Pad,
    controller: Controller,
    /// Whether Swift saw a controller on its last reading.
    controller_connected: bool,
    /// The buttons held on a keyboard, one bit per [`Pad`] index. In the simulator, mostly.
    keyboard: u32,
    overlay: Overlay,
    /// The area the on-screen controls are laid out in: origin and size, in points. It is the
    /// safe area, so no piece ends up under the notch.
    safe: [f32; 4],
    /// The whole screen, in points, and how many pixels a point is.
    screen: [f32; 2],
    scale: f32,
    /// The pause menu is on screen: the game is still and touch belongs to the menu.
    menu: bool,
    /// The game is paused by choice, not by failure.
    paused: bool,
    fast_forward: zeebx::speed::Toggle,
    fast_forwarding: bool,
    rewind: RewindController,
    /// Player 1's turbo, the only one a phone has.
    turbo: PortTurbo,
    /// What the next physical button becomes, in the settings.
    capturing: Option<settings_page::Capture>,

    /// The layout editor is on screen.
    editing: bool,
    /// The piece the editor is dragging, and where the finger grabbed it relative to its center.
    dragging: Option<(TouchControl, [f32; 2])>,
    /// The piece selected in the editor, the one the size bar grows and shrinks.
    chosen: Option<TouchControl>,

    /// BGRA8 of the current frame. Swift copies it before the next turn.
    frame: Vec<u8>,
    /// The generation and writes of the frame in `frame`: the screen repaints more often than the
    /// game draws, and without this key every turn would redo the whole conversion.
    frame_key: Option<(u64, u64)>,
    frame_serial: u32,
    width: u32,
    height: u32,
    overlay_look: OverlayLook,
    overlay_revision: u32,
    /// Bumped whenever the pieces are placed again.
    elements_serial: u32,
    /// What is written over the game: rewinding, fast-forwarding, turbo, paused.
    indicator: CString,
    /// The debug panel's line, when it is turned on.
    debug_line: CString,
    error: CString,
}

impl App {
    fn create(documents: &Path, support: &Path, language: &str) -> Self {
        let roms = documents.join("roms");
        let _ = std::fs::create_dir_all(&roms);
        let storage = StoragePaths::for_frontend(support, Some(support));
        let dir_error = storage.create_dirs().err().map(|error| error.to_string());

        let file = support.join("zeebx.json");
        let settings = Settings::load_from(&file);
        // The saved language, or the device's. The catalog only knows the built-in ones plus
        // whatever is in `lang/` inside the app's folder.
        let mut catalog = Catalog::new(&[support.join("lang")]);
        match &settings.language {
            Some(code) => catalog.select(code),
            None => catalog.select_best(language),
        };

        let mut app = Self {
            documents: documents.to_path_buf(),
            roms,
            file,
            settings,
            catalog,
            storage,
            states_root: support.to_path_buf(),
            revision: 0,
            games: Vec::new(),
            installed: Vec::new(),
            scan: None,
            image: Vec::new(),
            session: None,
            open_path: None,
            state_id: None,
            save_slots: std::array::from_fn(|_| None),
            last: Instant::now(),
            previous_pad: Pad::default(),
            controller: Controller::default(),
            controller_connected: false,
            keyboard: 0,
            overlay: Overlay::default(),
            safe: [0.0; 4],
            screen: [0.0; 2],
            scale: 1.0,
            menu: false,
            paused: false,
            fast_forward: Default::default(),
            fast_forwarding: false,
            rewind: Default::default(),
            turbo: Default::default(),
            capturing: None,
            editing: false,
            dragging: None,
            chosen: None,
            frame: Vec::new(),
            frame_key: None,
            frame_serial: 0,
            width: 0,
            height: 0,
            overlay_look: OverlayLook::default(),
            overlay_revision: 0,
            elements_serial: 0,
            indicator: CString::default(),
            debug_line: CString::default(),
            error: CString::default(),
        };
        if let Some(text) = dir_error {
            app.set_error(&text);
        }
        app.relocate_soundfont();
        app.start_scan();
        app
    }

    fn set_error(&mut self, text: &str) {
        self.error = c_text(text);
    }

    /// Saves the preferences. Every settings change goes through [`App::changed`], which calls
    /// this.
    fn save(&self) {
        if let Err(error) = self.settings.save_to(&self.file) {
            eprintln!("could not save the settings: {error}");
        }
    }

    /// The settings changed: they go to disk, the settings screen redraws and the on-screen
    /// controls are placed again, since size, position and which pieces show all live there.
    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.save();
        self.place_elements();
    }

    /// Rereads the ROM folder without blocking the thread that draws the interface.
    fn start_scan(&mut self) {
        if self.scan.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let roms = self.roms.clone();
        let spawned = std::thread::Builder::new()
            .name("zeebx-library".to_string())
            .spawn(move || {
                let games = library::scan(&roms);
                let installed = games
                    .iter()
                    .filter_map(|game| Some((game.clsid?, library::module_id(&game.path)?)))
                    .collect();
                let _ = tx.send((games, installed));
            });
        match spawned {
            Ok(_) => self.scan = Some(rx),
            Err(error) => eprintln!("could not start the library scan: {error}"),
        }
        self.revision = self.revision.wrapping_add(1);
    }

    /// Publishes a finished scan. Says whether the list changed in this call.
    fn finish_scan(&mut self, wait: bool) -> bool {
        let Some(rx) = self.scan.as_ref() else {
            return false;
        };
        let result = match wait {
            true => rx.recv().map_err(|_| TryRecvError::Disconnected),
            false => rx.try_recv(),
        };
        match result {
            Ok((games, installed)) => {
                self.games = games;
                self.installed = installed;
            }
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                eprintln!("the library scan ended without a result");
            }
        }
        self.scan = None;
        self.revision = self.revision.wrapping_add(1);
        true
    }

    fn library_json(&self) -> String {
        let games: Vec<_> = self
            .games
            .iter()
            .map(|game| json!({ "title": game.title, "cover": game.art.is_some() }))
            .collect();
        json!({ "scanning": self.scan.is_some(), "games": games }).to_string()
    }

    /// The pieces of the on-screen controls, for the current screen and settings.
    fn place_elements(&mut self) {
        let controls = &self.settings.on_screen_controls;
        let [_, _, width, height] = self.safe;
        let mut elements = match width > 0.0 && height > 0.0 {
            true => touch::build([width, height], controls.scale, &controls.positions, &controls.sizes),
            false => Vec::new(),
        };
        // Shortcut pieces only when the feature is in use: see `OnScreenControls::show`. The
        // editor follows the same rule — arranging a button that does not appear helps nobody.
        elements.retain(|element| controls.show(element.touch_control, &self.settings));
        self.overlay.set_elements(elements);
        self.elements_serial = self.elements_serial.wrapping_add(1);
    }

    /// From Swift's coordinates, which are the whole screen's, to the safe area the pieces live
    /// in.
    fn to_safe(&self, x: f32, y: f32) -> [f32; 2] {
        [x - self.safe[0], y - self.safe[1]]
    }

    /// Whether the fingers go to the game: a game on screen, nothing over it.
    fn touch_plays(&self) -> bool {
        self.session.is_some() && !self.menu && !self.editing
    }

    fn view_state(&mut self) -> ViewState {
        let controls = &self.settings.on_screen_controls;
        let visible = match self.editing {
            true => true,
            false => self.session.is_some() && !self.menu && self.overlay.visible(controls),
        };
        // In the editor the pieces show up even with opacity turned all the way down: nobody
        // drags what they can't see.
        let opacity = match self.editing {
            true => controls.opacity.max(70),
            false => controls.opacity,
        };
        let playing = visible && !self.editing;
        let look = OverlayLook {
            visible,
            opacity,
            buttons: if playing { self.overlay.buttons } else { 0 },
            shortcuts: match playing {
                true => {
                    let held = self.overlay.shortcuts;
                    [held.turbo, held.fast_forward, held.rewind]
                }
                false => [false; 3],
            },
            axes: match playing {
                true => self.overlay.axes.map(f32::to_bits),
                false => [0; 4],
            },
            elements: self.elements_serial,
            chosen: self.chosen.filter(|_| self.editing).map(TouchControl::key_id),
        };
        if look != self.overlay_look {
            self.overlay_look = look;
            self.overlay_revision = self.overlay_revision.wrapping_add(1);
        }

        // "Whole pixels" is about the device's pixels, not its points: the frame is fitted in
        // pixels and handed back in points.
        let scale = self.scale.max(1.0);
        let area = [self.screen[0] * scale, self.screen[1] * scale];
        let aspect = match self.height {
            0 => 4.0 / 3.0,
            height => self.width as f32 / height as f32,
        };
        let graphics = &self.settings.graphics;
        let [width, height] = fit_viewport(area, graphics.scaling, graphics.keep_aspect, aspect);
        ViewState {
            frame_serial: self.frame_serial,
            frame_width: width / scale,
            frame_height: height / scale,
            smooth: i32::from(graphics.smooth),
            overlay_revision: self.overlay_revision,
            overlay_opacity: i32::from(opacity),
        }
    }

    /// The pieces as they are to be drawn now. Empty when they are hidden.
    fn elements_out(&self) -> Vec<ElementOut> {
        if !self.overlay_look.visible {
            return Vec::new();
        }
        let look = &self.overlay_look;
        let [dpad_up, dpad_down, dpad_left, dpad_right] = input::DPAD;
        self.overlay
            .elements
            .iter()
            .map(|element: &Element| {
                let control = element.touch_control;
                let down = |index: usize| look.buttons & (1 << index) != 0;
                let pressed = match control {
                    TouchControl::Dpad => [dpad_up, dpad_down, dpad_left, dpad_right]
                        .into_iter()
                        .enumerate()
                        .fold(0, |bits, (arm, index)| bits | (u32::from(down(index)) << arm)),
                    TouchControl::Turbo => u32::from(look.shortcuts[0]),
                    TouchControl::FastForward => u32::from(look.shortcuts[1]),
                    TouchControl::Rewind => u32::from(look.shortcuts[2]),
                    other => u32::from(other.index().is_some_and(down)),
                };
                let [knob_x, knob_y] = match control.axes() {
                    Some([horizontal, vertical]) => [
                        f32::from_bits(look.axes[horizontal]),
                        f32::from_bits(look.axes[vertical]),
                    ],
                    None => [0.0, 0.0],
                };
                ElementOut {
                    kind: TouchControl::ALL.iter().position(|known| *known == control).unwrap_or(0)
                        as i32,
                    center_x: element.center[0] + self.safe[0],
                    center_y: element.center[1] + self.safe[1],
                    half_width: element.middle[0],
                    half_height: element.middle[1],
                    // Android's rule: a round piece is a circle only when its halves are equal.
                    // HOME is wider than it is tall, and is drawn as a pill like the triggers.
                    round: i32::from(control.is_round() && element.middle[0] == element.middle[1]),
                    pressed,
                    knob_x,
                    knob_y,
                    // The highlight applies to the whole group: choosing 3 lights all four.
                    highlighted: i32::from(look.chosen == Some(control.key_id())),
                }
            })
            .collect()
    }

    /// A game's cover, or a save state's thumbnail, laid out the way `CGImage` takes it.
    fn cover(&mut self, index: usize) -> Option<(u32, u32)> {
        let art = self.games.get(index)?.art.as_ref()?;
        self.image.clear();
        self.image.reserve(art.rgba.len());
        for pixel in art.rgba.chunks_exact(4) {
            // `CGImage` wants the color already multiplied by the alpha; the core keeps it apart.
            let alpha = u32::from(pixel[3]);
            let mix = |channel: u8| ((u32::from(channel) * alpha + 127) / 255) as u8;
            self.image.extend_from_slice(&[mix(pixel[2]), mix(pixel[1]), mix(pixel[0]), pixel[3]]);
        }
        Some((art.width as u32, art.height as u32))
    }

    fn thumbnail(&mut self, slot: usize) -> Option<(u32, u32)> {
        let thumbnail = self.save_slots.get(slot)?.as_ref()?.thumbnail.as_ref()?;
        let pixels: Vec<u16> = thumbnail
            .rgb565
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        if pixels.len() != thumbnail.width * thumbnail.height {
            return None;
        }
        self.image.clear();
        bgra_from_rgb565(&pixels, &mut self.image);
        Some((thumbnail.width as u32, thumbnail.height as u32))
    }

    fn slots_json(&self) -> String {
        let slots: Vec<_> = self
            .save_slots
            .iter()
            .map(|slot| match slot {
                Some(saved) => json!({
                    "used": true,
                    "size": save_slots::size(saved.bytes),
                    "thumbnail": saved.thumbnail.is_some(),
                }),
                None => json!({ "used": false, "size": "", "thumbnail": false }),
            })
            .collect();
        json!({ "available": self.state_id.is_some(), "paused": self.paused, "slots": slots })
            .to_string()
    }
}

/// Console RGB565 to BGRA8, which is what a little-endian `CGImage` with alpha first asks for.
fn bgra_from_rgb565(pixels: &[u16], output: &mut Vec<u8>) {
    output.reserve(pixels.len() * 4);
    for &pixel in pixels {
        let r5 = (pixel >> 11) & 0x1f;
        let g6 = (pixel >> 5) & 0x3f;
        let b5 = pixel & 0x1f;
        // The 5-bit channel becomes 8 by repeating the high bits in the low ones: otherwise the
        // console's white (31) comes out as 248 and the screen looks washed out.
        let r = ((r5 << 3) | (r5 >> 2)) as u8;
        let g = ((g6 << 2) | (g6 >> 4)) as u8;
        let b = ((b5 << 3) | (b5 >> 2)) as u8;
        output.extend_from_slice(&[b, g, r, 255]);
    }
}

/// A text for Swift. A NUL in the middle would cut it short on the other side; it is dropped.
fn c_text(text: &str) -> CString {
    CString::new(text.replace('\0', "")).unwrap_or_default()
}

/// A text Swift owns from here on, and gives back through [`zeebx_ios_free`].
fn owned(text: &str) -> *mut c_char {
    c_text(text).into_raw()
}

fn path(ptr: *const c_char) -> Option<PathBuf> {
    c_string(ptr).filter(|text| !text.is_empty()).map(PathBuf::from)
}

fn app_from_ptr<'a>(ptr: *mut App) -> Option<&'a mut App> {
    if ptr.is_null() {
        None
    } else {
        // The pointer is what `zeebx_ios_create` returned and `zeebx_ios_destroy` has not yet
        // consumed. Swift uses it from a single thread.
        Some(unsafe { &mut *ptr })
    }
}

fn c_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // The pointer came from Swift in this call and stays valid until it returns.
    Some(unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
}

/// Writes an image's size where Swift asked, if it asked.
fn write_size(size: (u32, u32), width: *mut u32, height: *mut u32) {
    if !width.is_null() {
        unsafe { *width = size.0 };
    }
    if !height.is_null() {
        unsafe { *height = size.1 };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_create(
    documents: *const c_char,
    support: *const c_char,
    language: *const c_char,
) -> *mut App {
    let (Some(documents), Some(support)) = (path(documents), path(support)) else {
        return ptr::null_mut();
    };

    // **iOS is not `macos` for `config_dir`.** Without a `HOME` inside the sandbox, the core
    // falls back to the current directory — which is not writable here — and a SoundFont or
    // catalog lookup looks like a ROM error. The Documents folder lives at the container root;
    // its parent is the place the system treats as the app's home.
    //
    // Only on this target. In a host `cargo test` the process is cargo's, and changing its `HOME`
    // would change it for everything running alongside.
    #[cfg(target_os = "ios")]
    if let Some(home) = documents.parent() {
        unsafe { std::env::set_var("HOME", home) };
    }

    let language = c_string(language).unwrap_or_default();
    Box::into_raw(Box::new(App::create(&documents, &support, &language)))
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_destroy(app: *mut App) {
    if !app.is_null() {
        drop(unsafe { Box::from_raw(app) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_free(text: *mut c_char) {
    if !text.is_null() {
        drop(unsafe { CString::from_raw(text) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_tr(app: *mut App, key: *const c_char) -> *mut c_char {
    let (Some(app), Some(key)) = (app_from_ptr(app), c_string(key)) else {
        return ptr::null_mut();
    };
    owned(app.catalog.get(&key))
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_revision(app: *mut App) -> u32 {
    app_from_ptr(app).map_or(0, |app| app.revision)
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_scan(app: *mut App) {
    if let Some(app) = app_from_ptr(app) {
        app.start_scan();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_scan_poll(app: *mut App) -> i32 {
    app_from_ptr(app).map_or(0, |app| i32::from(app.finish_scan(false)))
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_library_json(app: *mut App) -> *mut c_char {
    app_from_ptr(app).map_or(ptr::null_mut(), |app| owned(&app.library_json()))
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_cover(
    app: *mut App,
    index: i32,
    width: *mut u32,
    height: *mut u32,
) -> *const u8 {
    let Some(app) = app_from_ptr(app) else {
        return ptr::null();
    };
    let Some(size) = usize::try_from(index).ok().and_then(|index| app.cover(index)) else {
        return ptr::null();
    };
    write_size(size, width, height);
    app.image.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_start(app: *mut App, index: i32) -> i32 {
    let Some(app) = app_from_ptr(app) else {
        return -1;
    };
    let game = usize::try_from(index).ok().and_then(|index| app.games.get(index));
    let Some(path) = game.map(|game| game.path.clone()) else {
        app.set_error("game not in the list");
        return -1;
    };
    match app.open(&path) {
        true => 0,
        false => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_stop(app: *mut App) {
    if let Some(app) = app_from_ptr(app) {
        app.close();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_restart(app: *mut App) -> i32 {
    match app_from_ptr(app).map(App::restart) {
        Some(true) => 0,
        _ => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_step(app: *mut App) -> i32 {
    app_from_ptr(app).map_or(NO_SESSION, App::step)
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_frame(
    app: *mut App,
    width: *mut u32,
    height: *mut u32,
) -> *const u8 {
    let Some(app) = app_from_ptr(app) else {
        return ptr::null();
    };
    if app.frame.is_empty() || app.width == 0 || app.height == 0 {
        return ptr::null();
    }
    write_size((app.width, app.height), width, height);
    app.frame.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_layout(
    app: *mut App,
    width: f32,
    height: f32,
    safe_x: f32,
    safe_y: f32,
    safe_width: f32,
    safe_height: f32,
    scale: f32,
) {
    let Some(app) = app_from_ptr(app) else {
        return;
    };
    let safe = [safe_x, safe_y, safe_width, safe_height];
    if app.screen == [width, height] && app.safe == safe && app.scale == scale {
        return;
    }
    app.screen = [width, height];
    app.safe = safe;
    app.scale = scale;
    app.place_elements();
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_view(app: *mut App, out: *mut ViewState) {
    let Some(app) = app_from_ptr(app) else {
        return;
    };
    if !out.is_null() {
        unsafe { *out = app.view_state() };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_elements(app: *mut App, out: *mut ElementOut, capacity: i32) -> i32 {
    let Some(app) = app_from_ptr(app) else {
        return 0;
    };
    let elements = app.elements_out();
    let count = elements.len().min(usize::try_from(capacity).unwrap_or(0));
    if out.is_null() {
        return 0;
    }
    // `out` is an array of `capacity` elements that Swift owns for the length of this call.
    unsafe { std::slice::from_raw_parts_mut(out, count) }.copy_from_slice(&elements[..count]);
    count as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_element_label(kind: i32) -> *const c_char {
    // The labels are the core's, and they never change: built once, they live as long as the
    // process, and the pointer with them.
    static LABELS: std::sync::OnceLock<Vec<CString>> = std::sync::OnceLock::new();
    let labels = LABELS.get_or_init(|| {
        TouchControl::ALL.iter().map(|control| c_text(control.label())).collect()
    });
    usize::try_from(kind)
        .ok()
        .and_then(|kind| labels.get(kind))
        .map_or(ptr::null(), |label| label.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_touch(app: *mut App, id: i64, phase: i32, x: f32, y: f32) {
    let Some(app) = app_from_ptr(app) else {
        return;
    };
    if !app.touch_plays() {
        return;
    }
    let point = app.to_safe(x, y);
    match phase {
        0 => app.overlay.down(id, point, &app.settings.on_screen_controls),
        1 => app.overlay.moved(id, point),
        _ => app.overlay.up(id),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_controller(
    app: *mut App,
    connected: i32,
    buttons: u32,
    left_x: f32,
    left_y: f32,
    right_x: f32,
    right_y: f32,
) {
    let Some(app) = app_from_ptr(app) else {
        return;
    };
    let connected = connected != 0;
    if connected != app.controller_connected {
        app.controller_connected = connected;
        app.revision = app.revision.wrapping_add(1);
    }
    if !connected {
        app.controller.clear();
        return;
    }
    let pressed = app.controller.update(buttons, [left_x, left_y, right_x, right_y]);
    if app.controller.active() {
        app.overlay.saw_controller();
    }
    // The lowest bit that went down, if the capture takes it. One button per press: two fingers
    // landing together must not bind two things.
    let captured = (0..controller::NAMES.len())
        .find(|bit| pressed & (1 << bit) != 0 && app.capture_accepts(*bit));
    if let Some(bit) = captured {
        app.captured_button(controller::NAMES[bit]);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_button(app: *mut App, index: u32, pressed: i32) {
    let Some(app) = app_from_ptr(app) else {
        return;
    };
    if index as usize >= input::BUTTONS {
        return;
    }
    match pressed != 0 {
        true => app.keyboard |= 1 << index,
        false => app.keyboard &= !(1 << index),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_button_by_name(name: *const c_char) -> i32 {
    c_string(name)
        .and_then(|name| Pad::button_by_name(&name))
        .map_or(-1, |index| index as i32)
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_release_input(app: *mut App) {
    if let Some(app) = app_from_ptr(app) {
        app.release_input();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_set_menu(app: *mut App, open: i32) {
    if let Some(app) = app_from_ptr(app) {
        app.set_menu(open != 0);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_set_paused(app: *mut App, paused: i32) {
    if let Some(app) = app_from_ptr(app) {
        app.paused = paused != 0;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_slots_json(app: *mut App) -> *mut c_char {
    app_from_ptr(app).map_or(ptr::null_mut(), |app| owned(&app.slots_json()))
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_slot_thumbnail(
    app: *mut App,
    slot: i32,
    width: *mut u32,
    height: *mut u32,
) -> *const u8 {
    let Some(app) = app_from_ptr(app) else {
        return ptr::null();
    };
    let Some(size) = usize::try_from(slot).ok().and_then(|slot| app.thumbnail(slot)) else {
        return ptr::null();
    };
    write_size(size, width, height);
    app.image.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_save_state(app: *mut App, slot: i32) -> i32 {
    let (Some(app), Ok(slot)) = (app_from_ptr(app), usize::try_from(slot)) else {
        return -1;
    };
    match app.save_state(slot) {
        true => 0,
        false => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_load_state(app: *mut App, slot: i32) -> i32 {
    let (Some(app), Ok(slot)) = (app_from_ptr(app), usize::try_from(slot)) else {
        return -1;
    };
    match app.load_state(slot) {
        true => 0,
        false => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_indicator(app: *mut App) -> *const c_char {
    app_from_ptr(app).map_or(ptr::null(), |app| app.indicator.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_debug_line(app: *mut App) -> *const c_char {
    app_from_ptr(app).map_or(ptr::null(), |app| app.debug_line.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_settings_json(app: *mut App, tab: *const c_char) -> *mut c_char {
    let Some(app) = app_from_ptr(app) else {
        return ptr::null_mut();
    };
    owned(&app.settings_json(&c_string(tab).unwrap_or_default()))
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_settings_set(app: *mut App, id: *const c_char, value: i32) {
    if let (Some(app), Some(id)) = (app_from_ptr(app), c_string(id)) {
        app.settings_set(&id, value);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_editor(app: *mut App, open: i32) {
    if let Some(app) = app_from_ptr(app) {
        app.set_editing(open != 0);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_editor_touch(app: *mut App, phase: i32, x: f32, y: f32) {
    let Some(app) = app_from_ptr(app) else {
        return;
    };
    let point = app.to_safe(x, y);
    match phase {
        0 => app.editor_grab(point),
        1 => app.editor_drag(point),
        _ => app.editor_drop(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_editor_resize(app: *mut App, steps: i32) {
    if let Some(app) = app_from_ptr(app) {
        app.editor_resize(steps);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_editor_reset(app: *mut App) {
    if let Some(app) = app_from_ptr(app) {
        app.editor_reset();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_editor_selection(app: *mut App) -> *mut c_char {
    app_from_ptr(app)
        .and_then(|app| app.editor_selection())
        .map_or(ptr::null_mut(), |text| owned(&text))
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_error(app: *mut App) -> *const c_char {
    app_from_ptr(app)
        .map(|app| app.error.as_ptr())
        .unwrap_or(ptr::null())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app over an empty sandbox of its own, and the folder to erase afterwards.
    pub(crate) fn test_app(name: &str) -> (PathBuf, App) {
        let root = std::env::temp_dir().join(format!("zeebx-ios-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let documents = root.join("Documents");
        let support = root.join("Library/Application Support");
        std::fs::create_dir_all(&documents).unwrap();
        std::fs::create_dir_all(&support).unwrap();
        let mut app = App::create(&documents, &support, "en");
        app.finish_scan(true);
        (root, app)
    }

    /// The number a `#define` of the header gives `name`.
    fn defined(name: &str) -> Option<i32> {
        include_str!("../include/zeebx_ios.h").lines().find_map(|line| {
            let mut words = line.split_whitespace();
            (words.next()? == "#define" && words.next()? == name).then_some(())?;
            words.next()?.trim_matches(['(', ')']).parse().ok()
        })
    }

    /// Swift only knows the header. What it says has to be what this side does.
    #[test]
    fn the_header_says_the_same_numbers_as_this_side() {
        for (name, code) in [
            ("ZEEBX_NO_SESSION", NO_SESSION),
            ("ZEEBX_FRAME", FRAME),
            ("ZEEBX_RUNNING", RUNNING),
            ("ZEEBX_AHEAD", AHEAD),
            ("ZEEBX_STOPPED", STOPPED),
        ] {
            assert_eq!(defined(name), Some(code), "{name}");
        }
        for (bit, name) in controller::NAMES.iter().enumerate() {
            // `LeftTrigger2` is `LEFT_TRIGGER2`, and `DPadUp` is `DPAD_UP`.
            let mut shouted = String::new();
            let mut previous = 'A';
            for letter in name.chars() {
                if letter.is_ascii_uppercase() && !previous.is_ascii_uppercase() {
                    shouted.push('_');
                }
                shouted.push(letter.to_ascii_uppercase());
                previous = letter;
            }
            assert_eq!(defined(&format!("ZEEBX_PAD_{shouted}")), Some(bit as i32), "{name}");
        }
        let piece = |control| TouchControl::ALL.iter().position(|known| *known == control);
        assert_eq!(defined("ZEEBX_PIECE_DPAD").map(|n| n as usize), piece(TouchControl::Dpad));
        assert_eq!(
            defined("ZEEBX_PIECE_LEFT_STICK").map(|n| n as usize),
            piece(TouchControl::LeftStick)
        );
        assert_eq!(
            defined("ZEEBX_PIECE_RIGHT_STICK").map(|n| n as usize),
            piece(TouchControl::RightStick)
        );
        // The two structs are plain 4-byte fields on both sides: the size is the field count.
        assert_eq!(std::mem::size_of::<ViewState>(), 6 * 4);
        assert_eq!(std::mem::size_of::<ElementOut>(), 10 * 4);
        assert!(TouchControl::ALL.len() <= 16, "the buffer Swift hands to zeebx_ios_elements");
    }

    #[test]
    fn rgb565_white_and_black_become_opaque_bgra() {
        let pixels = [0u16, 0xffff, 0xf800];
        let mut bgra = Vec::new();
        bgra_from_rgb565(&pixels, &mut bgra);
        assert_eq!(&bgra[0..4], &[0, 0, 0, 255]);
        assert_eq!(&bgra[4..8], &[255, 255, 255, 255]);
        assert_eq!(bgra[8], 0, "blue");
        assert_eq!(bgra[9], 0, "green");
        assert_eq!(bgra[10], 255, "red");
        assert_eq!(bgra[11], 255);
    }

    #[test]
    fn a_null_pointer_creates_nothing() {
        assert!(zeebx_ios_create(ptr::null(), ptr::null(), ptr::null()).is_null());
        let empty = CString::new("").unwrap();
        assert!(zeebx_ios_create(empty.as_ptr(), empty.as_ptr(), ptr::null()).is_null());
    }

    #[test]
    fn button_names_follow_the_controller() {
        assert_eq!(zeebx_ios_button_by_name(c"b1".as_ptr()), 0);
        assert_eq!(
            zeebx_ios_button_by_name(c"up".as_ptr()) as usize,
            Pad::button_by_name("up").unwrap()
        );
        assert_eq!(
            zeebx_ios_button_by_name(c"home".as_ptr()),
            zeebx_ios_button_by_name(c"back".as_ptr())
        );
        assert_eq!(zeebx_ios_button_by_name(ptr::null()), -1);
        assert_eq!(zeebx_ios_button_by_name(c"nao-existe".as_ptr()), -1);
    }

    #[test]
    fn an_empty_dir_lists_zero_and_starts_no_game() {
        let root = std::env::temp_dir().join(format!("zeebx-ios-vazia-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let documents = root.join("Documents");
        let support = root.join("Library/Application Support");
        std::fs::create_dir_all(&documents).unwrap();
        std::fs::create_dir_all(&support).unwrap();
        let documents_c = CString::new(documents.to_str().unwrap()).unwrap();
        let support_c = CString::new(support.to_str().unwrap()).unwrap();

        let app = zeebx_ios_create(documents_c.as_ptr(), support_c.as_ptr(), c"pt-BR".as_ptr());
        assert!(!app.is_null());
        app_from_ptr(app).unwrap().finish_scan(true);
        let library = zeebx_ios_library_json(app);
        let text = unsafe { CStr::from_ptr(library) }.to_str().unwrap().to_string();
        zeebx_ios_free(library);
        let library: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(library["scanning"], false);
        assert_eq!(library["games"].as_array().unwrap().len(), 0);

        assert!(zeebx_ios_cover(app, 0, ptr::null_mut(), ptr::null_mut()).is_null());
        assert_eq!(zeebx_ios_start(app, 0), -1);
        assert!(!zeebx_ios_error(app).is_null());
        assert_eq!(zeebx_ios_step(app), NO_SESSION);
        assert!(zeebx_ios_frame(app, ptr::null_mut(), ptr::null_mut()).is_null());
        assert_eq!(zeebx_ios_save_state(app, 0), -1);
        assert_eq!(zeebx_ios_restart(app), -1);
        zeebx_ios_button(app, 0, 1);
        zeebx_ios_controller(app, 1, 1, 0.0, 0.0, 0.0, 0.0);
        zeebx_ios_touch(app, 1, 0, 10.0, 10.0);
        zeebx_ios_stop(app);
        zeebx_ios_destroy(app);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_device_language_is_used_until_one_is_chosen() {
        let (root, app) = test_app("idioma");
        assert_eq!(app.catalog.current(), "en");
        drop(app);
        let support = root.join("Library/Application Support");
        let app = App::create(&root.join("Documents"), &support, "pt_PT");
        assert_eq!(app.catalog.current(), "pt-BR", "the nearest Portuguese");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_labels_handed_to_swift_are_the_cores() {
        let home = TouchControl::ALL.iter().position(|c| *c == TouchControl::Home).unwrap();
        let label = unsafe { CStr::from_ptr(zeebx_ios_element_label(home as i32)) };
        assert_eq!(label.to_str().unwrap(), "HOME");
        assert!(zeebx_ios_element_label(-1).is_null());
        assert!(zeebx_ios_element_label(TouchControl::ALL.len() as i32).is_null());
    }

    #[test]
    fn the_pieces_follow_the_safe_area_and_hide_without_a_game() {
        let (root, mut app) = test_app("pecas");
        app.screen = [844.0, 390.0];
        app.safe = [47.0, 0.0, 750.0, 369.0];
        app.scale = 3.0;
        app.place_elements();

        // No game and no editor: nothing to draw.
        app.view_state();
        assert!(app.elements_out().is_empty());

        app.set_editing(true);
        let first = app.view_state();
        let pieces = app.elements_out();
        assert!(!pieces.is_empty());
        assert!(pieces.iter().all(|piece| piece.center_x - piece.half_width >= 47.0));
        assert!(pieces.iter().all(|piece| piece.center_x + piece.half_width <= 797.0));
        assert_eq!(first.overlay_opacity, 70, "the editor never hides what it edits");

        // Asking again with nothing changed is not a reason to redraw.
        assert_eq!(app.view_state().overlay_revision, first.overlay_revision);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn whole_pixels_count_the_devices_pixels() {
        let (root, mut app) = test_app("escala");
        app.screen = [844.0, 390.0];
        app.scale = 3.0;
        app.settings.graphics.scaling = zeebx::ui::settings::Scaling::Integer;
        // 2532x1170 pixels hold the 640x480 frame twice; in points that is 320 tall.
        let view = app.view_state();
        assert_eq!(view.frame_height, 320.0);
        assert!((view.frame_width - 426.666_66).abs() < 0.01);
        let _ = std::fs::remove_dir_all(root);
    }
}
