//! The state of the Qt interface: the settings, the library, the desktop input and the open game.
//!
//! **It lives in a `thread_local`, not in a `QObject`.** cxx-qt creates each `QObject` from QML,
//! with no arguments, and the game screen, the library and the settings need to see the same state.
//! Everything runs on the interface thread — the emulator too, see
//! `docs/implementation/01-architecture.md` —, so a `RefCell` is enough; a `Mutex` would be
//! synchronization with nobody on the other side.
//!
//! **Never emit a signal from inside [`with`].** A signal calls into QML right away, QML may call
//! back into a `QObject` that also enters [`with`], and the second borrow of the `RefCell` crashes
//! the program. The caller takes what it needs in here and emits afterwards.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Instant;

use zeebx::egui::Key;
use zeebx::input;
use zeebx::input::padview::PadArt;
use zeebx::library::{self, Game};
use zeebx::session::Z_WHEEL;
use zeebx::input::bindings::Device;
use zeebx::ui::calibration::{Calibration, Status};
use zeebx::ui::zwheel_metadata::{self, ZWheelMetadata, GameEntry};
use zeebx::ui::debug_panel;
use zeebx::ui::{updater, discord};
use zeebx::ui::navigation::{self, Command, Navigation};
use zeebx::video::icon::{self, Image};
use zeebx::ui::host_input::DesktopInput;
use zeebx::ui::i18n::{self, Catalog};
use zeebx::ui::running_game::{self, Opening, RunningGame, Report, Output, TickTurbo};
use zeebx::ui::save_slots;
use zeebx::ui::screenshot;
use zeebx::ui::settings::{self, KeyCombo, GameConfig, Modifiers, AspectRatio, Settings};
use zeebx::ui::themes::{self, Theme};
use zeebx::video::rasterizer::GpuFrame;
use zeebx::video::display::Framebuffer;

use super::bridge::qobject as gl;

/// The image for games that have none: the emulator's logo, as in egui.
const FALLBACK_IMAGE: &[u8] = include_bytes!("../../../../assets/zeebx.png");

thread_local! {
    static CORE: RefCell<Option<Core>> = const { RefCell::new(None) };
}

/// Runs `f` with the interface state, creating it the first time.
pub fn with<T>(f: impl FnOnce(&mut Core) -> T) -> T {
    CORE.with(|emu_core| {
        let mut emu_core = emu_core.borrow_mut();
        f(emu_core.get_or_insert_with(Core::create))
    })
}

/// Drops the open game while its GL context is still alive. `launch` calls this after the engine
/// exits and before destroying the context — see the order described there.
pub fn shutdown() {
    CORE.with(|emu_core| {
        if let Some(mut emu_core) = emu_core.borrow_mut().take() {
            emu_core.close();
        }
    });
}

/// What one turn of the game asks of the window. See [`Core::step`].
#[derive(Default)]
pub struct Tick {
    /// The game exited on its own and there is nowhere to go back to: the game window closes.
    pub closed: bool,
    /// Another game opened in this turn — through the Z-Wheel —, and the status line changes: its
    /// title, or why it did not open.
    pub state: Option<String>,
    /// Why the game stopped, already translated; empty while it runs. A game that stopped keeps
    /// its last frame on display, and the reason has to show up somewhere.
    pub stopped: String,
    /// The Boomerang calibration notice in this frame, if there is one. See
    /// [`zeebx::ui::calibration`].
    pub warning: Option<OnScreenNotice>,
    /// A screenshot finished saving since the previous turn.
    pub screenshot: Option<ScreenshotNotice>,
    /// What to write over the game — fast-forward, turbo on —; empty hides it.
    pub indicators: String,
    /// The speed and frames per second of the latest sample, for the status bar.
    pub pace: Option<(u32, u32)>,
    /// The lines of the metrics box in the corner, the first one with the frames per second; empty
    /// with the option off. See [`Core::metrics`].
    pub metrics: Vec<String>,
    /// This tick's RetroAchievements notices: the recognized game, an achievement.
    pub achievements: Vec<super::achievements::AchievementNotice>,
}

/// What to say about a screenshot that finished, already translated.
pub struct ScreenshotNotice {
    pub text: String,
    pub failed: bool,
    /// The `file://` address of the game's folder, so a click on the notice opens it.
    pub dir: String,
}

/// What to say about a save state shortcut, already translated.
pub struct StateNotice {
    pub text: String,
    pub failed: bool,
}

impl StateNotice {
    fn ok(text: String) -> Self {
        Self { text, failed: false }
    }

    fn failed(text: String) -> Self {
        Self { text, failed: true }
    }
}

/// What a save state shortcut or menu item asks for. See [`Core::run_state_request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateRequest {
    Save,
    Load,
    Next,
    Previous,
    Undo,
}

/// What the saving thread answers: the file, or the reason for the failure, and the folder.
type SaveResult = (Result<PathBuf, String>, PathBuf);

/// The calibration notice with its texts already in place, as the window draws it.
pub struct OnScreenNotice {
    pub title: String,
    /// Empty once done.
    pub state: String,
    pub stopped: bool,
    /// In radians.
    pub rotation: f32,
}

/// What the game screen shows. See [`Core::frame`].
pub enum Frame<'a> {
    /// The GPU rasterizer's texture, at the internal resolution, without going back to the CPU.
    Gpu(GpuFrame),
    /// The console screen in RGB565 — the 2D, the software 3D, or the GPU with something drawn
    /// over it in 2D.
    Screen(&'a Framebuffer),
}

