//! QML bridge objects specific to `zeebx-htpc`:
//!
//! - [`qobject::HtpcController`] (singleton `HtpcController`): exposes the active HTPC theme,
//!   per-theme resolved color tokens, controller button prompts, gamepad navigation polling,
//!   Quick Menu actions and HTPC settings.
//! - [`qobject::FolderBrowser`]: 100% gamepad-navigable filesystem directory picker so the HTPC
//!   interface never needs an OS mouse/keyboard file dialog (`rfd`).

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
        include!("cxx-qt-lib/qlist.h");
        type QList_i32 = cxx_qt_lib::QList<i32>;
    }

    extern "RustQt" {
        /// Singleton controller exposing HTPC themes, per-theme colors, button prompts, gamepad
        /// polling and Quick Menu helpers to both the host shell and community QML themes.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, theme_id, cxx_name = "themeId")]
        #[qproperty(QString, theme_name, cxx_name = "themeName")]
        #[qproperty(QString, theme_url, cxx_name = "themeUrl")]
        #[qproperty(i32, theme_version, cxx_name = "themeVersion")]
        #[qproperty(bool, dark_theme, cxx_name = "darkTheme")]
        #[qproperty(QString, bg_color, cxx_name = "bgColor")]
        #[qproperty(QString, bg_alt_color, cxx_name = "bgAltColor")]
        #[qproperty(QString, surface_color, cxx_name = "surfaceColor")]
        #[qproperty(QString, surface_solid_color, cxx_name = "surfaceSolidColor")]
        #[qproperty(QString, border_color, cxx_name = "borderColor")]
        #[qproperty(QString, text_color, cxx_name = "textColor")]
        #[qproperty(QString, muted_color, cxx_name = "mutedColor")]
        #[qproperty(QString, accent_color, cxx_name = "accentColor")]
        #[qproperty(QString, accent_text_color, cxx_name = "accentTextColor")]
        #[qproperty(QString, accent_soft_color, cxx_name = "accentSoftColor")]
        #[qproperty(QString, accent_glow_color, cxx_name = "accentGlowColor")]
        #[qproperty(QString, reel_bg_color, cxx_name = "reelBgColor")]
        #[qproperty(QString, warning_color, cxx_name = "warningColor")]
        #[qproperty(QString, error_color, cxx_name = "errorColor")]
        #[qproperty(QString, prompt_ok, cxx_name = "promptOk")]
        #[qproperty(QString, prompt_back, cxx_name = "promptBack")]
        #[qproperty(QString, prompt_x, cxx_name = "promptX")]
        #[qproperty(QString, prompt_y, cxx_name = "promptY")]
        #[qproperty(QString, prompt_l1, cxx_name = "promptL1")]
        #[qproperty(QString, prompt_r1, cxx_name = "promptR1")]
        #[qproperty(QString, prompt_start, cxx_name = "promptStart")]
        #[qproperty(QString, prompt_select, cxx_name = "promptSelect")]
        #[qproperty(QString, prompt_quick, cxx_name = "promptQuick")]
        #[qproperty(i32, overscan_percent, cxx_name = "overscanPercent")]
        #[qproperty(QString, controller_status, cxx_name = "controllerStatus")]
        #[qproperty(QString, active_game_title, cxx_name = "activeGameTitle")]
        #[qproperty(QString, active_game_cover, cxx_name = "activeGameCover")]
        #[qproperty(i32, slot_serial, cxx_name = "slotSerial")]
        #[qproperty(QString, fallback_notice, cxx_name = "fallbackNotice")]
        type HtpcController = super::HtpcControllerRust;

        /// Polls all connected gamepads and returns HTPC command codes (see `htpc_input.rs`).
        #[qinvokable]
        #[cxx_name = "pollCommands"]
        fn poll_commands(
            self: Pin<&mut HtpcController>,
            in_game: bool,
            quick_menu_open: bool,
        ) -> QList_i32;

        /// Mutes held buttons across screen transitions so a press never leaks to the next screen.
        #[qinvokable]
        #[cxx_name = "muteNavigation"]
        fn mute_navigation(self: &HtpcController);

        /// Theme selection and community theme discovery (`htpc-themes/`).
        #[qinvokable]
        #[cxx_name = "themeNames"]
        fn theme_names(self: &HtpcController) -> QStringList;

        #[qinvokable]
        #[cxx_name = "currentThemeIndex"]
        fn current_theme_index(self: &HtpcController) -> i32;

        #[qinvokable]
        #[cxx_name = "selectTheme"]
        fn select_theme(self: Pin<&mut HtpcController>, index: i32);

        #[qinvokable]
        #[cxx_name = "reloadThemes"]
        fn reload_themes(self: Pin<&mut HtpcController>);

        #[qinvokable]
        #[cxx_name = "themesDir"]
        fn themes_dir(self: &HtpcController) -> QString;

        /// Per-theme palette and accent color selection.
        #[qinvokable]
        #[cxx_name = "paletteLabels"]
        fn palette_labels(self: &HtpcController) -> QStringList;

        #[qinvokable]
        #[cxx_name = "currentPaletteIndex"]
        fn current_palette_index(self: &HtpcController) -> i32;

        #[qinvokable]
        #[cxx_name = "selectPalette"]
        fn select_palette(self: Pin<&mut HtpcController>, index: i32);

        #[qinvokable]
        #[cxx_name = "accentLabels"]
        fn accent_labels(self: &HtpcController) -> QStringList;

        #[qinvokable]
        #[cxx_name = "currentAccentIndex"]
        fn current_accent_index(self: &HtpcController) -> i32;

        #[qinvokable]
        #[cxx_name = "selectAccent"]
        fn select_accent(self: Pin<&mut HtpcController>, index: i32);

        /// Called by the host shell if an external theme QML fails to load in `Loader`.
        #[qinvokable]
        #[cxx_name = "fallbackToDefault"]
        fn fallback_to_default(self: Pin<&mut HtpcController>, failed_name: &QString);

        /// Button prompt style, Quick Menu combo and TV overscan.
        #[qinvokable]
        #[cxx_name = "promptLabels"]
        fn prompt_labels(self: &HtpcController) -> QStringList;

        #[qinvokable]
        #[cxx_name = "currentPromptIndex"]
        fn current_prompt_index(self: &HtpcController) -> i32;

        #[qinvokable]
        #[cxx_name = "selectPrompt"]
        fn select_prompt(self: Pin<&mut HtpcController>, index: i32);

        #[qinvokable]
        #[cxx_name = "quickComboLabels"]
        fn quick_combo_labels(self: &HtpcController) -> QStringList;

        #[qinvokable]
        #[cxx_name = "currentQuickComboIndex"]
        fn current_quick_combo_index(self: &HtpcController) -> i32;

        #[qinvokable]
        #[cxx_name = "selectQuickCombo"]
        fn select_quick_combo(self: Pin<&mut HtpcController>, index: i32);

        #[qinvokable]
        #[cxx_name = "setOverscan"]
        fn set_overscan(self: Pin<&mut HtpcController>, percent: i32);

        /// ROMs directory helpers (used by Onboarding and in-app FolderPicker).
        #[qinvokable]
        #[cxx_name = "hasRomsDir"]
        fn has_roms_dir(self: &HtpcController) -> bool;

        #[qinvokable]
        #[cxx_name = "romsDir"]
        fn roms_dir(self: &HtpcController) -> QString;

        #[qinvokable]
        #[cxx_name = "setRomsDir"]
        fn set_roms_dir(self: &HtpcController, path: &QString);

        /// In-game Quick Menu helpers.
        #[qinvokable]
        #[cxx_name = "refreshActiveGame"]
        fn refresh_active_game(self: Pin<&mut HtpcController>);

        #[qinvokable]
        #[cxx_name = "setPaused"]
        fn set_paused(self: &HtpcController, paused: bool);

        #[qinvokable]
        #[cxx_name = "port0DeviceLabel"]
        fn port0_device_label(self: &HtpcController) -> QString;

        #[qinvokable]
        #[cxx_name = "togglePort0Device"]
        fn toggle_port0_device(self: Pin<&mut HtpcController>) -> QString;

        #[qinvokable]
        #[cxx_name = "port0DpadToAxes"]
        fn port0_dpad_to_axes(self: &HtpcController) -> bool;

        #[qinvokable]
        #[cxx_name = "togglePort0DpadToAxes"]
        fn toggle_port0_dpad_to_axes(self: &HtpcController) -> bool;

        #[qinvokable]
        #[cxx_name = "scalingLabel"]
        fn scaling_label(self: &HtpcController) -> QString;

        #[qinvokable]
        #[cxx_name = "cycleScaling"]
        fn cycle_scaling(self: &HtpcController) -> QString;

        #[qinvokable]
        #[cxx_name = "bumpSlotSerial"]
        fn bump_slot_serial(self: Pin<&mut HtpcController>);

        #[qinvokable]
        #[cxx_name = "slotHasThumbnail"]
        fn slot_has_thumbnail(self: &HtpcController, slot: i32) -> bool;

        #[qinvokable]
        #[cxx_name = "syncAfterGlobalChange"]
        fn sync_after_global_change(self: Pin<&mut HtpcController>);
    }

    impl cxx_qt::Initialize for HtpcController {}

    extern "RustQt" {
        /// Gamepad-driven directory browser so the HTPC UI never needs an OS mouse dialog.
        #[qobject]
        #[qml_element]
        #[qproperty(QString, current_path, cxx_name = "currentPath")]
        #[qproperty(QStringList, entries)]
        #[qproperty(i32, rom_count, cxx_name = "romCount")]
        #[qproperty(bool, has_parent, cxx_name = "hasParent")]
        type FolderBrowser = super::FolderBrowserRust;

        /// Opens the browser at `initial` (falls back to ROMs dir, home dir, or root).
        #[qinvokable]
        fn open(self: Pin<&mut FolderBrowser>, initial: &QString);

        /// Enters the directory at `index` (index 0 is parent when `hasParent` is true).
        #[qinvokable]
        fn enter(self: Pin<&mut FolderBrowser>, index: i32);

        /// Navigates up to the parent directory.
        #[qinvokable]
        #[cxx_name = "goUp"]
        fn go_up(self: Pin<&mut FolderBrowser>);
    }
}

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Instant;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QList, QString, QStringList};

