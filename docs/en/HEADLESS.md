# With your own frontend

If you already have a frontend — a console shell, game shelf, or arcade cabinet — you may want a binary without Zeebx's interface. The headless binary is configured through `config.ini`:

```bash
cargo build --release -p zeebx-headless
./target/release/zeebx-headless "roms/Quake.zip"
```

A game is required and there is no default: another program calls this binary and decides what to open. On first run it writes a complete, commented `config.ini` and prints its location. Options and keys are in English, while comments in the generated file are in Portuguese.

The binary opens a window containing only the game, or no window and sends frames through a pipe for your program to draw. Graphics, audio, and controls use the same settings as the desktop interface, represented in INI form. See [`frontends/headless/LEIAME.md`](../../frontends/headless/LEIAME.md).

Each release includes `zeebx-headless-<system>.zip`, with the binary, `config.ini`, and readme. See [INSTALLATION.md](INSTALLATION.md#which-file-to-download).