pub struct Core {
    pub settings: Settings,
    pub catalog: Catalog,
    /// The factory themes and the ones from the folders, read once at startup, like the languages.
    /// See [`zeebx::ui::themes`].
    pub themes: Vec<Theme>,
    pub host_input: DesktopInput,
    /// The controller art, with each button's silhouette. Empty if the art fails to open: the
    /// controls screen still works through the list.
    pub art: Option<PadArt>,
    running_game: Option<RunningGame>,
    /// Where the open session was opened from: it is what [`Core::restart`] opens again.
    open_path: Option<PathBuf>,
    /// The open game's settings, when it has its own: the global ones with its sections on top.
    /// It is from here, and not from `settings`, that the step reads the controls and the pace.
    /// See [`GameConfig`].
    custom_settings: Option<Settings>,
    /// The settings of the game the settings window is editing, by key: read from disk once, and
    /// not for every value QML asks for. See [`Core::with_game`].
    edit_cache: Option<(String, Settings)>,
    /// The key of the game whose settings are in `settings` right now, in the middle of a
    /// [`Core::with_game`]. See [`Core::save_settings`].
    editing: Option<String>,
    /// The context the GPU rasterizer borrows. Created on the first open, not at startup: it
    /// needs the `QGuiApplication` up. With it, everything that touches the session — the step,
    /// opening, and the end of a session, which frees textures — happens with the context current.
    gl: Option<Arc<glow::Context>>,
    /// "software", or the GPU the context found: it goes to the status line.
    gpu: Option<String>,
    /// The games in the ROMs folder.
    pub games: Vec<Game>,
    /// What the list shows, in title order: `(title, index into games)`. The Z-Wheel is left out
    /// of it and opens from the button, as in egui, and the search keeps only what matches.
    pub list: Vec<(String, usize)>,
    /// What is typed in the search.
    search: String,
    pub z_wheel: Option<PathBuf>,
    /// Covers, names, logos and descriptions the Z-Wheel carries for the games.
    pub zwheel_metadata: Option<ZWheelMetadata>,
    /// The image for games that have none.
    fallback: Option<Image>,
    /// Changes on every scan. It goes into the image addresses: Qt caches them by address, and
    /// without this a cover replaced in the folder would stay the old one after searching again.
    pub generation: u32,
    /// Whether each game has a cover next to it, by index into `games`: the answer reads the disk,
    /// and QML asks for every row that appears. Rebuilt on every scan.
    sidecar_covers: HashMap<usize, bool>,
    navigation: Navigation,
    /// The Discord presence, following what the interface shows.
    pub presence: discord::PresenceTracker,
    /// The new-version check in progress, and what it answered.
    update_check: Option<Receiver<updater::Response>>,
    pub updater: Option<updater::Response>,
    /// A new version arrived and its notice has not been dismissed yet.
    update_notice: bool,
    /// The check in progress was requested from the menu: whoever asked wants the answer even
    /// with no new version. See [`Core::update_check_result`].
    check_from_menu: bool,
    update_check_result: Option<String>,
    /// The new version being downloaded and installed. The notice shows the progress.
    pub update_installation: Option<updater::UpdateTask>,
    /// The report of the run, saved automatically. See [`Report`].
    report: Report,
    /// The log window was closed in this run. Reset when another game opens: closing dismisses
    /// the log of **this** run, not the preference.
    pub log_dismissed: bool,
    calibration: Calibration,
    /// The keys pressed in the game window, as `egui::Key`. See
    /// [`zeebx::ui::host_input::key_pressed`].
    key_events: HashSet<Key>,
    /// Where the screenshot saving threads answer. See [`Core::screenshot`].
    save_results: (Sender<SaveResult>, Receiver<SaveResult>),
    /// The save state shortcuts' slot, zero-based. It carries over from one game to the next, as
    /// in PCSX2: someone who keeps everything in 3 does not want to pick 3 again for every game.
    slot: usize,
    /// The RetroAchievements achievements. See [`super::achievements`].
    pub achievements: super::achievements::InCore,
}

impl Core {
    fn create() -> Self {
        let settings = Settings::load();
        // The same language as egui: the one chosen in the settings, or the system's.
        let mut catalog = Catalog::new(&settings::language_dirs());
        match &settings.language {
            Some(code) => {
                catalog.select(code);
            }
            None => {
                catalog.select_best(&i18n::system_language());
            }
        }
        let mut emu_core = Self {
            settings,
            catalog,
            themes: themes::load(&themes::dirs()),
            host_input: DesktopInput::start(),
            art: PadArt::builtin()
                .inspect_err(|error| eprintln!("controller art: {error:?}"))
                .ok(),
            running_game: None,
            open_path: None,
            custom_settings: None,
            edit_cache: None,
            editing: None,
            gl: None,
            gpu: None,
            games: Vec::new(),
            list: Vec::new(),
            search: String::new(),
            z_wheel: None,
            zwheel_metadata: None,
            fallback: icon::decode(FALLBACK_IMAGE).ok(),
            generation: 0,
            sidecar_covers: HashMap::new(),
            navigation: Navigation::default(),
            presence: discord::PresenceTracker::default(),
            update_check: None,
            updater: None,
            update_notice: false,
            check_from_menu: false,
            update_check_result: None,
            update_installation: None,
            report: Report::default(),
            log_dismissed: false,
            calibration: Calibration::default(),
            key_events: HashSet::new(),
            save_results: mpsc::channel(),
            slot: 0,
            achievements: Default::default(),
        };
        emu_core.search_again();
        // At startup, the question to GitHub, as in egui: the answer arrives through the library's
        // timer, and the notice waits for the startup one to get out of the way.
        if emu_core.settings.updates.on_launch && updater::Installation::current().warn() {
            emu_core.check_for_updates();
        }
        emu_core
    }

    /// Scans the ROMs folder again, and rereads the Z-Wheel and its collection. It is the "search
    /// again".
    pub fn search_again(&mut self) {
        self.games = self
            .settings
            .roms_dir
            .as_deref()
            .map(library::scan)
            .unwrap_or_default();
        if let Err(error) = library::sync_catalog(&self.games) {
            eprintln!("game catalog: {error}");
        }
        self.sidecar_covers.clear();
        self.refresh_z_wheel();
    }

    /// Resolves where to open the Z-Wheel from and rereads its collection, without scanning the
    /// folder again. It is what changing the Z-Wheel in the settings does: each game's cover and
    /// name may change along with it.
    pub fn refresh_z_wheel(&mut self) {
        self.z_wheel = library::z_wheel_for(self.settings.z_wheel_path.as_deref(), &self.games);
        self.zwheel_metadata = self.z_wheel.as_deref().and_then(ZWheelMetadata::load);
        self.generation = self.generation.wrapping_add(1);
        self.rebuild_list();
    }

    /// Switches the interface language. The Z-Wheel's official names are per language, so the
    /// list is rebuilt along with it. Returns whether the language exists.
    /// The chosen theme, if there is one and it was read. A theme that vanished from the folder,
    /// or that did not open, is the system palette, without erasing the choice: once the file is
    /// back, so is the theme.
    pub fn theme(&self) -> Option<&Theme> {
        let key_id = self.settings.theme.as_deref()?;
        self.themes.iter().find(|theme| theme.key_id == key_id)
    }

    pub fn set_language(&mut self, code: &str) -> bool {
        if !self.catalog.select(code) {
            return false;
        }
        self.settings.language = Some(code.to_string());
        self.rebuild_list();
        true
    }

