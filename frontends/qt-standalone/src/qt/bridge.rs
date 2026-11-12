//! The game screen, and the C++ glue it calls.
//!
//! The frame goes into the Qt Quick scene graph through `FrameItem` (`cpp/frame.h`), which is the
//! base of `GameScreen`: with the rasterizer on the GPU, its texture goes in wrapped, without
//! coming back to the CPU and at the internal resolution; otherwise, the RGB565 screen is uploaded
//! as an image (`QImage::Format_RGB16` **is** RGB565), and only when it changed, the same rule as
//! egui. The game itself (opening, running, switching) lives in [`super::emu_core`].

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qsizef.h");
        type QSizeF = cxx_qt_lib::QSizeF;
        include!("cxx-qt-lib/qrectf.h");
        type QRectF = cxx_qt_lib::QRectF;
        include!("cxx-qt-lib/qimage.h");
        type QImage = cxx_qt_lib::QImage;
        include!("cxx-qt-lib/qlist.h");
        type QList_i32 = cxx_qt_lib::QList<i32>;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    unsafe extern "C++Qt" {
        include!("frame.h");
        #[qobject]
        type FrameItem;
    }

    // See `cpp/gl_qt.h`.
    #[namespace = "zeebx"]
    unsafe extern "C++" {
        include!("gl_qt.h");
        fn prepare_gl();
        fn gl_create() -> bool;
        fn gl_make_current() -> bool;
        fn gl_done_current();
        fn gl_destroy();
        fn apply_icon();
        fn gl_function(name: &str) -> usize;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = FrameItem]
        #[qproperty(QString, run_state, cxx_name = "runState")]
        #[qproperty(QString, stopped)]
        #[qproperty(QString, debug_panel, cxx_name = "debugPanel")]
        #[qproperty(QList_i32, timeline, cxx_name = "timeline")]
        #[qproperty(QString, notice_title, cxx_name = "noticeTitle")]
        #[qproperty(QString, notice_status, cxx_name = "noticeStatus")]
        #[qproperty(bool, notice_still, cxx_name = "noticeStill")]
        #[qproperty(f64, notice_rotation, cxx_name = "noticeRotation")]
        #[qproperty(QString, screenshot_text, cxx_name = "screenshotText")]
        #[qproperty(bool, screenshot_failed, cxx_name = "screenshotFailed")]
        #[qproperty(QString, screenshot_dir, cxx_name = "screenshotDir")]
        #[qproperty(i32, screenshot_serial, cxx_name = "screenshotSerial")]
        #[qproperty(QString, state_text, cxx_name = "stateText")]
        #[qproperty(bool, state_failed, cxx_name = "stateFailed")]
        #[qproperty(i32, state_serial, cxx_name = "stateSerial")]
        #[qproperty(QString, speed)]
        #[qproperty(QString, indicators)]
        #[qproperty(QStringList, metrics)]
        type GameScreen = super::GameScreenRust;

        /// One pass: input, emulation and, if the screen changed, a new frame.
        #[qinvokable]
        fn step(self: Pin<&mut Self>);

        /// A `Qt::Key` pressed or released.
        #[qinvokable]
        fn key(self: Pin<&mut Self>, code: i32, pressed: bool);

        /// The window lost focus: no key stays pressed.
        #[qinvokable]
        fn release(self: Pin<&mut Self>);

        /// Closes the open game. The game window calls this when it is closed.
        #[qinvokable]
        fn close(self: Pin<&mut Self>);

        /// Whether the `Qt::Key` is the screenshot shortcut. See `shortcuts.screenshot`.
        #[qinvokable]
        #[cxx_name = "isScreenshotShortcut"]
        fn is_screenshot_shortcut(self: &Self, code: i32) -> bool;

        /// Saves the frame on screen. `timestamp` is the local time, ready for the file name.
        /// The notice arrives later, through `screenshotSerial`.
        #[qinvokable]
        fn screenshot(self: Pin<&mut Self>, timestamp: &QString);

        /// Whether the `Qt::Key`, with the event's `Qt::KeyboardModifiers`, is a save state
        /// shortcut. Asked before anything else in the game window: the combination only matches
        /// with the same modifiers, and a save state's Shift+F9 must not turn into the
        /// screenshot's F9.
        #[qinvokable]
        #[cxx_name = "isStateShortcut"]
        fn is_state_shortcut(self: &Self, code: i32, modifiers: i32) -> bool;

        /// Does what the save state shortcut asks for. The notice arrives through `stateSerial`.
        #[qinvokable]
        #[cxx_name = "stateShortcut"]
        fn state_shortcut(self: Pin<&mut Self>, code: i32, modifiers: i32);

        /// The save state from the menu: `action` 0 saves, 1 loads and 2 undoes the last load, in
        /// `slot` (zero-based), which becomes the current one. The notice arrives through
        /// `stateSerial`.
        #[qinvokable]
        #[cxx_name = "stateFromMenu"]
        fn state_from_menu(self: Pin<&mut Self>, action: i32, slot: i32);

        /// The open game's slots, one per line, for the menu.
        #[qinvokable]
        #[cxx_name = "stateSlots"]
        fn state_slots(self: &Self) -> QStringList;

        /// Which slots hold a state: 1 or 0 per slot, in order. The menu's Load disables the
        /// empty ones.
        #[qinvokable]
        #[cxx_name = "occupiedSlots"]
        fn occupied_slots(self: &Self) -> QList_i32;

        #[qinvokable]
        #[cxx_name = "currentSlot"]
        fn current_slot(self: &Self) -> i32;

        #[qinvokable]
        #[cxx_name = "canUndoLoad"]
        fn can_undo_load(self: &Self) -> bool;

        /// Turns fast-forward on or off from the menu, and returns how it ended up.
        #[qinvokable]
        #[cxx_name = "toggleFastForward"]
        fn toggle_fast_forward(self: Pin<&mut Self>) -> bool;

        #[qinvokable]
        #[cxx_name = "fastForwardLocked"]
        fn fast_forward_locked(self: &Self) -> bool;

        /// Pauses or resumes the game. Returns whether it is now paused.
        #[qinvokable]
        fn pause(self: Pin<&mut Self>) -> bool;

        /// Whether the `Qt::Key` is the restart shortcut. See `shortcuts.restart`.
        #[qinvokable]
        #[cxx_name = "isResetShortcut"]
        fn is_reset_shortcut(self: &Self, code: i32) -> bool;

        /// Restarts the open game. Returns whether it restarted; on failure, the previous one
        /// keeps going, and the status line says why.
        #[qinvokable]
        fn restart(self: Pin<&mut Self>) -> bool;

        /// Rereads the open game's title for the status line. The window calls this when it
        /// appears.
        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);

        /// How the game window opens: 0 windowed, 1 maximized, 2 fullscreen. See
        /// `graphics.game_window`.
        #[qinvokable]
        #[cxx_name = "windowMode"]
        fn window_mode(self: &Self) -> i32;

        /// Whether closing the open game asks for confirmation. See `graphics.confirm_on_close`.
        #[qinvokable]
        #[cxx_name = "confirmOnClose"]
        fn confirm_on_close(self: &Self) -> bool;

        /// The width the game window had when it was closed, in points, or zero with no saved
        /// size. See `graphics.game_window_size`.
        #[qinvokable]
        #[cxx_name = "windowWidth"]
        fn window_width(self: &Self) -> f64;

        /// The height the game window had when it was closed, in points, or zero with no saved
        /// size. See `graphics.game_window_size`.
        #[qinvokable]
        #[cxx_name = "windowHeight"]
        fn window_height(self: &Self) -> f64;

        /// Saves the game window's current size. As in the main one: after a while without
        /// changing size, and on close. See `graphics.game_window_size`.
        #[qinvokable]
        #[cxx_name = "saveWindowSize"]
        fn save_window_size(self: &Self, width: f64, height: f64);

        /// The game exited by itself and there is nowhere to go back to: the window closes, like
        /// egui's.
        #[qsignal]
        fn closed(self: Pin<&mut Self>);

        #[inherit]
        fn size(self: &Self) -> QSizeF;

        #[inherit]
        #[cxx_name = "showTexture"]
        fn show_texture(
            self: Pin<&mut Self>,
            texture: u32,
            clip_u: f32,
            clip_v: f32,
            dest: QRectF,
            smooth: bool,
        );

        #[inherit]
        #[cxx_name = "showImage"]
        fn show_image(self: Pin<&mut Self>, image: &QImage, dest: QRectF, smooth: bool);

        #[inherit]
        fn place(self: Pin<&mut Self>, dest: QRectF, smooth: bool);

        #[inherit]
        fn clear(self: Pin<&mut Self>);
    }

    // Without this the generated constructor passes the `QObject*` parent to the base, which
    // wants a `QQuickItem*`.
    impl cxx_qt::Initialize for GameScreen {}
}

