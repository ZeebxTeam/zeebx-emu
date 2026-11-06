# 21 — Migrating the interface to Qt

Plan for replacing `egui`/`eframe` with Qt 6, with [cxx-qt](https://github.com/KDAB/cxx-qt) as the
bridge between Rust and Qt. This document is a **plan**, not a description of what exists: each
phase, when finished, should be moved to the subject's document (the [10](10-interface.md) in
particular) and marked here as done.

## Status

| Phase | Situation |
|---|---|
| 0 — proof of feasibility | done on Linux (`frontends/classical-standalone/src/qt/`, feature `ui-qt`); in `qt.yml`, Linux, Windows x64 and macOS Intel passed, and macOS ARM64 awaits the new run with Qt 6.11 |
| 1 — decouple | done: the painter's `Viewport`, key translation, `ui::running_game::RunningGame` and `ui::host_input::DesktopInput` |
| 2 — structure | done: Qt interface core, `Library` as a model, main window and game window |
| 3 — game window | done: GPU texture without a copy, framing, stop and debug bars, calibration notice, title and window mode |
| 4 — library | done: grid and slider, Z-Wheel metadata covers and names, search, controller navigation |
| 5 — settings | done: the six tabs, the controller drawing with clicks by silhouette, capture, axes, Boomerang, Discord and updates |
| 6 — auxiliary windows | done: saves, run log, startup notice and new-version notice |
| 7 — review | done: the text keys of the two interfaces compared, a test that every key used exists, and what was missing in Qt (icon, `app_id`, minimum sizes, fullscreen in the library) |
| 8 — eframe cut | done: the egui interface left after 0.4.2. `eframe` and `ui::App` left the core, and the command line went to `frontends/cli` (binary `zeebx`, without Qt) |
| 9 — build and packaging | done: `release.yml` builds the AppImage, the NSIS and the `.dmg` with Qt bundled, and the `.deb` with the system's Qt; `ci.yml` installs Qt on the six targets; `qt.yml` does the same as the release and opens each installer before finishing |

**The two frontends were split after phase 9.** Qt is no longer a feature of the egui standalone:
it lives in `frontends/qt-standalone` (package `zeebx-standalone-qt`, binary `zeebx-qt`), and egui
went back to being the only content of `frontends/egui-standalone` (package
`zeebx-standalone-egui`, binary `zeebx`), which used to be called `classical-standalone`. The
`ui-qt` feature, `zeebx qt` and `zeebx egui` ceased to exist. For a while Qt stayed as legacy,
compiled only by `qt.yml`; then it went back into the release as the main interface, and 0.4.0,
0.4.1 and 0.4.2 shipped with both. **After 0.4.2 the egui interface left** (end of phase 8). The
text of the phases below describes the arrangement of each period, with the feature.

## Where egui really is

The starting point was the assumption that egui lives only in `src/ui/`. It does not — and what
leaks is what weighs the most in the migration:

| Coupling | Where | Weight |
|---|---|---|
| ~~`eframe::glow` in the core~~ | solved in 0.3.0: the core uses `glow` directly | — |
| **The GPU rasterizer runs in eframe's GL context** | `App::gl` → `Session::start_with` → `GpuState::create` | structural: see below |
| Key names | `Source::Key { name }` stores `egui::Key::name()` | `settings.json` compatibility |
| `egui::ViewportBuilder`/`ViewportCommand` | `ui/settings.rs` | move to the egui side |
| `egui::epaint::ViewportInPixels` | `ui/gpu.rs` (`GlPainter`) | replace with our own type |
| The drawing itself | `ui/app.rs` (3,400 lines), `ui/app/vitrine.rs` | rewrite |

`input/bindings.rs` does **not** use `egui::Key`: keys are already stored by name. But the name is
egui's, and it is what is written in the configuration of whoever already uses the
emulator. Qt needs a `Qt::Key → egui name` table, with a test, and not a new format. **And the
comparison is by key, never by text:** the same button appears with two spellings in the file —
the factory mapping writes `"ArrowUp"`, and the controls screen writes `egui::Key::name()`, which
is `"Up"`. egui and headless convert both sides with `egui::Key::from_name`, which accepts both.
The Qt bridge compared the text, and the keyboard d-pad did not reach the games — only the Z-Wheel,
which navigates by BREW keys.

`glutin` and `ab_glyph` **stay**. The first provides the pbuffer for `run` and `bench`, where all
measurement is done; the second is `IDISPLAY_DrawText`. Neither belongs to the interface.

### The borrowed context

`GpuState::create` receives the window's context instead of creating one, and the comment there
says why: the core runs on the interface thread, and a context of its own made current on it turns
off eframe's — the window freezes. In Qt the same holds, with a better way out: `QOpenGLContext`
has explicit `makeCurrent`/`doneCurrent`, and Qt Quick makes its context current before drawing.
So the rasterizer can have a context **of its own, offscreen and shared** with Qt Quick's
(`Qt::AA_ShareOpenGLContexts`):

- it survives the game window closing, which the Z-Wheel reopening games appreciates;
- the large texture from `gpu_frame()` is visible to the scene graph without a copy;
- the step becomes `makeCurrent → Session::step → doneCurrent`, without competing with anyone for
  the context.

## Why Qt Quick and not Widgets

