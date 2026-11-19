# Inventory of `docs/vendor/`

Third-party material mirrored locally (~1.3 GB). **Not versioned** — see `.gitignore`.

## `tripleoxygen/` — mirror of <https://www.tripleoxygen.net/files/devices/zeebo/>

| Folder | Highlights |
|---|---|
| `doc/` | **ZeeboDeveloperGuide0.97.pdf**, **BREWOemAPIReferenceforMSM.pdf**, Inside BREW, BREW Programming Guide, ARM1136/ARM926 TRMs, BREW+OpenGL ES talks, CodeAuth via digital signing, **ZeeboSDKPackage-0.93/1.2.4.zip** |
| `brew/` | OpenGL ES 1.0.3 samples + extension headers, QLib, Qualcomm demos, `starting_brew.pdf`, `conftest.sig` |
| `dump/nand/1.1.2/partitions/` | **Full firmware 1.1.2**: PBL-adjacent, QCSBL, OEMSBL1/2, APPSBL, AMSS, APPS, MIBIB (+ md5) |
| `dump/ram/1.1.2/` | RAM dumps per region (0x80000000, 0xA0400000, 0xA8600000, 0xE0000000, 0xFFFF0000…) |
| `dump/regs/` | Dumps of register blocks |
| `mod/` | Patched OEMSBL2 (UART/GPIO, NAND MPU), signed `zloader` |
| `research/` | **`brew/vtbl.ods` + `IFILEMGR_VTBL_Z200.txt` (real firmware vtables)**, `Analysis 0x00000000.txt`, `memory_map.ods`, `mainboard_pinout.ods`, `jtag arm11.txt`, `tvenc/` (captured framebuffers), `peteca/` (saves) |
| `hardware/` | MSM7201A OpenOCD configs (ARM11/ARM9), `zeebojtag_r2` adapter, EL panel, `peripheral/joystick_descriptor.txt`, Boomerang controller docs |
| `content/` | Z-Wheel packages (`274755*.7z`, incl. the MX version), `hid_devices.cfg`, `ids.txt`, `flixfile.dat` |
| `datasheet/` | Hynix NAND/Hynix MCP, TUSB2036 (USB hub), EL panel driver |
| `driver/` | The console's USB drivers for Windows |
| `code/`, `homebrew/`, `media/` | Blinkenlichten, OpenZeebo art, UART pinout, RGB565 splash |

## `repos/` — clones

| Repo | Note |
|---|---|
| `openzeebo` | Sample BREW apps, zloader, nand_util, `ggzupk.py`, ARM11 JTAG in asm |
| `zeebo_doom` | Ported PrBoom (a real BREW portability layer) |
| `zeeutils` | Only LICENSE on the default branch |
| `ggzbrewtools` | GGZ unpacker/packer (C++17 + Boost) |
| `kernel_zeebo` | MSM Linux kernel tree (~900 MB) |

## `sdk/` — Zeebo SDK 1.2.4, extracted

- `zeebo-sdk-1.2.4/ZeeboSDKPackage-1.2.4/` — MSIs (`ZeeboSDKInstaller.msi`, `QXBinaryDistribInstaller.msi`
  = QXEngine), Adreno Profiler, OpenGL ES 1.5.3 extension for BREW SDK 4.x, driver, Developer Guide.
- `samples/unpacked/MSM7500_OGLES_qcom_sdk_samples.release_02.15.07.beta1/` — **official samples
  in C** with `.mif`/`.bid`/`.mak`: `simple_vbo`, `simple_dot3`, `simple_atitc`, `simple_shadow`,
  `simple_spotlight`, `simple_point_sprites`, `simple_drawtexture`, `simple_matrix_palette`.
- `samples/unpacked_conftest/` — the conftest source.

### Extracted MSIs (via p7zip)

Flow used: `7z x <installer>.msi` produces the MSI tables and a payload CAB with GUID names;
extract the CAB and rename the files from each one's `FILE:` comment / include guard.

