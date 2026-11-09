//! Zeebx on iOS: the same session, a different screen.
//!
//! The emulation is done by [`zeebx::session::Session`]. This file is the border with UIKit: Swift
//! asks for a scan, a step and a frame, and hands back buttons. The window, touch and the real
//! controller live in the Xcode project, because it is the one that owns the UIKit lifecycle.
//!
//! **There is no Dynarmic on this target.** The JIT code allocator asks for an executable page,
//! and iOS — the simulator included — refuses that to a third-party app. The core already has the
//! interpreter for the case where the JIT cannot run; here it is the one.
//!
//! Calls come from a single thread. The audio mixer is what crosses threads, and that is already
//! the `cpal` contract on desktop.

use std::ffi::{CStr, CString, c_char};
use std::path::PathBuf;
use std::ptr;
use std::time::Instant;

use zeebx::input::{self, Pad};
use zeebx::library;
use zeebx::session::{self, MAX_SLICE, Session, Step};
use zeebx::storage::StoragePaths;

/// The same numbers as `include/zeebx_ios.h`. The test below checks what can be checked from
/// this side; the header is what Swift includes.
const NO_SESSION: i32 = -1;
const FRAME: i32 = 0;
const RUNNING: i32 = 1;
const AHEAD: i32 = 2;
const STOPPED: i32 = 3;

/// The app, on this side of the border.
///
/// Public because the `extern "C"` signature names it. The fields stay private: Swift only sees
/// the pointer, and there is no constructor outside the functions in here.
pub struct App {
    roms: PathBuf,
    games: Vec<library::Game>,
    /// One `CString` per title, so the returned pointer stays valid.
    titles: Vec<CString>,
    session: Option<Session>,
    pad: Pad,
    previous_pad: Pad,
    last: Instant,
    /// BGRA8 of the current frame. Swift copies it before the next turn.
    frame: Vec<u8>,
    width: u32,
    height: u32,
    error: CString,
    storage: StoragePaths,
}

impl App {
    fn set_error(&mut self, text: &str) {
        self.error = CString::new(text).unwrap_or_else(|_| CString::new("error").expect("no NUL"));
    }

    fn scan(&mut self) -> i32 {
        self.games = library::scan(&self.roms);
        self.titles = self
            .games
            .iter()
            .map(|game| {
                CString::new(game.title.as_str())
                    .unwrap_or_else(|_| CString::new("game").expect("no NUL"))
            })
            .collect();
        self.titles.len() as i32
    }

    fn publish(&mut self) {
        let Some(session) = self.session.as_ref() else {
            self.frame.clear();
            self.width = 0;
            self.height = 0;
            return;
        };
        let screen = session.screen();
        self.width = screen.width();
        self.height = screen.height();
        self.frame = bgra_from_rgb565(screen.pixels());
    }
}

/// Console RGB565 to BGRA8, which is what a little-endian `CGImage` with alpha first asks for.
fn bgra_from_rgb565(pixels: &[u16]) -> Vec<u8> {
    let mut output = Vec::with_capacity(pixels.len() * 4);
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
    output
}

