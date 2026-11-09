# Libretro core on Linux AArch64 handhelds

The `zeebx_libretro.so` core is built for **Linux AArch64**. The same file serves the 64-bit
handhelds of this family:

- R36S/R35S/RGB20S with ArkOS or AeolusUX/ArkOS-R3XS;
- R36S with dArkOS or dArkOSen-R36S;
- RG40XX-H with MustardOS (muOS) Pixie `2502.0` or newer.

References for the systems used to define the target:

- <https://github.com/christianhaitian/arkos>
- <https://github.com/AeolusUX/ArkOS-R3XS>
- <https://github.com/christianhaitian/dArkOS>
- <https://github.com/djparentx/dArkOSen-R36S>
- <https://muos.dev/release/archive/2502_0>

## What is compatible

The `zeebx_libretro_linux-aarch64.zip` package contains two files that must stay together:

```text
zeebx_libretro.so
zeebx_libretro.info
```

The core does not depend on X11, Wayland, EGL, ALSA, udev or GTK. Video, audio and controls come
in through the Libretro ABI. If RetroArch does not offer an OpenGL ES 3 context, the core accepts
the refusal and uses the software rasterizer; it does not crash for lack of a compatible GPU.

On Linux AArch64 the hardware rendering request is `RETRO_HW_CONTEXT_OPENGLES3`, that is,
GLES 3.0. VAO, FBO blit, MSAA and the `#version 300 es` shader already belong to that version;
asking for 3.2 made a perfectly sufficient Panfrost 3.1 refuse the context and send all drawing to
the processor. On desktop it still asks for OpenGL Core 3.3, since the context is different. Both
paths use the same frontend FBO.

## The image does not fill the screen: it is the frontend's integer scaling

The console outputs **640x480**, and the frontend is what enlarges it — no core setting changes
that size. With **integer scaling** on (`Settings > Video > Scaling > Integer Scale`, or
`video_scale_integer = "true"` in `retroarch.cfg`), RetroArch only presents the image in
**multiples** of 640x480: on a 1080p screen that is 2x, that is, **1280x960 with bars around it**.

Turn integer scaling off for the image to fill the screen. If the window still goes past the
screen, look at `video_scale`, which is its factor: 3x of 640x480 gives 1920x1440, which does not
fit in 1080p.

Two things that are **not** this defect, so no time is lost on them:

- The image is **4:3**, so on a 16:9 screen there are side bars either way. That is the aspect
  ratio, and whoever wants to stretch it changes RetroArch's `Aspect Ratio`.
- `zeebx_resolucao_interna` **does not change the image size**. It is supersampling: the same
  frame, with smoother polygon edges (measured on Rally Master Pro: 15,920 distinct colors at 1x
  and 86,935 at 2x, with the same 1280x960 capture). Whoever only wants to fill the screen leaves
  it at 1x.

## What each controller button does in Zeebx

The Zeebo controller's diamond is **not** numbered in the order the eyes read it: **1 is at the
bottom, 2 on the left, 3 at the top and 4 on the right**. The mapping to the RetroPad keeps the
**hand position**, not the number:

| device | where it is | RetroPad |
|---|---|---|
| Button 1 | bottom | `B` |
| Button 2 | left | `Y` |
| Button 3 | top | `X` |
| Button 4 | right | `A` |
| HOME | middle | `Select` |
| ZL / ZR | shoulders | `L` / `R` |
| d-pad | cross | `D-Pad` |
| two sticks | — | left and right analogs |

RetroArch's controls screen shows these names with the position next to them — *"Button 1
(bottom)"* — so you can check without memorizing. If you prefer another order, remapping is
RetroArch's own (Settings → Input), and the same goes for the app, which has a controls screen.

## Measuring on the device: `test_zeebx.sh`

The device test is **one script and one menu**, no SSH and no typing commands by hand:

1. Copy `tools/test_zeebx.sh` to the card's **ports** folder (`EASYROMS/ports/` on ArkOS,
   `ROMS/Ports/` on muOS).
2. On the device: **Ports** → **test_zeebx**. It runs on its own, on the device's screen.
3. There are twelve runs of twenty seconds (~5 min). At the end, the card holds
   `logs-zeebx-<date>/` with `summary.txt` and the log of each run.

Each run runs a fixed number of frames and measures **wall time**: the figure is *real frames per
second*, and **60 means full speed at that setting**. The runs compare the factory setting with
the Handheld profile and with tile discard — the latter **only pays off on a tile-based GPU**, and
that is why the device is the only place where it gets measured.

