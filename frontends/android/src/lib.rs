//! Zeebx on Android: the same emulation, a different screen.
//!
//! This package is the whole frontend. It knows nothing about emulating — that is done by
//! [`zeebx::session::Session`], the same one the desktop window and the command line drive. What
//! lives here is what changes per platform: where the ROMs come from, how the frame reaches the
//! screen and how the device's button becomes a console button.
//!
//! **The event loop is ours, not eframe's.** The reason is in [`host_input`]: through winit the
//! controller buttons arrive as `Key::Unidentified` and the stick axes do not arrive at all.
//! Driving [`android_activity::AndroidApp`] directly, the input is what Android sent. The price
//! is the window lifecycle and the GL context, which live in [`screen`].
//!
//! The screens are one per file — [`library`], [`settings_page`], [`selector`] and [`game`] — and
//! the texts and options come from the core itself: the same `Settings` as desktop, the same
//! language catalog, the same debug panel.
//!
//! **3D can run on the GPU.** The GL context of [`screen`] is the same one the session receives,
//! which turns on desktop's `gpu_rasterizer` here — and with it the internal resolution,
//! antialiasing, anisotropic filtering and the wide aspect ratio, which are empty functions in the
//! software rasterizer.

mod settings_page;
mod library;
mod host_input;
mod game;
mod selector;
mod system;
mod screen;
mod theme;
mod touch;
mod widgets;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent, WindowManagerFlags};

use zeebx::input::Pad;
use zeebx::session::Session;
use zeebx::storage::StoragePaths;
use zeebx::ui::save_slots;
use zeebx::ui::i18n::Catalog;
use zeebx::ui::library::{self, Game};
use zeebx::ui::settings::{OnScreenControlsMode, Settings};

use host_input::HostInput;
use screen::{Gpu, Screen};

/// The point where Android comes in.
///
/// `NativeActivity` loads the `.so`, finds this symbol and calls it on a thread of its own. From
/// here down, everything is ours.
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("Zeebx"),
    );
    // **The core's log has a destination on Android.** Until now this was the only frontend that
    // did not drain the ring: desktop prints to `stderr`, the core sends through `retro_log`,
    // headless prints — and Android had nowhere. Here the destination is `log`, which
    // `android_logger` just installed and which comes out in `logcat` with the right tag and
    // severity.
    //
    // The level starts at the core's default (`Warning`); the `ZEEBX_LOG` variable applies if it
    // exists.
    zeebx::logging::from_env();

    // A Rust panic writes to stderr, and Android throws stderr away: without this the app dies
    // without leaving a single line in `logcat`. The hook sends the message and the location to
    // the system log before the process goes down.
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        log::error!("panic at {location}: {info}");
    }));

    log::info!("Zeebx starting");

    let internal = app
        .internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/local/tmp"));

    // **Android does not set `HOME`.** Without it the core's `settings::config_dir` falls back
    // to its last resort, the current directory — which in an Android app is `/`. The loader
    // then tries to extract the `.zip` files into `/cache` and mount the device root at
    // `/device`, and both fail with "permission denied" in an error that *looks* like the ROM's.
    //
    // Setting `HOME` to the app's private directory makes every rule that already exists point
    // to the right place, without a single line of `cfg` in the core.
    unsafe { std::env::set_var("HOME", &internal) };

    // Settings live in the app's internal directory, which belongs to it alone: the core's
    // `Settings` is the same as desktop's, and takes an explicit path precisely for this.
    let file = internal.join("zeebx.json");

    // The ROM folder is chosen by the user and stored in settings. The default is `zeebo/roms`
    // at the storage root, which is where a collection usually is — and reading it requires
    // "access to all files", which the manifest declares and the app requests.
    let external = app.external_data_path().map(|dir| dir.join("roms"));
    if let Some(dir) = &external {
        let _ = std::fs::create_dir_all(dir);
    }
    let fallback = PathBuf::from("/sdcard/zeebo/roms");

    // The app's internal folder serves as a shortcut in the picker: it is the only one Android
    // lets us read with no permission at all.
    let own_dir = internal.join("roms");
    let _ = std::fs::create_dir_all(&own_dir);

    // True fullscreen: on a handheld the status bar only steals height from the game — and it
    // was what covered the path in the folder picker.
    app.set_window_flags(WindowManagerFlags::FULLSCREEN, WindowManagerFlags::empty());
    // The flag does not remove the navigation bar: it belongs to the view, not the window, and
    // goes away through JNI.
    system::hide_navigation_bar(&app);

    let mut emulator = Emulator::create(file, fallback, external, own_dir, app.clone());
    run_loop(app, &mut emulator);
}