cxx-qt generates `QObject`s in Rust — properties, signals, slots, inheritance from
`QAbstractListModel` and from `QQuickPaintedItem` — to be used from **QML**. There are no QtWidgets
bindings: every `QDialog` or `QListWidget` would be hand-written C++. The drawing stays in QML, the
state and logic in Rust, and the C++ glue is limited to what cxx-qt-lib does not cover:

1. **GL setup**, before `QGuiApplication`: `QQuickWindow::setGraphicsApi(OpenGL)` (on macOS the
   default is Metal, on Windows D3D), `QSurfaceFormat` 3.3 core — the same contract eframe
   delivers today and that the shaders of `GlPainter` and `GpuState` expect — and
   `Qt::AA_ShareOpenGLContexts`.
2. **The GL function loader** (`QOpenGLContext::getProcAddress`), for
   `glow::Context::from_loader_function_cstr`.
3. **The game screen with a native texture**: a `QQuickItem` with `updatePaintNode`, because there
   is no `QSGNode` binding.
4. **The cover provider** (`QQuickImageProvider`).

### A single thread, and the `basic` render loop

The emulator stays on the interface thread ([01](01-architecture.md#the-emulator-runs-on-the-interface-thread)).
In Qt Quick this has a consequence: the `threaded` render loop, the default on Linux with Mesa,
draws the scene graph on another thread — which would read the rasterizer's texture while the main
thread is already writing the next frame into it. `basic` draws on the main thread, as egui did. It
stays pinned until there is a measured reason to change; if there is, the texture needs to be
copied in `updatePaintNode`, the only moment in which both threads are stopped together.

The game step follows the window's frame — a QML `FrameAnimation` calls the step every frame, at
the vsync pace, like egui's `request_repaint`. A zero-interval `QTimer` would be a CPU at 100% for
nothing.

### License

The project moved from `GPL-2.0-only` to **`GPL-2.0-or-later`**. The dependency that pinned the
license to version 2, unicorn, was removed, and the default CPU backend is Dynarmic; each package's
`Cargo.toml` and [`AGENTS.md`](../../AGENTS.md) already say so. `LICENSE` is still the GPLv2 text,
which is what "or later" asks for.

"Or later" is what frees the migration: the binary that links Qt is distributed, as a whole,
under the GPLv3, which Zeebx's code allows. And with GPLv3 the question ends for the modules in use: `QtCore`, `QtGui`, `QtQuick`, `QtQml` and
`QtNetwork` declare, in the installed Qt 6.11,

```text
SPDX-License-Identifier: LicenseRef-Qt-Commercial OR LGPL-3.0-only OR GPL-2.0-only OR GPL-3.0-only
```

and both LGPLv3 and GPLv3 combine with GPLv3. Each new module still needs its header checked,
because not every Qt module has the same list. cxx-qt is MIT/Apache.

## The phases

### 0 — Proof of feasibility

Before migrating any screen, prove the four points that could make the plan unfeasible:

1. a QML window with text coming from a Rust property;
2. the RGB565 frame of a `Session` drawn by a `QQuickPaintedItem` with
   `QImage::Format_RGB16` — which is exactly RGB565, with no conversion on the CPU;
3. a `glow::Context` taken from a Qt context, with `GpuState` running on it;
4. the build on the three systems.

It sits behind the `ui-qt` feature and the `zeebx qt [game]` subcommand, without touching the egui
path:

```bash
cargo run --release -p zeebx-classical-standalone --features ui-qt -- \
    qt "Crash Bandicoot Nitro Kart 3D (Brazil) (Es,Pt).zip"
```

The code lives in `frontends/classical-standalone/` (`src/qt/`, `qml/`), and not in the core: the
core is also that of the Libretro core and of Android, and windows are a frontend matter. The
`ui-qt` feature belongs to the standalone and stays out of every job in `ci.yml` and
`release.yml`, which compile by `-p` without `--all-features`.

What the proof showed, on Linux with Wayland and NVIDIA (driver 615, Qt 6.11, cxx-qt 0.10):

- **Points 1 to 3 work.** Pac-Mania appears through `Format_RGB16`, and Crash Nitro Kart runs with
  `GpuState` in an offscreen, shared Qt context, going through `glow` — the same `GpuState`, with no
  change at all in the core.
- **The format needs to say `setRenderableType(OpenGL)`.** Without it, the request for 3.3 core
  comes back `EGL_BAD_MATCH` on Wayland and Qt Quick aborts when opening the window; on X11 it
  passed. The `qFatal` goes to the journal, not to the terminal — `QT_FORCE_STDERR_LOGGING=1`
  brings it back.
- **An item's constructor needs `impl cxx_qt::Initialize`.** Without it cxx-qt generates a
  constructor that passes the `QObject*` parent to the base, and `QQuickPaintedItem` wants a
  `QQuickItem*`.
- **The C++ glue was small:** GL setup and the offscreen context, about 80 lines in
  `src/qt/cpp/`. GL functions pass to Rust as `rust::Str`, not `const char*`.
- **The offscreen context has to be destroyed by hand, and in the right order.** Left to C++'s
  static destructors, it died after `QGuiApplication`, and **closing the window ended in SIGSEGV**
  inside `~QOffscreenSurface` — no test caught it, because they all ended by `timeout`. The order
  now is: the engine (the screen releases the session with the context alive), the context
  (`gl_destroy`), `QGuiApplication`. Measured by closing the window through QML: exit 0 on both
  rasterizers.

**`qt.yml`** (point 4) passed on Linux, Windows x64 and macOS Intel with Qt 6.8.3. macOS ARM64
broke inside one of Qt's own headers: 6.8.3's `qyieldcpu.h` calls `__yield()`, which the new
Xcode's clang only accepts with `<arm_acle.h>`. 6.11 tries `__builtin_arm_yield` first, and the
workflow moved to it — **except on Windows, which stays on 6.10**. From 6.11 on, Qt's repository
for Windows has one folder per architecture (`qt6_6112/qt6_6112_msvc2022_64/`), and the single
index that `aqtinstall` 3.3.0 looks for gives a 404; the step failed with "Failed to locate XML
data". 6.10 still has the old index, and the `__yield` is a defect only on ARM. **This applies to
phase 9 too:** packaging for Windows with `aqtinstall` pins Qt to 6.10 until it understands the new
format.

What is missing is input, which exists — keyboard by the egui
name and controllers through `gilrs`, with the mapping from `settings.json` — but was not exercised
by hand. The large frame from `gpu_frame()` still goes through the RGB565 read: wrapping the
texture without a copy belongs to phase 3.

### 1 — Decouple the core from eframe

**The scope changed with 0.3.0.** `egui` does not leave the core: the Android frontend draws with
it, and headless uses `egui::Key` as the canonical key name. What the migration replaces is the
desktop window — the `eframe` of `classical-standalone` —, and the egui the core carries for the
other frontends does not get in Qt's way. Two items of the original plan stopped applying because
of this: `no_construtor`/`comandos` in `settings.rs` cost Qt nothing, since it just does not call
them; and the canonical list of keys is already `egui::Key`, checked by the bridge's test.

Done:

- ~~`glow` as a direct dependency~~ — came with 0.3.0.
- ~~Own `Viewport` in the `GlPainter`~~. `ui::gpu::Viewport`, with `From<ViewportInPixels>` for
  whoever has egui on screen. Headless built a `ViewportInPixels` by hand without having any egui;
  now it just converts at the call, and its logic (and tests) stayed as they were.
- ~~Key translation in the core~~. `input::active_avks` and `input::transitions` left
  `App`, which also had an identical copy of `input::avk_for`. The
  Qt interface started delivering `EVT_KEY` with them, like egui's.

- ~~The game loop outside `App`~~. **`ui::running_game::RunningGame`**, in the core and without a
  toolkit: the session, the keys delivered, the previous frame's controller, the slice clock, the
  pause and the "opened by the Z-Wheel". `keyboard_changed` receives each keyboard event — a tap
  that starts and ends between two frames has to arrive as a press and a release —, `advance`
  delivers the input and does the step, and `launch_request`/`output` say what comes next: carry
  on, launch a ClassID, reopen the Z-Wheel or close. `RunningGame::open` builds the session from
  `Settings` — internal resolution, aspect ratio, enhancements, fog, sound, the installed games for
  the Z-Wheel, the inherited screen. The rule for where to open the Z-Wheel became
  `library::z_wheel_for`.

  Both windows use the same running game. egui's was left only with reading the keyboard from
  `egui::Context`, finding the ClassID's path in the library and drawing; Qt's started handling
  launches from the Z-Wheel and the return to it, and closes the window when the game exits with
  nowhere to go back to.

  **What did not change, on purpose.** An opening that fails with another game running reset the
  input and the pause of the game that stays: it still resets them
  (`RunningGame::forget_input`), however debatable that is — the goal of the cut is not to change
  behavior.

Missing:

- ~~The desktop input outside `App`~~. **`ui::host_input::DesktopInput`**: the host controllers
  (`gilrs`), the Wii Remotes and the motion sensors, read through each port's mapping — each port's
  controller, the sensor that feeds its Boomerang, and the axis swap of the Wii Remote and of
  Nintendo controllers. The keyboard is the only part each window reads: it delivers the set of
  pressed `egui::Key` — egui through `keys_down`, which is exactly what `key_down` checks; Qt
  through the `Qt::Key → egui::Key` table —, and `key_pressed` checks the mapping against it, in
  both spellings. It stays on the desktop because `gilrs` belongs to the desktop, but without a
  window egui. `App` kept the methods as pass-throughs, and the controls screen did not change; the
  Qt window started sending real motion instead of the fixed rest.
- The rest of `App` — library, settings, saves — is not split: each screen is rewritten in QML in
  phases 4 to 6, on the same core pieces (`library`, `settings`, `saves`, `zwheel_metadata`).

**Measured:** the core and standalone tests pass, with and without `ui-qt` (the only failure,
`video::gpu::tests::save_state_recreates_textures_and_the_second_unit`, fails the same way on a
clean HEAD and is not from this change); in the Qt window, Crash Nitro Kart on the GPU, Pac-Mania in
software and the Z-Wheel on the GPU run. **Not checked:** the Boomerang through the Qt window — a
Wii Remote or a controller with a sensor on the Boomerang port, in Crash Nitro Kart. **Checked by
hand**, in both windows: the Z-Wheel launching a game, the game going back to it through the menu,
and the game opened from the library closing the window on exit.

### 2 — Qt structure next to egui

`zeebx qt` opens the library, and `zeebx qt <game>` opens the game directly; `zeebx` with no
argument is still egui. The code belongs to `frontends/classical-standalone/`, behind the `ui-qt`
feature.

- **`qt/emu_core.rs`**: the interface state — settings, games, Z-Wheel, desktop input, the running
  game and the GL context — in a `thread_local` with `RefCell`. It is not an `Rc<RefCell<…>>`
  passed from hand to hand, as the plan said: cxx-qt creates each `QObject` from QML, with no
  arguments, and there is no way to hand it over. The rule this imposes is at the top of the file:
  **never emit a signal from inside `with`**, because QML may call back and the second borrow
  brings the program down.
- **Thin `QObject`s**: `GameScreen` only keeps the frame that is on screen and forwards keys;
  `Library` is a `QAbstractListModel` with the roles `title` and `path`, and opens games.
- **Two system windows**, as in egui: `Main.qml` with the list and the Z-Wheel button, and
  `Game.qml`, shown when a game opens and which closes the game when it is closed. `Esc` closes and
  `P` pauses, as in egui.
- **The GL context always exists**, created on the first opening, and it is the setting
  (`graphics.gpu_rasterizer`) that decides whether the rasterizer goes to the GPU — as in egui,
  which always has the window's context. The proof of feasibility's `--gpu` was removed.

What the plan foresaw here is left for later: the `ui-egui` feature, which would take `eframe` out
of the standalone. `eframe` comes through the core's `desktop` feature, and separating it is phase
8.

### 3 — The game window

**The frame goes to the scene graph without returning to the CPU.** `GameScreen` has as its base
`FrameItem` (`src/qt/cpp/frame.h`), a C++ `QQuickItem` with `updatePaintNode` — cxx-qt has no
`QSGNode` binding. With the rasterizer on the GPU, the texture from `gpu_frame()` goes in wrapped
(`QSGOpenGLTexture::fromNative`), at the internal resolution and with the 16:9 columns; in all
other cases — 2D, software, or the GPU with 2D on top —, the RGB565 screen is uploaded as an image,
only when it changed. Measured on Crash Nitro Kart with internal resolution 3×: practically all
the menu's frames go through the texture, and the image comes out in the right orientation.

- **The wrapper declares the texture as 1×1.** Qt uses the size only to convert the clip into 0
  to 1 coordinates, and the rasterizer already delivers the clip that way; the core's GL 3.3 does
  not ask the driver for the real size.
- **A `glFinish` after each step**, in the rasterizer's context: the specification only
  guarantees to another context what the first one has finished. The cost was not measured — the
  game's `eglSwapBuffers` already reads the frame back, which waits for the GPU just the same.
- **The framing is a single one.** `ui::running_game::fit_viewport` left `App`, with its four
  tests; egui and Qt compute the same rectangle, and the "window's" aspect ratio follows its size in
  both.

What else egui's window did and Qt's started doing:

- **The stop bar**: the translated reason (`play.failed`) and what to do (`play.stop.hint`).
- **The debug panel**, as a bar that shrinks the game. The texts went out to
  `debug_panel::Texts`, without egui, which the core's `panel` also started using; the timeline is
  a `Canvas` with the same scale.
- **The Boomerang calibration notice.** The logic — when it opens, whether the controller is still,
  when it finishes and disappears, the steering-wheel rotation — left `App` for `ui::calibration`,
  with three tests; the sensor's description, for `PortSensor::describe`. The image goes into the
  QML resources through `qt-build-utils`, which gives an alias to a file in `assets/`.
- **The library title**, without the folder fingerprint; **the configured window mode**
  (`graphics.janela_do_jogo`, and `graphics.janela` for the main one); **F11 and Alt+Enter**.
- **The size the window was left at**, in `graphics.tamanho_da_janela` and
  `graphics.tamanho_da_janela_do_jogo`. Only in windowed mode: maximized or fullscreen, the size is
  the screen's, and storing it would make the window reopen at the fullscreen size. Qt writes it
  after half a second without a size change, so as not to write the whole `settings.json` for
  every pixel of a border drag; egui writes as soon as the size changes, in the frame loop.
- **"Paused" translated**, with the `play.paused` key in all four languages — egui showed nothing.

What is missing is the Z-Wheel's official name in the title, which depends on the Z-Wheel metadata
(phase 4). **Not checked:** the calibration notice with a real Boomerang.

**The frame's alpha, found after the phases.** Double Dragon and the other Data East games showed
"bugs in white" in Qt and not in egui. The game clears the background with zero alpha; the frame
goes to the scene graph as an opaque texture, drawn without blending, and that alpha ended up in
the window. On NVIDIA, on Wayland, the window has an alpha channel — 8 bits, even with `prepare_gl`
asking for zero, measured through `QSG_INFO` —, and the compositor showed through those pixels what
was behind it. Checked by capturing the window: the title screen's background came out
`(0,0,0,0)`, and the same frame through `zeebx session` came out black. Now the texture is read
with alpha 1 (`GL_TEXTURE_SWIZZLE_A`, in `frame.cpp`), which only changes the read, and not the
rasterizer that draws into it; and `prepare_gl` asks for the window without alpha, which is what Qt
on Wayland checks to mark it as opaque. After the fix, the game area in the capture comes out all
opaque. egui did not have this because eframe's window is opaque.

### 4 — Library

**The rules are a single set; the drawing belongs to each window.** They left egui's `vitrine.rs`
for the core: the search (`library::matches_search`, with the tests), controller navigation
(`ui::navigation`, with repeat on hold and three tests), the Z-Wheel's official name
(`zwheel_metadata::title_of`) and the choice of each game's image (`zwheel_metadata::game_image` —
cover next to it, Z-Wheel cover, `.mif` icon, fallback — and `upscale_nearest`). egui started using
them without changing behavior.

- **The model.** `Library` has the roles `title`, `description`, `classId`, `cover`,
  `sharpCover`, `logo` and `rating`, and the list comes from the interface core already sorted,
  without the Z-Wheel and filtered by the search. `item(line)` serves the slider, which shows the
  games around the selection wrapping around the list, and not one row per delegate.
- **Images arrive through `image://zeebx/…`**, from a C++ `QQuickImageProvider`
  (`cpp/images.h`) that calls Rust through a `rust::Fn`. The address carries a generation number:
  Qt caches images by address, and without it "search again" would show the old covers. The
  provider is synchronous on purpose — the state lives in a `thread_local` of the interface thread.
- **The cover next to the game is asked of the disk once per scan.** egui just did not feel the
  cost because it kept each game's texture; a Qt model's `data()` is called for every row that
  appears. The rule started receiving the answer from the caller
  (`zwheel_metadata::has_sidecar_cover`).
- **The library's keyboard belongs to QML**, which already moves through the grid with the arrows;
  the controller arrives through `Library.commands`, read every 33 ms with the main window focused
  and no game open — as in egui, where the controller generates no event. Passing the keyboard
  through Rust too would make an arrow mapped on the controller move two steps, which egui avoids by
  merging the two first.
- **The grid** (`LibraryGrid.qml`) and **the slider** (`LibrarySlider.qml`) follow egui's
  measurements and curves: the 136 card with the 112×145 frame, the cylinder roll with the panels at
  0.42 radians, the boxes shrinking with distance, the `1 − e^(−12·dt)` spring. The names are not
  `Grid`/`Slider` because `Slider` is already a Qt Quick Controls type.
- **The style is Fusion.** Basic, the Qt Quick Controls default, paints the fields and the text with
  its own palette, and the window's background followed the system's: in a dark theme, dark text
  on a dark background. Fusion follows the system palette in everything.
  `QT_QUICK_CONTROLS_STYLE` still rules.
- **The wheel moves the slider through a `MouseArea`, not a `WheelHandler`.** `WheelHandler` only
  accepts the mouse by default — `acceptedDevices` 1, checked on Qt 6.11 — and only one axis: on a
  laptop, the touchpad's two-finger scroll did not move the slider. Reproduced with
  `qmltestrunner`, with the real slider and a fake library; the synthetic event is always a mouse
  one, so the touchpad itself was checked by hand.
- The top bar has the Z-Wheel, the search (Ctrl+F, Esc clears, Enter plays the selected one) and
  Zeeboids' "unlock sync". The settings and saves come in with their windows.

Checked on screen, through `grabToImage` on a copy of the configuration: the grid and the slider,
with the Z-Wheel covers and logos, the official names and the rating. **Not checked:** navigation
with a real controller.

**The arrows, found after the phases.** Two defects left the arrows without moving the list, and
simulated keyboard input on a virtual screen measured both, through the grid's `currentIndex` and
the slider's `cursor`:

- **In slider mode, the arrows moved the hidden grid.** The grid and the slider declared
  `focus: true`, and Qt gives focus to only one, the first. Now focus belongs to whichever is
  visible, and switching the mode in the settings passes it to the new view.
- **A clicked button kept the keyboard.** After "Search again" or the settings, the arrows went to
  the button. The bar's buttons no longer take focus, and the search's ✕ and a click on a cover give
  the arrows back to the list, as the search's Esc already did.


### 5 — Settings

A system window (`SettingsWindow.qml`), with egui's six tabs. The name is not
`SettingsModel.qml` because `SettingsModel` is already the Rust `QObject` registered in the same
module.

- **One key, not one property per option.** There are dozens of options; `SettingsModel`
  (`src/qt/settings.rs`) reads and writes by the `settings.json` key (`graphics.smooth`,
  `audio.volume`), gives the list choices already translated, and on each write saves and applies
  the effect right away, like egui: the volume and the fog in the open game, and the internal
  resolution, the aspect ratio and the enhancements by remaking the destination on the GPU — with
  the GL context current, because it is GL.
- **The language changes right away.** QML's `tr()` is a call, and a binding is only redone when
  it reads a property that changed. The `Language` singleton has a `version` that each file's
  `tr()` reads; switching the language changes it, and every text is redone. The library list is
  rebuilt along with it, because the Z-Wheel's official names are per language.
- **What belonged to `App` and both windows need** went to the core: the Discord presence
  (`discord::PresenceTracker`), the labels of the resolution and of the levels
  (`settings::*_label`) and switching a port's controller (`Player::switch_controller`, with a
  test). The Qt window gained the Discord presence and the new-version check.