use zeebx::input::bindings::Device;
use zeebx::ui::htpc_themes::{
    self, ACCENT_PRESETS, DEFAULT_THEME_ID, HtpcTheme, PaletteOption, ResolvedColors,
};
use zeebx::ui::settings::{PromptStyle, QuickMenuCombo, Scaling};
use zeebx_qt_core::emu_core;

use crate::htpc_input::{self, HtpcNavigation};

struct HtpcRuntime {
    themes: Vec<HtpcTheme>,
    navigation: HtpcNavigation,
    last_pad_check: Option<Instant>,
}

impl HtpcRuntime {
    fn new() -> Self {
        Self {
            themes: htpc_themes::load(&htpc_themes::dirs()),
            navigation: HtpcNavigation::default(),
            last_pad_check: None,
        }
    }
}

thread_local! {
    static HTPC: RefCell<Option<HtpcRuntime>> = const { RefCell::new(None) };
}

fn with_htpc<T>(f: impl FnOnce(&mut HtpcRuntime) -> T) -> T {
    HTPC.with(|cell| {
        let mut guard = cell.borrow_mut();
        f(guard.get_or_insert_with(HtpcRuntime::new))
    })
}

fn active_theme_from<'a>(themes: &'a [HtpcTheme], id: &str) -> &'a HtpcTheme {
    themes
        .iter()
        .find(|t| t.id == id)
        .or_else(|| themes.iter().find(|t| t.id == DEFAULT_THEME_ID))
        .unwrap_or(&themes[0])
}