use std::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QImage, QImageFormat, QList, QRectF, QString, QStringList};

use zeebx::egui::Key;
use zeebx::ui::running_game::fit_viewport;
use zeebx::ui::settings::{WindowMode, Modifiers};

use super::emu_core::{self, Frame};

#[derive(Default)]
pub struct GameScreenRust {
    run_state: QString,
    stopped: QString,
    /// The debug panel's texts on one line; empty with the panel off.
    debug_panel: QString,
    /// The panel's timeline, in `speed, frames` pairs, from the oldest sample to the newest.
    timeline: QList<i32>,
    /// The Boomerang calibration notice: an empty title hides the notice. The rotation is in
    /// degrees, which is what QML's `rotation` uses.
    notice_title: QString,
    notice_status: QString,
    notice_still: bool,
    notice_rotation: f64,
    /// The notice for the last screenshot. The serial changes with each one, so the QML shows it
    /// again even when the text is the same as the previous one's.
    screenshot_text: QString,
    screenshot_failed: bool,
    screenshot_dir: QString,
    screenshot_serial: i32,
    /// The notice for the last save state shortcut, the same way as the screenshot's.
    state_text: QString,
    state_failed: bool,
    state_serial: i32,
    /// The speed and frames per second, for the main window's status bar.
    speed: QString,
    /// The fast-forward's "▶▶ 3x" and the "Turbo"; empty hides them.
    indicators: QString,
    /// The lines of the metrics box in the corner; empty hides it.
    metrics: QStringList,
    /// The screen that went to the `FrameItem` as an image: generation and writes. If equal,
    /// there is nothing to upload. `None` when what is there is the GPU texture, or nothing.
    key_id: Option<(u64, u64)>,
}