The games deliberately include **Zenonia** (which only draws 2D: on the GPU path it goes black and
on the processor it shows up — it is the pair that documents the defect on the device) and **Zeebo
Sports Peteca**.

The script saves the Core Options and restores them at the end, and writes the
`glBlitFramebuffer` warning to the log if the GPU driver fails the blit check (see front 5 in
`docs/OPTIMIZING_V0.3.0.md`).

## Installation

1. Download the **Linux AArch64** artifact from the release/Actions.
2. Extract the two files.
3. Copy them to the cores directory RetroArch shows in `Settings > Directory > Cores`.
   On images that keep a cores folder on the card, use that folder; do not rename the `.so` or
   separate the `.info` from it.
4. Open RetroArch once, load `zeebx_libretro` and only then scan the ROMs folder.
5. Use `.zip`, `.7z` or `.mod`. The `.info` file declares `mod|zip|7z` and `block_extract=true`.

The card may have 32-bit ARM libraries for PortMaster. That does not change this package: the core
needs an AArch64 RetroArch and does not load in an `armhf`/32-bit RetroArch.


## muOS (RG40XX-H): what muOS reads, and from where

Measured on the muOS image on the device's card (64-bit kernel, AArch64 RetroArch). muOS does
**not** load the core from a RetroArch cores folder: the launcher calls

```sh
nice --20 retroarch -v -f $RA_ARGS -L "$MUOS_SHARE_DIR/core/$CORE" "$FILE"
```

that is, `/opt/muos/share/core/<core>`. The one that picks the core is the assignment file
(`info/assign/<System>/<core>.ini`), and the core's `.info` lives in
`/opt/muos/share/emulator/retroarch/info/`.

The content filesystem is a **unionfs** (`/opt/muos/script/mount/union.sh`): `/mnt/union/ROMS`
joins, in this order, `USB/ROMS`, `SDCARD/ROMS` and `ROMS` from the system card. With two cards,
the ROMs card is the `SDCARD`, and the ROMs go in **`/ROMS/Zeebo`** on it — not in `ROMS/ROMS` on
the system card.

What muOS needs, and where:

| Piece | Path | What it is |
|---|---|---|
| Core | `/opt/muos/share/core/zeebx_libretro.so` | the AArch64 `.so` |
| Info | `/opt/muos/share/emulator/retroarch/info/zeebx_libretro.info` | metadata RetroArch reads |
| System | `/opt/muos/share/info/assign/Zeebo/{global.ini,zeebx.ini}` | system name, default core and the launch command |
| Assignment | `/opt/muos/share/info/assign/assign.json` | `friendly` → system folder |
| Folder name | `MUOS/info/name/folder.json` | folder name → displayed name |
| Covers | `MUOS/info/catalogue/Zeebo/{box,grid}/<game>.png` | art, named after the `.zip` without extension |
| Games | `ROMS/Zeebo/*.zip` on the ROMs card | show up in `/mnt/union/ROMS/Zeebo` |

Two measured traps, both about permissions:

1. `assign.json` is `root:root 0644`, but the **directory** belongs to the user. Since removing a
   file depends on the directory's permission, you can delete it and rewrite it with the new key
   without `sudo`. Without the `zeebo` key, the system does not show up — `assign.sh` only runs
   from the card (the *Refresh Automatic Core Assign* task) and on reset, not on every boot.
2. `emulator/retroarch/{database,playlists,thumbnails}` are `root:root 0750`: you cannot write to
   them through the mounted card. They are not needed to play through muOS; they only serve
   RetroArch's own menu.

**Library compatibility, measured before copying:** the core requires at most `GLIBC_2.34` and
`GLIBCXX_3.4.31`; the device ships glibc up to `GLIBC_2.38` and `libstdc++.so.6.0.32` with
`GLIBCXX` up to `3.4.32`. Nothing missing — and this check is what avoids the classic "the core
does not show up" symptom from a refused `dlopen`.

### Installing

With the system card's ROOTFS partition mounted, it is **one command**. If you are using the files
from an artifact/release, pass `--so` and `--info`; that way the installer does not require a
local x86 build:

```bash
python3 tools/install_core.py --muos /media/$USER/ROOTFS \
  --roms /media/$USER/ROMS \
  --so zeebx_libretro.so --info zeebx_libretro.info \
  --soundfont GeneralUser-GS.sf2 --font tectoy.ttf \
  --rom 'Double Dragon (Brazil) (Es,Pt).zip' 
```

For a checkout with a local build, `--so`/`--info` can be omitted. The script makes a dated
backup, copies core + `.info`, creates the muOS assignments, updates the folder name when the ROMs
partition is mounted, installs optional ROMs/SoundFont and checks the SHA-256.