fn path(ptr: *const c_char) -> Option<PathBuf> {
    if ptr.is_null() {
        return None;
    }
    // The pointer came from Swift in this call and stays valid until it returns.
    let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
    if text.is_empty() {
        return None;
    }
    Some(PathBuf::from(text.as_ref()))
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
    Some(
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned(),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_create(
    documents: *const c_char,
    support: *const c_char,
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

    let roms = documents.join("roms");
    let _ = std::fs::create_dir_all(&roms);
    let storage = StoragePaths::for_frontend(&support, Some(&support));
    let dir_error = storage.create_dirs().err().map(|error| error.to_string());

    let mut app = App {
        roms,
        games: Vec::new(),
        titles: Vec::new(),
        session: None,
        pad: Pad::default(),
        previous_pad: Pad::default(),
        last: Instant::now(),
        frame: Vec::new(),
        width: 0,
        height: 0,
        error: CString::new("").expect("empty"),
        storage,
    };
    if let Some(text) = dir_error {
        app.set_error(&text);
    }
    app.scan();
    Box::into_raw(Box::new(app))
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_destroy(app: *mut App) {
    if !app.is_null() {
        drop(unsafe { Box::from_raw(app) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_scan(app: *mut App) -> i32 {
    app_from_ptr(app).map(|app| app.scan()).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_title(app: *mut App, index: i32) -> *const c_char {
    let Some(app) = app_from_ptr(app) else {
        return ptr::null();
    };
    let Ok(index) = usize::try_from(index) else {
        return ptr::null();
    };
    app.titles
        .get(index)
        .map(|title| title.as_ptr())
        .unwrap_or(ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_start(app: *mut App, index: i32) -> i32 {
    let Some(app) = app_from_ptr(app) else {
        return -1;
    };
    let Ok(index) = usize::try_from(index) else {
        app.set_error("game not in the list");
        return -1;
    };
    let Some(game) = app.games.get(index) else {
        app.set_error("game not in the list");
        return -1;
    };
    let path = game.path.clone();
    let installed: Vec<(u32, String)> = app
        .games
        .iter()
        .filter_map(|game| Some((game.clsid?, library::module_id(&game.path)?)))
        .collect();

    // No GPU: on iOS the frame is the software rasterizer's, painted by UIKit. The core's GL
    // context asks for EGL, and there is none here.
    match Session::start_software_with_storage_installed(
        &path,
        zeebx::DEFAULT_PORTS,
        zeebx::config::ZWheel::default(),
        &app.storage,
        &installed,
    ) {
        Ok(mut session) => {
            // A device with no audio output cannot keep the game from running. The reason goes
            // into the error only when the session itself fails to start; missing sound goes to
            // the log.
            if let Some(reason) = session.set_audio(true, 100) {
                eprintln!("no sound: {reason}");
            }
            app.session = Some(session);
            app.pad = Pad::default();
            app.previous_pad = Pad::default();
            app.last = Instant::now();
            app.error = CString::new("").expect("empty");
            app.publish();
            0
        }
        Err(error) => {
            app.set_error(&error.to_string());
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_stop(app: *mut App) {
    let Some(app) = app_from_ptr(app) else {
        return;
    };
    app.session = None;
    app.pad = Pad::default();
    app.previous_pad = Pad::default();
    app.frame.clear();
    app.width = 0;
    app.height = 0;
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_step(app: *mut App) -> i32 {
    let Some(app) = app_from_ptr(app) else {
        return NO_SESSION;
    };
    if app.session.is_none() {
        return NO_SESSION;
    }

    let before = app.previous_pad;
    let pad = app.pad;
    {
        let session = app.session.as_mut().expect("checked above");
        session.set_port_pad(0, pad);
        for (avk, pressed) in input::controller_key_events(&before, &pad) {
            session.set_key(avk, pressed);
        }
    }
    app.previous_pad = pad;

    // The budget is the real time since the previous turn, clamped to the ceiling: it is how
    // much the game needs to emulate to keep up with the wall clock. The ceiling exists so a turn
    // that took long — the app came back from the background — does not become a jump of minutes.
    let now_instant = Instant::now();
    let slice = now_instant
        .saturating_duration_since(app.last)
        .min(MAX_SLICE);
    app.last = now_instant;

    let intermediate = app
        .session
        .as_mut()
        .expect("checked above")
        .show_intermediate_frame();
    if intermediate {
        app.publish();
        return FRAME;
    }

    let step = app
        .session
        .as_mut()
        .expect("checked above")
        .step(slice, true);
    if step == Step::Stopped {
        let normal = app
            .session
            .as_ref()
            .is_some_and(session::Session::exited_normally);
        let reason = app
            .session
            .as_ref()
            .and_then(session::Session::stopped_reason)
            .unwrap_or_default();
        app.session = None;
        app.pad = Pad::default();
        app.previous_pad = Pad::default();
        if !normal {
            app.set_error(&reason);
        }
        return STOPPED;
    }
    app.publish();
    match step {
        Step::Presented => FRAME,
        Step::Ahead => AHEAD,
        Step::Running => RUNNING,
        Step::Stopped => STOPPED,
    }
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
    if !width.is_null() {
        unsafe { *width = app.width };
    }
    if !height.is_null() {
        unsafe { *height = app.height };
    }
    app.frame.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_button(app: *mut App, index: u32, pressed: i32) {
    let Some(app) = app_from_ptr(app) else {
        return;
    };
    let index = index as usize;
    if index >= input::BUTTONS {
        return;
    }
    app.pad.press(index, pressed != 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_axis(app: *mut App, index: u32, value: i32) {
    let Some(app) = app_from_ptr(app) else {
        return;
    };
    app.pad.set_axis(index as usize, value);
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_button_by_name(name: *const c_char) -> i32 {
    let Some(name) = c_string(name) else {
        return -1;
    };
    Pad::button_by_name(&name)
        .map(|index| index as i32)
        .unwrap_or(-1)
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

    #[test]
    fn rgb565_white_and_black_become_opaque_bgra() {
        let pixels = [0u16, 0xffff, 0xf800];
        let bgra = bgra_from_rgb565(&pixels);
        assert_eq!(&bgra[0..4], &[0, 0, 0, 255]);
        assert_eq!(&bgra[4..8], &[255, 255, 255, 255]);
        assert_eq!(bgra[8], 0, "blue");
        assert_eq!(bgra[9], 0, "green");
        assert_eq!(bgra[10], 255, "red");

        assert_eq!(bgra[11], 255);
    }

    #[test]
    fn a_null_pointer_creates_nothing() {
        assert!(zeebx_ios_create(ptr::null(), ptr::null()).is_null());
        let empty = CString::new("").unwrap();
        assert!(zeebx_ios_create(empty.as_ptr(), empty.as_ptr()).is_null());
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

    fn test_dir(name: &str) -> (PathBuf, CString, CString) {
        let root = std::env::temp_dir().join(format!("zeebx-ios-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let documents = root.join("Documents");
        let support = root.join("Library/Application Support");
        std::fs::create_dir_all(&documents).unwrap();
        std::fs::create_dir_all(&support).unwrap();
        let documents_c = CString::new(documents.to_str().unwrap()).unwrap();
        let support_c = CString::new(support.to_str().unwrap()).unwrap();
        (root, documents_c, support_c)
    }

    #[test]
    fn an_empty_dir_lists_zero_and_starts_no_game() {
        let (root, documents, support) = test_dir("vazia");
        let app = zeebx_ios_create(documents.as_ptr(), support.as_ptr());
        assert!(!app.is_null());
        assert_eq!(zeebx_ios_scan(app), 0);
        assert!(zeebx_ios_title(app, 0).is_null());
        assert_eq!(zeebx_ios_start(app, 0), -1);
        assert!(!zeebx_ios_error(app).is_null());
        assert_eq!(zeebx_ios_step(app), NO_SESSION);
        assert!(zeebx_ios_frame(app, ptr::null_mut(), ptr::null_mut()).is_null());
        zeebx_ios_button(app, 0, 1);
        zeebx_ios_axis(app, 0, 40);
        zeebx_ios_stop(app);
        zeebx_ios_destroy(app);
        let _ = std::fs::remove_dir_all(root);
    }
}