impl cxx_qt::Initialize for qobject::GameScreen {
    fn initialize(self: Pin<&mut Self>) {}
}

/// What the pass tells the `FrameItem` to do. It leaves [`emu_core::with`] ready, because the item
/// can only be called afterwards: see the rule at the top of `emu_core.rs`.
enum Show {
    Texture { texture: u32, clip: [f32; 2] },
    Image { key_id: (u64, u64), image: QImage },
    SameImage,
    Nothing,
}

/// A window's mode as the QML reads it: 0 windowed, 1 maximized, 2 fullscreen.
pub fn mode(mode: WindowMode) -> i32 {
    match mode {
        WindowMode::Window => 0,
        WindowMode::Maximized => 1,
        WindowMode::Fullscreen => 2,
    }
}

/// The rectangle of size `size`, centered in the area.
fn centered(area: [f32; 2], size: [f32; 2]) -> QRectF {
    QRectF::new(
        f64::from((area[0] - size[0]) / 2.0),
        f64::from((area[1] - size[1]) / 2.0),
        f64::from(size[0]),
        f64::from(size[1]),
    )
}

impl qobject::GameScreen {
    pub fn step(mut self: Pin<&mut Self>) {
        let size = self.size();
        let area = [size.width() as f32, size.height() as f32];
        let current_key = self.rust().key_id;
        let (tick, show, dest, smooth_filter, debug_panel) = emu_core::with(|emu_core| {
            let tick = emu_core.step(area);
            let debug_panel = emu_core.debug_panel();
            // Copied first: `frame` borrows the whole core to bring the frame back from the GPU.
            let (scaling, keep_aspect, smooth) = {
                let graphics = &emu_core.effective().graphics;
                (graphics.scaling, graphics.keep_aspect, graphics.smooth)
            };
            // The experimental wide frame has its own aspect ratio, and so does a phone game's
            // screen (240×400 is 3:5); everything else is the console's 4:3.
            let (show, height, aspect) = match emu_core.frame() {
                Some(Frame::Gpu(frame)) => (
                    Show::Texture {
                        texture: frame.texture.0.get(),
                        clip: frame.clip,
                    },
                    480.0,
                    frame.ratio,
                ),
                Some(Frame::Screen(screen)) => {
                    let key_id = (screen.generation(), screen.writes());
                    let show = match Some(key_id) == current_key {
                        true => Show::SameImage,
                        false => Show::Image {
                            key_id,
                            image: rgb565_image(
                                &screen.to_rgb565_bytes(),
                                screen.width() as usize,
                                screen.height() as usize,
                            ),
                        },
                    };
                    let height = screen.height().max(1) as f32;
                    (show, height, screen.width() as f32 / height)
                }
                None => (Show::Nothing, 480.0, 4.0 / 3.0),
            };
            let fitted = fit_viewport(area, scaling, keep_aspect, height, aspect);
            (tick, show, centered(area, fitted), smooth, debug_panel)
        });
        match show {
            Show::Texture { texture, clip } => {
                self.as_mut().rust_mut().key_id = None;
                self.as_mut().show_texture(texture, clip[0], clip[1], dest, smooth_filter);
            }
            Show::Image { key_id, image } => {
                self.as_mut().rust_mut().key_id = Some(key_id);
                self.as_mut().show_image(&image, dest, smooth_filter);
            }
            Show::SameImage => self.as_mut().place(dest, smooth_filter),
            Show::Nothing => {}
        }
        if let Some(state) = tick.state.map(|text| QString::from(&text)) {
            if self.run_state() != &state {
                self.as_mut().set_run_state(state);
            }
        }
        let (texts, history) = debug_panel.unwrap_or_default();
        let texts = QString::from(&texts.join("   ·   "));
        if self.debug_panel() != &texts {
            self.as_mut().set_debug_panel(texts);
        }
        let history: Vec<i32> = history
            .into_iter()
            .flat_map(|(speed, frames)| [speed as i32, frames as i32])
            .collect();
        if Vec::<i32>::from(self.timeline()) != history {
            self.as_mut().set_timeline(QList::from(history));
        }
        let warning = tick.warning.unwrap_or(emu_core::OnScreenNotice {
            title: String::new(),
            state: String::new(),
            stopped: false,
            rotation: 0.0,
        });
        let title = QString::from(&warning.title);
        if self.notice_title() != &title {
            self.as_mut().set_notice_title(title);
        }
        let state = QString::from(&warning.state);
        if self.notice_status() != &state {
            self.as_mut().set_notice_status(state);
        }
        if *self.notice_still() != warning.stopped {
            self.as_mut().set_notice_still(warning.stopped);
        }
        let rotation = f64::from(warning.rotation).to_degrees();
        if *self.notice_rotation() != rotation {
            self.as_mut().set_notice_rotation(rotation);
        }
        if let Some(warning) = tick.screenshot {
            self.as_mut().set_screenshot_text(QString::from(&warning.text));
            self.as_mut().set_screenshot_failed(warning.failed);
            self.as_mut().set_screenshot_dir(QString::from(&warning.dir));
            let generation = self.screenshot_serial().wrapping_add(1);
            self.as_mut().set_screenshot_serial(generation);
        }
        let speed = QString::from(
            &tick
                .pace
                .map(|(speed, frames)| format!("{speed}% · {frames} FPS"))
                .unwrap_or_default(),
        );
        if self.speed() != &speed {
            self.as_mut().set_speed(speed);
        }
        let indicators = QString::from(&tick.indicators);
        if self.indicators() != &indicators {
            self.as_mut().set_indicators(indicators);
        }
        let mut metrics = QStringList::default();
        for line in &tick.metrics {
            metrics.append(QString::from(line));
        }
        if self.metrics() != &metrics {
            self.as_mut().set_metrics(metrics);
        }
        let stopped = QString::from(&tick.stopped);
        if self.stopped() != &stopped {
            self.as_mut().set_stopped(stopped);
        }
        if tick.closed {
            self.as_mut().clear();
            self.as_mut().rust_mut().key_id = None;
            self.as_mut().closed();
        }
    }

