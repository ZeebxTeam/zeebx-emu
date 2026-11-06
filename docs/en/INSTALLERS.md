# Installers and releases

Installers are built with [cargo-packager](https://github.com/crabnebula-dev/cargo-packager), configured under `[package.metadata.packager]` in `frontends/qt-standalone/Cargo.toml`. Run it from that directory. **It does not deploy Qt**: each format has its own packaging step, and [`release.yml`](../../.github/workflows/release.yml) contains the full recipe.

- **`.deb`**: uses system Qt and must be built against it. On Ubuntu 24.04, after installing the packages in [BUILDING.md](BUILDING.md), run `cargo packager --release --formats deb`.
- **AppImage**: bundles Qt using `linuxdeploy-plugin-qt`, with `QMAKE` pointing to Qt, `QML_SOURCES_PATHS` set to `frontends/qt-standalone/qml`, `EXTRA_PLATFORM_PLUGINS=libqwayland.so`, and `EXTRA_QT_MODULES=waylandcompositor`. Build on Ubuntu 22.04 with Qt from `aqtinstall`, as CI does.
- **Windows**: run `windeployqt --release --no-translations --qmldir frontends/qt-standalone/qml --dir target/qt-implantado target/release/zeebx-qt.exe`, then package with `--formats nsis`.
- **macOS**: use `--formats app`, run `macdeployqt` on the `.app`, recreate the ad-hoc signature with `codesign --force --deep --sign -`, then create the `.dmg` with `hdiutil`.

Files are placed in `target/pacotes/`.

## Releases

Pushing a version tag (`v0.1.0` or `0.1.0`) to GitHub triggers [`release.yml`](../../.github/workflows/release.yml), which creates a draft release named after the tag. The emulator checks these releases for updates at startup.

See [INSTALLATION.md](INSTALLATION.md#which-file-to-download) for which release file to download.
