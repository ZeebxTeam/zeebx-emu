# 01 — Zeebo hardware

Primary source: `vendor/tripleoxygen/doc/ZeeboDeveloperGuide0.97.pdf` (Zeebo Inc., 2009),
section 2.1 and chapter 3. Complemented by dumps and OpenZeebo research.

## Specifications (from the Developer Guide, verbatim, condensed)

| Item | Value |
|---|---|
| Chipset | **Qualcomm MSM7201A** + RTR6280 (RF) + PM7540 (PMIC) |
| Application CPU | **ARM11 (ARMv6, ARM1136)** @ 528 MHz + **QDSP-5** |
| Modem CPU | ARM9 + QDSP-4 (integrated in the MSM7201A) |
| GPU | **Adreno 130** (ATI/AMD Imageon Z430 lineage) + MDP |
| RAM | 128 MB DDR SDRAM + 32 MB DDR stacked on the MSM7201A |
| ROM | 1 GB NAND Flash |
| Video | Composite (RCA), PAL-M/NTSC, **VGA 640×480, 4:3** |
| Audio | MIDI (72 voices, 512 kB wavetable, 44 kHz), MP3, PCM, ADPCM, CMX, QCELP |
| 3D API | **OpenGL ES 1.0+ Common Profile** (via BREW extension) |
| 2D API | BREW 2D (IDisplay / IGraphics / IBitmap) |
| I/O | 3× USB 2.0 host (accessories), 1× USB OTG mini-B (dev/service), SD, RCA |
| Network | GSM/GPRS/EDGE quad band + UMTS/HSDPA/HSUPA tri band |
| OS | **BREW 4.0.2** (customized) |
| Console ID | IMEI |
| Stated performance | 1.6 M triangles/s; textured fill rate 63 M px/s (2 textures) |

Important note for emulation: **the gamepads are USB HID** and reach the game through
`IHID` / `IHIDDevice` (buttons, axes, rumble, hot-plug) — it is not a cell-phone BREW keypad.
The files `vendor/tripleoxygen/content/hid_devices.cfg` and
`vendor/tripleoxygen/hardware/peripheral/joystick_descriptor.txt` describe the real mapping.

## Graphics architecture

The Developer Guide (ch. 3) documents the Adreno 130 pipeline:
- **Tiled/binning** rendering (geometry binning → render per bin), with a command *ring buffer*
  between CPU and GPU;
- **MDP** (Mobile Display Processor) does composition/scan-out concurrently with 3D;
- Memory interfaces (internal SMI vs external EBI) with a large performance impact —
  irrelevant for HLE, relevant if some game assumes timings/latencies.
- Supported OpenGL ES extensions and limitations are in chapters 7.2 to 7.4 — including
  **ATITC** (ATI texture compression), matrix palette, point sprites, DrawTexture.
  The official samples are in `vendor/sdk/samples/unpacked/MSM7500_OGLES_*`.

For HLE, what matters is the **OpenGL ES 1.x API + extensions**, not the Adreno itself:
the natural translation on the host is GL ES 1.1 → OpenGL/ANGLE, or a fixed-pipeline
reimplementation on top of GL ES 2/Vulkan. ATITC textures need software decoding on the host.

## Memory, boot and JTAG (low-level material)

From `vendor/tripleoxygen/research/` and `vendor/tripleoxygen/dump/`:

- Boot chain on the NAND: **PBL (ROM) → QCSBL → OEMSBL1/OEMSBL2 → APPSBL → APPS/AMSS**.
  All partitions are dumped in `dump/nand/1.1.2/partitions/` (firmware 1.1.2),
  with `md5.txt`.
- `research/Analysis 0x00000000.txt`: RE notes on the bootloader — QCSBL/OEMSBL offsets,
  the front EL panel GPIO (`GPIO_OUT_0` 0xA9200800, bit 15, active low), SD card detection,
  the register-based NAND read protocol (0xA0A00000/04/10/100), and — very useful —
  the address `0x11428800` with the system's **list of App IDs**
  (DEFAULT `0x1008000`, EM `0x1007001`, CONFTEST `0x1A2345F`, TECTOY `0x1070798`)
  and system-mode flags at `0x114287F4/F5`.
- `research/memory_map.ods`: MSM7201A memory map (derived from the HTC Dream/RAPH, same chipset
  family) — SRAM/SMI at `0xA8000000`, peripheral blocks, etc.
- `research/jtag arm11.txt`, `hardware/msm7201a_arm11.cfg`, `msm7201a_arm9.cfg`,
  `hardware/zeebojtag_r2.zip`: OpenOCD configs and JTAG adapter for the console.
- `dump/ram/1.1.2/` and `dump/regs/`: dumps of RAM regions and register blocks
  (0x80000000, 0xA8600000, 0xB8000900, 0xFFFEF000).
- `research/tvenc/`: framebuffers captured from the TV encoder (useful as a reference for the
  expected output: RGB565 format, resolution, overscan).

This material is only needed if you want **LLE** (running the real firmware). For HLE it serves
as *ground truth*: you can disassemble `APPS.bin` and see how Qualcomm implemented each API.
