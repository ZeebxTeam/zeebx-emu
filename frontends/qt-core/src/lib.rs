//! Shared Qt Quick bridge for Zeebx desktop (`zeebx-qt`) and HTPC (`zeebx-htpc`).
//!
//! Exposes the `zeebx` QML module (`GameScreen`, `Library`, `Language`, `SettingsModel`,
//! `Saves`, `Notices`, `Theme`), the shared OpenGL context glue, the `image://zeebx/…` provider
//! and [`launch`] to bootstrap any Qt Quick frontend cleanly with the right destruction order.

pub mod bridge;
pub mod dialogs;
pub mod emu_core;
pub mod library;
pub mod settings;
pub mod theme;

use std::process::ExitCode;

use cxx_qt::casting::Upcast;
use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QString, QUrl, QVariant};

/// Launches a Qt Quick frontend using the shared `zeebx` QML module and OpenGL context.
///
/// `app_id` sets the Wayland desktop file name; `qml_url` is the root `qrc:/...` QML file;
/// `game` is an optional ROM path passed on the command line; `apply_palette` applies the desktop
/// Fusion `QPalette` after loading the QML engine.
pub fn launch(
    app_id: &str,
    qml_url: &str,
    game: Option<&str>,
    apply_palette: bool,
) -> ExitCode {
    // Before the `QGuiApplication`: after it, the graphics API and the format are already chosen.
    bridge::qobject::prepare_gl();
    let mut owned_app = QGuiApplication::new();
    let mut owned_engine = QQmlApplicationEngine::new();
    let (Some(mut app), Some(mut engine)) = (owned_app.as_mut(), owned_engine.as_mut()) else {
        eprintln!("error: Qt did not open");
        return ExitCode::FAILURE;
    };
    // Before the first window. On Wayland there is no icon in pixels: the compositor matches this
    // name with the installed `.desktop` and takes the icon from there, like egui's `app_id`.
    app.as_mut().set_application_name(&QString::from("Zeebx"));
    QGuiApplication::set_desktop_file_name(&QString::from(app_id));
    bridge::qobject::apply_icon();
    let mut properties = cxx_qt_lib::QMap::<cxx_qt_lib::QMapPair_QString_QVariant>::default();
    let game = QString::from(game.unwrap_or_default());
    properties.insert(QString::from("initialGame"), QVariant::from(&game));
    engine.as_mut().set_initial_properties(&properties);
    // The library's covers, logos, ratings and save-state thumbnails: `image://zeebx/…`.
    library::register_images(engine.as_mut().upcast_pin());
    engine.load(&QUrl::from(qml_url));
    if apply_palette {
        // **The theme after `load`, and before the first window appears.** Applied earlier, only
        // the loose items (the grid cards) followed it: the windows and the Fusion controls are
        // born with the style's palette, taken from the system, and only switch to the program's
        // when it changes with them already created. Measured on Qt 6.4 and 6.12.
        theme::apply();
    }
    let output = app.exec();
    // **The order is the fix.** First the engine; then the game, released with the GL context
    // still alive, because its textures live there; then the context; and only then the
    // `QGuiApplication`. Left to the end of the process, the offscreen context died after Qt,
    // and closing the window ended in SIGSEGV inside `~QOffscreenSurface`.
    drop(owned_engine);
    emu_core::shutdown();
    bridge::qobject::gl_destroy();
    drop(owned_app);
    match output {
        0 => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}