- **The controls tab** has the port and device selector, the host controller — with the
  configured one in the list even when disconnected, as egui shows it —, the mapping with capture by
  key or by button, the axes with the live value, and the drawing. The drawing comes through the
  image provider: the base art and each silhouette already tinted in the current color; the click
  is tested against the silhouette in `PadArt::hit`. With a Boomerang port, the preview rotates
  with the sensor, with calibration and unlocking the sensors through `pkexec`. The live state is
  re-read every 33 ms with the tab visible, and `version` only changes when the configuration
  changes — the window is not redone on every controller read.
- **A binding depends on the version through an argument, not through a loose expression.** QML is
  compiled ahead of time (`qmlcachegen`), and the compiler discards a read whose value is not used:
  `(cfg.version, cfg.deviceKind())` lost the version read, and the binding stopped depending on it.
  The symptom was switching the device from Boomerang to Z-Pad and the tab staying on the Boomerang
  preview — and the same defect left the language not switching right away. Measured through the
  binding, in the binary: after the switch, Rust answered Z-Pad and the binding stayed on Boomerang;
  with interpreted QML (`QML_DISABLE_DISK_CACHE=1 QV4_FORCE_INTERPRETER=1`), it followed.
  `qmltestrunner` interprets, and that is why the wheel test would catch none of this. The form that
  stayed is `dependsOn([versions], value)`: the argument is evaluated at the call, and the read
  happens.
