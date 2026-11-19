# Zeebx on the PlayStation Vita

The core is the same. What changes is who paints, who plays the sound and who presses the button.

This app doesn't go through RetroArch. The core RetroArch loads is still in
[`frontends/vita`](../vita/build.sh), a static `.a`. Here the binary is the `eboot.bin` of a VPK:
the game list, the frame and the sound belong to this process.

## What it needs

A Vita with Homebrew (Henkaku or the equivalent). The VPK doesn't install on a stock device.

The image goes in pixel for pixel, centered. Stretching 640×480 to fill the screen repeated one
column and skipped the next, and 3D at half resolution made the drawing blocky. When a frame
doesn't fit in the refresh, the following ones run without drawing until the clock catches up. On
startup the app asks for 444 MHz on the ARM and 222 on the bus; what the device accepted goes in
the log. The speed on this device has not been measured. The processor here is the interpreter:
dynarmic doesn't emit ARMv7. The music uses the soundfont.

## What hasn't been measured on the device yet

This applies to this app and to the RetroArch core, which use the same core.

- **The game's memory.** On the Vita the game heap is 48 MiB, and not 64 as on desktop: with 64,
  vitaGL couldn't find the frame's temporary block. The region the emulator reserves for itself
  (`INTERNAL_BASE`, in `src/loader/mod.rs`) adds another 16 MiB, and here newlib reserves the
  whole block, without desktop's overcommit. Nobody has measured yet whether 48 + 16 leaves room
  for the frame, or whether Quake runs with 48. If a game closes without warning or says it ran
  out of memory, this is the first suspect.
- **Save states across platforms don't work.** With a heap of a different size, a state saved on
  desktop, on Android or on another platform's core is refused here, and one from here is refused
  there. The emulator says the region doesn't match and the game carries on as it was; nothing is
  corrupted.

## Where the games go

`ux0:data/zeebx/roms`, in VitaShell. A `.zip` or a `.mod` both work, with the same scan as
desktop. Saves go in `ux0:data/zeebx/saves`. What the app logs — including a panic — goes to
`ux0:data/zeebx/zeebx.log`.

## The buttons

| Vita | Zeebo |
|---|---|
| X | Button 1, bottom |
| Square | Button 2, left |
| Triangle | Button 3, top |
| Circle | Button 4, right |
| L | ZL |
| R | ZR |
| Start | Start |
| Select | HOME |
| D-pad | D-pad |
| Left analog stick | Stick |
| Right analog stick | The other stick |

In the list, the D-pad chooses, X starts, O closes the app and Triangle rescans the folder.
Holding Select and Start for half a second leaves the game and goes back to the list. A stick at
rest doesn't move: there is a small dead zone around the center.

## Building

The toolchain is the `vitasdk/vitasdk` image. On a Mac:

```bash
./frontends/vita-standalone/build.sh
```

The VPK comes out in `frontends/vita-standalone/out/zeebx.vpk`. Inside the image, or in the
`vita.yml` job:

```bash
./frontends/vita-standalone/build.sh --local
```

`cargo test -p zeebx-vita` proves the mapping, the list and the frame drawing on desktop. It
doesn't bring up the device.