struct Snapshot {
    theme_id: String,
    theme_name: String,
    theme_url: String,
    colors: ResolvedColors,
    prompts: PromptStyle,
    quick_combo: QuickMenuCombo,
    overscan: i32,
    controller_status: String,
}

fn build_snapshot() -> Snapshot {
    let (
        wanted_id,
        per_theme,
        global_theme,
        zeebx_themes,
        lang,
        prompts,
        quick_combo,
        overscan,
        controller_status,
    ) = emu_core::with(|core| {
        let id = core.settings.htpc.theme.clone();
        let per = core.settings.htpc.theme_colors.get(&id).cloned();
        let pad_label = core.connected_gamepad_label();
        let status = if pad_label.is_empty() {
            core.catalog.get("htpc.status.no_pad").to_string()
        } else {
            let dev = match core.port0_device() {
                Device::Boomerang => "Boomerang",
                Device::Controller => "Dragon",
                Device::Keyboard => "Keyboard",
                Device::ZPad => "Z-Pad",
            };
            format!("P1 · {dev}")
        };
        (
            id,
            per,
            core.settings.theme.clone(),
            core.themes.clone(),
            core.catalog.current().to_string(),
            core.settings.htpc.prompts,
            core.settings.htpc.quick_menu,
            core.settings.htpc.overscan_percent.min(8) as i32,
            status,
        )
    });

    with_htpc(|rt| {
        let theme = active_theme_from(&rt.themes, &wanted_id);
        let colors = htpc_themes::resolve_colors(
            theme,
            per_theme.as_ref(),
            global_theme.as_deref(),
            &zeebx_themes,
        );
        Snapshot {
            theme_id: theme.id.clone(),
            theme_name: theme.name(&lang).to_string(),
            theme_url: theme.entry_url.clone(),
            colors,
            prompts,
            quick_combo,
            overscan,
            controller_status,
        }
    })
}

