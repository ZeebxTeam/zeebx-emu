# Building

Rust 1.88 or newer.

```bash
cargo build --release
```

## Desktop dependencies

The interface is `zeebx-qt`, built with **Qt 6.4 or newer** in `frontends/qt-standalone`. The command-line tool is `zeebx` in `frontends/cli` and does not need Qt. Debian, Ubuntu, and derivatives:

```bash
sudo apt install build-essential cmake ninja-build pkg-config python3 clang libclang-dev \
    libglib2.0-dev libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev \
    qt6-base-dev qt6-base-dev-tools qt6-declarative-dev qt6-declarative-dev-tools qmake6 \
    qt6-wayland qml6-module-qtquick qml6-module-qtquick-controls qml6-module-qtquick-layouts \
    qml6-module-qtquick-shapes qml6-module-qtquick-templates qml6-module-qtquick-window \
    qml6-module-qtqml-workerscript
```

## Build targets

```bash
cargo build --release --locked -p zeebx-cli
cargo build --release --locked -p zeebx-standalone-qt
cargo build --release --locked -p zeebx-headless
cargo build --release --locked -p zeebx-libretro
```

Build the Libretro core by itself so Cargo cannot unify its features with another package. See [LIBRETRO-CORE.md](LIBRETRO-CORE.md).

## Android

Android requires the SDK, NDK, JDK, and Gradle as described in [`frontends/android/LEIAME.md`](../../frontends/android/LEIAME.md). Then run:

```bash
./frontends/android/compilar.sh --apk
```

## iOS

iOS requires a Mac with Xcode and the `aarch64-apple-ios` and `aarch64-apple-ios-sim` Rust targets:

```bash
./frontends/ios/compilar.sh --app
```

More build and release details are in [INSTALLERS.md](INSTALLERS.md).
