# 20 — The Boomerang, and controllers with a sensor in its place

The Boomerang is the Zeebo's motion controller: d-pad, buttons 1 and 2, HOME and an
accelerometer, wireless, with a USB receiver. The Boomerang Sports games (Queimada, Tênis, Vôlei,
Peteca), Crash Nitro Kart and the Zeebo Extreme games recognize it and switch to motion play. Here
it is emulated, and what moves it is the sensor of the controller chosen for the port: a Wii
Remote, a Pro Controller, a DualShock 4, a DualSense.

None of this came from documentation: the report format was read in the code of Zeebo Sports
Queimada (`dodgeball.mod`) and checked against Crash Nitro Kart 3D (`cnk2.mod`).

## 1. How the game recognizes the device

By the VID/PID pair from `GetDeviceInfo`. Queimada classifies it at `0x35130`:

| VID:PID | Type | Entry in `hid_devices.cfg` |
|---|---|---|
| `1A5C:3033` | 3 | "Zeebo Game Controller" — the **Z-Pad** |
| `15A2:0003`, `1A5C:7033` | 4 | "Zeebo Accelerometer Controller" — the **Boomerang** |
| `1EAA:0135` | 5 | "New Zeebo Game Controller" — the **Dragon** |

Z-Pad and Dragon have the same buttons and axes (the grip changes), so for the game the difference
is only that pair. Which pair belongs to which model is a deduction from the order of the file's
entries; `1EAA:0135` is also the one in the USB descriptor captured from a console, and it is what
the emulator has always announced. The three became "the console sees" options on each port
(`Device::ZPad`, `Device::Controller` — the Dragon, whose serialized name stays the old `Controle`
— and `Device::Boomerang`).

One receiver serves **two** Boomerangs: `GetConnectedDevices` lists a single device for all the
ports with a Boomerang.

## 2. The report: the position structure is not axes

`AEEHIDPositionInfo` has `bRelativeAxes` and 24 integers. On the Boomerang they are the receiver's
raw report, and the game takes it apart (`0x4231c`, `0x42458`, `0x40b0c`, `0x413d0`, `0x412f4`).
Counting from the first integer after `bRelativeAxes`:

| Field | Content |
|---|---|
| 1, 2, 3 | accelerometer X, Y, Z: one byte per axis, center `0x80` |
| 4 | buttons, **inverted logic** (a bit at 0 is pressed): bit 0 button 1, 1 button 2, 2 HOME, 3 left, 4 down, 5 right, 6 up |
| 5 | bit 0: which of the two players the packet belongs to; the rest is the type. Type 0 is "disconnected"; 1 to 4 connect, and field 6 carries a device datum that depends on the type (type 1 carries, in bits 4–3, the sampling rate the calibration uses) |
| 8 | 8-bit counter. The game expects it to advance one by one; a jump marks a lost packet |

At startup the game sends a command through `Rumble` (`0x2000|…`, `0x300`) — the Boomerang's entry
in the cfg has a vibration output report — and **waits for the counter to change** to consider the
receiver alive (`0x42350`, up to 5 million attempts). With the counter stopped, the game freezes
there.

### Buttons only count on a read without a new packet

Queimada reads the position three times per frame and processes a player's buttons through that
player's virtual table, every frame. In practice, a single player receiving a packet on every read
never had the press processed; with the two players alternating, each one has reads "without a
packet of its own" in between. That is why the emulated receiver (`Machine::boomerang_packet`):

- produces a new packet every 10 ms of virtual time (`BOOMERANG_PERIOD_US`), and the reads in
  between repeat the last one;
- **always alternates the two players**; the one without a controller goes as type 0.

### The disconnected player carries the first one's acceleration

Crash Nitro Kart reads the accelerometer **without looking at the player bit**. With the second
player going still, half the reads were the motion and half were rest: the kart turned at random and
the calibration passed instantly. The disconnected player's packet carries the first player's
acceleration — Queimada ignores the acceleration of a disconnected player.

## 3. Crash Nitro Kart reads another way

It registers `RegisterForPositionChange` and only reads the position inside that notification,
through the SDK's normalized analog sticks (`AEEHIDThumbsticks.c`, `0x496d8`), which find each axis
by the UID from `GetAxesInfo`. Two consequences:

- **The receiver wakes the game up nonstop.** `Machine::boomerang_report` fires the signal every
  10 ms with a Boomerang port, even with the acceleration unchanged. Without it the calibration got
  one read and waited for the others forever.
- **The axis table is the Boomerang's**, with Z and RZ swapped relative to the controller
  (`Z=0x0106C4CF`, `RZ=0x0106C4CE`, as in the cfg). With the controller's table, gravity fell in the
  wrong field and the calibration never accepted the reading.

Its calibration (`0x45a64`) wants 15 samples with X and Y between 101 and 159 and Z between 162 and
220, and writes `ACCELHORIZONTALCENTER`/`ACCELVERTICALCENTER` when done. The Z range centers 1 g at
about 63 units; the emulator uses 60 (`SCALE`). The steering is linear:
`(X−127)·0.9659 + (Y−127)·0.2881` — gravity projected on the controller's length, rotated about
17° by the curve of the arm.

## 4. The frame of reference

Queimada rotates X and Y by 20° (`0.94` and `0.342`) before using them; the emulator applies the
inverse rotation when building the packet, so the game arrives at the acceleration delivered.

CNK says to hold the Boomerang lying flat, with both hands and the face toward the player, turning
it like a steering wheel. **The Boomerang's length is its X; the Wii Remote's is its Y.** The Wii
Remote held sideways (d-pad on the left) enters the Boomerang's frame of reference as `[y, x, z]`.
The direction of X was set by hand: with the opposite sign, turning right sent the kart left.

