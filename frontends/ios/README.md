# Zeebx on iOS

The core is the same. What changes is who paints and who presses the button.

The emulation — BREW, the CPU, the rasterizer, the loader — is the `zeebx` library. This
directory is the frontend: a static library in Rust and an Xcode project in Swift. The Swift side
emulates nothing. It asks for a turn, receives a frame in BGRA and hands back the touch.

## Why it isn't Dynarmic

On desktop the core recompiles the ARM blocks to machine code. On iOS that doesn't fit. The
allocator Dynarmic uses on an ARM64 host, oaknut, maps the page as executable and then changes
the protection with `mprotect`. iOS refuses that change for a third-party app — the executable
page belongs to the system. The simulator is also `TARGET_OS_IPHONE`, so the refusal applies to
both.

The interpreter in `src/cpu/interpreter.rs` takes that place, the same one it already takes on
`wasm32`. It was not measured against the JIT. A game that runs comfortably on desktop walks more
slowly here; what doesn't change is the conversation with BREW.

## The cut

| Stays | Doesn't go into this frontend |
|---|---|
| `audio` (`cpal`, which on iOS is Core Audio) | `desktop`: eframe, glutin, gilrs, rfd |
| software rasterizer | rasterizer on the graphics card — the core's offscreen context needs EGL, and there is none here |

The frame the session publishes is already the device's, in RGB565. The library converts it to
BGRA8 once per turn, which is the layout a little-endian `CGImage` with alpha first accepts.

## Where the ROMs go

`Documents/roms`, inside the sandbox. `Info.plist` turns on file sharing, so the folder shows up
in the Files app and, with the device connected to Finder, in Zeebx's tab. A loose `.mod` or a
`.zip` both work: what picks the module inside the package is the same `library::scan` as
desktop.

Cache, saves and the virtual device live in `Library/Application Support`, in the
`StoragePaths` layout — the bundle is not where the game writes.

## Building

A Mac with Xcode, and the two targets in rustup:

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
```

```bash
./frontends/ios/build.sh              # the simulator library
./frontends/ios/build.sh --device     # the device library
./frontends/ios/build.sh --app        # the simulator .app
./frontends/ios/build.sh --app-device
```

The simulator `.app` comes out in `frontends/ios/build/iphonesimulator/`. The device one, in
`frontends/ios/build/iphoneos/`. Without `DEVELOPMENT_TEAM` the device one is not signed. That
binary is `zeebx-ios.ipa`, inside `zeebx-ios.zip`: classic AltStore signs it with the Apple ID of
whoever installs it. `./frontends/ios/build.sh --package` makes two zips, each with its own
readme: [`README-simulator.txt`](README-simulator.txt) in the simulator one and
[`README-altstore.txt`](README-altstore.txt) in the device one. To install with your own developer
account:

```bash
DEVELOPMENT_TEAM=XXXXXXXXXX ./frontends/ios/build.sh --app-device
```

Xcode calls the same script in the build phase (`--build-phase`). There is no second list of
targets inside `project.pbxproj`.

The simulator this script produces is arm64, the one for Macs with Apple Silicon. The Intel
simulator (`x86_64-apple-ios`) was left out: current Xcode no longer offers it as a destination.

## What the screen does

The list is what the scan found. Opening a game hands the controller to the first port — the same
`DEFAULT_PORTS` as desktop — and turns on sound. The speed limit stays on: this frontend doesn't
have the settings screen yet, and a phone that runs ahead only burns battery.

Touch covers the D-pad, the four face buttons and the L and R triggers. A real controller comes in
through `GCController`, with the same map as Android: south on `b1`, east on `b2`, west on `b3`,
north on `b4`. In the simulator, the keyboard is the desktop window's: arrows, Z/X/C/V, Q/W, and
H/Backspace/Enter for HOME.

## What isn't here yet

- **The settings screen.** Resolution, speed and the button map are the defaults.
- **3D on the graphics card.** There is no GL context of ours. The game draws in the software
  rasterizer.
- **JIT.** It depends on a permission Apple doesn't give third-party apps.