- `zeebo-sdk-named/` — payload of **ZeeboSDKInstaller.msi**. It is only the Zeebo *delta* over the
  BREW SDK: the **HID extension** (`AEEIHID.h`, `AEEIHIDDevice.h`, `AEEHIDButtons.{c,h}`,
  `AEEHIDThumbsticks.{c,h}`, `AEEHIDDevice_{Joystick,Keyboard,Mouse}.h`), plus BREW Device
  Configurator resources, bitmaps and simulator DLLs. `AEEHIDButtons.h` is by **Tectoy Digital**
  and maps button UIDs to the standard Zeebo gamepad IDs.
- `qxengine-named/` — payload of **QXBinaryDistribInstaller.msi** (786 files): 62 **QXEngine**
  headers (`QXModel.h`, `QXRenderMgr.h`, `QXTexture.h`, `QXAnimation.h`, `QXMathFixed.h`,
  `QXPack.h`, `QXFileManager.h`…), 43 ARM static libraries (`ar archive`), complete samples
  (QCastle, CrateHunt, LightingSample, ProjectedShadow, AnimatedModel) and — importantly —
  Qualcomm's original **`AEEModGen.c` and `AEEAppGen.c`**, which document the exact ABI of
  `AEEMod_Load` / `IModule` / `AEEClsCreateInstance`.
- `msi-zeebosdk/`, `msi-qxengine/` — raw MSI tables; `zeebo-sdk-files/`, `qxengine-files/` —
  payload with GUID names (kept for traceability).

## `brew-src/` — BREW sources and examples extracted from the RARs

- `43957791BREWSourceCode/BookDemo/Extension/AEE.h` — **complete `AEE.h`** (72 KB, BREW 2.x era)
- `78575279brewexamples/examples/` — assorted BREW examples

## `sdk-brew/` — **complete BREW SDK 4.0.2** (what was missing)