    /// What the interface does on every read, with or without a game: tells Discord what is
    /// happening and collects the answer of the new-version check.
    pub fn each_frame(&mut self) {
        let class_id = self.running_game.as_ref().map(|running_game| running_game.session().class_id());
        let title = self.title();
        let discord = &self.settings.discord;
        self.presence
            .update(discord.active, &self.catalog, class_id, title, &discord.covers_url);
        if self.achievements.each_frame(&mut self.settings) {
            self.save_settings();
        }
        if let Some(channel) = &self.update_check {
            match channel.try_recv() {
                Ok(response) => {
                    self.update_notice = matches!(response, updater::Response::NewVersion(_));
                    // A new version opens the usual notice; the rest only shows to whoever asked.
                    if std::mem::take(&mut self.check_from_menu) {
                        self.update_check_result = match &response {
                            updater::Response::NewVersion(_) => None,
                            updater::Response::UpToDate => Some(self.catalog.format(
                                "update.up_to_date",
                                &[("current", updater::CURRENT_VERSION)],
                            )),
                            updater::Response::Failed(reason) => Some(
                                self.catalog
                                    .format("update.check_failed", &[("reason", reason)]),
                            ),
                        };
                    }
                    self.updater = Some(response);
                    self.update_check = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.update_check = None,
            }
        }
    }

    /// The startup notice has not been dismissed in this version yet. A new version shows it again.
    pub fn startup_notice_pending(&self) -> bool {
        self.settings.notice_dismissed_in_version.as_deref() != Some(updater::CURRENT_VERSION)
    }

    /// The startup notice got out of the way. `for_good` is the "don't show again": it stores the
    /// version.
    pub fn dismiss_startup_notice(&mut self, for_good: bool) {
        if for_good {
            self.settings.notice_dismissed_in_version = Some(updater::CURRENT_VERSION.to_owned());
            if let Err(error) = self.settings.save() {
                eprintln!("could not save the settings: {error}");
            }
        }
    }

    /// The new version to announce, and its page, if there is a pending notice.
    pub fn update_notice(&self) -> Option<&updater::Release> {
        match (&self.updater, self.update_notice) {
            (Some(updater::Response::NewVersion(release)), true) => Some(release),
            _ => None,
        }
    }

    pub fn dismiss_update_notice(&mut self) {
        self.update_notice = false;
    }

    /// Shows the new-version notice again, if there is one: the installation goes through it.
    pub fn reopen_update_notice(&mut self) {
        self.update_notice = matches!(self.updater, Some(updater::Response::NewVersion(_)));
    }

    /// Starts downloading and installing the announced new version. An installation in progress
    /// is not started again.
    pub fn install_update(&mut self) {
        let in_progress = self.update_installation.as_ref().is_some_and(|a| !a.finished());
        if let (Some(updater::Response::NewVersion(release)), false) = (&self.updater, in_progress) {
            self.update_installation = Some(updater::install(release, "latest-qt.json"));
        }
    }

    /// Saves the settings and replaces this run with the installed version.
    pub fn restart_into_new_version(&self) -> ! {
        if let Err(error) = self.settings.save() {
            eprintln!("could not save the settings: {error}");
        }
        updater::restart()
    }

    /// What the open game wrote, and the emulator's complaints about it.
    pub fn log(&self) -> Vec<String> {
        self.running_game.as_ref().map(|p| p.session().log()).unwrap_or_default()
    }

    /// Where this run's report is saved automatically.
    pub fn report_path(&self) -> Option<PathBuf> {
        self.running_game.as_ref().map(RunningGame::report_path)
    }

    /// Asks GitHub whether there is a new version. The answer arrives through [`Core::each_frame`].
    pub fn check_for_updates(&mut self) {
        self.updater = None;
        let pre = self.settings.updates.prereleases;
        self.update_check = Some(updater::search(pre));
    }

    /// The check requested from the Help menu. The new-version notice is the usual one; "up to
    /// date" and the failure come back through [`Core::update_check_result`], which at startup
    /// would stay silent.
    pub fn check_for_updates_from_menu(&mut self) {
        self.check_from_menu = true;
        self.update_check_result = None;
        self.check_for_updates();
    }

    /// The answer of the menu check that is not a new version, once.
    pub fn update_check_result(&mut self) -> Option<String> {
        self.update_check_result.take()
    }

    pub fn checking_for_update(&self) -> bool {
        self.update_check.is_some()
    }

    /// Changes the open game's session, with the GL context current: the internal resolution, the
    /// aspect ratio and the enhancements rebuild the target on the GPU, and that is GL.
    ///
    /// **Only when what is being edited is what the game uses.** One game's settings do not touch
    /// another that is open, and the global ones do not touch an open game that has its own.
    pub fn with_session(&mut self, f: impl FnOnce(&mut zeebx::session::Session)) {
        let open_key = self.running_game.as_ref().and_then(RunningGame::key_id);
        let applies = match &self.editing {
            Some(key_id) => open_key == Some(key_id.as_str()),
            None => self.custom_settings.is_none(),
        };
        if !applies {
            return;
        }
        let with_gpu = self.gl.is_some();
        if let Some(running_game) = self.running_game.as_mut() {
            in_context(with_gpu, || f(running_game.session_mut()));
        }
    }

    /// The settings that apply to the open game right now: its own, when it has them, or the
    /// global ones.
    pub fn effective(&self) -> &Settings {
        self.custom_settings.as_ref().unwrap_or(&self.settings)
    }

    /// Runs `f` with the settings of the game of `key_id` in place of the global ones, when it has
    /// its own; without `key_id`, or without its own, it runs with the global ones.
    ///
    /// **Why swap, and not pass another reference.** The settings window reads and writes
    /// `emu_core.settings` in almost a hundred places; a game's window is the same window, with the
    /// swap around each call. During the swap, [`Core::save_settings`] writes to the game's file,
    /// and [`Core::with_session`] only touches the open game if it is that one. Everything runs on
    /// the interface thread, between two turns of the game: no step sees the swap.
    pub fn with_game<T>(&mut self, key_id: &str, f: impl FnOnce(&mut Self) -> T) -> T {
        if key_id.is_empty() {
            return f(self);
        }
        let cached = self.edit_cache.take().filter(|(which, _)| which == key_id);
        let for_game = match cached {
            Some((_, for_game)) => Some(for_game),
            None => GameConfig::load(key_id).map(|config| config.about(&self.settings)),
        };
        let Some(mut for_game) = for_game else {
            return f(self);
        };
        std::mem::swap(&mut self.settings, &mut for_game);
        self.editing = Some(key_id.to_string());
        let response = f(self);
        self.editing = None;
        std::mem::swap(&mut self.settings, &mut for_game);
        // The open game, if it is this one, starts reading what just changed.
        if self.running_game.as_ref().and_then(RunningGame::key_id) == Some(key_id) {
            self.custom_settings = Some(for_game.clone());
        }
        self.edit_cache = Some((key_id.to_string(), for_game));
        response
    }

    /// Saves the settings that are in `settings`: the game's in the middle of a
    /// [`Core::with_game`], the global ones outside it.
    pub fn save_settings(&self) {
        let saved = match &self.editing {
            Some(key_id) => GameConfig::from_settings(&self.settings).save(key_id),
            None => self.settings.save(),
        };
        if let Err(error) = saved {
            eprintln!("could not save the settings: {error}");
        }
    }

    /// Whether the game of `key_id` has its own settings.
    pub fn custom(&self, key_id: &str) -> bool {
        settings::game_settings_path(key_id).is_file()
    }

    /// Turns the own settings of the game of `key_id` on or off.
    ///
    /// Turning on copies the current global ones: it is the starting point, and the game only
    /// becomes different when someone changes something. Turning off deletes the file, and the
    /// game goes back to the global ones — even while open.
    pub fn customize(&mut self, key_id: &str, enabled: bool) {
        if key_id.is_empty() || enabled == self.custom(key_id) {
            return;
        }
        self.edit_cache = None;
        let outcome = match enabled {
            true => GameConfig::from_settings(&self.settings).save(key_id),
            false => GameConfig::remove(key_id),
        };
        if let Err(error) = outcome {
            eprintln!("could not change the game's settings: {error}");
            return;
        }
        if self.running_game.as_ref().and_then(RunningGame::key_id) == Some(key_id) {
            self.custom_settings = enabled.then(|| self.settings.clone());
        }
    }

    /// The search changed.
    pub fn set_search(&mut self, search: &str) {
        self.search = search.to_string();
        self.rebuild_list();
    }

    /// Rebuilds the list without scanning the folder: the search, the language or the Z-Wheel
    /// changed.
    pub fn rebuild_list(&mut self) {
        let language = self.catalog.current().to_string();
        let zwheel_metadata = self.zwheel_metadata.as_ref();
        let mut list: Vec<(String, usize)> = (0..self.games.len())
            .filter(|&i| self.games[i].clsid != Some(Z_WHEEL))
            .map(|i| (zwheel_metadata::title_of(zwheel_metadata, &self.games[i], &language), i))
            .filter(|(title, _)| library::matches_search(title, &self.search))
            .collect();
        list.sort_by_key(|(title, _)| title.to_lowercase());
        self.list = list;
    }

    /// How many games the library has, not counting the Z-Wheel and ignoring the search.
    pub fn total(&self) -> usize {
        self.games.iter().filter(|game| game.clsid != Some(Z_WHEEL)).count()
    }

    pub fn current_search(&self) -> &str {
        &self.search
    }

    pub fn searching(&self) -> bool {
        !self.search.trim().is_empty()
    }

    /// The Z-Wheel's entry for the row's game, if it knows the game.
    pub fn info(&self, line: usize) -> Option<&GameEntry> {
        let (_, index) = self.list.get(line)?;
        let class_id = self.games[*index].clsid?;
        self.zwheel_metadata.as_ref()?.info(class_id)
    }

    /// The image of the game at index `index` in `games`. See [`zwheel_metadata::game_image`].
    pub fn cover(&mut self, index: usize) -> Option<&Image> {
        let game = self.games.get(index)?;
        let sidecar = *self
            .sidecar_covers
            .entry(index)
            .or_insert_with(|| zwheel_metadata::has_sidecar_cover(game));
        let info = game.clsid.and_then(|class_id| self.zwheel_metadata.as_ref()?.info(class_id));
        zwheel_metadata::game_image(game, info, self.fallback.as_ref(), sidecar)
    }

    /// The image QML asked for, by its address without the scheme: `cover/<generation>/<index>`,
    /// `logo/<generation>/<class_id>` or `rating/<generation>/<class_id>`.
    pub fn image(&mut self, address: &str) -> Option<&Image> {
        let mut parts = address.split('/');
        let (kind, _generation, key_id) = (parts.next()?, parts.next()?, parts.next()?);
        let key_id: usize = key_id.parse().ok()?;
        match kind {
            "cover" => self.cover(key_id),
            "logo" => self.zwheel_metadata.as_ref()?.info(key_id as u32)?.logo.as_ref(),
            "rating" => self.zwheel_metadata.as_ref()?.info(key_id as u32)?.rating.as_ref(),
            _ => None,
        }
    }

    /// The controller art for QML: `controle/base`, or `controle/parte/<index>/<rrggbbaa>` — a
    /// button's silhouette already in the requested color. The silhouette only stores the opacity;
    /// the color is the one the screen picks on the spot (lit, under the cursor, waiting for the
    /// capture), as in egui.
    pub fn controller_image(&self, address: &str) -> Option<Image> {
        let art = self.art.as_ref()?;
        let mut parts = address.split('/').skip(1);
        match parts.next()? {
            "base" => Some(Image {
                width: art.width,
                height: art.height,
                rgba: art.base.clone(),
            }),
            "parte" => {
                let part = art.parts().get(parts.next()?.parse::<usize>().ok()?)?;
                let color = u32::from_str_radix(parts.next()?, 16).ok()?.to_be_bytes();
                let mut rgba = Vec::with_capacity(part.alpha.len() * 4);
                for alpha in &part.alpha {
                    let alpha = (u32::from(*alpha) * u32::from(color[3]) / 255) as u8;
                    rgba.extend_from_slice(&[color[0], color[1], color[2], alpha]);
                }
                Some(Image {
                    width: part.width,
                    height: part.height,
                    rgba,
                })
            }
            _ => None,
        }
    }

    /// The ports changed in the settings: with a game open, it applies right away, as in egui.
    /// Storing it and applying only on the next session would make the setting look like it did
    /// not take.
    pub fn ports_changed(&mut self) {
        let ports = std::array::from_fn(|port| {
            self.settings
                .controls
                .player(port)
                .filter(|player| player.enabled)
                .map(|player| player.device_kind)
        });
        // Through `with_session`: one game's ports do not go to another that is open.
        self.with_session(|session| session.set_ports(ports));
    }

    /// The commands the controller sends to the library in this read.
    ///
    /// **Only the controller.** The library's keyboard belongs to QML, which already moves through
    /// the list with the arrows: passing the keys here too would make an arrow mapped on the
    /// controller move two steps. With a game open, or the main window without focus, the
    /// controller does not belong to the library.
    pub fn commands(&mut self, listening: bool) -> Vec<Command> {
        if !listening || self.running_game.is_some() {
            self.navigation.mute();
            return Vec::new();
        }
        let pads: Vec<_> = self
            .host_input
            .pads(&self.settings.controls, &self.settings.shortcuts, &HashSet::new())
            .into_iter()
            .map(|(_, pad)| pad)
            .collect();
        let held_now = navigation::pressed([false; 5], &pads);
        self.navigation.commands(held_now, Instant::now())
    }

    /// What the game screen shows right now.
    ///
    /// **The large texture only when it is what is on screen**: [`Session::gpu_frame`] returns it
    /// only if nothing was drawn in 2D after the last `eglSwapBuffers`. With a HUD through
    /// `IDisplay`, a message box or the Z-Wheel, which composes the 3D on the CPU, the 640×480
    /// screen is what counts.
    ///
    /// **The 640×480 screen first receives the frame the GPU left pending**, as in egui. On the
    /// GPU, `eglSwapBuffers` does not read the frame back: the CPU screen only receives it when
    /// someone asks ([`Session::materialize_gl_frame`]). At the 1x internal resolution the large
    /// texture does not exist and the window always shows the screen, and read without this it
    /// held the frame from the last time the game drew in 2D: Ultimate Chess 3D flickered between
    /// the scene and black.
    ///
    /// [`Session::gpu_frame`]: zeebx::session::Session::gpu_frame
    /// [`Session::materialize_gl_frame`]: zeebx::session::Session::materialize_gl_frame
    pub fn frame(&mut self) -> Option<Frame<'_>> {
        let with_gpu = self.gl.is_some();
        let running_game = self.running_game.as_mut()?;
        if running_game.session().gpu_frame().filter(|_| with_gpu).is_none() {
            // It is GL: reads the frame from the GPU, with the game's context current.
            in_context(with_gpu, || running_game.session_mut().materialize_gl_frame());
        }
        let session = self.running_game.as_ref()?.session();
        let gpu = self.gl.as_ref().and(session.gpu_frame());
        Some(match gpu {
            Some(frame) => Frame::Gpu(frame),
            None => Frame::Screen(session.screen()),
        })
    }

