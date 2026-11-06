# Headless frontend

This frontend runs a game without the desktop interface and reads its settings from `config.ini`. It is intended for programs that provide their own library, menu, or display shell.

Build with:

```bash
cargo build --release -p zeebx-headless
```

The game path is required. On first launch the program writes a fully commented configuration file and reports its location. See the [English usage guide](../../docs/en/HEADLESS.md) and the Portuguese [`LEIAME.md`](LEIAME.md) for configuration details.