Origin: [Zeebo SDK + BREW SDK 4.0.2 + BREW MP SDK — Internet Archive](https://archive.org/details/zeebo-sdk-brew-sdk-4.0.2-brew-mp-sdk)
(1.17 GB). `brew-sdk-402-bundle.zip` is the raw download; `bundle/` is it extracted.

| Path | Contents |
|---|---|
| `brew-402/BREW 4.0.2 SP19/sdk/inc/` | **158 BREW SDK 4.0.2 headers**: `AEE.h`, `AEEShell.h`, `AEEDisp.h`, `AEEFile.h`, `AEEStdLib.h`, `AEEGraphics.h`, `AEESound.h`, `AEEMedia.h`, `AEEBitmap.h`, `AEECallback.h`… — **the exact version the Zeebo runs** |
| `brew-402/…/sdk/inc/gles/` | `egl.h`, `gl.h`, `glESext.h`, `glQUALCOMM.h`, `gles_1_0/`, `gles_1_1/` |
| `brew-402/…/inc/` | +146 headers and `.bid` files (ClassIDs of every system class) |
| `brew-402/…/sdk/src/` | `AEEModGen.c`, `AEEAppGen.c`, `EGL_1x.c`, `GLES_1x.c`, `GLES_ext.c`, `mod_malloc.c`, `glibc_stubs.c`, thrdutil |
| `brew-402/…/documentation/API Reference/` | **API reference in HTML** (every interface, signatures and semantics) |
| `brew-402/…/bin/Simulator.exe` | Qualcomm's BREW simulator (x86) — a behavior reference for the APIs |
| `brew-402/…/devicepacks/` | Device packs (handset profiles for the simulator) |
| `toolset/…/bin/elf2mod.exe` | **The `elf2mod`** that produces `.mod` files from ARM ELF |
| `toolset/…/bin/elf2mod/src/gnu/elf2mod.x` | **Official GNU linker script** — confirms `ro-base = 0x0`, `__module_start__`, `.text.AEEMod_Load` forced at the start, ER_RO/ER_RW layout |
| `bundle/BMP_SDKMP_7.12.5_SETUP_3.exe` | BREW MP SDK 7.12.5 (892 MB) — **not extracted yet**, optional |

### Pending
Two RARs use the RAR5 method, not supported by the installed p7zip
(`503611776Inside_BREW.rar`, `11715848BREWProgrammingguide.rar`). To open them:

```bash
brew install unar && unar -o brew-src docs/vendor/tripleoxygen/doc/503611776Inside_BREW.rar
```

(solved with `unar` — both files are already extracted in `brew-src/`.)

The BREW MP SDK 7.12.5 inside `bundle/` remains unextracted; it is only worth it if we want to
compare APIs between BREW 4.0.2 and BREW MP.

## The archive is online, and where it changes the plan

`tripleoxygen.net/files/devices/zeebo/` is still public. What was downloaded into `vendor/`, which
git ignores — firmware and console material do not go into the repository:

| File | What it is |
|---|---|
| `content/hid_devices.original.cfg` | The console's controller mapping, with the entry for the Zeebo controller |
| `content/274755_(727)_full.7z` | The full Z-Wheel package, 606 files |
| `dump/nand/1.1.2/partitions/1.1.2_APPS.bin` | The applications partition, 21 MB, an ARM ELF |
| `dump/nand/1.1.2/1.1.2.bin` | The whole NAND, 128 MB |

Three things came out of it, and the third is the one that changes course.

### `hid_devices.original.cfg` disproved one of our deductions

The entry `VID:0x1EAA:PID:0x0135` carries `AXIS:X:0x0106C40C`, which is the UID of a button. The
project's original table was right and the "fix" I had made, deduced from the game binaries, was
wrong. The story is told in [09-input.md](implementation/09-input.md).

### The official Z-Wheel package is the same as what we already had

Same file list, same `tt_prefs.db`. In other words, `fontsize.map` and `zeeboprefs.dat` **do not
belong to the game**: they belong to the device. And `fontsize.map` does not appear anywhere in
the NAND — neither in the firmware nor in the whole image. The game looks for a file that may
never have existed, and its "files not found" is noise, not a clue.

### The interface classes are console modules

This is the big discovery. The strings in `1.1.2_APPS.bin` show that the Zeebo's BREW loads
extensions that are **ARM binaries like any game**:

```
fs:/mod/widgets/widgets.mod        fs:/mod/forms/forms.mod
fs:/mod/framewidget/framewidget.mod fs:/mod/imenu/imenu.mod
fs:/mod/icontrols/icontrols.mod    fs:/mod/htmlwidget/htmlwidget.mod
fs:/mod/isql/isql.mod              fs:/mod/ssl/ssl.mod
fs:/mod/btfe/qcsans_ttf.mod        fs:/shared/fonts/tectoy.ttf
```

That is: the classes we are identifying slot by slot in the Z-Wheel — the root form, the
collection, the TrueType font — **are not BREW API we need to write**. They are modules the
console loaded, and that the emulator could load the same way it loads a game. `isql.mod` is the
proof in reverse: we reimplemented it in Rust without knowing a ready-made module existed.

That does not make the work done useless — `brew/sql.rs` works and does not depend on console
material —, but it reorders the queue: before reimplementing an interface toolkit, it is worth
trying to **run the console's own**.

What is missing for that is extracting the files from the NAND. It uses EFS2, Qualcomm's file
system, and the `.mod` files do not show up as contiguous blocks with the `BREW` header — only the
path strings, inside the firmware. Extracting them requires an EFS2 reader, and that is the next
task on this thread.

## Where the extension modules really are

The previous conclusion — "the interface classes are modules that can be loaded" — was right about
their nature and wrong about their location. Searched for in the dump's file system, the `.mod`
files are not there: no page of `EFS2APPS` has a BREW module header, and the name `widgets.mod`
appears only once in the whole image, inside the firmware's string table.

They are **inside `1.1.2_APPS.bin`**, the 21 MB ARM ELF. The five ClassIDs the Z-Wheel asks for
are all there:

| Class | Occurrences in the firmware | What it is |
|---|---:|---|
| `0x01001011` | 26 | root form |
| `0x0100104f` | 3 | the collection |
| `0x01035156` | 3 | the TrueType font |
| `0x01028e51` | 4 | still unnamed |
| `0x0102c4e8` | 3 | the `SQLMGR` |

That swaps "loading the console's module" for something else, more laborious and more reliable:
**disassembling the firmware to read the vtables**. `tools/disassemble.py` already does that, and
the difference is that there is no deduction there — the method table is written down.

It is the path that replaces the probe: instead of discovering one slot per run, read the whole
vtable at once.