    /// The open game's title and where it draws.
    pub fn state(&self) -> String {
        let Some(title) = self.title() else {
            return String::new();
        };
        let graphics = &self.effective().graphics;
        let location = match (&self.gpu, graphics.gpu_rasterizer) {
            (Some(gpu), true) => format!("GPU: {gpu}"),
            _ => "software".to_string(),
        };
        format!("{title} — {location}")
    }

    /// The open game's name.
    ///
    /// The session only knows the extraction folder, which carries the package's fingerprint
    /// (`Zeebo-Extreme-Boia-Cross-21503726-1788761080`). The name comes from the library by
    /// ClassID — the Z-Wheel's official one when it describes the game —, and only without it the
    /// folder, without the two numbers at the end. The egui rule.
    pub fn title(&self) -> Option<String> {
        let session = self.running_game.as_ref()?.session();
        let class_id = session.class_id();
        if let Some(game) = self.games.iter().find(|game| game.clsid == Some(class_id)) {
            return Some(zwheel_metadata::title_of(self.zwheel_metadata.as_ref(), game, self.catalog.current()));
        }
        let title = library::strip_fingerprint(session.title());
        (!title.is_empty()).then_some(title)
    }

    /// Opens a game chosen in the main window. It was not the Z-Wheel that asked: when the game
    /// exits on its own, the game window closes instead of going back to it.
    pub fn open_from_library(&mut self, path: &Path) -> Result<(), String> {
        let opened = self.open(path);
        if let Some(running_game) = self.running_game.as_mut() {
            running_game.opened_by_z_wheel = false;
        }
        opened
    }