fn prompt_glyphs(style: PromptStyle, combo: QuickMenuCombo) -> [&'static str; 9] {
    let quick = match combo {
        QuickMenuCombo::L3R3 => "L3+R3",
        QuickMenuCombo::StartSelect => "START+SEL",
        QuickMenuCombo::Guide => "GUIDE",
    };
    match style {
        PromptStyle::Xbox => ["A", "B", "X", "Y", "LB", "RB", "MENU", "VIEW", quick],
        PromptStyle::PlayStation => ["✕", "○", "□", "△", "L1", "R1", "OPT", "SHARE", quick],
        PromptStyle::Nintendo => ["B", "A", "Y", "X", "L", "R", "+", "−", quick],
        PromptStyle::ZPad => ["1", "4", "2", "3", "ZL", "ZR", "START", "HOME", quick],
    }
}

#[derive(Default)]
pub struct HtpcControllerRust {
    theme_id: QString,
    theme_name: QString,
    theme_url: QString,
    theme_version: i32,
    dark_theme: bool,
    bg_color: QString,
    bg_alt_color: QString,
    surface_color: QString,
    surface_solid_color: QString,
    border_color: QString,
    text_color: QString,
    muted_color: QString,
    accent_color: QString,
    accent_text_color: QString,
    accent_soft_color: QString,
    accent_glow_color: QString,
    reel_bg_color: QString,
    warning_color: QString,
    error_color: QString,
    prompt_ok: QString,
    prompt_back: QString,
    prompt_x: QString,
    prompt_y: QString,
    prompt_l1: QString,
    prompt_r1: QString,
    prompt_start: QString,
    prompt_select: QString,
    prompt_quick: QString,
    overscan_percent: i32,
    controller_status: QString,
    active_game_title: QString,
    active_game_cover: QString,
    slot_serial: i32,
    fallback_notice: QString,
}

impl cxx_qt::Initialize for qobject::HtpcController {
    fn initialize(self: Pin<&mut Self>) {
        self.apply_snapshot();
    }
}

impl qobject::HtpcController {
    fn apply_snapshot(mut self: Pin<&mut Self>) {
        let snap = build_snapshot();
        let g = prompt_glyphs(snap.prompts, snap.quick_combo);

        self.as_mut().set_theme_id(QString::from(&snap.theme_id));
        self.as_mut().set_theme_name(QString::from(&snap.theme_name));
        self.as_mut().set_theme_url(QString::from(&snap.theme_url));
        self.as_mut().set_dark_theme(snap.colors.dark);
        self.as_mut().set_bg_color(QString::from(&snap.colors.background));
        self.as_mut()
            .set_bg_alt_color(QString::from(&snap.colors.background_alt));
        self.as_mut()
            .set_surface_color(QString::from(&snap.colors.surface));
        self.as_mut()
            .set_surface_solid_color(QString::from(&snap.colors.surface_solid));
        self.as_mut()
            .set_border_color(QString::from(&snap.colors.border));
        self.as_mut().set_text_color(QString::from(&snap.colors.text));
        self.as_mut().set_muted_color(QString::from(&snap.colors.muted));
        self.as_mut()
            .set_accent_color(QString::from(&snap.colors.accent));
        self.as_mut()
            .set_accent_text_color(QString::from(&snap.colors.accent_text));
        self.as_mut()
            .set_accent_soft_color(QString::from(&snap.colors.accent_soft));
        self.as_mut()
            .set_accent_glow_color(QString::from(&snap.colors.accent_glow));
        self.as_mut()
            .set_reel_bg_color(QString::from(&snap.colors.reel_bg));
        self.as_mut()
            .set_warning_color(QString::from(&snap.colors.warning));
        self.as_mut().set_error_color(QString::from(&snap.colors.error));

        self.as_mut().set_prompt_ok(QString::from(g[0]));
        self.as_mut().set_prompt_back(QString::from(g[1]));
        self.as_mut().set_prompt_x(QString::from(g[2]));
        self.as_mut().set_prompt_y(QString::from(g[3]));
        self.as_mut().set_prompt_l1(QString::from(g[4]));
        self.as_mut().set_prompt_r1(QString::from(g[5]));
        self.as_mut().set_prompt_start(QString::from(g[6]));
        self.as_mut().set_prompt_select(QString::from(g[7]));
        self.as_mut().set_prompt_quick(QString::from(g[8]));

        self.as_mut().set_overscan_percent(snap.overscan);
        self.as_mut()
            .set_controller_status(QString::from(&snap.controller_status));
        let next = self.theme_version().wrapping_add(1);
        self.as_mut().set_theme_version(next);
    }

