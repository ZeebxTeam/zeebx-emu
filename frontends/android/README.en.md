# Zeebx on Android

The core is the same. What changes is the frontend that runs it.

Emulation — BREW, CPU, rasterizer, and loader — lives in the `zeebx` library. It is used by frontends (desktop Qt, command line, headless, Libretro, iOS, and this Android app), as well as tests. No emulation code needs to know which frontend is running.

This directory is `frontends/android/` because it is **one** frontend, not *the* Android frontend. If another target is added — a console or browser, for example — it belongs alongside this one, while the core stays where it is.

## Platform split

Code that depends on a desktop window is behind `cfg(not(target_os = "android"))`:

| Shared across platforms | Desktop only |
|---|---|
| `audio`, `brew`, `cpu`, `input`, `loader`, `machine`, `video`, `session` | `ui::window`, `ui::atualizacao`, `ui::discord` |

The Android build uses the Oboe audio backend through `cpal`. Build requirements and commands are in [`LEIAME.md`](LEIAME.md).