    /// Replaces the session with the one of the game at `path`. On failure, the previous one
    /// carries on, as in egui.
    fn open(&mut self, path: &Path) -> Result<(), String> {
        if self.gl.is_none() {
            match qt_context() {
                Ok((context, gpu)) => {
                    self.gl = Some(context);
                    self.gpu = Some(gpu);
                }
                Err(error) => eprintln!("{error} — continuing with the software rasterizer"),
            }
        }
        self.key_events.clear();
        // The calibration counts belong to the session: the new one starts from zero.
        self.calibration.restart();
        if let Some(running_game) = self.running_game.as_mut() {
            running_game.forget_input();
        }
        // The serial is turned on together with the start, not afterwards: the applet's
        // constructor runs inside the opening, and what it does when it is born has to be in the
        // capture.
        let serial = self
            .settings
            .debug
            .log
            .then(|| running_game::serial_path(&library::title_for(path)));
        self.report.forget();
        self.log_dismissed = false;
        // The game's own, when it has them: they apply at opening and, once stored, in the step.
        let custom_settings = library::game_key(path)
            .and_then(|key_id| GameConfig::load(&key_id))
            .map(|config| config.about(&self.settings));
        let opening = Opening {
            settings: custom_settings.as_ref().unwrap_or(&self.settings),
            serial: serial.as_deref(),
            gl: self.gl.clone(),
            installed: self
                .games
                .iter()
                .filter_map(|game| Some((game.clsid?, library::module_id(&game.path)?)))
                .collect(),
        };
        let previous = self.running_game.as_mut();
        let with_gpu = self.gl.is_some();
        let mut running_game = in_context(with_gpu, || RunningGame::open(path, opening, previous))
            .map_err(|error| error.to_string())?;
        // The previous one lets go of the game first: unloading afterwards would unload the new one.
        if let Some(old) = self.running_game.as_mut() {
            self.achievements.disable(old.session_mut());
        }
        self.achievements.enable(running_game.session_mut());
        // The previous one goes away here, and with the context current: its textures live in it.
        self.custom_settings = custom_settings;
        self.open_path = Some(path.to_path_buf());
        in_context(with_gpu, || self.running_game = Some(running_game));
        Ok(())
    }

    /// Drops the session with its context current: freeing textures with the Qt Quick context
    /// current would delete Qt Quick's.
    pub fn close(&mut self) {
        if let Some(running_game) = self.running_game.as_mut() {
            self.achievements.disable(running_game.session_mut());
        }
        if self.running_game.is_some() {
            in_context(self.gl.is_some(), || self.running_game = None);
            self.custom_settings = None;
            self.open_path = None;
        }
        self.key_events.clear();
    }

    /// The debug panel, when on: the texts and the timeline, as `(speed, frames)` from the oldest
    /// sample to the newest. See [`zeebx::ui::debug_panel`].
    pub fn debug_panel(&self) -> Option<(Vec<String>, Vec<(u32, u32)>)> {
        let debug = &self.settings.debug;
        let session = self.running_game.as_ref()?.session();
        if !debug.overlay {
            return None;
        }
        let texts = debug_panel::Texts::fresh(
            &self.catalog,
            debug,
            session.sample(),
            session.memory(),
            session.clock_ms(),
        );
        let texts = [texts.speed, texts.clock, texts.memory]
            .into_iter()
            .flatten()
            .collect();
        let history = match debug.timeline {
            true => session.history().map(|a| (a.speed, a.fps)).collect(),
            false => Vec::new(),
        };
        Some((texts, history))
    }

    /// Whether closing the open game asks for confirmation. The Z-Wheel closes without asking: it
    /// is the home screen, and keeps nothing that could be lost.
    pub fn confirm_on_close(&self) -> bool {
        self.settings.graphics.confirm_on_close
            && self
                .running_game
                .as_ref()
                .is_some_and(|running_game| running_game.session().class_id() != Z_WHEEL)
    }

    pub fn toggle_pause(&mut self) {
        if let Some(running_game) = self.running_game.as_mut() {
            running_game.toggle_pause();
        }
    }

    pub fn paused(&self) -> bool {
        self.running_game.as_ref().is_some_and(RunningGame::paused)
    }