## 5. The Wii Remote

`input/wiimote.rs`. Linux's `hid-wiimote` driver splits the controller into input devices:
"Nintendo Wii Remote" (buttons) and "Nintendo Wii Remote Accelerometer" (`ABS_RX/RY/RZ`, about 100
units per g). gilrs sees at most the buttons, so the reading is ours: one thread per device reads
the raw `struct input_event`, and a search thread finds new controllers every second, grouping
buttons and accelerometer by the parent HID device. It needs no library and no extra permission —
the nodes already have an ACL for the session user. Outside Linux there is no reading.

The controller list shows "Wii Remote 1" and "Wii Remote 2", and its buttons are mapping sources
like any controller's (see section 6). With nothing chosen, the first Wii Remote goes to the first
Boomerang, with the buttons added through the typical mapping.

### Calibration in the settings

"Calibrate" gathers half a second of still readings and stores the offset and the scale in
thousandths of g (`MotionCalibration`), so that rest face up becomes `[0, 0, 1]`. It **refuses** a
reading that is not face up and does not correct an offset larger than 0.25 g — and a stored one
outside that is ignored. Calibrated sideways, the offset was 1 g: CNK saw doubled gravity (Z = 249)
and never left the calibration screen. The games still run their own calibration on top of this
one.

### The calibration notice

An optional toast in the corner of the game window shows the Boomerang tilting and whether the
controller is still, and closes when the game confirms. No game tells the system, so the signals
are the debug messages: "calib" or CNK's `ACCEL 1` open it; `ACCEL…CENTER` closes it.

**The Boomerang's first read is not a signal:** every game reads the controller at startup, and the
Boomerang Sports games only calibrate after character selection. They also leave no text trail at
that moment (the name of Vôlei's calibration screens only goes through the BREW functions during
preload), and the calibration state sits in an object whose layout changes among the four games —
detecting them calls for reading each one.

## 6. The sensor belongs to the chosen controller

The Boomerang port always looked for a Wii Remote. With a Pro Controller chosen on it, the
Boomerang stayed still: the controller answered through the buttons and its sensor was never read.

The Linux drivers put the sensor on a sibling device of the controller, marked with
`INPUT_PROP_ACCELEROMETER` (bit 6 of the `properties` file): "Pro Controller (IMU)" next to "Pro
Controller", in `hid-nintendo`; "… Motion Sensors" in `hid-playstation`. `input/sensors.rs` finds
those nodes and reads them like the Wii Remote's. With a gyroscope, acceleration comes in
`ABS_X/Y/Z` (and rotation in the `R` ones); the scale is the `resolution` of `input_absinfo`, which
the drivers fill in units per g (4096 on the Pro Controller).

The sensor is tied to the controller by the **system** name (gilrs's `os_name`, not the mapping
name, "Nintendo Switch Pro Controller"), by the VID/PID pair and by the order among identical
controllers. The controllers are held in the normal grip, which is already the steering-wheel one.
`hid-nintendo` puts the Pro Controller's length on Y, like the Wii Remote: with the axes passed
straight through, the controller standing up, facing the player, rotated the preview ninety degrees
to the right. Nintendo controllers (VID `057e`) swap X and Y, with the direction of X opposite to
the Wii Remote's (`[-y, x, z]`): with Y passed as it came, turning left sent the kart right. The
others pass straight through until someone measures.

**The sensor node usually comes without permission.** udev gives the session user access
(`uaccess`) to what it marks as a joystick, and the sensor is marked as an accelerometer: here the
Pro Controller's `event23` has the ACL and `event22`, the IMU's, does not. The Boomerang preview and
`zeebx controls` say when this is the case, with the rule that solves it:

```
SUBSYSTEM=="input", KERNEL=="event*", ENV{ID_INPUT_ACCELEROMETER}=="1", TAG+="uaccess"
```

The preview offers a button, "Unlock the sensor", that does this through `pkexec`: the system's
password window appears once, and a single command writes
`/etc/udev/rules.d/70-zeebx-sensores.rules`, reloads the rules and re-runs the input devices
through them (`udevadm trigger --action=change`), which gives the ACL to the sensors already
plugged in. The number 70 comes before `73-seat-late.rules`, which is what applies `uaccess`.
Reading a sensor without permission is retried every second, so it comes back on its own, without
reconnecting the controller. If `pkexec` fails (no password agent, password refused), the rule is
shown to be created by hand.

With no controller chosen, what held before still holds: the first Wii Remote to the first
Boomerang.

### The Wii Remote is a mappable controller

Its buttons were added to the port's through a fixed table, and choosing it on a port kept the
keyboard mapping — as a Z-Pad it only had the d-pad and two buttons. Now the buttons are sources
like gilrs's (`WiiUp`, `WiiA`, `Wii1`, `WiiPlus`, `WiiHome`…), capturable on the controls screen,
and choosing it brings the typical mapping: d-pad on the d-pad, 1 and A on button 1, 2 and B on
button 2, minus and plus on buttons 3 and 4, HOME on HOME. Whoever had already chosen it gets that
mapping the first time the emulator opens. The fixed table remained only for the Boomerang with no
controller chosen.

## 7. Tools

- `zeebx wiimote [--seconds=N]` shows the controller live.
- `zeebx controls` lists each controller's sensor, with the acceleration or the permission
  warning.
- `zeebx session <zip> --boomerang` puts a Boomerang on port 1; `--motion=ms:x:y:z,...` gives the
  acceleration from each instant on, in g; `--wiimote` uses the connected Wii Remote.
- `zeebx run <zip> --ports=boomerang` to trace `IHID` with `--trace=HID`.
