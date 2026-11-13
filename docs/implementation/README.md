# Implementation

How Zeebx is built on the inside. The `0x` series at the root of [`docs/`](../) is **research** —
what is known about the console and the platform; this series is **implementation** — what is
written in the code and why.

The rule that holds across all these documents: they explain **decisions**, they do not repeat
the code. Where a number looks arbitrary, the text says where it came from; where a choice had an
alternative, the text says which one was discarded and why. What is obvious from reading the
function signature is not here.

| Doc | Contents |
|---|---|
| [01-architecture.md](01-architecture.md) | The modules, the main loop and how a game call turns into work on the host |
| [02-cpu-and-memory.md](02-cpu-and-memory.md) | The ARM core, the guest memory map, the heap and the objects |
| [03-api-dispatch.md](03-api-dispatch.md) | Vtables, the trampoline, the interfaces and how a new method gets in |
| [04-time.md](04-time.md) | Virtual clock, timers, callbacks, threads and busy waiting |
| [05-video-2d.md](05-video-2d.md) | `IDisplay`, `IBitmap`, the DIB, clipping and the cost of the surface |
| [06-video-3d.md](06-video-3d.md) | EGL, OpenGL ES, the software rasterizer and the texture formats |
| [07-audio.md](07-audio.md) | RIFF/WAVE, IMA ADPCM, the mixer and `IMedia`/`ISound` |
| [08-files-and-resources.md](08-files-and-resources.md) | `.mod`, `.mif`, `.bar`, the virtual file system and the `.zip`s |
| [09-input.md](09-input.md) | The console's controller, `IHID` and the configurable mapping |
| [10-interface.md](10-interface.md) | Shared UI state and desktop-only window modules, plus libraries, settings, languages and controller visualization |
| [11-compatibility.md](11-compatibility.md) | What runs, what does not, and what is missing in each case |
| [12-rasterizer-and-parallelism.md](12-rasterizer-and-parallelism.md) | Why splitting the work per draw call did not work, and what did |
| [13-unknown-classes.md](13-unknown-classes.md) | How to find out which interface a class without a header is: the probe |
| [14-z-wheel-and-efs2.md](14-z-wheel-and-efs2.md) | Where the console's store stopped, and the missing link in the NAND reader |
| [15-whats-missing-from-the-nand.md](15-whats-missing-from-the-nand.md) | Class by class, what still has to come out of the dump |
| [16-network-and-the-bridge.md](16-network-and-the-bridge.md) | HTTP, AES, and the per-module bridge that Zeeboids required |
| [17-tests.md](17-tests.md) | How to check what we say works, and the per-test ROM scan |
| [18-the-z-wheel-carousel.md](18-the-z-wheel-carousel.md) | From the black screen to drawing in the Z-Wheel: the steps on the guest side, for any emulator |
| [19-arm-jit-and-renderers.md](19-arm-jit-and-renderers.md) | Dynarmic, the ARM renderers' bottleneck and how to validate without changing BREW time |
| [20-boomerang-and-wii-remote.md](20-boomerang-and-wii-remote.md) | The motion controller: the receiver's report read from the games' code, and the Wii Remote in its place |
| [21-qt-migration.md](21-qt-migration.md) | Plan for replacing egui with Qt Quick through cxx-qt, and what the core library borrows from the window |
| [22-screenshots.md](22-screenshots.md) | The game window's F9: which frame comes out, at what size, where it is saved, and the RetroAchievements yardstick |
| [24-speed.md](24-speed.md) | Frame limit, frameskip, fast-forward, turbo and rewind: the game's pace against the wall clock |
| [25-desktop-save-states.md](25-desktop-save-states.md) | The ten slots per game, saving and loading with the GPU, undo, and the shortcuts with the Mac's ⌘ |
| [26-themes.md](26-themes.md) | The palettes over Fusion, each theme's file, how the color reaches the windows and the contrast math |
| [27-window-frame.md](27-window-frame.md) | The menu bar, the toolbar with the Lucide icons and the status bar around the library |
| [28-game-in-the-main-window.md](28-game-in-the-main-window.md) | The game in place of the library, the separate window as an option, and one stage for each |
| [29-settings-with-sidebar.md](29-settings-with-sidebar.md) | The categories in a column with icons, and each option's explanation at the bottom |
| [30-redesign-tweaks.md](30-redesign-tweaks.md) | The Z-Wheel as home, the game down to the bottom of the window with the metrics in the corner, the Z-Pad and the Dragon on one button, and the question before closing the game |

## How to measure

These measurements live in the repository precisely so nobody has to guess:

```bash
# throughput of the recompiled ARM core
cargo run --release -p zeebx-cli -- bench <game.mod>

# one game without a window, with the API call summary at the end
cargo run --release -p zeebx-cli -- run path/to/game.mod --seconds=6 --trace

# the whole library, only until the applet exists
ZEEBX_ROM=roms cargo test --release scan::tests::the_given_rom_opens -- --nocapture
```

The test commands are all in [17-tests.md](17-tests.md).

`--trace` accepts a filter (`--trace=Bitmap`), and the summary at the end counts **how many times
each method was called** — it is what pointed out Pac-Mania's two bottlenecks and the APIs that
were missing in nine games.