    /// A key from the game window. Each event goes to the session right away: see
    /// [`RunningGame::keyboard_changed`].
    pub fn key(&mut self, key: Key, pressed: bool) {
        match pressed {
            true => self.key_events.insert(key),
            false => self.key_events.remove(&key),
        };
        if let Some(running_game) = self.running_game.as_mut() {
            running_game.keyboard_changed(self.key_events.iter().filter_map(|key| input::avk_for(*key)));
        }
    }

    /// The game window lost focus: no key stays pressed.
    pub fn release_keys(&mut self) {
        self.key_events.clear();
    }

    /// Restarts the open game, from scratch, as if it had just been chosen.
    ///
    /// **It is opening the same path again**, not a machine reset: opening is the only path that
    /// sets everything up the way the game expects — its settings, the ports, the sound, the
    /// Z-Wheel it came from. What the game saved to disk stays; what it did not save is lost, as
    /// with a console's reset button. On failure, the previous session carries on.
    pub fn restart(&mut self) -> Result<(), String> {
        let Some(path) = self.open_path.clone() else {
            return Err("no game open".to_string());
        };
        let via_z_wheel = self.running_game.as_ref().is_some_and(|p| p.opened_by_z_wheel);
        self.open(&path)?;
        if let Some(running_game) = self.running_game.as_mut() {
            running_game.opened_by_z_wheel = via_z_wheel;
            running_game.reset_clock();
        }
        Ok(())
    }

    /// Whether the key is the restart one. Like the screenshot one, it is checked before the game.
    pub fn is_reset_shortcut(&self, key: Key) -> bool {
        Key::from_name(&self.settings.shortcuts.restart) == Some(key)
    }

    /// Whether the key is the screenshot one. The game window asks before passing it to the game:
    /// the shortcut wins over a button mapped to the same key.
    pub fn is_screenshot_shortcut(&self, key: Key) -> bool {
        Key::from_name(&self.settings.shortcuts.screenshot) == Some(key)
    }

    /// What the save state shortcut asks for, if the key, with these modifiers, is one of them.
    fn state_request(&self, key: Key, modifiers: Modifiers) -> Option<StateRequest> {
        let shortcuts = &self.settings.shortcuts;
        let wanted = Some(KeyCombo { key, modifiers });
        [
            (&shortcuts.save_state, StateRequest::Save),
            (&shortcuts.load_state, StateRequest::Load),
            (&shortcuts.next_slot, StateRequest::Next),
            (&shortcuts.previous_slot, StateRequest::Previous),
            (&shortcuts.undo_load, StateRequest::Undo),
        ]
        .into_iter()
        .find(|(text, _)| KeyCombo::read(text) == wanted)
        .map(|(_, request)| request)
    }

    /// Whether the key, with these modifiers, is a save state shortcut. The game window asks
    /// before anything else: the shortcut wins over a button mapped to the same key.
    pub fn is_state_shortcut(&self, key: Key, modifiers: Modifiers) -> bool {
        self.state_request(key, modifiers).is_some()
    }

    /// Does what the save state shortcut asks, if the key is one of them: the notice, and whether
    /// the console went back to another instant. `None` when the key is no shortcut at all, or
    /// there is no game open.
    ///
    /// The slots live with the rest of the profile, in `<config>/states/<content>/`, the same
    /// layout as Android. See `docs/implementation/25-desktop-save-states.md`.
    pub fn state_shortcut(
        &mut self,
        key: Key,
        modifiers: Modifiers,
    ) -> Option<(StateNotice, bool)> {
        let request = self.state_request(key, modifiers)?;
        self.run_state_request(request, None)
    }

    /// Runs the save state request on `slot`, or on the current slot. Chosen from the menu, the
    /// slot becomes the current one, as in PCSX2: the following F1 and F3 go to it. `None` with no
    /// game open.
    pub fn run_state_request(
        &mut self,
        request: StateRequest,
        slot: Option<usize>,
    ) -> Option<(StateNotice, bool)> {
        if let Some(slot) = slot.filter(|slot| *slot < save_slots::SLOTS) {
            self.slot = slot;
        }
        let with_gpu = self.gl.is_some();
        let running_game = self.running_game.as_mut()?;
        let root = zeebx::config::config_dir();
        let number = |slot: usize| (slot + 1).to_string();
        let catalog = &self.catalog;
        let failure = |reason: String| {
            StateNotice::failed(catalog.format("play.state.error", &[("reason", &reason)]))
        };
        let warning = match request {
            StateRequest::Next | StateRequest::Previous => {
                self.slot = match request {
                    StateRequest::Next => (self.slot + 1) % save_slots::SLOTS,
                    _ => (self.slot + save_slots::SLOTS - 1) % save_slots::SLOTS,
                };
                let busy = running_game
                    .session()
                    .content()
                    .and_then(|id| save_slots::list(&root, id.as_str())[self.slot].clone());
                let status = match busy {
                    Some(slot) => catalog.format(
                        "play.state.used",
                        &[("size", &save_slots::size(slot.bytes))],
                    ),
                    None => catalog.get("play.state.empty").to_string(),
                };
                let text = catalog.format(
                    "play.state.selected",
                    &[("slot", &number(self.slot)), ("status", &status)],
                );
                return Some((StateNotice::ok(text), false));
            }
            StateRequest::Save => {
                match in_context(with_gpu, || running_game.save_state(&root, self.slot)) {
                    Ok(_) => StateNotice::ok(
                        catalog.format("play.state.saved", &[("slot", &number(self.slot))]),
                    ),
                    Err(reason) => failure(reason),
                }
            }
            StateRequest::Load => {
                let empty = running_game
                    .session()
                    .content()
                    .is_some_and(|id| save_slots::list(&root, id.as_str())[self.slot].is_none());
                if empty {
                    let text =
                        catalog.format("play.state.empty_slot", &[("slot", &number(self.slot))]);
                    return Some((StateNotice::failed(text), false));
                }
                match in_context(with_gpu, || running_game.load_state(&root, self.slot)) {
                    Ok(_) => {
                        let undo = KeyCombo::read(&self.settings.shortcuts.undo_load)
                            .filter(|_| running_game.can_undo_load());
                        let text = match undo {
                            Some(key) => catalog.format(
                                "play.state.loaded.undo",
                                &[("slot", &number(self.slot)), ("key", &key.label())],
                            ),
                            None => catalog
                                .format("play.state.loaded", &[("slot", &number(self.slot))]),
                        };
                        return Some((StateNotice::ok(text), true));
                    }
                    Err(reason) => failure(reason),
                }
            }
            StateRequest::Undo => {
                if !running_game.can_undo_load() {
                    let text = catalog.get("play.state.nothing_to_undo").to_string();
                    return Some((StateNotice::failed(text), false));
                }
                match in_context(with_gpu, || running_game.undo_load()) {
                    Ok(()) => {
                        let text = catalog.get("play.state.undone").to_string();
                        return Some((StateNotice::ok(text), true));
                    }
                    Err(reason) => failure(reason),
                }
            }
        };
        Some((warning, false))
    }

