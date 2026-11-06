# Usage

With no arguments, the emulator opens its interface. From the command line:

```bash
cargo run --release -- run "roms/Quake.zip" --window
```

ZIP files are extracted to a cache, and the correct `.mod` is selected automatically. `--seconds=N` sets how many seconds of virtual time to emulate without a window; with a window, the game runs until you close it.

## Keyboard controls

| Key | Zeebo control |
|---|---|
| Arrow keys | D-pad |
| Z, X, Space, C, V | Buttons 1, 2, 3, and 4 |
| Q, W | ZL and ZR |
| F, G | Left and right analog sticks |
| H, Backspace, Enter | HOME |

## Debug report

`run` reports where the game stopped, what it requested that the emulator does not yet provide, and log messages left in the binary by its developers through `DBGPRINTF` and ARM semihosting. This report feeds the project's backlog. Debug options are documented in [ARCHITECTURE.md](../../ARCHITECTURE.md).

## With your own frontend

To integrate the emulator into your own frontend, see [HEADLESS.md](HEADLESS.md).
