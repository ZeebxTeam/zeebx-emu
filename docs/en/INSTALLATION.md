# Installation

Ready-made builds are on the [releases](https://github.com/ZeebxTeam/zeebx-emu/releases) page: the Windows installer, macOS `.dmg`, Linux `.deb` and AppImage, Android APK, iOS archives, the Vita VPK, and the Libretro core. On **macOS** and **Linux**, you can also install through [Homebrew](https://brew.sh):

```bash
brew install --cask zeebxteam/tap/zeebx-qt
```

## Which file to download

| File | Use |
|---|---|
| `zeebx-standalone-qt-linux-x86_64.deb`, `.AppImage` | Desktop interface for Linux |
| `zeebx-standalone-qt-windows-x86_64-setup.exe` | Desktop interface for Windows |
| `zeebx-standalone-qt-macos-arm64.dmg`, `-x86_64.dmg` | Desktop interface for Apple Silicon or Intel Macs |
| `zeebx-headless-<system>.zip` | Headless binary, `config.ini`, and readme |
| `zeebx-android-arm64-v8a.apk` | Android app |
| `zeebx-ios-simulator.zip` | iOS Simulator build and its readme |
| `zeebx-ios.zip` | IPA for classic AltStore, and its readme |
| `zeebx-vita.zip` | PlayStation Vita app (a VPK), without RetroArch |
| `zeebx_libretro-<system>.zip` | Libretro core and matching `.info` file |

The `zeebx-qt` Homebrew cask requires macOS 13 or newer. On Linux it installs the AppImage at `~/Applications/zeebx-qt.AppImage`.

## First launch on macOS

The first launch warns that Apple cannot verify Zeebx: the build is ad-hoc signed, not signed by Apple. Allow it in **System Settings > Privacy & Security > Open Anyway**, or run `xattr -dr com.apple.quarantine "/Applications/Zeebx Qt.app"` after moving the app to Applications. On an Apple Silicon Mac, use the `macos-arm64` build; the Intel build runs through Rosetta.

## Android

The APK is signed with a **debug key** for sideloading (`adb install`), not for the Play Store.

## iOS

The simulator is arm64. On a device, `zeebx-ios.zip` contains an IPA for classic AltStore to sign with an Apple ID. To build iOS from source, see [BUILDING.md](BUILDING.md#ios).
