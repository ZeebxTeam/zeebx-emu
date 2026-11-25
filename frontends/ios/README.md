# Zeebx on iOS

The core is the same. What changes is who paints and who presses the button.

The emulation — BREW, the CPU, the rasterizer, the loader — is the `zeebx` library. This
directory is the frontend: a static library in Rust and an Xcode project in Swift. The Swift side
emulates nothing. It asks for a turn, receives a frame in BGRA and hands back the touch.

## The frontend

**The content is the same as desktop and Android; the form is iOS's.** The grid uses
`library::scan`; the settings write the same `Settings`, in the same format; the labels come from
the same `ui::i18n`; the controls drawn on screen are placed and hit-tested by the same
`input::touch` as Android's; the save states are `ui::save_slots`, in the same layout.

**Swift draws, and decides nothing.** Which options the settings have, what a finger presses,
what a controller button is bound to, when the game is paused: all of that is on the Rust side of
the border, and crosses it as plain data. The reason is practical before it is tidy. The Swift
half only compiles on a Mac with Xcode; the Rust half is an ordinary crate, and `cargo test -p
zeebx-ios` runs on any host. What can be wrong in a way worth catching is kept where a test
reaches it.

| File | What it is |
|---|---|
| `src/lib.rs` | the border: the app state and every `zeebx_ios_*` function |
| `src/game.rs` | one turn of the loop — shortcuts, rewind, controller, turbo, step — and the pause menu |
| `src/overlay.rs` | the fingers on the on-screen controls, and what they press |
| `src/editor.rs` | the screen that moves and resizes those controls |
| `src/controller.rs` | the physical controller: the factory map and what the player rebound |
| `src/settings_page.rs` | the settings screen, described as rows for Swift to draw |
| `include/zeebx_ios.h` | the contract. A test checks its numbers against the Rust side |
| `app/Zeebx/Bridge.swift` | the shared state, the texts, the controller reading |
| `app/Zeebx/LibraryView.swift` | the game grid, with covers and search |
| `app/Zeebx/GameView.swift` | the frame on screen and the display link that asks for turns |
| `app/Zeebx/TouchOverlayView.swift` | the drawing of the on-screen controls |
| `app/Zeebx/SettingsView.swift` | one view per **kind** of row; it knows no option by name |
| `app/Zeebx/TouchEditorView.swift` | the layout editor's canvas and its size bar |
| `app/Zeebx/PauseMenuView.swift` | back, pause, restart, stop and the save state slots |

The settings screen is the clearest case of the split. `settings_page.rs` answers with a list —
a switch, a choice, a slider, a row that does something — and `SettingsView.swift` paints each
kind the way iOS paints settings. A new option is one more row in Rust, with a test, and not a
line of Swift.

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
| `soundfont` (`rustysynth`, pure Rust) | |
| software rasterizer | rasterizer on the graphics card — the core's offscreen context needs EGL, and there is none here |

The frame the session publishes is already the device's, in RGB565. The library converts it to
BGRA8 once per turn, which is the layout a little-endian `CGImage` with alpha first accepts.

## Where the ROMs go

`Documents/roms`, inside the sandbox. `Info.plist` turns on file sharing, so the folder shows up
in the Files app and, with the device connected to Finder, in Zeebx's tab. A loose `.mod` or a
`.zip` both work: what picks the module inside the package is the same `library::scan` as
desktop.

Cache, saves and the virtual device live in `Library/Application Support`, in the
`StoragePaths` layout — the bundle is not where the game writes. The settings are `zeebx.json` in
that same folder, and the save states are under `states/`.

A `.sf2` bank for MIDI music goes in the app's folder too — at its root, or in `soundfonts` — and
shows up in the audio settings. **The app's folder moves**: iOS gives the container a new path
when the app is updated, and the bank's path is saved whole. A bank that is no longer where it
was is looked for again by its file name.

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

**The library** is a grid of covers with a search field. With a controller connected the grid
has a cursor: the D-pad or the stick moves it and A opens the game.

**The game** fills the screen at the scaling chosen in the settings — whole pixels, fit or fill —
and "whole pixels" counts the device's pixels, not its points. A small button at the top, beside
ZL, opens the menu; so does Esc on a keyboard, and so does the app leaving the screen. The menu
has back, pause, restart, stop and the ten save state slots, with their thumbnails.

**The controls drawn on screen** are Android's: the D-pad, the two sticks, the four buttons, ZL,
ZR and HOME, plus a piece each for turbo, fast-forward and rewind while those are in use. They
are laid out inside the safe area, so no piece ends up under the notch. Automatic mode hides them
at the first press of a physical controller and brings them back at the first touch. The editor
moves each piece and resizes it, and the settings have the overall size and the opacity.

**A real controller** comes in through `GCController`. Out of the box it is the map this frontend
always had, and the same as Android's: south on `b1`, east on `b2`, west on `b3`, north on `b4`,
the upper triggers on `zl`/`zr`, Menu on HOME. In the settings each Zeebo button can be given to
another controller button, and fast-forward, rewind and the turbo key each take one. The bindings
are stored under the names desktop uses (`South`, `LeftTrigger`), in the same `settings.json`.

**The settings** are the same sections as desktop: general, graphics, speed, audio, controls,
debug and about. Graphics has only what the software rasterizer does — the internal resolution,
the antialiasing and the wide aspect ratio are empty functions without the graphics card.

In the simulator, the keyboard is the desktop window's: arrows, Z/X/C/V, Q/W, and
H/Backspace/Enter for HOME.

## What isn't here yet

- **3D on the graphics card.** There is no GL context of ours. The game draws in the software
  rasterizer.
- **JIT.** It depends on a permission Apple doesn't give third-party apps.
- **Choosing the ROM folder.** It is `Documents/roms`. Another folder would be a security-scoped
  bookmark from the system's picker, and the core's loader opens a file path.
- **A controller in the settings and in the pause menu.** The grid and the game obey it; those
  two screens are touched.
- **The debug panel's graph.** The three lines of numbers are there; the graph of the last
  minute is not drawn.
- **A second player.** One controller, on port one.
