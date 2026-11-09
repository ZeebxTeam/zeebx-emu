# Zeebx 

🇧🇷 [Leia em português](README.pt-BR.md)

An emulator for the Zeebo, the console TecToy launched in 2009 in Brazil and Mexico.

The Zeebo was digital-only: the games came from TecToy's store, which went offline. About 60
titles are left, and they can only be played on surviving consoles or through console modding.
This project exists to help preserve these gems that were part of our history.

In development. Today, 56 of the 62 titles get past loading and draw.

![Screenshot](docs/screenshot.png)

## Useful links

[Discord server: https://discord.gg/D96HjsKTPa](https://discord.gg/D96HjsKTPa)

[GitHub: https://github.com/ZeebxTeam](https://github.com/ZeebxTeam)

## Installation

The ready-made builds are in the [releases](https://github.com/ZeebxTeam/zeebx-emu/releases): the
Windows installer, the macOS `.dmg`, the Linux `.deb` and AppImage, the Android APK and the
Libretro core. The table of which file is which is in
[Installers and releases](#installers-and-releases).

On **macOS** and **Linux** you can install it through [Homebrew](https://brew.sh), with the
[project's tap](https://github.com/ZeebxTeam/homebrew-tap):

```bash
brew install --cask zeebxteam/tap/zeebx-qt
```

On macOS it goes to Applications (it needs macOS 13 or newer); on Linux, the AppImage goes to
`~/Applications/zeebx-qt.AppImage`. The old egui interface lasted until 0.4.2: the
`zeebxteam/tap/zeebx` cask stays on it.

The emulator updates itself in the AppImage, the Windows installer and the macOS `.app`: when a
new version comes out, it tells you and offers "Update now". In the settings, under Updates, you
can turn off the check on startup and choose whether pre-release versions count.

# Notes for Contributors

Please, when opening a PR, always target the development branch or the branch matching the change being made.
Do not open a PR against the master branch, since that is where we organize and concentrate our release build CI.

For new frontend targets, always follow the little rule of keeping it inside the "frontends" folder, for example:
frontends/android/
frontends/ios/
frontends/headless/
frontends/libretro/
frontends/cli/
frontends/qt-standalone/
frontends/switch/
frontends/vita/
frontends/vita-standalone/

And also adjust [.github/workflows/release.yml](.github/workflows/release.yml) to point to a build target during our CI, so that the target is guaranteed to ship along when the release is created!

The same notes are in [CONTRIBUTING.en.md](CONTRIBUTING.en.md). These are suggested details only to keep our repository organized!

## How it works

Emulating the Zeebo is not emulating a console: it is reimplementing Qualcomm BREW 4.0.2. The game
is an ARM binary that never touches hardware — it calls system interfaces through pointer tables.
So the way to go is to run the ARM code on an emulated core and serve each API call on the host.

The vtables we hand the game point to addresses that **do not exist** in the memory map. When the
game calls a method, the core aborts the instruction fetch and the address tells us which
interface and which method were requested. There is no stub, and no glue code.

The full design is in [ARCHITECTURE.md](ARCHITECTURE.md).

## Compatibility

Most ROMs run without problems; some games may hang before starting up or while running.

Games that use the Boomerang can be played with a controller compatible with motion controls, such as Wii Remote, 
Nintendo Switch Pro Controller, Dualshock 4, Dualsense, etc.

3D games support experimental resolution features, and can reach resolutions of up to 4K in 16:9.

The state of each title, with the addresses of each stop, is in
[COMPATIBILITY.md](COMPATIBILITY.md).

For frontends such as Android and Libretro, this compatibility list may not apply. We ask that you report any problems on those versions too.

## Building

Rust 1.88 or newer.

```bash
cargo build --release
```

### What else needs to be installed

The interface is `zeebx-qt`, in **Qt 6** (`frontends/qt-standalone`, Qt 6.4 or newer). The
command line — `zeebx run`, `zeebx session`, `zeebx bench`, for measuring and debugging — is
`zeebx` (`frontends/cli`), without Qt. Both use native dependencies for `dynarmic`, audio and
controllers; only the interface asks for Qt. The egui interface was removed after 0.4.2. Debian,
Ubuntu and derivatives:

```bash
sudo apt install build-essential cmake ninja-build pkg-config python3 clang libclang-dev \
    libglib2.0-dev libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev \
    qt6-base-dev qt6-base-dev-tools qt6-declarative-dev qt6-declarative-dev-tools qmake6 \
    qt6-wayland qml6-module-qtquick qml6-module-qtquick-controls qml6-module-qtquick-layouts \
    qml6-module-qtquick-shapes \
    qml6-module-qtquick-templates qml6-module-qtquick-window qml6-module-qtqml-workerscript
```

On Arch: `qt6-base qt6-declarative qt6-wayland`. The build finds Qt through `qmake6` or `qmake`;
to use another one, point `QMAKE` at it.

```bash
cargo build --release -p zeebx-standalone-qt     # target/release/zeebx-qt, the interface
cargo build --release -p zeebx-cli               # target/release/zeebx, the command line
```

The Libretro core links neither the desktop interface nor the host audio/controller libraries:

```bash
cargo build --release -p zeebx-libretro
```

The PlayStation Vita core is a static library, on the same pattern as the Switch one. The script
and what RetroArch expects of it are in [`frontends/vita/build.sh`](frontends/vita/build.sh).

The app that opens on its own on the Vita, without RetroArch, is the VPK from
[`frontends/vita-standalone`](frontends/vita-standalone/README.md).

To check the dependencies before the standalone build:

```bash
python3 tools/check_build_deps.py
```

### Installers and releases

The installers come out of [cargo-packager](https://github.com/crabnebula-dev/cargo-packager),
with the configuration in `[package.metadata.packager]` in the `Cargo.toml` of
`frontends/qt-standalone/`, run from inside that folder. **It does not deploy Qt**: each format
has its own step, and [`release.yml`](.github/workflows/release.yml) is the complete recipe.

- **`.deb`**: uses the system Qt, so it has to be built against it — on Ubuntu 24.04, with the
  packages above: `cargo packager --release --formats deb`.
- **AppImage**: carries its own Qt, through `linuxdeploy-plugin-qt`, with `QMAKE` pointing to Qt,
  `QML_SOURCES_PATHS` set to `frontends/qt-standalone/qml`, `EXTRA_PLATFORM_PLUGINS=libqwayland.so`
  and `EXTRA_QT_MODULES=waylandcompositor`. Build it on Ubuntu 22.04 with Qt from `aqtinstall`,
  like the CI: on Arch, linuxdeploy's `strip` does not recognize the system libraries, and the Qt
  there brings plugins with dependencies linuxdeploy cannot find.
- **Windows**: `windeployqt --release --no-translations --qmldir frontends/qt-standalone/qml
  --dir target/qt-implantado target/release/zeebx-qt.exe`, and then `--formats nsis`.
- **macOS**: `--formats app`, `macdeployqt` on the `.app`, the ad-hoc signature redone with
  `codesign --force --deep --sign -`, and the `.dmg` through `hdiutil`.

The files end up in `target/pacotes/`.

A version tag (`v0.1.0` or `0.1.0`) pushed to GitHub triggers
[`release.yml`](.github/workflows/release.yml), which assembles the release as a draft, with the
title equal to the tag. The emulator looks for new versions in these releases when it opens.

There are two formats on each of the four systems, and the file name says which is which:

| | |
|---|---|
| `zeebx-standalone-qt-linux-x86_64.deb`, `.AppImage` | the emulator with the interface, to install |
| `zeebx-standalone-qt-windows-x86_64-setup.exe` | same, on Windows |
| `zeebx-standalone-qt-macos-arm64.dmg`, `-x86_64.dmg` | same, on both Macs |
| `zeebx-headless-<system>.zip` | the binary without an interface, with the `config.ini` and the readme |
| `zeebx-android-arm64-v8a.apk` | the Android app |
| `zeebx-ios-simulator.zip` | the simulator build and the readme |
| `zeebx-ios.zip` | the IPA for classic AltStore and the readme |
| `zeebx-vita.zip` | the Vita app, a VPK, without RetroArch |

On macOS, the first launch warns that Apple could not verify Zeebx: this build is signed ad-hoc,
not by Apple. It is unblocked in System Settings > Privacy & Security > "Open Anyway", or with
`xattr -dr com.apple.quarantine "/Applications/Zeebx.app"` after dragging it to Applications (the
Qt one is called `Zeebx Qt.app`). The image carries a `README.txt` next to the app. On a Mac with
Apple silicon, use `macos-arm64`: `macos-x86_64` runs through Rosetta, and that was where the
games stopped in issue 53.

The APK is signed with the **debug key**, which is the one Gradle generates by itself: it is good
for sideloading (`adb install`), not for the Play Store — that one requires the release key,
which cannot live in a public repository. The same [`build.sh`](frontends/android/build.sh) used
on a local machine is the one that runs in CI; it accepts the `ANDROID_SDK_ROOT` the runners
export and whatever `gradle` is on the path.

iOS comes out of a Mac with Xcode. The core becomes a static library and Xcode builds the `.app`:

```bash
./frontends/ios/build.sh --app
```

The simulator build is arm64. For the device, the same script with `--app-device` asks for a
development team (`DEVELOPMENT_TEAM`) to install directly; without it, the `.app` comes out
unsigned and `--package` puts it, as `zeebx-ios.ipa`, inside `zeebx-ios.zip`, for classic
AltStore to sign with an Apple ID. Each zip carries its own `README.txt`: the simulator one, with
the build inside a `simulator/` folder, and the device one. iOS does not let the process map
executable code, so on this target the core is the interpreter, not Dynarmic. See
[`frontends/ios/README.md`](frontends/ios/README.md).

## Using it

The Qt interface is `zeebx-qt`: that is the program that opens the window. The command-line binary is `zeebx` (`frontends/cli`). With no subcommand it prints usage and exits; it does not open the interface.

```bash
cargo run --release -p zeebx-cli -- run "roms/Quake.zip" --window
```

Zips are extracted to a cache and the right `.mod` is picked automatically. `--seconds=N` sets
how many seconds of virtual time to emulate when there is no window; with a window, it runs until
you close it.

The keyboard controls:

| Key | Zeebo control |
|---|---|
| Arrows | D-pad |
| Z | Button 1 |
| X or Space | Button 2 |
| C | Button 3 |
| V | Button 4 |
| Q, W | ZL and ZR |
| F | Left stick click (L3) |
| G | Right stick click (R3) |
| H, Backspace, Enter | HOME |

F and G click the sticks. They do not move them. The default keyboard map has no stick axes.

`run` reports where the game stopped, what it asked for that we don't have, and the log the
developers themselves left in the binary — through `DBGPRINTF` and through ARM semihosting. That
report is the project's backlog. The debugging options are in [ARCHITECTURE.md](ARCHITECTURE.md).

### With your own frontend

Whoever already has a frontend — one that simulates the console's shell, a game shelf, an arcade
cabinet — does not want Zeebx's interface on top of the screen they built themselves. For that
there is a binary with no interface at all, configured by a `config.ini`:

```bash
cargo build --release -p zeebx-headless
./target/release/zeebx-headless "roms/Quake.zip"
```

The game is required and there is no default: this binary is called by another program, which
knows what it wants to open. On the first run it writes a complete, commented `config.ini`, and
says where. The options and the keys in the file are in English, like the commands, and so are
the comments.

It opens a window with only the game — or none, sending the frames through a pipe for your
program to paint. Graphics, audio and controllers come from the same fields the interface saves,
only in INI. See [`frontends/headless/README.md`](frontends/headless/README.md).

## Libretro core and muOS

The Libretro core is packaged with the `.info` and can be installed in RetroArch. The muOS card
uses the AArch64 core in `opt/muos/share/core`, and the MIDI soundfont is optional. The
documented installation is in [`docs/libretro/LIBRETRO_PLAN.md`](docs/libretro/LIBRETRO_PLAN.md).
The Zeebo playlist, DAT and covers are generated by the tools in the `tools/` folder.

## Platforms

Linux, Windows, and macOS are where the desktop interface is focused. Android and iOS already have release builds, and the PlayStation Vita has its own app.

## Games

The repository does not distribute games. Put yours in `roms/`, which git ignores, or in any other folder, and set it in the program's settings.

## Documentation

- [ARCHITECTURE.md](ARCHITECTURE.md) — how the emulator is built
- [docs/](docs/README.md) — the research: the console, the BREW platform, the file formats and
  the state of the art of Zeebo emulation
- [docs/implementation/](docs/implementation/README.md) — each subsystem, with the decisions and
  the reason for each one
- [TODO.md](TODO.md) — the logbook: what is done, what is missing and what has already cost dearly
- [CONTRIBUTING.en.md](CONTRIBUTING.en.md) — pull requests, the `development` branch, and new frontends

## License

Zeebx's code is **GPL-2.0-or-later**: the GPLv2 text is in [LICENSE](LICENSE).

**The binaries that ship in the release are distributed under GPLv3**, with the text in
[LICENSE-GPL3](LICENSE-GPL3). This is not a license change: all of them link libraries that are
Apache-2.0 only (`ab_glyph`, `sevenz-rust2`, `zopfli`, `cpal`, `winit`…), and Apache-2.0 is
compatible with GPLv3 but not with GPLv2. The "or later" is what allows this combination. The Qt
frontend would be GPLv3 anyway, because of Qt (see
[21-qt-migration.md](docs/implementation/21-qt-migration.md)).

Whoever wants Zeebx under GPLv2 can build the code, but not with those dependencies.


## Who makes Zeebx

Everyone who has ever contributed code to Zeebx. The full list is in GitHub's
[contributors tab](https://github.com/ZeebxTeam/zeebx-emu/graphs/contributors).

<!-- contributors -->
<table>
  <tr>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/rebquaker"><img src="https://avatars.githubusercontent.com/u/329117678?v=4&s=100" width="100px;" alt="rebquaker"/><br /><sub><b>rebquaker</b></sub></a></td>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/requeijaum"><img src="https://avatars.githubusercontent.com/u/5564635?v=4&s=100" width="100px;" alt="requeijaum"/><br /><sub><b>requeijaum</b></sub></a></td>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/Meowni2"><img src="https://avatars.githubusercontent.com/u/330453534?v=4&s=100" width="100px;" alt="Meowni2"/><br /><sub><b>Meowni2</b></sub></a></td>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/humbertodias"><img src="https://avatars.githubusercontent.com/u/9255997?v=4&s=100" width="100px;" alt="humbertodias"/><br /><sub><b>humbertodias</b></sub></a></td>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/J0aoSiqueira"><img src="https://avatars.githubusercontent.com/u/107425883?v=4&s=100" width="100px;" alt="J0aoSiqueira"/><br /><sub><b>J0aoSiqueira</b></sub></a></td>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/maskofsin"><img src="https://avatars.githubusercontent.com/u/171617836?v=4&s=100" width="100px;" alt="maskofsin"/><br /><sub><b>maskofsin</b></sub></a></td>
  </tr>
</table>
<!-- /contributors -->

## Thanks

- **tripleoxygen** — reverse engineering of the Zeebo's hardware and firmware, and the public
  material that makes this project possible :)
- **[Requeijaum](https://github.com/requeijaum)** — great support in understanding a good part of
  BREW and in fixing bugs
- The **Zeebo Clube** (Facebook) and **Zeebo Eterno** (Telegram) groups, for the comparisons with
  the real console that bring fidelity to the project, and the whole Zeebo community that has been
  supporting, testing and giving feedback since the beginning <3
