# Zeebx on PlayStation Vita

The core is shared; the frontend handles drawing, audio, and input.

This application runs without RetroArch. The RetroArch core remains in [`frontends/vita`](../vita/compilar.sh) as a static `.a` library. This standalone app builds an `eboot.bin` into a VPK; the game library, display, and audio run in this process.

## Requirements and display

A Vita running homebrew (HENkaku or equivalent) is required. The VPK cannot be installed on a stock device.

Frames are drawn pixel-for-pixel and centered. Scaling 640×480 to fill the screen duplicated one column and skipped another, while half-resolution 3D looked blocky. If a frame misses its refresh interval, subsequent frames are skipped until the clock catches up. At startup the app requests 444 MHz ARM and a 222 MHz bus; the accepted values appear in the log. Performance has not been measured on the device. The CPU backend is the interpreter because Dynarmic does not emit ARMv7 code. Music uses the soundfont.

## Measurements still needed on hardware

These limits apply to both this app and the RetroArch core, which share the same core library.

- **Game memory.** The Vita game heap is 48 MiB, compared with 64 MiB on desktop: at 64 MiB, vitaGL could not find the frame's temporary block. The emulator's internal region (`INTERNAL_BASE` in `src/loader/mod.rs`) adds 16 MiB, and newlib reserves the whole block instead of overcommitting as desktop systems do. It has not been measured whether 48 + 16 MiB leaves enough room for the frame or whether Quake runs with a 48 MiB heap.
- **Save states are not cross-platform compatible.** A state saved with a different heap size cannot be loaded on another platform.

Build instructions are in [`LEIAME.md`](LEIAME.md).