    pub fn poll_commands(
        mut self: Pin<&mut Self>,
        in_game: bool,
        quick_menu_open: bool,
    ) -> QList<i32> {
        let now = Instant::now();
        let (pressed, maybe_status) = emu_core::with(|core| {
            core.each_frame();
            core.host_input.gamepads.poll();
            let combo = core.settings.htpc.quick_menu;
            let pressed = htpc_input::read_gamepads(&core.host_input.gamepads, combo);

            let refresh_status = with_htpc(|rt| {
                let due = rt
                    .last_pad_check
                    .is_none_or(|t| now.saturating_duration_since(t).as_millis() >= 900);
                if due {
                    rt.last_pad_check = Some(now);
                }
                due
            });

            let status = if refresh_status {
                let pad_label = core.connected_gamepad_label();
                Some(if pad_label.is_empty() {
                    core.catalog.get("htpc.status.no_pad").to_string()
                } else {
                    let dev = match core.port0_device() {
                        Device::Boomerang => "Boomerang",
                        Device::Controller => "Dragon",
                        Device::Keyboard => "Keyboard",
                        Device::ZPad => "Z-Pad",
                    };
                    format!("P1 · {dev}")
                })
            } else {
                None
            };
            (pressed, status)
        });

        if let Some(status) = maybe_status {
            let next = QString::from(&status);
            if *self.controller_status() != next {
                self.as_mut().set_controller_status(next);
            }
        }

        let quick_menu_only = in_game && !quick_menu_open;
        let cmds = with_htpc(|rt| rt.navigation.poll(pressed, quick_menu_only, now));
        let codes: Vec<i32> = cmds.into_iter().map(|c| c as i32).collect();
        QList::from(codes)
    }

    pub fn mute_navigation(&self) {
        with_htpc(|rt| rt.navigation.mute());
    }

    pub fn theme_names(&self) -> QStringList {
        let lang = emu_core::with(|core| core.catalog.current().to_string());
        with_htpc(|rt| {
            let mut list = QStringList::default();
            for theme in &rt.themes {
                list.append(QString::from(theme.name(&lang)));
            }
            list
        })
    }

    pub fn current_theme_index(&self) -> i32 {
        let id = emu_core::with(|core| core.settings.htpc.theme.clone());
        with_htpc(|rt| {
            rt.themes
                .iter()
                .position(|t| t.id == id)
                .unwrap_or(0) as i32
        })
    }

    pub fn select_theme(mut self: Pin<&mut Self>, index: i32) {
        let chosen = with_htpc(|rt| {
            usize::try_from(index)
                .ok()
                .and_then(|i| rt.themes.get(i))
                .map(|t| t.id.clone())
        });
        let Some(id) = chosen else {
            return;
        };
        emu_core::with(|core| {
            core.settings.htpc.theme = id;
            core.save_settings_file();
        });
        self.as_mut().set_fallback_notice(QString::default());
        self.apply_snapshot();
    }

    pub fn reload_themes(mut self: Pin<&mut Self>) {
        with_htpc(|rt| {
            rt.themes = htpc_themes::load(&htpc_themes::dirs());
        });
        self.as_mut().set_fallback_notice(QString::default());
        self.apply_snapshot();
    }

    pub fn themes_dir(&self) -> QString {
        let dir = zeebx::config::config_dir().join("htpc-themes");
        let _ = std::fs::create_dir_all(&dir);
        QString::from(&dir.display().to_string())
    }

