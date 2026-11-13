# 10 — The interface

The shared UI state lives in `src/ui/` and is used by the core and multiple frontends. The
library, settings, saves, localization, navigation, running-game state, screenshots and themes
are not desktop-only. The GL painter is gated by the `gl` feature; the updater, Discord presence,
host input adapter and raw window module are gated by the `desktop` feature. The `desktop` feature
implies `gpu`, which in turn implies `gl`; the Android frontend uses the shared UI state without
enabling the desktop feature.

The desktop interface is Qt 6 / Qt Quick, with [cxx-qt](https://github.com/KDAB/cxx-qt) as the bridge to Rust:
`frontends/qt-standalone`, binary `zeebx-qt`. Separate system windows, not tabs: the library is
the main window, and the game, the settings, the saves and the log each open in their own. The
rules of each screen live in the core (`src/ui/`), and the window only displays them; the
migration, from the egui interface that existed up to 0.4.2, is in [21](21-qt-migration.md).

## Library

`ui/library.rs` scans the ROM folder looking for `.mod` and `.zip` up to four levels deep, with a
cap of 2000 games so that a folder picked by accident does not freeze the interface. The scan
sorts, because `read_dir` does not promise an order and a list that changes positions between two
launches is confusing.

Two layouts coexist: the console installs as `<Title>/mod/<id>/<name>.mod` with the `.mif` in a
sibling `mif/`; the SDK samples put the two side by side.

The title comes from the **folder**, not the file: in the console layout the file name is a dull
numeric identifier.

### Each card's image

In this order: an image with the same name next to the game (`Quake.png` next to `Quake.zip`), the
largest icon in the `.mif`, and finally the built-in logo.

The order exists because **there is no cover inside the ROMs** — the largest icon of the older
titles is 65×42, which is a menu icon. The image next to the game is the way out for anyone who
wants real art.

The card **scales up without interpolating** when the image is smaller than the frame: a 26-pixel
icon appears magnified four times, and interpolating would turn it into a blur — the square block
is what the console showed. A large image comes in already scaled down, and then interpolation is
what prevents the jaggies.

### The Z-Wheel lends its covers

The ROMs carry no covers, but the Z-Wheel package carries those of 59 games, in
`mod/274755/assets/games/<game_id>/` (`boxartlg.jpg`, `boxart.bmp`, `rating.jpg` and the
description in three languages). `ui/zwheel_metadata.rs` reads them straight from the `.zip` or
the folder, without extracting:

- the `tt_game_info` database (SQLite, copied to the cache to be opened) links `game_id` to the
  applet's `class_id` — the same ClassID the library uses as its key — and to the cover folder;
  `TITLETEXT` gives the official name per language (the `lang_id` is BREW's letter code in
  little-endian, `"pt  "`);
- the `asset_cache` database links the stage scenes (`assets/stage_slides/<dslid>/`, type 5 in the
  `ASSETS` table) to an `owner`, the `game_id`; the scene carries the logo for the top reel as the
  texture `slidebanner.qxt` (QXEngine: 40-byte header, format `0x0c` = colour-only ATITC). 12
  games have a logo.

The cover next to the game still outranks the Z-Wheel's, and the Z-Wheel's outranks the `.mif`
icon. The description drops the notice about the store that closed in 2011, which opens almost all
of them between `**`.

The Z-Wheel is configured in the General tab (file, folder or "Detect", which checks the `.mif`
ClassID in folders with "wheel" or "tectoy" in the name), leaves the list and opens from the
**▶ Z-Wheel** button on the top bar; a game opened from the library that exits on its own closes
its window.

### Grid and slider

`LibraryGrid.qml` and `LibrarySlider.qml`. In the grid, upright cards in the proportions of the
boxes; in the slider, one game at a time in the Z-Wheel style: the logo reel spinning like a
cylinder, the name, the boxes sliding (the position chases the selection with a damped spring,
and the list wraps around) and the rating with the description below.

Both modes are driven by the controller: D-pad, stick and arrow keys select, repeating when held;
button 1, Start, Enter or space open; HOME opens the Z-Wheel. Keyboard, D-pad and stick become a
single state before the press is detected, so an arrow also mapped on the controller does not
move twice. The library does not listen while any window is on top of it — the game, the
settings, the saves, a dialog —, and since the controller generates no Qt event, a 33 ms timer
reads it (`ui::navigation`).

### Search

The field on the top bar filters the list in both modes. A game stays if its title has all the
typed words, in any order. Case, accents and punctuation do not count, and dotted acronyms are
joined: "cnk" finds "C.N.K.", "boia" finds "Bóia". Changing the search moves the selection to the
first result, without the slider's spring sweeping across the list. With the field focused,
arrows and space belong to the text and the keyboard does not navigate; the controller keeps
navigating. Enter in the field opens the selected game. Ctrl+F goes to the field, and Esc and ✕
clear it and give the arrows back to the list.

## Settings

Four tabs: general, controls, graphics and audio. Everything is saved as JSON in the system's
configuration directory, written by hand (`config::config_dir`) because there are three known
rules and each fits in one line.

Every field is `#[serde(default)]`: **a missing, truncated or newer-version file has to let the
program open, not prevent it.** In the worst case, the default comes back.

Every tab scrolls: the graphics one grew beyond the window's height. The main window and the game
window open windowed, maximized or fullscreen (maximized by default), and `F11` or `Alt+Enter`
toggles fullscreen at any time. The default scaling is fit to window: in a maximized window,
integer pixels left a wide border for nothing.

In the controls tab, a port with a Boomerang swaps the controller drawing for the Boomerang
preview rotating with the Wii Remote, the calibrate button and the option for the in-game
calibration notice — see [20](20-boomerang-and-wii-remote.md).

## Languages

`ui/i18n.rs`. Four languages built into the binary (en, es, es-MX, pt-BR) and the option of adding
others as loose JSON — next to the executable, for a portable copy, or in the configuration
directory, for anyone who wants to translate without touching the installation. A file with the
code of a built-in one replaces it, which is how to fix a translation without waiting for a new
version.

A test compares the keys of the built-in languages: **a key that exists in only one of them is
text that shows up in English in the middle of Portuguese.**

## The controller's visual map

`input/padview.rs`. There are **two** pieces:

| | |
|---|---|
| `assets/controller.png` | the art — it only needs to look good |
| `assets/controller-map.svg` | the map: an invisible SVG of the same size, in which each shape's `id` is a button's name |

Separating the two lets the controller be redrawn without touching code, and spares the art from
having to carry an `id` on every stroke. A button the art does not draw simply has no shape in the
map: it does not light up, and stays configurable through the list.

The map is rasterized once, each shape becoming a **cropped silhouette**. From it come the two
things that matter: lighting up the right button and knowing which one the cursor is over.
Hit-testing the click by the silhouette, and not by a rectangular box, is what makes a cross
D-pad and a round button respond only where there is a button.

The cropping is not about saving disk: each silhouette becomes a texture on the graphics card,
and storing them at the size of the whole controller would cost tens of megabytes to draw buttons
a few pixels wide.

The art's dark background is cut out **from the edge** (a flood fill from the margins), not by
colour: the black outline inside the drawing is as dark as the background, and erasing it by
colour would leave the controller with no outline at all.

The screen also shows the **four axes live**. A stick that does not reach the emulator shows up
there as a stubborn zero — that is what tells "not mapped" apart from "mapped to the wrong axis".

## Window icon

The logo (`assets/zeebx.png`) is scaled down to 256 pixels per side at startup, by
`QImage::scaled` with smooth filtering — it is over a thousand, and an icon that size is megabytes
of texture to draw something that never exceeds a few pixels in the bar.

The reduction has to average the colour **premultiplied** by alpha. Without that, a transparent
pixel enters the sum with the colour it stores — usually black — and the edge of a logo with a
transparent background gets a dark halo.

A logo that fails to open does not stop the window from opening: the icon is simply not set. The
icon applies to all windows (`apply_icon`, in `frontends/qt-standalone/src/qt/cpp/gl_qt.cpp`).

**On Wayland there is no pixel icon**: the compositor matches `setDesktopFileName("zeebx")` with
the installed `zeebx.desktop` and takes the icon from there. **On macOS**, the icon comes from the
`.app` bundle.

## Game window title

The session only knows the extraction folder, which carries the package's fingerprint
(`Zeebo-Extreme-Boia-Cross-21503726-1788761080`). The window title comes from the library by
ClassID — the Z-Wheel's name when it describes the game, the package's when it does not —, and
only when that is missing the folder, without the two trailing numbers
(`library::strip_fingerprint`).

## Startup notice

At launch, a modal says the emulator is in development and asks the player to configure the
controller before playing. "Don't show again" stores the version in `aviso_dispensado_na_versao`;
a new version shows the notice again.

## Updates

`ui/updater.rs`. At startup (an option in the settings, on by default), a thread asks GitHub for
the `releases/latest` of `ZeebxTeam/zeebx-emu`. The tag counts with or without `v`; drafts and
pre-releases are left out. If there is a version greater than the binary's, a modal offers to open
the release page — after the startup notice, so the two do not stack. "Check now" repeats the
question from the settings. A 404 means "up to date": it is what comes back while there is no
release.

## Discord Rich Presence

`ui/discord.rs`. Discord talks over a local socket that can vanish at any moment, so the
connection lives in a thread: the interface says what to show every frame, and only a change
becomes a request. The thread connects, resends and retries every 15 s while Discord is closed.

| Where | Top line | Large image | Small icon |
|---|---|---|---|
| library | "In the menu" | `zeebx` | — |
| Z-Wheel | "In the Z-Wheel" | `zeebx` | — |
| game | "Playing {name}" | the cover | `zeebx` |

The name that appears on the profile is that of the Zeebx **application** in the Developer
Portal, whose ID is in `discord::APPLICATION_ID`. The images do not go through the socket either:
Discord only shows a key registered in the application or an `https` URL. The export and the
cover address exist in the code but are **out of the settings** for now. The export writes
`zeebx.png` and `jogo_<clsid>.png` (512×512, the whole cover centred) with the name equal to the
key, ready for the Art Assets. Whoever publishes the covers can give the address with `{chave}` or
`{clsid}`.

## Command line

The interface is `zeebx-qt` (`zeebx-qt <game>` opens the game directly). The command line is
`zeebx`, in `frontends/cli`, without Qt — it builds on a machine or in a container without it:

```bash
zeebx info game.mod                     # what the module declares
zeebx run game.mod --seconds=6 --trace  # runs without a window and summarizes the calls
```

| Option | What for |
|---|---|
| `--seconds=N` / `--frames=N` | how far to go |
| `--trace[=filter]` | the latest calls, and the per-method summary at the end |
| `--dump-gl=DIR` | one file per OpenGL frame |
| `--dump-audio=A.wav` | the audio mix |
| `--keys=SCRIPT` | input without a window |
| `--window` | the old `minifb` window |
| `zeebx session <zip> --boomerang --motion=ms:x:y:z` | the window session with a scripted Boomerang |
| `zeebx wiimote` | the live Wii Remote |
