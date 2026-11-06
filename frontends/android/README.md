# Zeebx on Android

The core is the same. What changes is who drives it.

The emulation — BREW, the CPU, the rasterizer, the loader — became the `zeebx` library, and the
ones consuming it are the frontends — the desktop interface (`frontends/qt-standalone`), the
command line (`frontends/cli`), headless, the Libretro core, iOS —, the tests, and the `.so` in
this directory. Not one line of emulation knows which of them it is running in.

This directory is `frontends/android/` because it is **a** frontend, not *the* Android frontend:
when there is another target — a console, a browser —, it goes in next to it, and the core stays
where it is.

## The cut

What depends on a desktop window is behind `cfg(not(target_os = "android"))`:

| Stays on every platform | Desktop only |
|---|---|
| `audio`, `brew`, `cpu`, `input`, `loader`, `machine`, `video`, `session` | `ui::window`, `ui::updater`, `ui::discord` |
| `ui::settings`, `ui::library`, `ui::zwheel_metadata`, `ui::saves`, `ui::i18n`, `ui::gpu` | `glutin`, `gilrs`, `minifb`, `rfd`, `ureq`, `discord-rich-presence` |

The second row of the first column is the point: the game library, the settings and the saves are
read **by the core** — `machine`, `loader` and `session` talk to them —, so they are not
interface, even though they live in `src/ui/`.

## The frontend

Not one line of Java or Kotlin: the system's `NativeActivity` loads `libzeebx_android.so` and
calls its `android_main`.

| File | What it is |
|---|---|
| `src/lib.rs` | the event loop, the app state and where each screen comes in |
| `src/screen.rs` | the `ANativeWindow` turning into an EGL surface, the context that outlives it, and `egui_glow` |
| `src/host_input.rs` | the Android event turning into a Zeebo button and an egui pointer |
| `src/library.rs` | the game grid, with the covers the core's `library::scan` already reads |
| `src/settings_page.rs` | the settings, over the same `Settings` and the same language catalog as desktop |
| `src/theme.rs` | the size of things on a screen you hold in your hands |
| `src/widgets.rs` | the touch pieces: the toggle switch, the segmented control, the option strip |
| `src/selector.rs` | the folder browser |
| `src/game.rs` | the frame on screen, the debug panel and the "back" question |
| `src/system.rs` | the two JNI calls: the file permission and opening an address |

**The content is the same as desktop; the form is not.** The grid uses `library::scan`, which is
the same one that finds the games on desktop; the settings write the same `Settings`, in the same
format; the labels come from the same `ui::i18n`, whose built-in languages are embedded in the
binary; and the speed panel is literally `ui::debug_panel::panel`. What was left out was left out
because it doesn't exist here: the window options don't apply to a single screen, and
controllers, Discord and updates are not hooked up in this frontend yet.

The form, that's another matter — and the first attempt, which was the desktop window shrunk
down, didn't work. Four things changed, and all four because of the device:

- **Size.** Body text at 17 and hints at 14, not 12 and 9; touch targets of 48 points, not 18. A
  handheld sits half an arm away from your face, not thirty centimeters, and what points is the
  thumb, not a one-pixel arrow.
- **The checkbox became a switch.** An 18-point `checkbox` can neither be read nor hit; a toggle
  switch tells its state by position and color. The `radio_button` dots became segmented buttons
  for the same reason.
- **The hints stay closed.** Zeebx's help texts are long — some are five lines —, and all of them
  open become a gray wall where you can't find what you were looking for. Each row has a `?`, and
  only one hint opens at a time.
- **The D-pad moves through the grid.** On a device with buttons, reaching the game without
  touching the screen is the normal path and not the alternative: a card is highlighted, the
  D-pad moves it, the scrolling follows it and button 1 opens it.

**There is no eframe here, and that is on purpose.** Through winit — which is what eframe drives
— the controller gets lost twice: a `Gamepad`'s key codes become `Key::Unidentified`, which egui
throws away, and the stick axes are `MotionEvent`s, which winit only reads when they are touch.
Intercepting in Java doesn't solve it either: `NativeActivity` delivers input through the native
queue and **does not go through the view hierarchy**, so a `dispatchKeyEvent` of ours would never
be called — this was measured. Driving `android-activity` directly, what arrives is what Android
sent: `Keycode::ButtonA`, `Axis::X`, the triggers and "back".