    fn current_palette_options(&self) -> (String, Vec<PaletteOption>) {
        let (theme_id, zeebx_themes, lang) = emu_core::with(|core| {
            (
                core.settings.htpc.theme.clone(),
                core.themes.clone(),
                core.catalog.current().to_string(),
            )
        });
        with_htpc(|rt| {
            let theme = active_theme_from(&rt.themes, &theme_id);
            (
                theme.id.clone(),
                htpc_themes::palette_options(theme, &zeebx_themes, &lang),
            )
        })
    }

    pub fn palette_labels(&self) -> QStringList {
        let (_, opts) = self.current_palette_options();
        emu_core::with(|core| {
            let mut list = QStringList::default();
            for opt in opts {
                let text = if opt.is_catalog_key {
                    core.catalog.get(&opt.label)
                } else {
                    opt.label.as_str()
                };
                list.append(QString::from(text));
            }
            list
        })
    }

    pub fn current_palette_index(&self) -> i32 {
        let (theme_id, opts) = self.current_palette_options();
        let current = emu_core::with(|core| {
            core.settings
                .htpc
                .theme_colors
                .get(&theme_id)
                .and_then(|c| c.palette.clone())
                .unwrap_or_default()
        });
        opts.iter()
            .position(|o| o.key == current)
            .unwrap_or(0) as i32
    }

    pub fn select_palette(self: Pin<&mut Self>, index: i32) {
        let (theme_id, opts) = self.current_palette_options();
        let Some(opt) = usize::try_from(index).ok().and_then(|i| opts.get(i)) else {
            return;
        };
        emu_core::with(|core| {
            let entry = core.settings.htpc.theme_colors.entry(theme_id).or_default();
            entry.palette = (!opt.key.is_empty()).then(|| opt.key.clone());
            core.save_settings_file();
        });
        self.apply_snapshot();
    }

    pub fn accent_labels(&self) -> QStringList {
        emu_core::with(|core| {
            let mut list = QStringList::default();
            for (_, key) in ACCENT_PRESETS {
                list.append(QString::from(core.catalog.get(key)));
            }
            list
        })
    }

    pub fn current_accent_index(&self) -> i32 {
        let theme_id = emu_core::with(|core| core.settings.htpc.theme.clone());
        let resolved_id = with_htpc(|rt| active_theme_from(&rt.themes, &theme_id).id.clone());
        let current = emu_core::with(|core| {
            core.settings
                .htpc
                .theme_colors
                .get(&resolved_id)
                .and_then(|c| c.accent.clone())
                .unwrap_or_default()
        });
        ACCENT_PRESETS
            .iter()
            .position(|(hex, _)| hex.eq_ignore_ascii_case(&current))
            .unwrap_or(0) as i32
    }

    pub fn select_accent(self: Pin<&mut Self>, index: i32) {
        let Some(&(hex, _)) = usize::try_from(index).ok().and_then(|i| ACCENT_PRESETS.get(i))
        else {
            return;
        };
        let theme_id = emu_core::with(|core| core.settings.htpc.theme.clone());
        let resolved_id = with_htpc(|rt| active_theme_from(&rt.themes, &theme_id).id.clone());
        emu_core::with(|core| {
            let entry = core.settings.htpc.theme_colors.entry(resolved_id).or_default();
            entry.accent = (!hex.is_empty()).then(|| hex.to_string());
            core.save_settings_file();
        });
        self.apply_snapshot();
    }

    pub fn fallback_to_default(mut self: Pin<&mut Self>, failed_name: &QString) {
        let failed = String::from(failed_name);
        let notice = emu_core::with(|core| {
            core.settings.htpc.theme = DEFAULT_THEME_ID.to_string();
            core.save_settings_file();
            core.catalog
                .format("htpc.theme.fallback", &[("theme", &failed)])
        });
        self.as_mut().set_fallback_notice(QString::from(&notice));
        self.apply_snapshot();
    }

    pub fn prompt_labels(&self) -> QStringList {
        emu_core::with(|core| {
            let mut list = QStringList::default();
            for style in PromptStyle::ALL {
                list.append(QString::from(core.catalog.get(style.key_id())));
            }
            list
        })
    }

    pub fn current_prompt_index(&self) -> i32 {
        let current = emu_core::with(|core| core.settings.htpc.prompts);
        PromptStyle::ALL
            .iter()
            .position(|&s| s == current)
            .unwrap_or(0) as i32
    }

