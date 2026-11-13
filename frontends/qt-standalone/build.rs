// The icon and the version go into the Windows `.exe`: that is what Explorer and the taskbar
// show.
//
// **It lives in this package because the resource only reaches the executable if it is compiled
// here.** At the root it got stuck in the library, which is not what the user opens. `assets/`
// stays at the repository root, hence the two levels up.
fn main() {
    // `CARGO_CFG_TARGET_OS` is the target's, not the build machine's: `cfg!` here would answer
    // for the system build.rs runs on.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/icons/zeebx.ico");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=no icon in the .exe: {error}");
        }
    }
    qt_ui();
    println!("cargo:rerun-if-changed=../../assets/icons/zeebx.ico");
    println!("cargo:rerun-if-changed=build.rs");
}

/// The bridge to Qt: the `QObject`s written in Rust, the C++ glue and the QML module. See
/// `docs/implementation/21-qt-migration.md`.
fn qt_ui() {
    use cxx_qt_build::{CxxQtBuilder, QmlModule};

    CxxQtBuilder::new_qml_module(
        QmlModule::new("zeebx")
            .qml_files(["qml/Main.qml", "qml/Game.qml", "qml/LibraryGrid.qml", "qml/LibrarySlider.qml", "qml/SettingsWindow.qml", "qml/SavesWindow.qml", "qml/LogWindow.qml", "qml/Icon.qml", "qml/GameStage.qml", "qml/AchievementToast.qml", "qml/AchievementsPage.qml"])
            .depend("QtQuick"),
    )
    .files(["src/qt/bridge.rs", "src/qt/library.rs", "src/qt/settings.rs", "src/qt/dialogs.rs", "src/qt/theme.rs", "src/qt/achievements.rs"])
    .include_dir("src/qt/cpp")
    .cpp_files([
        "src/qt/cpp/gl_qt.cpp",
        // The header goes in for moc: `FrameItem` has `Q_OBJECT`.
        "src/qt/cpp/frame.h",
        "src/qt/cpp/frame.cpp",
        "src/qt/cpp/images.cpp",
        "src/qt/cpp/palette.cpp",
    ])
    // The calibration notice image, the icon logo and the UI icon font, under `qrc:/zeebx/`.
    // `assets/` belongs to the project, not to this package.
    .qrc_resources(cxx_qt_build::QResources::new().resource(
        cxx_qt_build::QResource::new()
            .prefix("/zeebx")
            .file(qt_build_utils::QResourceFile::new("../../assets/boomerang.png").alias("boomerang.png"))
            .file(qt_build_utils::QResourceFile::new("../../assets/zeebx.png").alias("zeebx.png"))
            .file(qt_build_utils::QResourceFile::new("../../assets/icons/lucide.ttf").alias("lucide.ttf")),
    ))
    .qt_module("Quick")
    // Qt Qml requires Network on macOS.
    .qt_module("Network")
    .build();
    println!("cargo:rerun-if-changed=src/qt/cpp/gl_qt.h");
    println!("cargo:rerun-if-changed=src/qt/cpp/images.h");
    println!("cargo:rerun-if-changed=src/qt/cpp/palette.h");
}