/// Sends to `logcat` what the core logged since the last pass.
///
/// Severity is preserved: a core warning comes out as `WARN` in `logcat`, not as `INFO` like
/// everything else. That is what makes `adb logcat *:W` show what matters without the rest.
fn dump_log() {
    use zeebx::logging::Level;
    for line in zeebx::logging::drain() {
        let text = format!("[{}] {}: {}", line.level.tag(), line.target, line.text);
        match line.level {
            Level::Debug => log::debug!("{text}"),
            Level::Info => log::info!("{text}"),
            Level::Warning => log::warn!("{text}"),
            Level::Error | Level::Fatal => log::error!("{text}"),
        }
    }
}

/// The loop: collects events, lets the game run, draws, repeats.
fn run_loop(app: AndroidApp, emulator: &mut Emulator) {
    let ctx = egui::Context::default();
    // The core's interface was designed for a monitor. On a handheld, at arm's length, the same
    // size in points becomes illegible: the device density is the floor, and from there up it is
    // what the finger needs to hit a button.
    let points_per_pixel = scale(&app);
    ctx.set_pixels_per_point(points_per_pixel);
    theme::apply(&ctx);

    let mut host_input = HostInput::create(points_per_pixel);
    // The GPU is born with the first window and never dies: see [`screen::Gpu`]. The screen is
    // the surface, and that one comes and goes.
    let mut gpu: Option<Gpu> = None;
    let mut screen: Option<Screen> = None;
    let mut visible = false;
    let mut quit = false;
    let begin = Instant::now();

    while !quit {
        // The core's log goes out here, once per loop pass. With the ring empty — the common
        // case, at the default level — this is one lock and one read.
        dump_log();
        // With a window, the loop spins as fast as the buffer swap allows; without one, there is
        // nothing to draw and waiting is right — it is what keeps the app idle in the background
        // instead of burning battery.
        let wait = match visible && screen.is_some() {
            true => Some(Duration::ZERO),
            false => None,
        };

        app.poll_events(wait, |event| match event {
            PollEvent::Main(MainEvent::InitWindow { .. }) => {
                let throttle = emulator.settings.speed.fps_limit.limit_speed();
                let done = match gpu.as_mut() {
                    // From the second window on the context is reused: it is what keeps alive
                    // the GL objects the session created.
                    Some(gpu) => gpu.recreate_screen(&app, throttle),
                    None => Gpu::create(&app, throttle).map(|(create, first)| {
                        emulator.gl = Some(create.gl.clone());
                        gpu = Some(create);
                        first
                    }),
                };
                match done {
                    Ok(create) => screen = Some(create),
                    Err(error) => log::error!("the screen did not come up: {error}"),
                }
            }
            PollEvent::Main(MainEvent::TerminateWindow { .. }) => screen = None,
            PollEvent::Main(MainEvent::WindowResized { .. })
            | PollEvent::Main(MainEvent::ContentRectChanged { .. })
            | PollEvent::Main(MainEvent::ConfigChanged { .. }) => {
                if let (Some(gpu), Some(screen)) = (&gpu, &mut screen) {
                    gpu.resize(screen, &app);
                }
            }
            PollEvent::Main(MainEvent::Resume { .. }) | PollEvent::Main(MainEvent::GainedFocus) => {
                visible = true;
                if let (Some(gpu), Some(screen)) = (&gpu, &screen) {
                    gpu.resume(screen);
                }
            }
            PollEvent::Main(MainEvent::Pause) | PollEvent::Main(MainEvent::LostFocus) => {
                // Android can take focus away before delivering the controller's final
                // KeyUp/MotionEvent. If we kept the state, the direction would be stuck when the
                // activity comes back. On the console, losing the device equals releasing
                // everything.
                emulator.pad = Pad::default();
                emulator.overlay.release();
                host_input.physical.clear();
                visible = false
            }
            PollEvent::Main(MainEvent::Destroy) => quit = true,
            _ => {}
        });

        if quit {
            break;
        }

        host_input.begin_frame();
        // The finger goes to the on-screen controls only when the game is actually running. With
        // the "back" question open it has to reach that dialog's buttons, which are egui's.
        let in_game = emulator.location == View::Game
            && !emulator.confirming
            && emulator.settings.on_screen_controls.mode != OnScreenControlsMode::Never;
        if !in_game {
            emulator.overlay.release();
        }
        if let Ok(mut queue) = app.input_events_iter() {
            // The iterator hands over one event per call and wants the answer right away:
            // `Handled` tells Android that the event dies here — that is what stops "back" from
            // closing the activity out from under us.
            while queue.next(|event| {
                let overlay = in_game.then_some(&mut emulator.overlay);
                host_input.receive(event, &mut emulator.pad, overlay);
                InputStatus::Handled
            }) {}
        }
        // The physical buttons for the shortcuts, and the one that just went down for an open
        // capture.
        emulator.physical.clone_from(&host_input.physical);
        if let Some(name) = host_input.last_physical {
            emulator.captured_button(name);
        }
        // Android's "back" works on any screen. The controller's button 2 works on all of them
        // except in-game, where it belongs to the player -- there what closes is the device's
        // "back".
        if host_input.back || (host_input.ui_back && emulator.location != View::Game) {
            emulator.back();
        }

        let (Some(gpu), Some(screen)) = (gpu.as_mut(), screen.as_ref()) else {
            continue;
        };
        if !visible {
            continue;
        }

        let [width, height] = screen.size;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(
                    width as f32 / points_per_pixel,
                    height as f32 / points_per_pixel,
                ),
            )),
            time: Some(begin.elapsed().as_secs_f64()),
            // The device GPU's limit, not egui's guess: it decides how far the font atlas can
            // grow.
            max_texture_side: Some(gpu.painter.max_texture_side()),
            events: std::mem::take(&mut host_input.events),
            ..Default::default()
        };

        let output = ctx.run(raw, |ctx| emulator.draw(ctx));
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        gpu.paint(
            screen,
            &primitives,
            &output.textures_delta,
            output.pixels_per_point,
            emulator.settings.speed.fps_limit.limit_speed(),
        );
    }

    log::info!("Zeebx shutting down");
}