    /// The open game's slots for the menu, one per line: "Slot 3 · Used · 2.1 MB" or
    /// "Slot 4 · Empty". Empty with no game, or without the identity of its file.
    pub fn state_slots(&self) -> Vec<String> {
        let Some(id) = self.running_game.as_ref().and_then(|p| p.session().content()) else {
            return Vec::new();
        };
        let catalog = &self.catalog;
        save_slots::list(&zeebx::config::config_dir(), id.as_str())
            .iter()
            .enumerate()
            .map(|(slot, saved)| {
                let name = catalog.format("play.state.slot", &[("slot", &(slot + 1).to_string())]);
                let status = match saved {
                    Some(saved) => catalog.format(
                        "play.state.used",
                        &[("size", &save_slots::size(saved.bytes))],
                    ),
                    None => catalog.get("play.state.empty").to_string(),
                };
                format!("{name} · {status}")
            })
            .collect()
    }

    /// Which of the open game's slots hold a state, in order. All empty with no game.
    pub fn occupied_slots(&self) -> Vec<bool> {
        let Some(id) = self.running_game.as_ref().and_then(|p| p.session().content()) else {
            return vec![false; save_slots::SLOTS];
        };
        save_slots::list(&zeebx::config::config_dir(), id.as_str())
            .iter()
            .map(Option::is_some)
            .collect()
    }

    pub fn current_slot(&self) -> usize {
        self.slot
    }

    pub fn can_undo_load(&self) -> bool {
        self.running_game
            .as_ref()
            .is_some_and(RunningGame::can_undo_load)
    }

    /// Fast-forward from the menu: turns it on or off, and says how it ended up. See
    /// [`RunningGame::toggle_fast_forward_lock`].
    pub fn toggle_fast_forward_lock(&mut self) -> bool {
        self.running_game
            .as_mut()
            .is_some_and(RunningGame::toggle_fast_forward_lock)
    }

    pub fn fast_forward_locked(&self) -> bool {
        self.running_game.as_ref().is_some_and(RunningGame::fast_forward_locked)
    }

    /// Saves the frame that is on screen. `timestamp` is the local time, which comes from Qt.
    ///
    /// The read happens here, with the context current, because the texture can only be read in
    /// it; the PNG goes to a thread, and the answer arrives through [`Core::step`]. See
    /// `docs/implementation/22-screenshots.md`.
    pub fn screenshot(&mut self, timestamp: &str) {
        let title = self.title().unwrap_or_default();
        let with_gpu = self.gl.is_some();
        let Some(running_game) = self.running_game.as_mut() else {
            return;
        };
        let (width, height, rgb) = in_context(with_gpu, || running_game.session_mut().capture());
        let root = screenshot::dir(self.settings.screenshots_dir.as_deref());
        let dir = root.join(screenshot::safe_name(&title));
        let timestamp = timestamp.to_string();
        let response = self.save_results.0.clone();
        std::thread::spawn(move || {
            let recorded = screenshot::save(&root, &title, &timestamp, width, height, &rgb)
                .map_err(|error| error.to_string());
            match &recorded {
                Ok(path) => eprintln!("screenshot: {}", path.display()),
                Err(error) => eprintln!("screenshot not saved in {}: {error}", dir.display()),
            }
            let _ = response.send((recorded, dir));
        });
    }

    /// The notice of the screenshot that finished saving, if any finished. If several did, the
    /// last one.
    fn screenshot_notice(&self) -> Option<ScreenshotNotice> {
        let (recorded, dir) = self.save_results.1.try_iter().last()?;
        let address = screenshot::url_of(&dir);
        Some(match recorded {
            // Only the file name: the folder is the game's, and a click on the notice opens it.
            Ok(path) => ScreenshotNotice {
                text: self.catalog.format(
                    "play.screenshot.saved",
                    &[("file", &path.file_name().unwrap_or_default().to_string_lossy())],
                ),
                failed: false,
                dir: address,
            },
            Err(reason) => ScreenshotNotice {
                text: self.catalog.format("play.screenshot.failed", &[("reason", &reason)]),
                failed: true,
                dir: address,
            },
        })
    }

    /// One turn: input, emulation, and what the game asked for afterwards — launch another, go
    /// back to the Z-Wheel, close.
    ///
    /// `area` is the size of the game screen in the window, for the "window" aspect ratio.
    pub fn step(&mut self, area: [f32; 2]) -> Tick {
        // Before anything else: the notice has to go out also on the turn in which the game closed.
        let screenshot = self.screenshot_notice();
        let Some(running_game) = self.running_game.as_mut() else {
            return Tick {
                screenshot,
                ..Tick::default()
            };
        };
        // The game's own, when it has them. Through the field, not a method: `running_game`
        // above is already a borrow of `self`.
        let effective = self.custom_settings.as_ref().unwrap_or(&self.settings);
        let pads = self.host_input.pads(&effective.controls, &self.settings.shortcuts, &self.key_events);
        let motions = self.host_input.motions(&effective.controls);
        let keyboard = self.key_events.iter().filter_map(|key| input::avk_for(*key));
        // Fast-forward belongs to the session, not to a port: the key or the button of any
        // controller. See `docs/implementation/24-speed.md`.
        // Rewind comes first: while going back, the game does not advance and fast-forward is
        // suspended.
        //
        // **With the game's context current.** Going back one point restores the state, and the
        // state rebuilds the textures on the GPU. Outside `in_context`, they were deleted and
        // created in whatever context was current — Qt Quick's, or none —, and rewind did not work.
        let rewind = self.host_input.shortcut_pressed(&self.settings.shortcuts.rewind, &self.key_events);
        in_context(self.gl.is_some(), || running_game.read_rewind(rewind, &effective.speed.rewind));
        let pressed = self.host_input.shortcut_pressed(&self.settings.shortcuts.fast_forward, &self.key_events);
        let fast_forwarding = running_game.read_fast_forward(pressed, effective.speed.fast_forward.mode);
        let pace = effective.speed.pace_with(fast_forwarding);
        let turbo = TickTurbo {
            pressed: self.host_input.turbos(&effective.controls, &self.key_events),
            controls: &effective.controls,
            taps_per_second: effective.speed.turbo_per_second,
        };
        // The "window" aspect ratio follows the window's size; the target is only rebuilt when the
        // extra columns change. It is GL, so it goes with the context current.
        let from_window = (effective.graphics.ratio == AspectRatio::Window)
            .then(|| area[0] / area[1].max(1.0));
        let context = self.gl.clone();
        in_context(context.is_some(), || {
            if from_window.is_some() {
                running_game.session_mut().set_aspect_ratio(from_window);
            }
            running_game.advance(&pads, motions, keyboard, &pace, &turbo);
            // **What this context drew is only guaranteed for Qt Quick's after a `glFinish`.**
            // The frame's texture is written here and read there, and the OpenGL specification
            // only promises the change is visible to another context once the first has finished
            // the work. The game's `eglSwapBuffers` already reads the frame back to the CPU, which
            // waits for the GPU just the same; the cost left over for this one was not measured.
            if let Some(context) = &context {
                use glow::HasContext;
                unsafe { context.finish() };
            }
        });

        // The launch request comes before the exit: see [`RunningGame::launch_request`].
        let launched = running_game.launch_request().and_then(|class_id| {
            self.games
                .iter()
                .find(|game| game.clsid == Some(class_id))
                .map(|game| game.path.clone())
        });
        let (next, via_z_wheel) = match (launched, running_game.output()) {
            (Some(path), _) => (Some(path), true),
            (None, Output::Continue) => (None, false),
            (None, Output::ReopenZWheel) if self.z_wheel.is_some() => (self.z_wheel.clone(), false),
            (None, Output::ReopenZWheel | Output::Close) => {
                self.close();
                return Tick {
                    closed: true,
                    screenshot,
                    ..Tick::default()
                };
            }
        };
        if let Some(running_game) = &self.running_game {
            self.report.save(running_game);
        }
        let mut tick = Tick::default();
        if let Some(running_game) = self.running_game.as_mut() {
            let paused = running_game.paused();
            let (notices, changed) =
                self.achievements.after_step(running_game.session_mut(), paused, &mut self.settings, &self.catalog);
            tick.achievements = notices;
            if changed {
                self.save_settings();
            }
        }
        if let Some(path) = next {
            tick.state = Some(match self.open(&path) {
                Ok(()) => self.state(),
                Err(error) => {
                    self.catalog.format("play.open_failed", &[("reason", &error.to_string())])
                }
            });
            if let Some(running_game) = self.running_game.as_mut() {
                running_game.opened_by_z_wheel |= via_z_wheel;
                running_game.reset_clock();
            }
        }
        if let Some(reason) = self.running_game.as_ref().and_then(|p| p.session().stopped_reason()) {
            tick.stopped = self.catalog.format("play.failed", &[("reason", &reason)]);
        }
        tick.warning = self.calibration_notice();
        tick.screenshot = screenshot;
        tick.indicators = self.indicators();
        tick.pace = self.running_game.as_ref().map(|p| {
            let sample = p.session().sample();
            (sample.speed, sample.fps)
        });
        tick.metrics = self.metrics();
        tick
    }

