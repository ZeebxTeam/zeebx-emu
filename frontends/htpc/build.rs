fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/icons/zeebx.ico");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=no icon in the .exe: {error}");
        }
    }
    qt_htpc_qml();
    println!("cargo:rerun-if-changed=../../assets/icons/zeebx.ico");
    println!("cargo:rerun-if-changed=build.rs");
}

/// Builds the `zeebx.htpc` QML module: the HTPC bridge objects (`HtpcController`, `FolderBrowser`),
/// the fixed 10-foot host shell (`HtpcMain.qml`, `QuickMenu.qml`, `HtpcSettings.qml`,
/// `FolderPicker.qml`, `GameDetails.qml`) and the built-in `zwheel-hd` default theme.
fn qt_htpc_qml() {
    use cxx_qt_build::{CxxQtBuilder, QmlModule};

    CxxQtBuilder::new_qml_module(
        QmlModule::new("zeebx.htpc")
            .qml_files([
                "qml/HtpcMain.qml",
                "qml/QuickMenu.qml",
                "qml/HtpcSettings.qml",
                "qml/FolderPicker.qml",
                "qml/GameDetails.qml",
                "qml/themes/zwheel-hd/Main.qml",
            ])
            .depend("QtQuick"),
    )
    .files(["src/htpc_bridge.rs"])
    .qt_module("Quick")
    .qt_module("Network")
    .build();
}
