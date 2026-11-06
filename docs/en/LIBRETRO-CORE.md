# Libretro core and muOS

The Libretro core is distributed with its matching `.info` file and can be installed in RetroArch. On muOS, the AArch64 core goes in `opt/muos/share/core`; the MIDI bank is optional. The playlist, DAT, and Zeebo cover art can be generated with tools in `ferramentas/`.

Build the core:

```bash
cargo build --release -p zeebx-libretro
```

The core does not link the desktop interface or host audio/controller libraries. See [BUILDING.md](BUILDING.md#build-targets) for details.

The compatibility list in [COMPATIBILITY.md](COMPATIBILITY.md) may not apply to Libretro. Please report problems you find.