    pub fn select_prompt(self: Pin<&mut Self>, index: i32) {
        let Some(&style) = usize::try_from(index).ok().and_then(|i| PromptStyle::ALL.get(i))
        else {
            return;
        };
        emu_core::with(|core| {
            core.settings.htpc.prompts = style;
            core.save_settings_file();
        });
        self.apply_snapshot();
    }

    pub fn quick_combo_labels(&self) -> QStringList {
        emu_core::with(|core| {
            let mut list = QStringList::default();
            for combo in QuickMenuCombo::ALL {
                list.append(QString::from(core.catalog.get(combo.key_id())));
            }
            list
        })
    }

    pub fn current_quick_combo_index(&self) -> i32 {
        let current = emu_core::with(|core| core.settings.htpc.quick_menu);
        QuickMenuCombo::ALL
            .iter()
            .position(|&c| c == current)
            .unwrap_or(0) as i32
    }

    pub fn select_quick_combo(self: Pin<&mut Self>, index: i32) {
        let Some(&combo) = usize::try_from(index)
            .ok()
            .and_then(|i| QuickMenuCombo::ALL.get(i))
        else {
            return;
        };
        emu_core::with(|core| {
            core.settings.htpc.quick_menu = combo;
            core.save_settings_file();
        });
        self.apply_snapshot();
    }

    pub fn set_overscan(self: Pin<&mut Self>, percent: i32) {
        let clamped = percent.clamp(0, 8) as u8;
        emu_core::with(|core| {
            core.settings.htpc.overscan_percent = clamped;
            core.save_settings_file();
        });
        self.apply_snapshot();
    }

    pub fn has_roms_dir(&self) -> bool {
        emu_core::with(|core| core.settings.roms_dir.is_some())
    }

    pub fn roms_dir(&self) -> QString {
        emu_core::with(|core| {
            core.settings
                .roms_dir
                .as_ref()
                .map(|p| QString::from(&p.display().to_string()))
                .unwrap_or_default()
        })
    }

    pub fn set_roms_dir(&self, path: &QString) {
        let dir = PathBuf::from(String::from(path));
        if dir.is_dir() {
            emu_core::with(|core| core.set_roms_dir(dir));
        }
    }

    pub fn refresh_active_game(mut self: Pin<&mut Self>) {
        let (title, cover) = emu_core::with(|core| {
            (
                core.title().unwrap_or_default(),
                core.running_game_cover(),
            )
        });
        self.as_mut().set_active_game_title(QString::from(&title));
        self.as_mut().set_active_game_cover(QString::from(&cover));
    }

    pub fn set_paused(&self, paused: bool) {
        emu_core::with(|core| core.set_paused(paused));
    }

    pub fn port0_device_label(&self) -> QString {
        let label = emu_core::with(|core| match core.port0_device() {
            Device::Boomerang => core.catalog.get("controls.kind.boomerang").to_string(),
            Device::Controller => core.catalog.get("controls.kind.dragon").to_string(),
            Device::Keyboard => core.catalog.get("controls.kind.keyboard").to_string(),
            Device::ZPad => core.catalog.get("controls.kind.zpad").to_string(),
        });
        QString::from(&label)
    }

    pub fn toggle_port0_device(mut self: Pin<&mut Self>) -> QString {
        let label = emu_core::with(|core| {
            let next = match core.port0_device() {
                Device::Boomerang => Device::ZPad,
                _ => Device::Boomerang,
            };
            core.set_port0_device(next);
            match next {
                Device::Boomerang => core.catalog.get("controls.kind.boomerang").to_string(),
                _ => core.catalog.get("controls.kind.zpad").to_string(),
            }
        });
        self.as_mut().apply_snapshot();
        QString::from(&label)
    }

    pub fn port0_dpad_to_axes(&self) -> bool {
        emu_core::with(|core| {
            core.settings
                .controls
                .player(0)
                .is_some_and(|p| p.dpad_to_axes)
        })
    }

    pub fn toggle_port0_dpad_to_axes(&self) -> bool {
        emu_core::with(|core| {
            let player = core.settings.controls.player_mut(0);
            player.dpad_to_axes = !player.dpad_to_axes;
            let next = player.dpad_to_axes;
            core.save_settings_file();
            next
        })
    }

