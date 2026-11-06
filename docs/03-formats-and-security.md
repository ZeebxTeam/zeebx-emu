# 03 — File formats and security

## Anatomy of a title (observed in `roms/Z-Wheel`)

The Z-Wheel — the console's menu/shell, App ID **274755** — has the standard structure of a Zeebo
BREW app:

```
mif/274755.mif              <- Module Information File (metadata + icons)
mod/274755/                 <- module directory, name = App ID
    tectoy.mod              <- relocatable ARM binary (the "executable")
    tectoy.sig              <- signature / execution authorization
    tectoy.mif              <- referenced from inside the .sig
    tectoy.bar / *.brf      <- resources (strings, images, i18n: _pt, _es, _esmx)
    *.qxt / *.qxm / *.qxa   <- QXEngine assets (textures, models, animations)
    *.bmp *.png *.gif *.mp3 *.wav
    preload.xml, uiconfig.xml, tectoy.cfg, tt_prefs.db, rootca.cer
```

Note `rootca.cer` and `tt_prefs.db`: the Z-Wheel talked to TecToy's servers (store/OTA), which are
offline. An emulator will need to **stub/mock that network layer** so the menu does not hang — or
a local server that pretends to be the store.

### `.mod` — BREW module

Observed in the header of `tectoy.mod`:

```
00000000: 0e00 00ea  0f00 00ea   b <entry> ; b <entry2>   (2 ARM branches)
00000008: "BREW"                 magic
0000000c: 01 00 00 00            format version
00000010: 40 00 00 00            offset 0x40 (start of the real code / AEEMod_Load)
00000014: 01 01 00 00
00000018: 00 02 00 00            0x200
0000001c: f0 fd 08 00            0x0008FDF0  (code+data size)
00000020: 20/a0/40/90            table of internal offsets
00000034: f0 fd 08 00            repeats 0x0008FDF0
00000040: 17 40 2d e9 ...        first ARM code  -> AEEMod_Load
```

In other words: `.mod` **is not ELF**. It is the output of Qualcomm's `elf2mod`, which takes an ARM
ELF (`.text` based at 0, `.data`/`.bss` right after, `AEEMod_Load` as the first symbol of `.text`,
linked with `-Wl,--emit-relocs`) and produces a **self-relocating** binary with an embedded
relocation table. The emulator's loader has to: map the blob, apply the relocations for the chosen
base and jump to `AEEMod_Load`.

The **official linker script** (`vendor/sdk-brew/toolset/…/bin/elf2mod/src/gnu/elf2mod.x`)
confirms the layout `elf2mod` expects:

```
. = 0x0;                        /* ro-base at zero */
__module_start__ = .;
.text : { "*.o"(.text.AEEMod_Load)   /* AEEMod_Load forced at the start */
          *(.text .text.* ...) }
PHDRS { ER_RO PT_LOAD; ER_RW PT_LOAD; }
```

`elf2mod.exe` itself is in `vendor/sdk-brew/toolset/…/bin/` — you can run it under Wine and, above
all, **disassemble it to find out the exact format of the `.mod` relocation table**, which is the
only part of the format that is still inference.

Toolchain reference: [usernameak's BREW Development Guide](https://gist.github.com/usernameak/f7315e9a00ebc4febac5a16687b2409e)
describes the flow `arm-none-eabi-gcc -mword-relocations -fshort-wchar ... → elf2mod → .mod`,
which is the reverse path — useful for producing **test binaries of our own** with known source.

### `.mif` — Module Information File

A binary with an offset table at the start, followed by records. In `274755.mif` you can see
`image/jpg` and an embedded JPEG right after it (icon/boxart), plus applet references (ClassID,
type, flags) and privileges. The AEE reads the `.mif` **before** loading the `.mod` to know which
applets the module exposes.

### `.sig` — signature

`tectoy.sig` (2,712 bytes) is partly readable:

```
0100 0200 8004 4400 ...  "Zeebo, Inc."  ...  "tectoy.mif"
```

It is the execution authorization issued for that module/device. **Infuse requires `.mod`, `.mif`
and `.sig`** together, but since it is HLE it probably does not validate the cryptography — it only
uses the metadata. An HLE emulator can simply **skip the verification**; this is not an obstacle.

### `.bar` / `.brf` — resources

`.bar` is the standard BREW resource file (strings, bitmaps, i18n) produced by the BREW Resource
Editor. On the Zeebo there are also per-language `.brf` files (`tectoy_pt.brf`, `tectoy_es.brf`…).

### `.qxt` / `.qxm` / `.qxa` — QXEngine

Assets of **QXEngine**, Qualcomm's 3D engine distributed with the Zeebo SDK
(`QXBinaryDistribInstaller.msi`, in `vendor/sdk/zeebo-sdk-1.2.4/`). Texture, model and animation.
If a game uses QXEngine, it loads a **binary library** — which also has to run on top of your
BREW reimplementation (it is ordinary ARM code, so it runs in the JIT with no special handling, as
long as the BREW APIs beneath it exist).

### GGZ — compressed asset files

Several games (Double Dragon, Sonic BREW, DMC) pack assets in GGZ. There is an open unpacker:
[`Tuxality/ggzbrewtools`](https://github.com/Tuxality/ggzbrewtools) (cloned in `vendor/repos/ggzbrewtools`)
and a Python script in `vendor/repos/openzeebo/tools/ggzupk.py`.

## Console security (only matters for running on real hardware)

- Every Zeebo has a unique **`61u.key`** (14 alphanumeric characters) that enables the *Rear
  Diagnostic Port* / dev mode via SD card. It was generated by an internal Qualcomm/TecToy tool
  from the IMEI; that tool never leaked. Without it, the only known path is **soldering JTAG** and
  extracting the key from the console (adapter and OpenOCD configs in
  `vendor/tripleoxygen/hardware/`).
- Digital code signing: `vendor/tripleoxygen/doc/CodeAuthOnBrewMPThruDigitalSigning.pdf`.
- Boot chain and signed-bootloader patches: `vendor/tripleoxygen/mod/dwnmode_bl/` and
  `mod/nand/mod_1.1.2_OEMSBL2_*` (OEMSBL2 modified to enable UART/GPIO and disable the NAND MPU).

**None of this blocks an emulator.** It is only relevant for dumping games from a physical console
or running homebrew on the hardware.