/// Which screen is up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// The game grid.
    Library,
    /// Settings.
    Settings,
    /// The folder browser, at the given folder, to choose the ROM folder.
    FolderSelector(PathBuf),
    /// The same browser, to choose the `.sf2` bank for MIDI music.
    SoundfontSelector(PathBuf),
    /// A running game.
    Game,
    /// The on-screen controls editor.
    TouchEditor,
}

/// The whole app: what is on screen and what backs each screen.
pub struct Emulator {
    /// Where settings are read and written. It is a file in the same format as desktop's.
    file: PathBuf,
    settings: Settings,
    /// The interface texts, from the same catalog desktop uses. The built-in languages are
    /// embedded in the binary, so they work without any file on the device.
    catalog: Catalog,
    location: View,
    tab: settings_page::Tab,
    /// Which settings hint is open. Zeebx's help texts are long, and all of them open at once
    /// become a wall: only one at a time.
    hint: widgets::ExpandedHint,
    /// The app's private folder. Always readable, with no permission at all — it is the shortcut
    /// that works even when "access to all files" was not granted.
    own_dir: PathBuf,
    /// The app's private root. Save states live here, apart from the ROMs the user chose:
    /// updating/swapping the collection does not erase where each game was.
    states_root: PathBuf,
    /// Cache, shared system and persistent saves handed explicitly to the core.
    storage: StoragePaths,
    /// The bridge to the activity: it is through it that the permission is checked and requested.
    app: AndroidApp,
    /// The screen's GL context, once it is up. It is what the session uses to fill 3D on the
    /// GPU; without it, 3D is the CPU's.
    gl: Option<std::sync::Arc<glow::Context>>,
    /// What puts the GPU frame on screen. It lives behind an `Arc<Mutex<_>>` because what uses
    /// it is a closure egui keeps and calls in the middle of painting — it cannot borrow from
    /// the `Emulator`. It is born on the first paint, which is the first time there is a current
    /// context.
    gl_painter: std::sync::Arc<std::sync::Mutex<Option<zeebx::ui::gpu::GlPainter>>>,
    /// The games in the chosen folder, with title and cover, read by the same `library::scan`
    /// as desktop.
    games: Vec<Game>,
    /// Result of a background scan. While it exists, the old list stays usable and the
    /// interface does not freeze reading ZIPs/MIFs.
    scan: Option<
        std::sync::mpsc::Receiver<(PathBuf, Vec<Game>, Vec<(u32, String)>)>,
    >,
    /// Titles normalized once per scan, so the search does not redo lowercasing every frame.
    search_titles: Vec<String>,
    /// Indices the current search let through.
    filtered: Vec<usize>,
    /// Applets known to the library, prepared off the UI thread for the Z-Wheel boot.
    installed: Vec<(u32, String)>,
    /// The search filter.
    search: String,
    /// Which card the D-pad is pointing at, among the ones the search let through.
    selected: usize,
    /// The selection changed and has to come into view on the next frame.
    scroll: bool,
    /// How many columns the grid drew last time. It is the D-pad's step up and down — and only
    /// the paint knows that number.
    columns: usize,
    /// The covers already uploaded to the GPU, by game path. Without this every frame would
    /// rebuild the texture of every visible card.
    covers: HashMap<PathBuf, egui::TextureHandle>,
    session: Option<Session>,
    /// The console frame as an egui texture, replaced on every new frame.
    texture: Option<egui::TextureHandle>,
    /// What is uploaded in the texture: the frame's generation and writes, plus the filter. The
    /// screen repaints more often than the game draws, and without this key every repaint would
    /// redo the whole conversion.
    frame: Option<(u64, u64, bool)>,
    /// What receives focus when the arrow goes up from the grid's first row.
    ///
    /// It is an egui widget, and so the top bar is only reachable through egui focus -- the grid
    /// has its own cursor and the two cannot move together. The `Id` comes from painting the
    /// bar, because that is what creates the button.
    bar_focus: Option<egui::Id>,
    /// The 2D frame in RGB565 for the direct path through the GPU.
    ///
    /// Android can repaint the window several times without the game touching the framebuffer.
    /// Keeping the conversion avoids recreating/copying about 600 KiB per repaint on a 640x480
    /// screen.
    ///
    /// **There are two caches, and neither is redundant.** This one keeps the bytes and saves the
    /// CPU conversion; `GlPainter`'s `last_frame` keeps what was already uploaded and saves the
    /// trip over the bus. The same `(generation, writes)` key governs both.
    frame_565: Option<(u64, u64, std::sync::Arc<[u8]>)>,
    /// The buttons pressed now, fed by the native queue.
    pad: Pad,
    /// The controls drawn on screen, and the buttons the fingers press on them.
    overlay: touch::Overlay,
    /// The piece the editor is dragging, and where the finger grabbed it relative to its center.
    dragging: Option<(zeebx::input::touch::TouchControl, egui::Vec2)>,
    /// The piece selected in the editor, the one the size bar grows and shrinks.
    chosen: Option<zeebx::input::touch::TouchControl>,
    /// "Back" was pressed with a game open: the question is on screen.
    confirming: bool,
    /// The game is paused by choice, not by failure.
    paused: bool,
    /// BLAKE3 identity of the file that opened the session, the same rule as the core's storage.
    state_id: Option<String>,
    /// Where the open game came from: it is what "Restart" in the back dialog opens again.
    open_path: Option<std::path::PathBuf>,
    /// The five persistent slots of the current game.
    save_slots: [Option<save_slots::Slot>; save_slots::SLOTS],
    /// Thumbnails already converted/uploaded to egui, per slot.
    state_thumbnails: HashMap<usize, egui::TextureHandle>,
    /// An occupied slot that asked for a second confirmation before being replaced.
    state_overwrite: Option<usize>,
    /// Result of the last operation, shown in the menu without closing the window.
    state_message: Option<String>,
    /// Why the last game did not open, when it did not.
    error: Option<String>,
    last: Instant,
    /// The physical controller buttons pressed now, by desktop's name. See
    /// [`host_input::HostInput::physical`].
    physical: std::collections::HashSet<&'static str>,
    /// The fast-forward shortcut, which is held or toggled.
    fast_forward: zeebx::speed::Toggle,
    /// The return points and the rewind shortcut.
    rewind: zeebx::speed::rewind::RewindController,
    /// Player 1's turbo, the only one Android has.
    turbo: zeebx::speed::turbo::PortTurbo,
    /// Whether fast-forward was active on the last pass, for the "▶▶" on screen.
    fast_forwarding: bool,
    /// The shortcut waiting for a physical controller button, in the settings section.
    capturing: Option<settings_page::PhysicalShortcut>,
    /// A shortcut was just captured in this round. See [`Emulator::captured_button`].
    just_captured: bool,
}

