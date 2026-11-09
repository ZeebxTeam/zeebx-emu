# 04 — State of the art

## Infuse (Tuxality) — the only working emulator

- Repository: <https://github.com/Tuxality/Infuse> — **the repo is empty** (only `LICENSE`,
  `README.md` and `.gitignore`; 1 KB, last push 2024-03-31). The code **is not public**;
  GitHub serves as a showcase/issue tracker. The license GitHub reports is "Other (NOASSERTION)".
- Site/blog: <https://tuxality.net/projects/infuse_zeebo_emulator>
- Architecture as stated by the author:
  - **HLE**: a reimplementation of the BREW subsystem written from scratch, by clean reverse
    engineering;
  - CPU: **[dynarmic](https://github.com/merryhime/dynarmic)** (ARM JIT);
  - Graphics: **OpenGL ES 1.x**;
  - Audio: basic MIDI, PCM/ADPCM, MP3, mixing of multiple streams with host-independent
    resampling; waveOut (Windows), Core Audio (macOS), PulseAudio (Linux), Media Kit (Haiku)
    backends;
  - Input: HID, up to two gamepads;
  - Data input: `.mod` + `.mif` + `.sig`. **No BIOS needed.**
  - Platforms: Windows x86_64, Linux x86_64, macOS arm64, Steam Deck, Haiku, ARM (RG353v, exp.).
- Stated compatibility: **playable** — Double Dragon, Crash Nitro Kart 3D, Zeebo Family Pack.
  **In progress** — Quake II, Reckless Racing, the Asphalt series, Kingdom Hearts V-Cast.
- Open tool by the same author: [`ggzbrewtools`](https://github.com/Tuxality/ggzbrewtools) (C++17, GGZ asset unpacker).

The point: Infuse **proves that BREW HLE works**, and the A1 release (May 2024) is the reference
for correct behavior. But none of it is reusable as code.

## tripleoxygen / OpenZeebo — the open knowledge base

A Brazilian researcher who did the hardware/firmware work and published all of it:

- Full archive: <https://www.tripleoxygen.net/files/devices/zeebo/> (mirrored in `vendor/tripleoxygen/`)
- [`openzeebo`](https://github.com/tripleoxygen/openzeebo) — sample BREW apps (`intro`, `backup`)
  with Makefile, `.mif`, `.bar`, `.sig`; JTAG ARM code; `zloader` (alternative bootloader,
  fastboot protocol); `nand_util` (NAND read/write in Python); `qcserial.py`; `ggzupk.py`.
- [`zeebo_doom`](https://github.com/tripleoxygen/zeebo_doom) — a port of PrBoom 2.5.0 to the Zeebo.
  **The portability layer is the best living documentation of "which BREW API a real game uses".**
- [`zeeutils`](https://github.com/tripleoxygen/zeeutils) — a BREW configuration utility on the console.
- [`kernel_zeebo`](https://github.com/tripleoxygen/kernel_zeebo) — a Linux kernel tree for the Zeebo's MSM
  (an attempt to run Linux; useful as a reference for MSM7201A registers/peripherals).

## Software preservation

Public collections on the Internet Archive (use them as a source of test material):

- <https://archive.org/details/zeebo-romset-and-devtools> — ~945 MB, 68 games + dev tools
- <https://archive.org/details/zeebo-game-app-compilation-open-zeebo>
- <https://archive.org/details/zeebo-games-app-compilation>

The Zeebo's commercial library has ~57–68 titles. Since the store was digital and TecToy's servers
went offline, what exists today came from dumps of physical consoles made by the community
(the Zeebo Club group, GBAtemp, OpenZeebo).

## Community / useful threads

- GBAtemp — [Why is not there a Zeebo Emulator for PC?](https://gbatemp.net/threads/why-is-not-there-a-zeebo-emulator-for-pc.529860/)
- GBAtemp — [Jailbreak / 61u.key](https://gbatemp.net/threads/im-trying-to-figure-out-a-more-practical-and-easy-way-to-unlock-jailbreak-zeebo-consoles.653809/)
- GBAtemp — [Every game ever made for the Zeebo](https://gbatemp.net/threads/every-game-ever-made-for-the-zeebo.635635/)
- ResetEra — [Finally Zeebo emulation starts to get somewhere](https://www.resetera.com/threads/finally-zeebo-emulation-starts-to-get-somewhere-infuse-brew-emulator.749431/)