It backs up the previous core with the date in the name before overwriting, copies the `.so` and
the `.info`, creates the system assignments, adds the key to both JSON files and checks the
`sha256` at the end. Without a mounted card the command **refuses and says which partition to
mount** — this was measured, and it is better than writing to the wrong place.

What it does underneath, for whoever wants to check or do it by hand:

1. Copy the `.so` to `/opt/muos/share/core/` and the `.info` to
   `/opt/muos/share/emulator/retroarch/info/`.
2. Create `/opt/muos/share/info/assign/Zeebo/` with `global.ini` (`name`, `default=zeebx`,
   `catalogue`, `lookup=0`, `governor=performance` and a `[friendly] zeebo`) and `zeebx.ini`
   (`name=Zeebx`, `core=zeebx_libretro.so`, `exec=/opt/muos/script/launch/lr-general.sh`).
3. Add `"zeebo": "Zeebo"` to `info/assign/assign.json` and to `MUOS/info/name/folder.json`.
4. Put the games in `ROMS/Zeebo/` **on the ROMs card**.
5. Optional: covers in `MUOS/info/catalogue/Zeebo/box/` and `grid/`, with the file name the same
   as the `.zip` without extension.

The **Z-Wheel needs the games in the same folder**: it enumerates the applets installed next to
the content, so opening the wheel from inside `ROMS/Zeebo` shows the 63 titles.


## MIDI sample bank (optional)

Eleven games play their soundtrack as MIDI, and the score has no sound inside it — the synthesizer
is what turns it into sound. Without a bank, Zeebx uses its wavetable; with a bank, it plays the
real samples. Measured on the Double Dragon music: with the bank the centroid is within 2.5% of
the reference (Zeebulator with GeneralUser GS), against 38% for the wavetable.

Installing is copying **one file**:

```text
<device or profile root>/soundfonts/*.sf2
```

On muOS the device root is the core's system directory; on desktop, `~/.config/zeebx/aparelho`.
To try it without installing anything, `ZEEBX_SOUNDFONT=/path/to/Bank.sf2`.

Without the file the core works the same — the sound changes, not the loading. Measured: the
core's `.so` is 15.2 MB with the sample bank synthesizer compiled in, and `ldd` still shows only
libstdc++, libgcc, libm and libc.

## Verification without guessing

The `libretro` workflow checks the artifact itself:

- `file`/`readelf -h`: ELF64 `AArch64`;
- `ldd`: no dependency on host windowing, audio or input;
- `readelf -d`: no `RPATH`/`RUNPATH` pointing at the runner;
- `verify_core.py`: `dlopen`, 25 ABI symbols and `retro_api_version == 1`.

If RetroArch does not list the core, first check that RetroArch is 64-bit and that
`zeebx_libretro.info` is in the same folder. If video does not open in hardware, select the
system's video driver or let the core fall back to software; that is a supported degradation, not
a load failure.

## Evidence for the chips and firmwares

- Rockchip's official page for the RK3326 lists Cortex-A35, Mali-G31MP2 and **OpenGL ES 3.2**:
  <https://www.rock-chips.com/a/cn/product/RK33xilie/2018/0514/901.html>
- The official H700 brief lists 64-bit Cortex-A53, G31 and **OpenGL ES 3.2/Vulkan 1.1**:
  <https://www.allwinnertech.com/uploads/pdf/2021070513595227.pdf>
- Arm's official page confirms that Mali-G31 supports OpenGL ES 3.2:
  <https://www.arm.com/products/silicon-ip-multimedia/gpu/mali-g31>
- The versioned ArkOS keeps the arm64 package `libmali-rk-bifrost-g31-rxp0-wayland-gbm`; the
  binary contains the string `OpenGL ES 3.2` and the package control identifies PX30/RK3326:
  <https://github.com/christianhaitian/arkos/tree/main/12152020>
- dArkOSen-R36S includes a Vulkan ICD that points to `libmali.so`:
  <https://github.com/djparentx/dArkOSen-R36S/blob/main/usr/share/vulkan/icd.d/mali_icd.json>
- muOS's internal repository identifies the `rg40xx-h` device and configures RetroArch with
  `video_driver = "gl"`:
  <https://github.com/MustardOS/internal/blob/main/device/rg40xx-h/control/retroarch.resolution.cfg>
- The Mesa documentation records G31 as GLES 3.1 on the current Panfrost path:
  <https://docs.mesa3d.org/drivers/panfrost.html>