impl Emulator {
    fn create(
        file: PathBuf,
        fallback: PathBuf,
        external: Option<PathBuf>,
        own_dir: PathBuf,
        app: AndroidApp,
    ) -> Self {
        let states_root = own_dir
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| own_dir.clone());
        // Keeps cache and device at Android's historical paths (files/.config/zeebx), but turns
        // on the persistent overlay in saves/.
        let storage_base = states_root.join(".config");
        let storage = StoragePaths::for_frontend(&storage_base, Some(&storage_base));
        if let Err(error) = storage.create_dirs() {
            log::error!("could not create the persistent storage: {error}");
        }
        let mut settings = Settings::load_from(&file);
        // No folder chosen yet: the default, if it exists, otherwise the app's directory, which
        // always exists and never asks for permission.
        if settings.roms_dir.is_none() {
            settings.roms_dir = match fallback.is_dir() {
                true => Some(fallback),
                false => external,
            };
        }

        // The saved language, or the device's. The catalog only knows the built-in ones plus
        // whatever is in `lang/` inside the app's folder.
        let mut catalog = Catalog::new(&[own_dir.with_file_name("lang")]);
        match &settings.language {
            Some(code) => {
                catalog.select(code);
            }
            None => {
                catalog.select_best(&device_language(&app));
            }
        }