    /// The metrics box in the corner, when on: the frames per second, the speed and where the game
    /// draws — this last one left the bottom of the window along with the status bar, which
    /// disappears while the game runs in the main window, and with the separate window strip.
    fn metrics(&self) -> Vec<String> {
        let Some(running_game) = self.running_game.as_ref().filter(|_| self.settings.debug.metrics) else {
            return Vec::new();
        };
        let sample = running_game.session().sample();
        let graphics = &self.effective().graphics;
        let location = match (&self.gpu, graphics.gpu_rasterizer) {
            (Some(gpu), true) => self.catalog.format(
                "debug.metricas.gpu",
                &[("scale", &graphics.internal_resolution.to_string()), ("name", gpu)],
            ),
            _ => self.catalog.get("debug.metricas.software").to_string(),
        };
        vec![
            self.catalog.format("debug.metricas.fps", &[("fps", &sample.fps.to_string())]),
            self.catalog.format("debug.metricas.speed", &[("percent", &sample.speed.to_string())]),
            location,
        ]
    }

    /// What goes over the game: the "▶▶ 3x" while it fast-forwards and the "Turbo" of a toggle
    /// turbo that is on, or empty.
    fn indicators(&self) -> String {
        let Some(running_game) = self.running_game.as_ref() else {
            return String::new();
        };
        let decimal_comma = !self.catalog.current().starts_with("en");
        let speed = &self.effective().speed;
        let fast_forward = running_game.fast_forward_indicator(&speed.fast_forward, decimal_comma);
        let rewind = running_game.rewind_indicator().map(|no_points| match no_points {
            true => format!("◀◀ {}", self.catalog.get("rewind.empty")),
            false => "◀◀".to_string(),
        });
        let turbo = running_game.turbo_on().then(|| self.catalog.get("turbo.on_screen").to_string());
        rewind.into_iter().chain(fast_forward).chain(turbo).collect::<Vec<_>>().join("  ·  ")
    }

    /// The calibration notice in this frame, with the same texts as egui.
    fn calibration_notice(&mut self) -> Option<OnScreenNotice> {
        let calibration = self.running_game.as_ref()?.session().calibration();
        let controls = &self.custom_settings.as_ref().unwrap_or(&self.settings).controls;
        let port = controls
            .enabled_players()
            .find(|(_, player)| player.device_kind == Device::Boomerang)
            .map(|(index, _)| index);
        let reading = port.map(|port| {
            let motion = self.host_input.port_motion(controls, port);
            let has_sensor = self.host_input.raw_port_motion(controls, port).is_some();
            (motion, has_sensor)
        });
        let enabled = self.settings.motion.calibration_notice;
        let view = self.calibration.frame(calibration, reading, enabled, Instant::now())?;
        let port = port?;
        let key_id = match view.done {
            true => "calibration.toast.done",
            false => "calibration.toast.title",
        };
        let state = match view.status {
            None => String::new(),
            Some(Status::NoSensor) => self
                .host_input
                .port_sensor(controls, port)
                .describe(&self.catalog),
            Some(Status::Still) => self.catalog.get("calibration.toast.still").to_string(),
            Some(Status::Moving) => self.catalog.get("calibration.toast.moving").to_string(),
        };
        Some(OnScreenNotice {
            title: self.catalog.get(key_id).to_string(),
            state,
            stopped: view.status == Some(Status::Still),
            rotation: view.rotation,
        })
    }
}

/// Runs `f` with the rasterizer's context current, when there is one.
fn in_context<T>(with_gpu: bool, f: impl FnOnce() -> T) -> T {
    let current = with_gpu && gl::gl_make_current();
    let result = f();
    if current {
        gl::gl_done_current();
    }
    result
}

/// The offscreen context, ready for `glow`, and the GPU's name. `Err` says why it failed.
fn qt_context() -> Result<(Arc<glow::Context>, String), String> {
    if !gl::gl_create() {
        return Err("Qt did not create the offscreen context".into());
    }
    if !gl::gl_make_current() {
        return Err("the offscreen context did not become current".into());
    }
    let context = unsafe {
        glow::Context::from_loader_function_cstr(|name| {
            name.to_str().map_or(0, gl::gl_function) as *const std::ffi::c_void
        })
    };
    let gpu = {
        use glow::HasContext;
        unsafe { context.get_parameter_string(glow::RENDERER) }
    };
    gl::gl_done_current();
    Ok((Arc::new(context), gpu))
}