    pub fn key(self: Pin<&mut Self>, code: i32, pressed: bool) {
        if let Some(key) = qt_key(code) {
            emu_core::with(|emu_core| emu_core.key(key, pressed));
        }
    }

    pub fn is_state_shortcut(&self, code: i32, modifiers: i32) -> bool {
        let modifiers = qt_modifiers(modifiers);
        qt_key(code).is_some_and(|key| {
            emu_core::with(|emu_core| emu_core.is_state_shortcut(key, modifiers))
        })
    }

    pub fn state_shortcut(self: Pin<&mut Self>, code: i32, modifiers: i32) {
        let Some(key) = qt_key(code) else {
            return;
        };
        let modifiers = qt_modifiers(modifiers);
        let response = emu_core::with(|emu_core| emu_core.state_shortcut(key, modifiers));
        self.show_state_notice(response);
    }

    pub fn state_from_menu(self: Pin<&mut Self>, action: i32, slot: i32) {
        let request = match action {
            0 => emu_core::StateRequest::Save,
            1 => emu_core::StateRequest::Load,
            _ => emu_core::StateRequest::Undo,
        };
        let slot = usize::try_from(slot).ok();
        let response = emu_core::with(|emu_core| emu_core.run_state_request(request, slot));
        self.show_state_notice(response);
    }