So **3.2 is the advertised capability of the hardware and of libMali**, but it is not safe to
claim that every image with Panfrost exposes 3.2. The core requires only GLES 3.0 and accepts
Panfrost 3.1; it stays functional in software when not even GLES 3.0 is available.


## Current test procedure — R36S with ArkOS/AeolusUX/dArkOSen

The original R36S has two RetroArchs:

```text
64-bit: /opt/retroarch/bin/retroarch
32-bit: /opt/retroarch/bin/retroarch32
```

Use **only the 64-bit RetroArch** for Zeebx. The core is `aarch64` and does not load in the
`armhf` RetroArch. `retroarch32` is for old PortMaster cores and other 32-bit emulators.

### Paths

With the `EASYROMS` partition mounted by the system at `/roms`:

```text
Core:       /home/ark/.config/retroarch/cores/zeebx_libretro.so
Info:       /home/ark/.config/retroarch/cores/zeebx_libretro.info
ROMs:       /roms/zeebo/*.zip
BIOS/cache: /roms/bios/zeebx/
SoundFont:  /roms/bios/zeebx/aparelho/soundfonts/GeneralUser-GS.sf2
```

When the card is mounted on Linux, `EASYROMS` is the root that shows up as `/roms` on the device.
Do not create `EASYROMS/roms/zeebo`; the correct path is directly `EASYROMS/zeebo/`.

### Automatic installation (recommended)

Download these files into the same directory:

```text
zeebx_libretro.so
zeebx_libretro.info
GeneralUser-GS.sf2       (optional, recommended for real MIDI)
Double Dragon ...zip      (or other ROMs)
```

Mount the card's three partitions and run the repository's installer:

```bash
python3 tools/install_arkos.py \
  --rootfs /media/$USER/root \
  --roms /media/$USER/EASYROMS \
  --core zeebx_libretro.so \
  --info zeebx_libretro.info \
  --soundfont GeneralUser-GS.sf2 \
  --font tectoy.ttf \
  --rom 'Double Dragon (Brazil) (Es,Pt).zip'
```

Repeat `--rom` for each game. The installer:

- validates the core's ELF;
- makes a dated backup of the old core, `.info` and settings;
- installs the 64-bit `.so` and the `.info`;
- creates `EASYROMS/zeebo/` in the correct place (not `EASYROMS/roms/zeebo`);
- copies ROMs, SoundFont and the optional `tectoy.ttf` font;
- inserts/updates Zeebo in `es_systems.cfg` and validates the XML;
- does not unmount the card, so you can check the summary before removing it.

The installer does not depend on a local build. Use the `.so`/`.info` from the release's or
Actions' Linux AArch64 artifact. For old ArkOS with glibc 2.30, use the artifact compatible with
glibc 2.28; the regular CI core may require `GLIBC_2.34`.

### Manual installation

1. Mount the `root`, `EASYROMS` and `BOOT` partitions.
2. Copy the `.so` + `.info` pair to `home/ark/.config/retroarch/cores/`.
3. Put the games directly in `EASYROMS/zeebo/`.
4. Put the `.sf2` bank in `EASYROMS/bios/zeebx/aparelho/soundfonts/`.
5. If the system does not list Zeebo, check `etc/emulationstation/es_systems.cfg`.

The system entry must point to `/roms/zeebo/` and to the 64-bit core:

```xml
<name>zeebo</name>
<path>/roms/zeebo/</path>
<extension>.mod .MOD .zip .ZIP .7z .7Z</extension>
<command>sudo perfmax %GOVERNOR% %ROM%; nice -n -19 /usr/local/bin/retroarch -L /home/ark/.config/retroarch/cores/zeebx_libretro.so %ROM%; sudo perfnorm</command>
```

EmulationStation may also use the user override:

```text
/home/ark/.emulationstation/es_systems.cfg
```

If that file exists, it may take precedence over `/etc/emulationstation/es_systems.cfg`. After
any edit, validate the XML and restart EmulationStation. Do not leave a literal `&` inside XML
text: use `&amp;` (`2&gt;&amp;1` for the shell redirection).

### Recommended options on the R36S

In the 64-bit RetroArch, under **Quick Menu → Core Options**:

```text
zeebx_perfil = "portatil"
zeebx_limite_fps = "60"
zeebx_frameskip = "automatico"
```

The Handheld profile forces the wavetable, 22,050 Hz, 48 voices, an 8 MiB cache and no
supersampling. Volume and fog stay independent. If the game becomes unstable or loses the image,
use `zeebx_frameskip = "desligado"`; games that use `glReadPixels` turn frameskip off on their
own after the first read.

### The device's clock lies, and that fools whoever reads logs

