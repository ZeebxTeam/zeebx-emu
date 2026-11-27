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
    qt_desktop_qml();
    println!("cargo:rerun-if-changed=../../assets/icons/zeebx.ico");
    println!("cargo:rerun-if-changed=build.rs");
}

/// Compiles the desktop QML views into the `zeebx.desktop` QML module. The Rust `QObject`s and
/// OpenGL/frame glue live in `zeebx-qt-core` (`import zeebx`).
fn qt_desktop_qml() {
    use cxx_qt_build::{CxxQtBuilder, QmlModule};

    CxxQtBuilder::new_qml_module(
        QmlModule::new("zeebx.desktop")
            .qml_files([
                "qml/Main.qml",
                "qml/Game.qml",
                "qml/LibraryGrid.qml",
                "qml/LibrarySlider.qml",
                "qml/LibraryList.qml",
                "qml/SettingsWindow.qml",
                "qml/SavesWindow.qml",
                "qml/LogWindow.qml",
                "qml/Icon.qml",
                "qml/GameStage.qml",
            ])
            .depend("QtQuick"),
    )
    .qt_module("Quick")
    .qt_module("Network")
    .build();
}
