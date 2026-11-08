# 01 — Zeebo hardware

Primary source: `../vendor/tripleoxygen/doc/ZeeboDeveloperGuide0.97.pdf` (Zeebo Inc., 2009), section 2.1 and chapter 3. Supplemented by dumps and OpenZeebo research. Mirror paths in this page are under `docs/vendor/` (`../vendor/` from this file). That directory is local only and is not versioned.

## Specifications (summarized from the Developer Guide)

| Item | Value |
|---|---|
| Chipset | **Qualcomm MSM7201A** + RTR6280 (RF) + PM7540 (PMIC) |
| Application CPU | **ARM11 (ARMv6, ARM1136)** @ 528 MHz + **QDSP-5** |
| Modem CPU | ARM9 + QDSP-4 (integrated into the MSM7201A) |
| GPU | **Adreno 130** (ATI/AMD Imageon Z430 lineage) + MDP |
| RAM | 128 MB DDR SDRAM + 32 MB DDR stacked in the MSM7201A |
| ROM | 1 GB NAND Flash |
| Video | Composite (RCA), PAL-M/NTSC, **VGA 640×480, 4:3** |
| Audio | MIDI (72 voices, 512 kB wavetable, 44 kHz), MP3, PCM, ADPCM, CMX, QCELP |
| 3D API | **OpenGL ES 1.0+ Common Profile** (through BREW extension) |
| 2D API | BREW 2D (`IDisplay` / `IGraphics` / `IBitmap`) |
| I/O | 3× USB 2.0 host (accessories), 1× USB OTG mini-B (development/service), SD, RCA |
| Network | Quad-band GSM/GPRS/EDGE + tri-band UMTS/HSDPA/HSUPA |
| OS | **BREW 4.0.2** (customized) |
| Console ID | IMEI |
| Claimed performance | 1.6 M triangles/s; textured fill rate 63 M px/s (2 textures) |

Important for emulation: **the gamepads are USB HID** and are exposed to the game through `IHID` / `IHIDDevice` (buttons, axes, rumble, hot-plug). This is not a BREW mobile-phone keyboard. `../vendor/tripleoxygen/content/hid_devices.cfg` and `../vendor/tripleoxygen/hardware/peripheral/joystick_descriptor.txt` describe the actual mapping.

## Graphics architecture

The Developer Guide (chapter 3) documents the Adreno 130 pipeline:

- **Tile/binning rendering** (geometry binning followed by per-bin rendering), with a command *ring buffer* between CPU and GPU.
- The **MDP** (Mobile Display Processor) composites and scans out concurrently with 3D rendering.
- Memory interfaces (internal SMI vs. external EBI) have a major performance impact — irrelevant to HLE, but relevant if a game assumes particular timing or latency.
- Supported OpenGL ES extensions and limitations appear in chapters 7.2–7.4, including **ATITC** (ATI texture compression), matrix palette, point sprites, and DrawTexture. Official samples are in `../vendor/sdk/samples/unpacked/MSM7500_OGLES_*`.

For HLE, what matters is the **OpenGL ES 1.x API and its extensions**, not the Adreno itself. A natural host translation is GL ES 1.1 → OpenGL/ANGLE, or a fixed-function pipeline reimplementation over GL ES 2/Vulkan. ATITC textures need software decoding on the host.

## Memory, boot, and JTAG (low-level material)

From `../vendor/tripleoxygen/research/` and `../vendor/tripleoxygen/dump/`. The shorter paths in the list below sit inside that same `tripleoxygen/` tree:

- NAND boot chain: **PBL (ROM) → QCSBL → OEMSBL1/OEMSBL2 → APPSBL → APPS/AMSS**. All partitions are dumped in `dump/nand/1.1.2/partitions/` (firmware 1.1.2), with `md5.txt`.
- `research/Analysis 0x00000000.txt`: bootloader RE notes — QCSBL/OEMSBL offsets, front-panel EL GPIO (`GPIO_OUT_0` 0xA9200800, bit 15, active low), SD-card detection, NAND read protocol through registers (0xA0A00000/04/10/100), and the useful App ID list at `0x11428800` (DEFAULT `0x1008000`, EM `0x1007001`, CONFTEST `0x1A2345F`, TECTOY `0x1070798`) plus system-mode flags at `0x114287F4/F5`.
- `research/memory_map.ods`: MSM7201A memory map (derived from HTC Dream/RAPH, the same chipset family) — SRAM/SMI at `0xA8000000`, peripheral blocks, etc.
- `research/jtag arm11.txt`, `hardware/msm7201a_arm11.cfg`, `msm7201a_arm9.cfg`, `hardware/zeebojtag_r2.zip`: OpenOCD configs and JTAG adapter for the console.
- `dump/ram/1.1.2/` and `dump/regs/`: RAM-region and register-block dumps (`0x80000000`, `0xA8600000`, `0xB8000900`, `0xFFFEF000`).
- `research/tvenc/`: framebuffers captured from the TV encoder (useful output reference: RGB565 format, resolution, overscan).

This material is only needed for **LLE** (running the real firmware). For HLE it serves as *ground truth*: `APPS.bin` can be disassembled to see how Qualcomm implemented each API.