        let mut emulator = Self {
            location: View::Library,
            tab: settings_page::Tab::General,
            hint: None,
            own_dir,
            states_root,
            storage,
            app,
            gl: None,
            gl_painter: Default::default(),
            file,
            settings,
            catalog,
            games: Vec::new(),
            scan: None,
            search_titles: Vec::new(),
            filtered: Vec::new(),
            installed: Vec::new(),
            search: String::new(),
            selected: 0,
            scroll: false,
            columns: 4,
            covers: HashMap::new(),
            session: None,
            texture: None,
            frame: None,
            bar_focus: None,
            frame_565: None,
            pad: Pad::default(),
            overlay: Default::default(),
            dragging: None,
            chosen: None,
            confirming: false,
            paused: false,
            state_id: None,
            open_path: None,
            save_slots: std::array::from_fn(|_| None),
            state_thumbnails: HashMap::new(),
            state_overwrite: None,
            state_message: None,
            error: None,
            last: Instant::now(),
            physical: Default::default(),
            fast_forward: Default::default(),
            rewind: Default::default(),
            turbo: Default::default(),
            fast_forwarding: false,
            capturing: None,
            just_captured: false,
        };
        emulator.reload();
        emulator
    }

    /// An interface text.
    fn tr<'a>(&'a self, key_id: &'a str) -> &'a str {
        self.catalog.get(key_id)
    }

    /// Rereads the chosen folder without blocking the thread that draws the interface.
    fn reload(&mut self) {
        let dir = self.roms();
        if dir.as_os_str().is_empty() {
            self.scan = None;
            self.apply_games(dir, Vec::new(), Vec::new());
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let thread_dir = dir.clone();
        match std::thread::Builder::new()
            .name("zeebx-library".to_string())
            .spawn(move || {
                let games = library::scan(&thread_dir);
                let installed = games
                    .iter()
                    .filter_map(|game| {
                        Some((game.clsid?, library::module_id(&game.path)?))
                    })
                    .collect();
                let _ = tx.send((thread_dir, games, installed));
            })
        {
            Ok(_) => self.scan = Some(rx),
            Err(error) => {
                log::error!("could not start the library scan: {error}");
                self.scan = None;
            }
        }
    }

    /// Publishes a finished scan, ignoring the result for a folder that is no longer current.
    fn finish_reload(&mut self) {
        let result = match self.scan.as_ref() {
            Some(rx) => match rx.try_recv() {
                Ok(result) => Some(Ok(result)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(())),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
            },
            None => None,
        };
        match result {
            Some(Ok((dir, games, installed))) => {
                self.scan = None;
                if dir == self.roms() {
                    self.apply_games(dir, games, installed);
                }
            }
            Some(Err(())) => {
                self.scan = None;
                log::error!("the library scan ended without a result");
            }
            None => {}
        }
    }

    fn apply_games(
        &mut self,
        dir: PathBuf,
        games: Vec<Game>,
        installed: Vec<(u32, String)>,
    ) {
        self.games = games;
        self.installed = installed;
        self.search_titles = self
            .games
            .iter()
            .map(|game| game.title.to_lowercase())
            .collect();
        self.update_filter();
        self.covers.clear();
        self.selected = 0;
        self.scroll = true;
        log::info!("{} games in {}", self.games.len(), dir.display());
    }

    fn update_filter(&mut self) {
        let search = self.search.trim().to_lowercase();
        self.filtered.clear();
        self.filtered.extend(
            self.search_titles
                .iter()
                .enumerate()
                .filter(|(_, title)| search.is_empty() || title.contains(&search))
                .map(|(index, _)| index),
        );
        if self.filtered.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.filtered.len() - 1);
        }
    }

    /// The folder chosen now. Empty when none has been chosen yet.
    fn roms(&self) -> PathBuf {
        self.settings.roms_dir.clone().unwrap_or_default()
    }

    /// Saves the preferences. Every settings change goes through here.
    fn save(&self) {
        if let Err(error) = self.settings.save_to(&self.file) {
            log::error!("could not save the settings: {error}");
        }
    }

    /// Opens a game, with the graphics and sound settings currently in effect.
    fn open(&mut self, game_path: &std::path::Path) {
        self.error = None;
        let content = match self.storage.content_id(game_path) {
            Ok(id) => id,
            Err(error) => {
                let reason = error.to_string();
                log::error!("could not identify the content: {reason}");
                self.error = Some(
                    self.catalog
                        .format("play.failed", &[("reason", &reason)]),
                );
                return;
            }
        };
        let state_id = Some(content.as_str().to_string());
        match self
            .storage
            .migrate_legacy_package_writes(game_path, &content)
        {
            Ok(bytes) if bytes > 0 => {
                log::info!("migrated {bytes} bytes of old saves to the overlay");
            }
            Ok(_) => {}
            Err(error) => {
                // The old copy stays in the cache and the VFS can still read it. Failing the
                // migration must not keep the game from opening.
                log::warn!("could not migrate old saves: {error}");
            }
        }
        zeebx::audio::soundfont::set_soundfont(self.settings.audio.soundfont.clone());
        zeebx::audio::soundfont::set_effects(self.settings.audio.midi_effects);
        let graphics = self.settings.graphics.clone();
        // 3D on the GPU only applies if there is a GPU: before the first window there is no
        // context, and a session opened without one falls back to the software rasterizer on its
        // own.
        let on_gpu = graphics.gpu_rasterizer && self.gl.is_some();
        match Session::start_with_storage_installed(
            game_path,
            zeebx::DEFAULT_PORTS,
            None,
            on_gpu,
            self.gl.clone().filter(|_| on_gpu),
            self.settings.z_wheel,
            &self.storage,
            &self.installed,
        ) {
            Ok(mut session) => {
                log::info!("opened {}", session.title());
                session.set_internal_resolution(graphics.internal_resolution as usize);
                session.set_aspect_ratio(graphics.ratio.aspect(16.0 / 9.0));
                session.set_enhancements(
                    graphics.antialias as usize,
                    graphics.anisotropic as usize,
                );
                session.set_fog_allowed(graphics.fog);
                // Turning sound on here is safe **because the game has not started yet**:
                // `start_with` only prepares, and `EVT_APP_START` goes out on the loop's first
                // pass.
                let audio = self.settings.audio.clone();
                if let Some(error) = session.set_audio(audio.enabled, audio.volume) {
                    log::error!("no sound: {error}");
                }
                self.session = Some(session);
                self.open_path = Some(game_path.to_path_buf());
                self.state_id = state_id;
                self.save_slots = self
                    .state_id
                    .as_deref()
                    .map(|id| save_slots::list(&self.states_root, id))
                    .unwrap_or_else(|| std::array::from_fn(|_| None));
                self.state_thumbnails.clear();
                self.state_overwrite = None;
                self.state_message = None;
                self.paused = false;
                self.confirming = false;
                self.location = View::Game;
                self.last = Instant::now();
            }
            Err(error) => {
                log::error!("could not open {}: {error:?}", game_path.display());
                self.error = Some(
                    self.catalog
                        .format("play.failed", &[("reason", &error.to_string())]),
                );
            }
        }
    }

    /// Closes the game and goes back to the library.
    fn close(&mut self) {
        self.session = None;
        self.open_path = None;
        self.texture = None;
        self.frame = None;
        self.frame_565 = None;
        self.state_id = None;
        self.save_slots = std::array::from_fn(|_| None);
        self.state_thumbnails.clear();
        self.state_overwrite = None;
        self.state_message = None;
        self.confirming = false;
        self.paused = false;
        self.pad = Pad::default();
        // The return points belong to the game that closed; the turbo and fast-forward toggles
        // do not carry over to the next one.
        self.rewind = Default::default();
        self.fast_forward.disable();
        self.turbo.disable();
        self.location = View::Library;
    }

    /// Restarts the open game from scratch: closes and opens the same path, like a console's
    /// reset button. What the game wrote to disk stays, and so do the save states.
    fn restart(&mut self) {
        let Some(path) = self.open_path.clone() else {
            return;
        };
        self.close();
        self.open(&path);
    }

    /// Android's "back" button.
    ///
    /// It never closes the app on its own: with a game open it asks; on the other screens it goes
    /// up one level. In the library, exiting is the system's business.
    fn back(&mut self) {
        match &self.location {
            View::Game if self.confirming => self.confirming = false,
            View::Game => self.confirming = true,
            // In the picker, going up one folder is what "back" means; at the first one, leaving
            // it.
            View::FolderSelector(current) => {
                self.location = match current.parent() {
                    Some(above) if current != &self.own_dir => View::FolderSelector(above.to_path_buf()),
                    _ => View::Settings,
                }
            }
            View::SoundfontSelector(current) => {
                self.location = match current.parent() {
                    Some(above) if current != &self.own_dir => {
                        View::SoundfontSelector(above.to_path_buf())
                    }
                    _ => View::Settings,
                }
            }
            View::Settings => self.location = View::Library,
            View::TouchEditor => {
                self.dragging = None;
                self.chosen = None;
                self.location = View::Settings;
            }
            View::Library => {}
        }
    }

    /// Which of the screens is up.
    fn draw(&mut self, ctx: &egui::Context) {
        self.finish_reload();
        match self.location.clone() {
            View::Game => self.game(ctx),
            View::Settings => self.settings_page(ctx),
            View::TouchEditor => self.touch_editor(ctx),
            View::FolderSelector(current) => self.selector(ctx, &current),
            View::SoundfontSelector(current) => self.soundfont_selector(ctx, &current),
            View::Library => self.library(ctx),
        }
    }
}

