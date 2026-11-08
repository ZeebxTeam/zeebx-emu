# Usage

The Qt interface is `zeebx-qt`. The command-line binary is `zeebx`: with no subcommand it prints usage and exits.

```bash
cargo run --release -p zeebx-cli -- run "roms/Quake.zip" --window
```

ZIP files are extracted to a cache, and the correct `.mod` is selected automatically. `--seconds=N` sets how many seconds of virtual time to emulate without a window; with a window, the game runs until you close it.

## Keyboard controls

| Key | Zeebo control |
|---|---|
| Arrow keys | D-pad |
| Z | Button 1 |
| X or Space | Button 2 |
| C | Button 3 |
| V | Button 4 |
| Q, W | ZL and ZR |
| F | Left stick click (L3) |
| G | Right stick click (R3) |
| H, Backspace, Enter | HOME |

F and G click the sticks (L3 and R3). They do not move them. The default keyboard map has no stick axes.

## Debug report

`run` reports where the game stopped, what it requested that the emulator does not yet provide, and log messages left in the binary by its developers through `DBGPRINTF` and ARM semihosting. This report feeds the project's backlog. Debug options are documented in [ARCHITECTURE.md](../../ARCHITECTURE.md).

## With your own frontend

To integrate the emulator into your own frontend, see [HEADLESS.md](HEADLESS.md).
