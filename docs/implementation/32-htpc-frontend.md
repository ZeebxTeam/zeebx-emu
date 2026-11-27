# 32 — HTPC frontend (`zeebx-htpc`)

A dedicated 10-foot, gamepad-first standalone binary for living room TVs, HTPCs, Steam Deck, and
Steam Big Picture / Gamescope.

## Why a separate binary sharing the same `qt-core` bridge

`zeebx-qt` is a desktop interface with a menu bar, toolbar, separate auxiliary windows
(`SettingsWindow`, `SavesWindow`, `LogWindow`) and OS file dialogs (`rfd`). Inside Steam Big
Picture and Gamescope, secondary windows and native file pickers lose focus or never appear.

Keeping `zeebx-htpc` as its own workspace binary (`frontends/htpc`, binary `zeebx-htpc`) while
extracting the Rust/C++ Qt Quick bridge into `frontends/qt-core` (`zeebx-qt-core`) gives three
properties:

1. **Zero bridge duplication.** Both `zeebx-qt` and `zeebx-htpc` share the same `GameScreen`,
   offscreen shared OpenGL context, `Library`, `SettingsModel`, `Language`, `Theme` and
   `image://zeebx/…` image provider (extended with `image://zeebx/slot/<serial>/<slot>` for live
   RGB565 save-state thumbnails).
2. **Shared profile on disk.** Both binaries read and write the same `settings.json`, ROM library,
   saves, save states and `temas/` color palettes. HTPC-only choices live under `Settings::htpc`
   (`#[serde(default)]`) so `zeebx-qt` preserves them untouched and `zeebx-htpc` never overwrites
   the desktop window geometry.
3. **Launcher-ready CLI.** `zeebx-htpc` opens the fullscreen library; `zeebx-htpc <rom>` jumps
   straight into fullscreen play and exits cleanly when the game closes, returning focus to Steam
   or a frontend launcher.

## Fixed shell vs. pluggable QML themes

`HtpcMain.qml` keeps the critical host responsibilities inside the binary: the fullscreen window,
zero-copy `GameScreen`, in-game `QuickMenu`, 100% gamepad `HtpcSettings`, in-app `FolderPicker`
(replacing `rfd`), and automatic fallback recovery.

The showcase itself is loaded through a `Loader` from either the built-in **`zwheel-hd`** theme
(`qml/themes/zwheel-hd/Main.qml`) or any valid folder under:

```text
<executable folder>/htpc-themes/<id>/theme.json
<config>/htpc-themes/<id>/theme.json
```

If an external community theme fails to parse or load, `HtpcMain.qml` falls back to `zwheel-hd`
and displays a toast notice so the user can never be locked out.

## Per-theme color configuration

Color settings are stored per HTPC theme under `htpc.theme_colors.<theme_id>` (`palette` and
optional `accent`). Each HTPC theme can use:

- its own palettes declared in `theme.json` (e.g. `zwheel-hd`'s built-in `zeebo_classic`),
- the global Zeebx color theme (`settings.theme`), or
- any color theme from `temas/*.json` (factory or user-provided),

plus an optional accent override (`#rrggbb`) with automatic contrast text calculation.

## Why HTPC navigation reads gamepads directly

`zeebx::ui::navigation` reads the mapped `Pad`s of enabled console ports, which is right for a
desktop library whose keyboard and ports are already configured. On a fresh HTPC or Steam Deck, a
controller must navigate every screen immediately — including picking the ROMs folder on first
launch — before any console port is configured. `frontends/htpc/src/htpc_input.rs` therefore
polls standard controller buttons directly across all connected gamepads and watches the
configurable Quick Menu combo (`L3 + R3`, `Start + Select`, or `Guide`) without colliding with
the Zeebo's `HOME` button.
