/// Builds the shared `zeebx` QML module (`GameScreen`, `Library`, `Language`, `SettingsModel`,
/// `Saves`, `Notices`, `Theme`), the OpenGL / frame C++ glue and the shared QRC assets used by
/// both `zeebx-qt` (desktop) and `zeebx-htpc` (10-foot UI).
fn main() {
    use cxx_qt_build::{CxxQtBuilder, QmlModule};

    CxxQtBuilder::new_qml_module(QmlModule::new("zeebx").depend("QtQuick"))
        .files([
            "src/bridge.rs",
            "src/library.rs",
            "src/settings.rs",
            "src/dialogs.rs",
            "src/theme.rs",
        ])
        .include_dir("src/cpp")
        .cpp_files([
            "src/cpp/gl_qt.cpp",
            // The header goes in for moc: `FrameItem` has `Q_OBJECT`.
            "src/cpp/frame.h",
            "src/cpp/frame.cpp",
            "src/cpp/images.cpp",
            "src/cpp/palette.cpp",
        ])
        // Shared visual assets under `qrc:/zeebx/`.
        .qrc_resources(
            cxx_qt_build::QResources::new().resource(
                cxx_qt_build::QResource::new()
                    .prefix("/zeebx")
                    .file(
                        qt_build_utils::QResourceFile::new("../../assets/boomerang.png")
                            .alias("boomerang.png"),
                    )
                    .file(
                        qt_build_utils::QResourceFile::new("../../assets/zeebx.png")
                            .alias("zeebx.png"),
                    )
                    .file(
                        qt_build_utils::QResourceFile::new("../../assets/icons/lucide.ttf")
                            .alias("lucide.ttf"),
                    ),
            ),
        )
        .qt_module("Quick")
        // Qt Qml requires Network on macOS.
        .qt_module("Network")
        .build()
        .reexport_dependency("cxx-qt-lib")
        .export();

    println!("cargo:rerun-if-changed=src/cpp/gl_qt.h");
    println!("cargo:rerun-if-changed=src/cpp/images.h");
    println!("cargo:rerun-if-changed=src/cpp/palette.h");
    println!("cargo:rerun-if-changed=build.rs");
}