The button map is the same one desktop gives a modern controller: south on `b1`, east on `b2`,
west on `b3`, north on `b4`, the upper triggers on `zl`/`zr`, Start on `back` — the Zeebo
controller has no Start, and HOME takes its place.

### 3D on the graphics card

The GL context in `screen.rs` is the same one the session receives, so desktop's "render 3D on the
graphics card" applies here. **And it is what brings four other options to life:** in the software
rasterizer, `set_aspect_ratio`, `set_scale`, `set_antialias` and the anisotropic one are functions
with an empty body — only the graphics card knows how to do that. With 3D on the CPU, changing the
widescreen aspect ratio or the internal resolution did absolutely nothing, and that is why the
four stay grayed out while the graphics card is off.

**The context and the surface have different lifetimes, and `screen.rs` separates them for that
reason.** The Android window is born and dies on every app switch; the context cannot go with it,
because the rasterizer keeps textures, buffers and programs in it. Destroying it when going to the
background would leave the session with names of objects that no longer exist, and the first draw
on return would be a crash inside the driver. So the `Gpu` is born with the first window and lives
until the end; the `Screen` is only the surface, rebuilt with each window.

### The ROM folder

It is chosen in the app and saved in the same settings as desktop. The default is
`/sdcard/zeebo/roms`, and reading it requires "all files access", which the app itself asks for.
Whoever prefers not to grant anything uses the private directory, which always works:

```
adb push game.mod /sdcard/Android/data/io.github.zeebxteam.zeebx/files/roms/
```

The folder picker is ours, on top of `std::fs`, and not the system's: `ACTION_OPEN_DOCUMENT_TREE`
returns a `content://`, and the core's loader opens a file path.

## Building

```
./frontends/android/build.sh          # just the .so
./frontends/android/build.sh --apk    # the .so and the APK
```

The APK comes out in `apk/app/build/outputs/apk/debug/`, with two names: Gradle's
`app-debug.apk`, which is what `installDebug` and Android Studio use, and a copy as
`zeebx-android-<abi>.apk`, which is the name the release uses — next to a
`zeebx-standalone-qt-linux-x86_64.deb`, an `app-debug.apk` doesn't say which program it belongs
to.

This same script is the one that runs in CI, with no second copy of the paths inside the
workflow: it accepts the `ANDROID_SDK_ROOT` that GitHub's runners export and whatever `gradle` is
on the path, falling back to `$HOME` when there is neither. See
[`release.yml`](../../.github/workflows/release.yml).

The script expects the toolchain in `$HOME`, without `sudo`:

| What | Where | From where |
|---|---|---|
| rustup + `aarch64-linux-android` target | `~/.cargo` | `https://sh.rustup.rs` |
| JDK 17 | `~/Android/jdk` | Adoptium |
| SDK (platform 35, build-tools 35) and NDK r27.3 | `~/Android/sdk` | `sdkmanager` |
| Gradle 8.11 | `~/Android/gradle` | `services.gradle.org` |
| `cargo-ndk` | `~/.cargo/bin` | `cargo install cargo-ndk` |

The paths can be adjusted through the environment: `ANDROID_SDK_HOME`, `ANDROID_NDK_VERSION`,
`JAVA_HOME`, `GRADLE`, `ZEEBX_ANDROID_ABI`.

## Two stones in the road, and why they exist

**CMake couldn't find the NDK.** `dynarmic` compiles C++ through CMake, and `cmake-rs` builds the
command line by itself: it puts `CMAKE_SYSTEM_NAME=Android` and
`--target=aarch64-linux-android35` in the flags, but doesn't tell the NDK toolchain the ABI.
Without it the NDK assumes `armeabi-v7a` and adds `-march=armv7-a`, which clang refuses alongside
an aarch64 target — and the build dies in the compiler test, before any of our code. The
`ndk-toolchain.cmake` here pins the ABI and the API before reading the NDK toolchain.

## What isn't here yet

- **Sound.** `cpal` has an AAudio backend and already compiles, but nobody hooks it up yet.
- **Remappable controls.** The button map is fixed in `host_input.rs`. `input::bindings`, which
  is what desktop uses to let the user remap, is not hooked up here yet.
- **Sensors and Wii Remote.** They are still behind `cfg(target_os = "linux")`.