On 2026-09-24 the R36S had its clock at **2025-11-07**: the session logs were born with that date,
and whoever looked for "today's log" found nothing. File names and dates come from the device's
clock, **not** from the computer that copied the files — so, when investigating a session, search
by **modification order** and read the contents, never by the date in the name.

The same caution applies to the drivers: both devices are Mali-G31, but **they are not the same
driver**. Measured in the devices' logs:

| device | driver | GLES |
|---|---|---|
| R36S (ArkOS) | libMali **r13p0** | 3.2 |
| RG40XX-H (muOS) | libMali **r20p0** | 3.2 |

The same code can behave differently on the two, and a number from one does not hold for the
other. The line that says which one is in use is the first one in the core's log:

```text
Zeebx: GL real vendor=ARM; renderer=Mali-G31; version=OpenGL ES 3.2 v1.r13p0-…
```

### Logs and diagnostics

Turn these on temporarily in the file:

```text
/home/ark/.config/retroarch/retroarch.cfg
```

```ini
log_verbosity = "true"
log_to_file = "true"
log_to_file_timestamp = "true"
```

The logs end up in:

```text
/home/ark/.config/retroarch/logs/
```

Important messages:

```text
Zeebx: MIDI synthesizer selected: Auto
Zeebx: MIDI sample bank at ...; the soundtrack plays with the samples
Zeebx: no MIDI sample bank; the soundtrack plays with the wavetable. ...
Zeebx: drawing on the GPU
Zeebx: automatic frameskip asked the frontend for the audio buffer report
```

If `no MIDI sample bank` shows up, the `.sf2` path is wrong or the bank was not copied. If the
core does not load, check `file zeebx_libretro.so`: it must say `ELF 64-bit ... ARM aarch64`.
A regular Linux AArch64 core may require `GLIBC_2.34`; old ArkOS with glibc 2.30 needs the
compatible artifact built with `cargo zigbuild` and the `r36s_compat.c` shim.

## Current test procedure — RG40XX-H with muOS Loose Goose

Use the **AArch64 RetroArch**. Do not use `retroarch32`.

### muOS paths

On the system card (`ROOTFS`):

```text
Core:      /opt/muos/share/core/zeebx_libretro.so
Info:      /opt/muos/share/emulator/retroarch/info/zeebx_libretro.info
Config:    /opt/muos/share/info/config/Zeebx/zeebo.cfg
SoundFont: /opt/muos/share/emulator/retroarch/system/zeebx/aparelho/soundfonts/GeneralUser-GS.sf2
```

On the ROMs card:

```text
ROMs: /ROMS/Zeebo/*.zip
```

The expected assignment file is:

```text
/opt/muos/share/info/assign/Zeebo/zeebx.ini
```

```ini
[zeebx]
name=Zeebx
core=zeebx_libretro.so

[launch]
prep=
exec=/opt/muos/script/launch/lr-general.sh
done=
```

Do not put ROMs in `ROMS/ROMS/Zeebo`. In two-card setups, the correct directory is `ROMS/Zeebo`
on the card that muOS exposes as the ROMs partition.

### Recommended options on the RG40XX-H

To compare quality and performance:

```text
zeebx_perfil = "padrao"
zeebx_midi_backend = "auto"
zeebx_soundfont_taxa = "44100"
zeebx_limite_fps = "60"
zeebx_frameskip = "automatico"
```

To prioritize smoothness on the H700:

```text
zeebx_perfil = "portatil"
zeebx_limite_fps = "60"
zeebx_frameskip = "automatico"
```

After changing an option, look at the label text: volume, fog, frameskip and enhancements apply
right away; MIDI and rasterizer require reloading the content; rate/voices/cache apply from the
next song or discard on.

### muOS/RetroArch logs

On muOS, temporarily turn on `log_verbosity`, `log_to_file` and `log_to_file_timestamp` in the
64-bit RetroArch's `retroarch.cfg`. The logs usually end up in:

```text
/home/ark/.config/retroarch/logs/
```

The `SYSTEM_DIRECTORY` the core should report is usually `/roms/bios`; so the expected message for
the SoundFont is:

```text
/roms/bios/zeebx/aparelho/soundfonts/GeneralUser-GS.sf2
```

### Rollback

Always make a backup before replacing the core:

```text
zeebx_libretro.so.before-YYYYmmdd-HHMMSS
```

If the new core does not load, restore the previous `.so` and keep the matching `.info`. The most
common problem on old images is `GLIBC_2.34 not found`; in that case changing RetroArch settings
does not help — you need a core compiled against a compatible minimum glibc.
