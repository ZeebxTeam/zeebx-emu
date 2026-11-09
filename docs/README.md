# Zeebx — research documentation for a Zeebo emulator

A survey of what exists today, publicly, about the Zeebo (TecToy, 2009) and about the Qualcomm
BREW / MSM7201A platform it uses — with the relevant material mirrored locally in `docs/vendor/`.
That directory is local only and is not versioned.

The original Portuguese versions of these documents are archived in [`pt-BR/`](pt-BR/).

## Short conclusion

Emulating the Zeebo **is not emulating a console**, it is **reimplementing Qualcomm BREW 4.0.2**.
The game is an ARM binary (`.mod`) that does not talk to hardware: it calls COM-like interfaces
(`IShell`, `IDisplay`, `IGraphics`, `IHID`, `ISound`, `IFileMgr`, OpenGL ES through a BREW
extension). That favors **HLE** — running the game's ARM code on an interpreter/JIT and
intercepting the API calls, translating them for the host. It is exactly the path of Tuxality's
Infuse.

The public material is surprisingly good: the official Zeebo SDK, the Developer Guide, the BREW
OEM API reference, complete NAND dumps (bootloaders + AMSS + APPS), RAM/register dumps and
research on the vtables of the BREW interfaces in the real firmware.

This `0x` series is **research**: what is known about the console and the platform. How the
emulator is written is in [`implementation/`](implementation/).

## Index

| Doc | Contents |
|---|---|
| [01-hardware.md](01-hardware.md) | Console specifications, MSM7201A, Adreno 130, memory, JTAG |
| [02-brew-platform.md](02-brew-platform.md) | BREW 4.0.2, AEE, execution model, the APIs that matter |
| [03-formats-and-security.md](03-formats-and-security.md) | `.mod`, `.mif`, `.bar`, `.sig`, `.bid`, GGZ, signing, `61u.key` |
| [04-state-of-the-art.md](04-state-of-the-art.md) | Infuse, OpenZeebo, homebrew ports, romsets, community |
| [05-emulation-strategy.md](05-emulation-strategy.md) | Proposed architecture, phases, risks, technical decisions |
| [06-sources.md](06-sources.md) | All the sources, with links |
| [07-vendor-inventory.md](07-vendor-inventory.md) | What was downloaded and where it is |
| [implementation/](implementation/) | **How the emulator is built inside** — module by module, with the decisions and the reason for each one |

## Test material

`roms/` holds the titles used in development (also outside git). Already in place:
**Z-Wheel** (App ID `274755`), the console's own shell/menu — see
[03-formats-and-security.md](03-formats-and-security.md) for its anatomy.

## Warning

`vendor/` contains third-party material (Qualcomm/Zeebo documentation that was confidential at
the source, firmware dumps, SDKs) mirrored for research and preservation. Do not redistribute it;
and do not commit that folder to a public repository — `.gitignore` already excludes it.