- **With the settings open, the library listens to neither the controller nor the keyboard** — not
  even if the main window gets focus back. While mapping the controller, the pressed button opened a
  game by accident. The rule is a single one, `Main.overlaid`, true with any window on top (the
  game's, the settings', the saves' and the two notices); the mouse still works, as in egui. When it
  starts listening again, what was already pressed does not count: navigation is muted while it is
  not listening. Checked in the binary, opening the settings and giving focus back to the main
  window.
- **The tab's buttons do not take focus.** With it, capturing the space bar would click "assign"
  again and give up the capture.
- **"Put the frame on screen through GL" does not appear.** The Qt window always puts the frame
  through the scene graph, with the GPU texture when there is one.
- The folder and file pickers stay on `rfd`, as in egui.

**One change of behavior, on purpose:** in egui, "restore" on a port with a Wii Remote applied the
common controller mapping to the Wii Remote, while choosing it in the list applied its own.
`Player::default_for_controller` uses the same rule for both, and restore goes back to the Wii
Remote's.

Checked on screen, by capturing the window on a copy of the configuration: the six tabs, the
Boomerang preview reading a Pro Controller's sensor, and the controller drawing with the capture
pulsing and the sticks. **Not checked:** calibration and unlocking the sensors end to end.

### 6 — Auxiliary windows

The `QObject`s live in `src/qt/dialogs.rs` — `Saves`, `Notices` and `Log` —, and the rule of each
one in the core, so both interfaces use the same one:

- **What belonged to `App` went to the core.** `saves::all` rebuilds the manifests of old caches and
  lists the games' saves and the device's; `running_game::Report` writes the run report every two
  seconds, and `running_game::serial_path` says where the serial capture goes.
- **Saves** (`SavesWindow.qml`) is a system window, with egui's two sections — the games, and the
  device with the hint that it belongs to all of them. The list touches the disk: it is re-read on
  opening and after each deletion, and the `version` of `Saves` redoes the window. Deletion asks for
  confirmation in a modal dialog, and the message about what happened stays at the top.
- **Log** (`LogWindow.qml`) follows the game: it appears with the game open and `debug.log` on, and
  turning the option on mid-game opens it. Closing dismisses the log for **this** run, not the
  preference. The lines are re-read every 250 ms, only when they change, and the list stays pinned
  to the end; scrolling up to read unpins it, and only the user changes that. The first read
  happens before the window appears, with no height to go to the end, and that is why going to the
  end is repeated after layout. The bar has export, through `rfd`, and the report's path in view.
- **Who stays on top of whom.** In QML, a window declared inside another gets it as
  `transientParent`, and the window manager keeps the child always above the parent. The game's was
  a child of the main one, and clicking the main one during the game did not bring it to the front;
  now it has no parent, as in egui. The log's is a child of the game's, and stays on top of it —
  **but only if it is shown after the game window exists in the compositor.** Shown together with
  it, it reached KWin without a parent and stayed behind the game: a KWin script that lists the
  stack saw it as a loose window (`transient=false`). Now it waits for the game to become active.
  And it does not accept focus (`Qt.WindowDoesNotAcceptFocus`): KWin gave focus to the log when it
  appeared, and on Wayland focus only goes back to another window in response to a user action.
  Measured through KWin's stack: the log on top, a child of the game, and the game as the active
  window.
- **The notices** are modal dialogs on top of the library. The startup one appears while the version
  has not been dismissed; "Configure controller" opens the settings on the controls tab. The
  new-version one comes after it, never together, and "Download" opens the page in the browser.
- **The answer of the new-version check is read by the library's clock**, and not returned through
  `qt_thread().queue(...)`, as this plan foresaw. The 33 ms clock already exists for the
  controller, and the read is a `try_recv`; a signal coming from another thread would save nothing.

Checked on screen, by capturing the dialogs and windows on a copy of the configuration, with fake
saves: the two sections, the confirmation, the deletion removing the save from disk and from the
list, the two notices, and the settings opening on Controls. The log checked with Alien Breaker
Deluxe open: the lines, the list pinned to the end, the report and the serial written, and the
window disappearing on close with the game still open. **Not checked:** export, which opens the
system dialog, and the new-version notice coming from a real check.

### 7 — Review

`discord.rs`, `i18n.rs`, `saves.rs`, `zwheel_metadata.rs` and `library.rs` do not change. The texts
still come from `Catalog` — **not** from `qsTr` —, so the loose JSON files and the key test keep
working.

- **The shared modules.** Against `master`, `i18n.rs` is intact, and the other four only gained
  lines, none changed: it is what left `App` for both interfaces to use — the Discord presence, the
  save list, and functions of the library and of the Z-Wheel metadata. No `qsTr` in the QML.
- **The two interfaces, compared by text keys.** A key that egui uses and Qt does not is a function
  that was left behind. Only three remained: "put the frame on screen through GL", removed on
  purpose in phase 5; exporting the Discord images, which in egui is also off screen
  (`#[allow(dead_code)]`); and `settings.title`, which Qt started using in the settings title. The
  keys neither of them uses are built at runtime (`button.*`, `axis.*`) or belong to the Android
  frontend (`play.*`).
- **Every key used exists in the catalog** — `tests/ui_string_keys.rs`, in the root package: in the
  standalone's, with the Qt interface on, cxx-qt links the QML module into every target, and an
  integration test, without the bridges that belong to the binary, did not link. The `i18n.rs` test
  checks that the languages have the same keys among themselves; the other side was missing. A wrong
  key breaks nothing: `Catalog` returns it as itself, and the screen shows `saves.confirm.yes`
  instead of "Delete". In QML every literal that looks like a key counts, except the settings ones
  (`keyId:`, `v()`, `define()`, `options()`), which also have dots; in Rust, the first argument of
  calls to the catalog in `src/qt/`, `src/ui/` and the Android frontend. Checked by swapping a key
  for one that does not exist: the test points to the file and the line.
- **What has no text**, compared by what `eframe` received and by `App`'s `update`:
  - The icon and the `app_id`. Qt gave neither. Now the logo, reduced to 256 pixels as in egui, is
    the icon of every window (`apply_icon`, in `gl_qt.cpp`), and `setDesktopFileName("zeebx")`
    plays the role of `app_id`: on Wayland it is through it that the compositor finds
    `zeebx.desktop` and the icon. Running from `target/`, without the `.desktop` installed, the
    portal warns that it did not find the application; it is just the warning.
  - egui's minimum sizes: the library, 480×360; the settings, 420×320; the game, 320×240. The saves
    and log windows already had them.
  - F11 and Alt+Enter in the library too. In egui fullscreen works in the main window and in the
    game's; Qt only had it in the game's.

### 8 — eframe cut

**`eframe` leaves the standalone, not egui the project** — see phase 1. `ui-qt` as the default for
one version, then `eframe`, `ui::App` and, if replaced by
`QtQuick.Dialogs`, `rfd` go away. `--window` (`minifb`) is a pending decision in `TODO.md`,
independent of this one. The [10](10-interface.md), the layer diagram of
[`ARCHITECTURE.md`](../../ARCHITECTURE.md) and the comments in `video/context.rs` and
`video/gpu.rs` that talk about eframe are rewritten.

**The first stage, done.** Compiled with the `ui-qt` feature, `zeebx` with no arguments opens Qt,
and `zeebx egui` opens the old interface — so whoever finds a difference has something to compare
against. `zeebx qt [game]` still works. Checked through QML's import log
(`QT_LOGGING_RULES="qt.qml.import=true"`): `zeebx` loads QML, `zeebx egui` does not.

**The second stage, done: the `ui-qt` feature is on by default.** It waited for two things: phase
9, which taught CI to install and package Qt, and the license leaving GPL-2.0-only (see
*License*), which is now `GPL-2.0-or-later`. `ci.yml` installs Qt on the six targets, and
`release.yml` builds the installers with it. Without Qt, `--no-default-features` still compiles the
egui interface, and `tools/check_build_deps.py` checks Qt along with the rest. The order that
remains is:

1. phase 9 — done;
2. the license changed and the feature on by default — done;
3. a release with Qt as default and `zeebx egui` still there — there were three, from 0.4.0 to
   0.4.2, already with the two frontends split;
4. in the next one, the cut — done, below.

**The third stage, the cut, done after 0.4.2.** What left:

- **`frontends/egui-standalone`**, with its window and installers, and the `standalone-egui` job in
  `release.yml`. Whoever has the egui interface installed stays on 0.4.2: its updater reads
  `latest-egui.json`, which stopped being published, and the version notes explain the switch;
- **`eframe`**, from the `desktop` feature and from `Cargo.lock` — and with it `winit`, `wgpu` and
  `naga`, which only it pulled in —, `ui::App` (`src/ui/app.rs`, `src/ui/app/vitrine.rs`) and
  `WindowMode::no_construtor` and `comandos`, which only built its window;
- **from the Qt `.deb`**, `libxcursor1`, `libxrandr2`, `libxi6` and `libwayland-client0`: they were
  `eframe`'s, and the binary no longer links them (checked with `ldd`).

**The command line stayed, in its own package.** It lived in the egui binary — `run`, `session`,
`bench`, `info`, `controls`, `wiimote` —, and CI, the scripts and the documentation use it. Now it
is `frontends/cli` (package `zeebx-cli`), with the same `zeebx` binary and without Qt, to compile
on a machine or in a container without it. `ci.yml` compiles it on the six targets, which keeps the
core and its tests covered also on the two ARM64 ones, where Qt is not built; in the release, the
core tests went to the headless job, which was what ran them along with egui. The interface is
still `zeebx-qt`: it is the name that the shortcut, the `.desktop` and the updater of whoever has
it installed point to.

**`egui` stayed in the core, without a window.** It is the `egui::Key` key names, which are the
format written in `settings.json` and what the Qt bridge compares, and the
[`GlPainter`](../../src/ui/gpu.rs), which the headless window and Android use. The core re-exports
it (`zeebx::egui`), in place of the former `zeebx::eframe`. `rfd` also stayed: Qt uses it for the
folder and file pickers.

### 9 — Build and packaging

CI installs Qt on the three systems. `cargo packager` does not deploy Qt: each system gets its own
step (`linuxdeploy-plugin-qt`, `windeployqt`, `macdeployqt`), and the `.deb` comes to depend on
`libqt6gui6`, `libqt6qml6`, `libqt6quick6` and the QML modules used.

**First in `qt.yml`, then in `release.yml`.** While the license held it back, the installers came
out only as `qt.yml` artifacts, with a separate configuration, passed through `--config`. With the
feature on by default, the configuration moved to the standalone's `[package.metadata.packager]`
in `Cargo.toml`, and the same steps are in `release.yml`: the AppImage on Ubuntu 22.04, the `.deb`
in a separate job on 24.04, the NSIS and the two `.dmg`s. `qt.yml` does the same and, besides that,
opens each installer on a virtual screen before leaving it as an artifact — it is where to test
before a tag. `--config` brought two stumbles, which went away with it: without reading
`Cargo.toml`, `cargo packager` did not know the version, and 0.11.8 entered the *file*'s path as if
it were a folder, stopping with "Not a directory".

- **The cache key includes the system.** The AppImage job moved from Ubuntu 24.04 to 22.04 with the
  same key, and the "Build" step exited with 101 — while the same build passed in a clean 22.04
  container. The likely cause, not confirmed without the log: the cache restored the build scripts
  compiled on 24.04, linked against a newer glibc. The key is now `<name>-<system>`, in `qt.yml`
  and in `release.yml`, which is worth it either way.
- **The AppImage**, built on Ubuntu 22.04 like the release's, carries Qt 6.11.2 through
  `linuxdeploy-plugin-qt`. Wayland does not come by itself: you have to ask for the platform plugin
  (`libqwayland.so`, a single one from Qt 6.10 on) and the `waylandcompositor` module, without which
  the graphics integration (`wayland-graphics-integration-client`) is left out and the window has no
  GL on Wayland — checked by taking the AppImage apart. ALSA and `libgpg-error` are left out on
  purpose, through the AppImage's exclusion list: every desktop has them. Checked on clean Ubuntu
  22.04, 24.04 and Debian 13, on a virtual screen, with a game open. Built on Arch, `linuxdeploy`
  is no proof: the bundled `strip` does not know the libraries there, and the Qt there brings KDE
  image plugins with dependencies that the `aqtinstall` one does not have.
- **The `.deb` uses the system's Qt**, as the plan foresaw, and so it is compiled against it, in a
  separate job on Ubuntu 24.04: a Qt 6.11 binary does not run on an older Qt, and 24.04 ships 6.4.
  That leaves Ubuntu 22.04 out of the `.deb` with Qt (its Qt is 6.2), but not out of the AppImage.
  The dependencies are the libraries the binary links, checked with `ldd` and `dpkg -S`, the
  platform plugin, `qt6-wayland` and the QML modules the `import`s ask for. `gui`, `network` and
  `opengl` go with both names: Ubuntu 24.04 calls them with `t64`, and Debian 13 without. The job
  installs the `.deb` on clean Ubuntu 24.04 and Debian 13, with `apt` resolving everything, and
  opens it on a virtual screen.
- **Windows**: `windeployqt` gathers Qt into a folder, and the configuration tells NSIS to install
  it next to `zeebx.exe`, with the subfolders.
- **macOS**: `cargo packager` builds the `.app`, `macdeployqt` puts Qt inside, the ad-hoc signature
  is redone — on ARM, a modified binary without a valid signature is killed on opening — and
  `hdiutil` makes the `.dmg`.
- **What Windows and macOS have not had yet:** a run. The steps do not run outside their runners,
  and the first `qt.yml` with them is the proof.

**Three things packaging found outside it:**

- **The ALSA of egui's `.deb`, which already ships in the release**, installed a fake on a minimal
  system. The dependency was `libasound2 | libasound2t64`, and on Ubuntu 24.04 `libasound2` is
  satisfied by `liboss4-salsa-asound2`, a fake ALSA on top of OSS4: `apt` kept it, and the binary
  stopped on opening with "undefined symbol: snd_pcm_status_get_trigger_htstamp". With
  `libasound2t64` first, `apt` picks the real one. Fixed in both `.deb`s.
- **`zeebx qt <game>` on Qt 6.4 did not show the game window.** It was opened from inside the main
  window's `onCompleted`, and 6.4 left it unmapped; 6.11 showed it. Now it is opened after
  construction, with `Qt.callLater`. Opened from the library, both already worked.
- **The search shortcut** used `sequence: StandardKey.Find`, and where the system gives more than
  one key to "find", Qt 6.11 binds only the first and warns. It moved to `sequences`.

Ubuntu's Qt was compiled with the `gold` linker, and cxx-qt passes `-fuse-ld=gold` on to the link:
`rustc` warns that `gold` is deprecated. It is just the warning — the binary links and runs.

## Risks, from largest to smallest

1. The context shared between `GpuState` and the scene graph, above all on macOS, where OpenGL is
   deprecated but still behind the RHI. It is the reason phase 0 exists.
2. Qt in CI and in the packages of the three systems.
3. The fidelity of the cylindrical slider.
4. The old `settings.json`: egui's key names have to keep working.