    pub fn scaling_label(&self) -> QString {
        let label = emu_core::with(|core| {
            core.catalog
                .get(core.settings.graphics.scaling.key())
                .to_string()
        });
        QString::from(&label)
    }

    pub fn cycle_scaling(&self) -> QString {
        let label = emu_core::with(|core| {
            let cur = core.settings.graphics.scaling;
            let idx = Scaling::ALL.iter().position(|&s| s == cur).unwrap_or(0);
            let next = Scaling::ALL[(idx + 1) % Scaling::ALL.len()];
            core.settings.graphics.scaling = next;
            core.save_settings_file();
            core.catalog.get(next.key()).to_string()
        });
        QString::from(&label)
    }

    pub fn bump_slot_serial(mut self: Pin<&mut Self>) {
        let next = self.slot_serial().wrapping_add(1);
        self.as_mut().set_slot_serial(next);
    }

    pub fn slot_has_thumbnail(&self, slot: i32) -> bool {
        let Ok(index) = usize::try_from(slot) else {
            return false;
        };
        emu_core::with(|core| core.slot_image(&format!("slot/0/{index}")).is_some())
    }

    pub fn sync_after_global_change(self: Pin<&mut Self>) {
        self.apply_snapshot();
    }
}

// -----------------------------------------------------------------------------
// FolderBrowser: 100% gamepad-navigable directory picker (no OS mouse dialogs)
// -----------------------------------------------------------------------------

#[derive(Default)]
pub struct FolderBrowserRust {
    current_path: QString,
    entries: QStringList,
    rom_count: i32,
    has_parent: bool,
    paths: Vec<PathBuf>,
}

fn default_browse_dir() -> PathBuf {
    if let Some(roms) = emu_core::with(|core| core.settings.roms_dir.clone())
        && roms.is_dir()
    {
        return roms;
    }
    for var in ["HOME", "USERPROFILE"] {
        if let Ok(home) = std::env::var(var) {
            let p = PathBuf::from(home);
            if p.is_dir() {
                return p;
            }
        }
    }
    PathBuf::from("/")
}

fn count_rom_candidates(dir: &Path) -> i32 {
    let Ok(read) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut count = 0;
    for entry in read.flatten() {
        let p = entry.path();
        if p.is_file()
            && let Some(ext) = p.extension().and_then(|e| e.to_str())
            && matches!(ext.to_ascii_lowercase().as_str(), "zip" | "mod" | "7z")
        {
            count += 1;
        }
    }
    count
}

impl qobject::FolderBrowser {
    fn load_dir(mut self: Pin<&mut Self>, dir: PathBuf) {
        let canonical = std::fs::canonicalize(&dir).unwrap_or(dir);
        let parent_label = emu_core::with(|core| core.catalog.get("htpc.folder.parent").to_string());

        let mut names = QStringList::default();
        let mut paths = Vec::new();
        let has_parent = canonical.parent().is_some();
        if let Some(parent) = canonical.parent() {
            names.append(QString::from(&parent_label));
            paths.push(parent.to_path_buf());
        }

        let mut children: Vec<(String, PathBuf)> = std::fs::read_dir(&canonical)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if !path.is_dir() {
                    return None;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    return None;
                }
                Some((name, path))
            })
            .collect();
        children.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

        for (name, path) in children {
            names.append(QString::from(&name));
            paths.push(path);
        }

        let rom_count = count_rom_candidates(&canonical);
        self.as_mut().rust_mut().paths = paths;
        self.as_mut()
            .set_current_path(QString::from(&canonical.display().to_string()));
        self.as_mut().set_entries(names);
        self.as_mut().set_rom_count(rom_count);
        self.as_mut().set_has_parent(has_parent);
    }

    pub fn open(self: Pin<&mut Self>, initial: &QString) {
        let text = String::from(initial);
        let dir = if !text.is_empty() && Path::new(&text).is_dir() {
            PathBuf::from(text)
        } else {
            default_browse_dir()
        };
        self.load_dir(dir);
    }

    pub fn enter(self: Pin<&mut Self>, index: i32) {
        let target = usize::try_from(index)
            .ok()
            .and_then(|i| self.rust().paths.get(i))
            .cloned();
        if let Some(path) = target {
            self.load_dir(path);
        }
    }

    pub fn go_up(self: Pin<&mut Self>) {
        if *self.has_parent() {
            self.enter(0);
        }
    }
}