    pub fn state_slots(&self) -> QStringList {
        let slots = emu_core::with(|emu_core| emu_core.state_slots());
        let mut list = QStringList::default();
        for slot in &slots {
            list.append(QString::from(slot));
        }
        list
    }

    pub fn occupied_slots(&self) -> QList<i32> {
        let occupied = emu_core::with(|emu_core| emu_core.occupied_slots());
        QList::from(occupied.into_iter().map(i32::from).collect::<Vec<_>>())
    }

    pub fn current_slot(&self) -> i32 {
        emu_core::with(|emu_core| emu_core.current_slot() as i32)
    }

    pub fn can_undo_load(&self) -> bool {
        emu_core::with(|emu_core| emu_core.can_undo_load())
    }

    pub fn toggle_fast_forward(self: Pin<&mut Self>) -> bool {
        emu_core::with(|emu_core| emu_core.toggle_fast_forward_lock())
    }

    pub fn fast_forward_locked(&self) -> bool {
        emu_core::with(|emu_core| emu_core.fast_forward_locked())
    }

    /// The notice for a save state request, from the shortcut or from the menu.
    fn show_state_notice(
        mut self: Pin<&mut Self>,
        response: Option<(emu_core::StateNotice, bool)>,
    ) {
        let Some((warning, restored)) = response else {
            return;
        };
        // **The console went back to another instant**, and the restored screen may have the same
        // generation and the same writes as the image in the item: without forgetting the key, the
        // next pass would find it equal and leave the old one showing.
        if restored {
            self.as_mut().rust_mut().key_id = None;
        }
        self.as_mut().set_state_text(QString::from(&warning.text));
        self.as_mut().set_state_failed(warning.failed);
        let generation = self.state_serial().wrapping_add(1);
        self.as_mut().set_state_serial(generation);
    }

    pub fn is_screenshot_shortcut(&self, code: i32) -> bool {
        qt_key(code).is_some_and(|key| emu_core::with(|emu_core| emu_core.is_screenshot_shortcut(key)))
    }

    pub fn screenshot(self: Pin<&mut Self>, timestamp: &QString) {
        let timestamp = String::from(timestamp);
        emu_core::with(|emu_core| emu_core.screenshot(&timestamp));
    }

    pub fn release(self: Pin<&mut Self>) {
        emu_core::with(|emu_core| emu_core.release_keys());
    }

