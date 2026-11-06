# Zeebx — research documentation for a Zeebo emulator

A survey of what is publicly known about the Zeebo (TecToy, 2009) and the Qualcomm BREW / MSM7201A platform it uses, with relevant material mirrored locally in [`vendor/`](../../vendor/).

## Short conclusion

Emulating the Zeebo is **not console emulation**; it is **reimplementing Qualcomm BREW 4.0.2**. The game is an ARM binary (`.mod`) that does not talk to hardware: it calls COM-like interfaces (`IShell`, `IDisplay`, `IGraphics`, `IHID`, `ISound`, `IFileMgr`, OpenGL ES through the BREW extension). This favors **HLE**: run the game's ARM code in an interpreter/JIT and intercept API calls, translating them for the host. This is exactly the approach taken by Infuse, by Tuxality.

The public material is surprisingly good: the official Zeebo SDK, Developer Guide, BREW OEM API reference, complete NAND dumps (bootloaders + AMSS + APPS), RAM/register dumps, and research into BREW interface vtables in the real firmware.

This `0x` series is **research**: what is known about the console and platform. How the emulator is written is documented in [`implementation/`](implementation/README.md).

## Index

| Document | Contents |
|---|---|
| [01-hardware.md](01-hardware.md) | Console specifications, MSM7201A, Adreno 130, memory, JTAG |
| [02-plataforma-brew.md](02-plataforma-brew.md) | BREW 4.0.2, AEE, execution model, relevant APIs |
| [03-formatos-e-seguranca.md](03-formatos-e-seguranca.md) | `.mod`, `.mif`, `.bar`, `.sig`, `.bid`, GGZ, signatures, `61u.key` |
| [04-estado-da-arte.md](04-estado-da-arte.md) | Infuse, OpenZeebo, homebrew ports, ROM sets, community |
| [05-estrategia-de-emulacao.md](05-estrategia-de-emulacao.md) | Proposed architecture, phases, risks, technical decisions |
| [06-fontes.md](06-fontes.md) | Sources and links |
| [07-inventario-vendor.md](07-inventario-vendor.md) | What was downloaded and where it lives |
| [implementation/](implementation/README.md) | **How the emulator works internally** — subsystem by subsystem, with the decisions and the reasons behind them |

## Test material

| Directory | Contents |
|---|---|
| `roms/` | Test games (not distributed) |
| `vendor/` | Public third-party reference material; not versioned |

The sources are listed in [06-fontes.md](06-fontes.md). The `vendor/` directory is large and locally mirrored; it is intentionally excluded from Git.
