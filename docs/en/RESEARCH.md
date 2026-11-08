# Zeebx — research documentation for a Zeebo emulator

A survey of what is publicly known about the Zeebo (TecToy, 2009) and the Qualcomm BREW / MSM7201A platform it uses, with relevant material mirrored locally in `docs/vendor/`. That directory is local only and is not versioned.

## Short conclusion

Emulating the Zeebo is **not console emulation**; it is **reimplementing Qualcomm BREW 4.0.2**. The game is an ARM binary (`.mod`) that does not talk to hardware: it calls COM-like interfaces (`IShell`, `IDisplay`, `IGraphics`, `IHID`, `ISound`, `IFileMgr`, OpenGL ES through the BREW extension). This favors **HLE**: run the game's ARM code in an interpreter/JIT and intercept API calls, translating them for the host. This is exactly the approach taken by Infuse, by Tuxality.

The public material is surprisingly good: the official Zeebo SDK, Developer Guide, BREW OEM API reference, complete NAND dumps (bootloaders + AMSS + APPS), RAM/register dumps, and research into BREW interface vtables in the real firmware.

This `0x` series is **research**: what is known about the console and platform. How the emulator is written is documented in the [implementation guide](../implementacao/README.md) (Portuguese).

## Index

| Document | Contents |
|---|---|
| [01-hardware.md](01-hardware.md) | Console specifications, MSM7201A, Adreno 130, memory, JTAG |
| [02-brew-platform.md](02-brew-platform.md) | BREW 4.0.2, AEE, execution model, relevant APIs |
| [03-file-formats-and-security.md](03-file-formats-and-security.md) | `.mod`, `.mif`, `.bar`, `.sig`, `.bid`, GGZ, signatures, `61u.key` |
| [04-state-of-the-art.md](04-state-of-the-art.md) | Infuse, OpenZeebo, homebrew ports, ROM sets, community |
| [05-emulation-strategy.md](05-emulation-strategy.md) | Proposed architecture, phases, risks, technical decisions |
| [06-sources.md](06-sources.md) | Sources and links |
| [07-vendor-inventory.md](07-vendor-inventory.md) | What was downloaded and where it lives |
| [implementation guide](../implementacao/README.md) | **How the emulator works internally** — subsystem by subsystem, with the decisions and the reasons behind them (Portuguese) |

## Test material

| Directory | Contents |
|---|---|
| `roms/` | Test games (not distributed) |
| `docs/vendor/` | Public third-party reference material; local only, not versioned |

The sources are listed in [06-sources.md](06-sources.md). The `docs/vendor/` directory is large and locally mirrored; it is intentionally excluded from Git.