    pub fn close(mut self: Pin<&mut Self>) {
        emu_core::with(|emu_core| emu_core.close());
        // The closed game's texture no longer exists: the node must not keep pointing at it.
        self.as_mut().clear();
        self.as_mut().rust_mut().key_id = None;
        self.as_mut().set_stopped(QString::default());
    }

    pub fn pause(self: Pin<&mut Self>) -> bool {
        emu_core::with(|emu_core| {
            emu_core.toggle_pause();
            emu_core.paused()
        })
    }

    pub fn is_reset_shortcut(&self, code: i32) -> bool {
        qt_key(code).is_some_and(|key| emu_core::with(|emu_core| emu_core.is_reset_shortcut(key)))
    }

    pub fn restart(mut self: Pin<&mut Self>) -> bool {
        let (result, state) = emu_core::with(|emu_core| {
            let result = emu_core.restart();
            let state = match &result {
                Ok(()) => emu_core.state(),
                Err(error) => emu_core
                    .catalog
                    .format("play.restart_failed", &[("reason", &error.to_string())]),
            };
            (result, state)
        });
        self.as_mut().set_run_state(QString::from(&state));
        if result.is_err() {
            return false;
        }
        // **The texture of the session that ended no longer exists**, as in `close`: the node
        // must not keep pointing at it. When it did, Qt drew a frame with its number, and the new
        // session had already given that number to another texture: for one frame the game's menu
        // font showed up, in red, in Resident Evil 4, Ridge Racer and Rolimã. The next pass sends
        // the new image.
        self.as_mut().clear();
        self.as_mut().rust_mut().key_id = None;
        self.as_mut().set_stopped(QString::default());
        true
    }

    pub fn refresh(mut self: Pin<&mut Self>) {
        let state = emu_core::with(|emu_core| emu_core.state());
        self.as_mut().set_run_state(QString::from(&state));
    }

    pub fn window_mode(&self) -> i32 {
        emu_core::with(|emu_core| mode(emu_core.settings.graphics.game_window))
    }

    pub fn confirm_on_close(&self) -> bool {
        emu_core::with(|emu_core| emu_core.confirm_on_close())
    }

    pub fn window_width(&self) -> f64 {
        emu_core::with(|emu_core| {
            emu_core
                .settings
                .graphics
                .game_window_size
                .map_or(0.0, |[width, _]| width as f64)
        })
    }

    pub fn window_height(&self) -> f64 {
        emu_core::with(|emu_core| {
            emu_core
                .settings
                .graphics
                .game_window_size
                .map_or(0.0, |[_, height]| height as f64)
        })
    }

    pub fn save_window_size(&self, width: f64, height: f64) {
        emu_core::with(|emu_core| {
            emu_core.settings.graphics.game_window_size =
                (width > 0.0 && height > 0.0).then_some([width as f32, height as f32]);
            if let Err(error) = emu_core.settings.save() {
                eprintln!("could not save the settings: {error}");
            }
        })
    }
}

/// A `QImage` built over its own bytes requires lines aligned to four bytes. An odd width in
/// RGB565 is not: the line gets two bytes of padding.
fn rgb565_image(bytes: &[u8], width: usize, height: usize) -> QImage {
    let line = width * 2;
    let step = (line + 3) & !3;
    let data = match step == line {
        true => bytes.to_vec(),
        false => {
            let mut data = vec![0u8; step * height];
            for (dest, origin) in data.chunks_exact_mut(step).zip(bytes.chunks_exact(line)) {
                dest[..line].copy_from_slice(origin);
            }
            data
        }
    };
    // `Format_RGB16` is the `u16` in machine order; the framebuffer delivers little-endian, which
    // is the order of the three supported systems.
    unsafe { QImage::from_raw_bytes(data, width as i32, height as i32, QImageFormat::Format_RGB16) }
}

