# Zeebx without an interface

The core is the same. What changes is that there is no interface at all on top of it.

It exists for those who already have a frontend. A program that draws the console's shell, a game
shelf, an arcade cabinet — none of them wants Zeebx's library showing up over the screen they
built themselves. This binary opens the game it is given, obeys a `config.ini` and shows nothing
else.

```
zeebx-headless [OPTIONS] GAME
```

`GAME` is required: a `.mod`, or the `.zip` that holds it. **There is no default for it**, and
that is on purpose — this binary is called by another program, which knows what it wants to open,
and guessing something when nobody said anything would be opening what nobody asked for. To start
with the Z-Wheel, pass the Z-Wheel: it is a game like any other.

The file's `[system] z_wheel` is something else: it is where the console **returns** to when a
game exits on its own, as on the real device. Without it, a game that exits closes the emulator —
which is what an outside frontend usually wants, since its own screen is what comes back.

| Option | What it does |
|---|---|
| `--config=PATH` | the file to use; without it, it looks for a `config.ini` next to the executable and then in the system configuration folder |
| `--controllers` | lists the controllers the system sees, **with the names `config.ini` expects** |
| `--example` | writes a commented `config.ini` with the factory values to standard output |
| `--help`, `--version` | what you'd expect from them |

There are no more options, and that is on purpose: the rest is in the file. A frontend that needs
two profiles uses two files and two `--config=`.

## The configuration

The keys and values are in **English**, like the commands, and so are the file's comments. The
idea is that whoever writes a frontend reads the configuration without needing another language,
and whoever edits the file by hand has the explanation in the project's language.

Graphics, audio and controller mapping **are not a new model**: they are the same fields the
desktop interface saves in `settings.json`, arriving through INI because that is what gets edited
by hand. The key and button names are the same on both sides, so a mapping copied from the
desktop controls screen works here without translation — and that screen is still the easiest
way to find out what a button is called.

Everything is optional. A missing key is worth the default, and a misspelled key **shows up on
standard error** instead of silently falling back to the default — which is the only way whoever
edited the file finds out that `internal_resolutio` is not `internal_resolution`.

On the first run, when there is no file at all, it writes a factory one in the system
configuration folder and says where:

```
config: none found; wrote one at /home/you/.config/zeebx/config.ini
```

Never on top of one that exists — not even when the file is there and just didn't open, because
a `config.ini` full of settings is the work of whoever wrote it. With `--config=PATH` pointing to
something that doesn't exist yet, it is at **that** path that it is created: the person named the
file they want.

The file goes to the configuration folder and not next to the executable, which is the first
place searched: there the installed copy usually has no write permission, and one that did would
be a file inside the installation, which disappears on the next update.

It comes out **complete**, with everything written and nothing to guess — including the thirteen
buttons of each port. The ports block is generated from the core's `Controls::default()`, not
written by hand, and each line says what it would already be worth if it didn't exist; a test
pins this equality, because an example written by hand quietly goes stale the day a default
changes, and a file that lies about the default is worse than a file without the line.
`--example` prints the same text to standard output, for whoever wants to look at it without
creating anything.

Nothing is guessed. The core knows how to look for a Z-Wheel by itself — through the ROM folder
and, failing that, by name inside the home folder —, and that makes sense in the interface, where
it is a friendly guess for someone who hasn't configured anything yet. Not here: headless uses
only what the file points to.

The [`config.ini`](config.ini) in this directory is the example, commented line by line, and it
is what `--example` prints. A test checks that it is read without any warning: an example that
complains teaches the wrong thing.

## The two video modes

**`mode = window`** (or `fullscreen`) opens a window with the game and nothing else. The OpenGL
context is the window's, and that is what the session uses to rasterize 3D when `gpu_rasterizer`
is on: the frame the graphics card just filled goes to the screen without going back to the CPU.
`Esc` closes it, and that is not mappable — whoever ran it needs a way out that doesn't depend on
the file being right.

**`mode = none`** draws nothing. The frames go out through the `[dump]` section: to standard
output (`zeebx-headless game.mod | your-frontend`), to a file, to a FIFO — and then nothing goes
through the disk — or as one `.png` per frame. A frame is only written when it changes. Without a
window there is no keyboard, because there is no focus; whoever plays uses a controller, which
the system delivers without needing one. 3D on the graphics card is still possible: the core
opens an offscreen context, the same one `zeebx run` uses for measuring.

In the raw formats the frame is always the console's, 640×480, from beginning to end. It is the
only possible choice: the stream has no header, and a frame that changed size midway would make
whoever counts bytes on the other side lose step forever. That is why `internal_resolution` and
the widescreen aspect ratio don't reach the raw dump — they apply in the window. The `.png` is the
exception, because each file states its own size.

## On the three systems

Linux, Windows and macOS. `release.yml` builds a `zeebx-headless-<system>.zip` for each one, with
the binary, the commented `config.ini` and this file — `config.ini` goes along because the first
place the emulator looks for it is **next to the executable**, which is what makes a portable
copy work without touching the machine.

One difference worth knowing: in `none` mode with `gpu_rasterizer` on, 3D needs an OpenGL context
without a window, and that is EGL. On Linux it is always there; on Windows it comes with the
driver that installs it — NVIDIA's does — or with an ANGLE (`libEGL.dll` and `libGLESv2.dll`)
next to the executable; on macOS it doesn't exist. Where it is missing, the emulator says why and
carries on with the software rasterizer. **With a window this doesn't apply**: there the context
is the window's own, and all three systems have it.

## The files

| File | What it is |
|---|---|
| `src/main.rs` | the arguments, the windowless mode and where each mode comes in |
| `src/ini.rs` | the INI reader, written by hand because it is three rules |
| `src/config.rs` | `config.ini` turning into the `Settings` the core already understands |
| `src/console.rs` | the running console: the session, the input, the clock and the Z-Wheel chain — what the two modes have in common |
| `src/window.rs` | the raw window over winit and glutin, and its loop |
| `src/host_input.rs` | the winit keyboard and the host controllers turning into the ports' state |
| `src/dump.rs` | the frame going out without a window |

## Why not eframe

For the same reason the Android frontend doesn't use it: eframe drives an interface loop, and
here there is no interface — there is a frame. The loop is ours, on top of winit, and that is what
lets us treat input as the system delivers it and own the window's lifecycle.

This is about the loop, **not** about the build size. The `zeebx` this package depends on is the
whole library, and outside Android it brings `rfd`, Discord and `ureq` on its own: building
headless builds those along with it. Really slimming it down would mean putting the desktop
screens behind a core feature, which hasn't been done yet.

`egui` is still in the dependency list, but only for the types: `egui::Key` is the canonical name
of a key in `settings.json`, and `ViewportInPixels` is what the core's painter (`ui::gpu`)
receives. Nothing is drawn with it, and, since it is already in the tree because of the core, it
adds no compilation at all.