/// How many pixels one egui point is worth.
///
/// The device density alone does not work. A handheld like the RG505 declares itself `xhdpi` —
/// 320 dpi, two pixels per point — and its screen is 960 pixels wide: the whole interface would
/// fit in 480 points, which is less than a game grid needs. Density exists for a phone held at
/// thirty centimeters; a handheld is held with both hands, farther away, and its screen is wider
/// than it is tall.
///
/// So density comes in as a **ceiling**, and the floor is fitting a working screen: never less
/// than [`MIN_POINTS`] of usable width.
fn scale(app: &AndroidApp) -> f32 {
    let density = app
        .config()
        .density()
        .map(|dpi| dpi as f32 / 160.0)
        .unwrap_or(2.0);
    let width = app
        .native_window()
        .map(|window| window.width() as f32)
        .unwrap_or(MIN_POINTS);
    density.min(width / MIN_POINTS).clamp(1.0, 3.0)
}

/// How many egui points wide the screen needs to be for the interface to fit.
///
/// Six hundred is what holds a four-column grid with the title legible below, and still leaves
/// the top bar with name, count, search and two buttons on the same row.
const MIN_POINTS: f32 = 600.0;

/// The device language, like `pt-BR`. Empty when Android does not say.
fn device_language(app: &AndroidApp) -> String {
    let config = app.config();
    match (config.language(), config.country()) {
        (Some(lang), Some(country)) => format!("{lang}-{country}"),
        (Some(lang), None) => lang.to_string(),
        _ => String::new(),
    }
}

/// A core image turned into an egui texture.
fn texture_of(ctx: &egui::Context, name: &str, image: &zeebx::video::icon::Image) -> egui::TextureHandle {
    let color = egui::ColorImage::from_rgba_unmultiplied(
        [image.width, image.height],
        &image.rgba,
    );
    // The covers are enlarged `.mif` icons: interpolating is what avoids the giant blocks.
    ctx.load_texture(name, color, egui::TextureOptions::LINEAR)
}