/// The modifiers of a Qt event (`Qt::KeyboardModifiers`), as [`Modifiers`] stores them.
///
/// **On the Mac, Qt swaps the names**: what it calls `ControlModifier` is ⌘, and `MetaModifier`
/// is Control. Elsewhere, Meta is the system key (the Windows key, Super), which [`Modifiers`]
/// also stores in `cmd`. The keypad one does not count: the Enter there is the same Enter.
pub(super) fn qt_modifiers(bits: i32) -> Modifiers {
    const SHIFT: i32 = 0x0200_0000;
    const CONTROL: i32 = 0x0400_0000;
    const ALT: i32 = 0x0800_0000;
    const META: i32 = 0x1000_0000;
    let (ctrl, cmd) = match cfg!(target_os = "macos") {
        true => (bits & META != 0, bits & CONTROL != 0),
        false => (bits & CONTROL != 0, bits & META != 0),
    };
    Modifiers {
        ctrl,
        alt: bits & ALT != 0,
        shift: bits & SHIFT != 0,
        cmd,
    }
}

/// The name egui gives the key, from the `Qt::Key`. It is the name stored in `settings.json`:
/// switching interfaces must not undo anyone's mapping.
/// The `egui::Key` of a `Qt::Key`, going through the name.
pub(super) fn qt_key(code: i32) -> Option<Key> {
    key_name(code).and_then(|name| Key::from_name(&name))
}

fn key_name(code: i32) -> Option<String> {
    let name = match code {
        0x0100_0000 => "Escape",
        0x0100_0001 => "Tab",
        0x0100_0003 => "Backspace",
        // `Return` and the keypad's `Enter` are a single key to egui.
        0x0100_0004 | 0x0100_0005 => "Enter",
        0x0100_0006 => "Insert",
        0x0100_0007 => "Delete",
        0x0100_0010 => "Home",
        0x0100_0011 => "End",
        0x0100_0012 => "Left",
        0x0100_0013 => "Up",
        0x0100_0014 => "Right",
        0x0100_0015 => "Down",
        0x0100_0016 => "PageUp",
        0x0100_0017 => "PageDown",
        0x20 => "Space",
        0x2c => "Comma",
        0x2d => "Minus",
        0x2e => "Period",
        0x2f => "Slash",
        0x3b => "Semicolon",
        0x3d => "Equals",
        0x5b => "OpenBracket",
        0x5c => "Backslash",
        0x5d => "CloseBracket",
        0x60 => "Backtick",
        0x27 => "Quote",
        // Digits and letters: the `Qt::Key` is the uppercase ASCII itself, which is egui's name.
        0x30..=0x39 | 0x41..=0x5a => return char::from_u32(code as u32).map(String::from),
        // F1 to F35.
        0x0100_0030..=0x0100_0052 => return Some(format!("F{}", code - 0x0100_0030 + 1)),
        _ => return None,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::{Key, Modifiers, qt_modifiers, qt_key};

    /// The Mac's ⌘ reaches Qt as Control; Control, as Meta.
    #[test]
    fn the_qt_modifiers() {
        assert!(qt_modifiers(0).is_empty());
        // The keypad is nobody's modifier.
        assert!(qt_modifiers(0x2000_0000).is_empty());
        let shift = qt_modifiers(0x0200_0000);
        assert_eq!(
            shift,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            }
        );
        let control = qt_modifiers(0x0400_0000);
        match cfg!(target_os = "macos") {
            true => assert!(control.cmd && !control.ctrl),
            false => assert!(control.ctrl && !control.cmd),
        }
    }

    /// Qt's arrow is egui's arrow: from there on [`zeebx::ui::host_input::key_pressed`] applies,
    /// which accepts both spellings in `settings.json`.
    #[test]
    fn the_qt_arrow_is_the_egui_arrow() {
        assert_eq!(qt_key(0x0100_0013), Some(Key::ArrowUp));
    }

    #[test]
    fn qt_keys_have_the_egui_names() {
        // The `settings.json` of existing users stores these names.
        for (code, name) in [
            (0x0100_0013, "Up"),
            (0x0100_0004, "Enter"),
            (0x20, "Space"),
            (0x5a, "Z"),
            (0x34, "4"),
            (0x0100_0003, "Backspace"),
            (0x0100_0039, "F10"),
        ] {
            assert_eq!(qt_key(code), Key::from_name(name), "{name}");
            assert!(Key::from_name(name).is_some(), "{name} is not an egui key");
        }
    }
}
