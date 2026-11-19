# 09 — Input

## Three layers

```
host keyboard / controller   →   input/bindings.rs   →   input::Pad   →   IHID / IHIDDevice
     (who pressed)               (what triggers     (the console's    (how the game reads)
                                   what)              state)
```

No layer knows about the one before it. `input/bindings.rs` knows neither keyboard nor gamepad —
it only says *what* triggers *what*; whoever knows whether the `Z` key is down is the interface.
That is what lets it be tested without any hardware.

## The console's controller

`input/mod.rs` has the real table: **18 buttons and 4 axes**, with each one's UID. The UIDs come
from the console's `hid_devices.cfg`, and there are three deliberate additions, each with its
reason:

- **The four D-pad directions as buttons.** The console file has them only as `X`/`Y` axes, but
  games read them as buttons: with these UIDs present the Quake menu moves; without them the
  cursor stays put however much the axis changes. A digital D-pad on USB HID is usually reported
  both ways.
- **`Button_1` and `Button_3`.** They are missing from the console's list, which has `2` and `4`
  but puts an axis UID in place of one of them. Quake's own help screen names all four — "press
  1 to jump" —, so they exist on the controller.
- **`HOME` is BREW's `Back` and `Start`, together.** The controller has HOME printed on its shell,
  and `hid_devices.cfg` maps that physical button to `AEEUID_HIDJoystick_Back`. Double Dragon
  asks for "PRESS THE HOME BUTTON" and only responds to `back`. See *HOME arrives with two UIDs*,
  below.

### HOME arrives with two UIDs

Quake asks "Press the Home button to call the menu", and in the middle of a level the menu did not
open. Its UID table, at `0x7a4` in the module, subtracts `0x0106c3fe` from the UID and jumps
through a table: `Start` (`0x0106c402`) becomes index 9, `Back` (`0x0106c403`) falls into "unknown
button". Index 9 becomes `AVK_CLR`, and Quake binds the key that comes out of it, 174, to
`togglemenu` (`Key_SetBinding` at `0x8fcc`). Double Dragon is the opposite: on the title screen,
`back` opens the menu and `start` does nothing.

Both run on the same device, and the controller has a single button in the middle. So HOME
arrives with both UIDs, which is what `Pad::with_home_on_both` does at the port's input. **It was
not measured on the console.** What supports the choice is a survey of the UID tables that start
from `0x0106c3fe`:

| Game | `Start` | `Back` |
|---|---|---|
| Quake | recognizes | no |
| FIFA 09, Need for Speed, Quake II, Action Hero 3D | no | recognizes |
| Alien Breaker, Tork and Kral, Heavy Weapon | stores in a bit | stores in a bit |

In the three on the last row the library stores each button in a bit (`0x200` for `Start`,
`0x100` for `Back`), and I found no read of the `Start` bit in any of them. No game in the survey
gives the two different actions. Only in that direction: a hand-mapped `start` stays just `Start`.

### Axes

`X`, `Y`, `Z` and `RZ`, in words 1, 2, 3 and 6 of `AEEHIDPositionInfo`. The range is **an unsigned
byte, `0..=255`, resting at 128**.

That is not a choice: it is written in the game. Zeebo F.C. Super League converts each axis like
this, and the same snippet is in Tênis, in Zeeboids and in every title that uses this SDK layer:

```text
mvn   r0, #0x7f        ; r0 = -128
sxtah r4, r0, r4       ; value = (int16)axis - 128
```

It subtracts 128 to find the center, so the device's center is 128 — and it is also what the USB
sticks that `hid_devices.original.cfg` itself lists report, the Logitech Dual Action and the
RumblePad2. The Zeebo controller's descriptor is in the dump, but the part of the report that
would carry the limits came as `** UNAVAILABLE **`; before this, a signed 16-bit guess was used,
and the guess was wrong.

**The symptom of sending zero was the character walking on its own.** Zero is not the center, it
is the end stop: with the stick at rest, every game on this layer read `-128` on all four axes and
walked into a corner. Zeeboids escaped by accident — it treats "all four axes exactly at the
minimum" as "there is no stick here" and zeroes everything, which is a defense against exactly
what we were doing.

For the same reason, the twenty axis words the controller does **not** use go at the center, not
at zero: a game that reads one of them finds an axis at rest, not one pushed against the end
stop. Word zero of the struct stays zeroed, because it is not an axis — it is `bRelativeAxes`.

Inside the emulator the axis is stored **centered on zero** (`Pad::axes`, with `±128` travel),
which is the form the game works with after that subtraction. What translates to the device's
range is `IHIDDevice`, in the one place where it is the console doing the reading.

`Y` is **inverted** relative to the controller library: in HID the vertical axis grows downward,
and up is the low value. The right pair follows the same convention.

**The D-pad and the stick are separate channels.** Although the descriptor lists `X` and `Y`, the
digital D-pad arrives as `DPad_*` buttons; the left stick feeds `X` and `Y`. **We do not mirror
one onto the other by default**: releasing an arrow would send a false analog return-to-center
change, and games that use changes instead of state would start navigating twice. The mirror
exists as an option that is off, for games that only listen to the axis — see *The D-pad mirror,
and why it is an option*, below.

Whoever answers `GetAxesInfo` does not return values: it returns, in each word, the **UID of the
axis that occupies that word**. That is how the game finds out where each direction is, and that
is why the UID table must be right — a wrong UID raises no error at all, the game just does not
find the axis.

Three of the four UIDs are a **literal transcription** of the Zeebo controller entry
(`VID:0x1EAA:PID:0x0135`) in the console's `hid_devices.original.cfg`:

```text
AXIS:X:0x0106C40C      <- not this one
AXIS:Y:0x0106C4D1
AXIS:Z:0x0106C4CE
AXIS:RZ:0x0106C4CF
```

The file's `X` holds the UID of `Button_3`, and the swap is mirrored: the `BUTTON:3` of the same
entry holds `0x0106C4D0`, which is the `LeftThumb_X` of the file's other entries.

**We undo that swap, and it was a measurement that decided it.** With `0x0106C40C` on `X`, the
stick does not move left and right in any game — the game scans the `GetAxesInfo` table looking
for an axis UID, finds none for `X` and never stores its field; `Y`, `Z` and `RZ` move and only
the horizontal stays dead. With `0x0106C4D0`, the whole stick moves.

This does not contradict the lesson of the next section. The question there was "what are the
device's UIDs", and the source is the file. The question here is "what does the game look for in
the table", and the only possible source is the game.

### The fix that wasn't

Worth recording because it is a method error, not a code error.

I "fixed" `X` once, deducing it from the game binaries: the four values `c4ce`, `c4cf`, `c4d0`
and `c4d1` show up in dozens of titles, always in pairs inside the same constant pool, which looks
exactly like two sticks — and `0x0106c40c` shows up in no game at all. The conclusion was that
`X`/`Y` were `c4ce`/`c4cf`.

The deduction was right **for the file's other entries**, the PC controllers', where in fact
`AXIS:X:0x0106c4d0` and `AXIS:Y:0x0106c4d1`. For the Zeebo controller, no. It was plausible,
coherent and wrong, and only fell when the file turned up.

The lesson: **a primary source beats inference**. A well-built deduction from indirect evidence
can explain everything that was observed and still describe another device.

### Who reads what

Counting the `IHIDDevice` calls in ten seconds of each of the 62 ROMs:

- **Only the axis and the scan, never the button event**: the Data East arcade ports (Magical
  Drop 3, Karnov's Revenge, Wizard Fire, Street Hoop, Spin Master, Caveman Ninja, Dark Seal, Super
  BurgerTime). They call `GetPositionState` some five hundred times and `GetNextButtonEvent`
  **zero**.
- **Both, every frame**: the Zeebo Sports games, zeetris, Zeeboids, the Extreme series.
- **Almost only the button**: Quake and Tork and Kral.

That is why both channels stay. Turning off either one leaves part of the library with no input
at all — it was measured: with the D-pad only on the axes, the Tênis menu does not move; only on
the buttons, it moves and stays.

### The D-pad mirror, and why it is an option

The previous section names the games that only listen to the axis: the Data East arcade ports
call `GetPositionState` more than two thousand times in forty seconds and `GetNextButtonEvent`
**zero**. For those, a controller's D-pad does nothing — on the console it is a button, and no
button reaches the channel they read. Measured, with the D-pad pressed in all four directions,
five seconds each:

| game | axis off center, without the option | with the option | with a real stick |
|---|---|---|---|
| Rally Master Pro | 0 | 4 | 4 |
| Magical Drop III | 0 | 1212 | — |
| Zeeboids | 0 | 600 | — |

The middle column is the one that matters, and it comes from `Machine::off_center_axis_reads`:
**the call count says the game asks; this one says the answer arrived.** Without it, a port that
queries the axis every frame looks the same with the D-pad released and pressed — and that almost
left this change without proof, because the `GetPositionState` count does not change with the
D-pad.

**It is an option, off by default, and there are two.** In the core, `zeebx_dpad_to_analog_p1`
and `zeebx_dpad_to_analog_p2` (`disabled|enabled`), one per port; in the standalone, the
equivalent checkbox on the controls screen, which is also per port. Two and not one because the
console has two ports (`input::PORTS = 2`) and they are two players: whoever plays with a stick
on controller 1 does not decide for the owner of controller 2. In the standalone the setting lives
in `Player::dpad_to_axes` and goes down to `settings.json` on its own.

The reason it is not the default is the same one that took the D-pad off the axes: **the defect is
per game**. Zeeboids queries both channels every frame — 1,212 `GetPositionState` and 1,219
`GetNextButtonEvent` in forty seconds —, so with the option on it receives the same press twice;
that is what undid the attempt of `e705840` in `4418fe9`. Games that read both channels leave it
off; games that only read the axis turn it on.

**The order matters, and it is the detail nobody guesses.** The mirror writes the axis *and
returns it to the center* when the direction is not pressed, because that is what a stick does —
and it is the same "false return-to-center change" that `Pad::press` notes as the risk of the
idea. That forces the mirror to run **before** the analog loop, in `Player::pad`, and before the
harness script: the real stick, when it exists and is outside the dead zone, must have the last
word. Written in the opposite order, the mirror would erase the stick resting at the center and
the D-pad would beat the analog.

The three frontends that go through [`Player::pad`] obey the option: the desktop, the windowless
one (through the `dpad_to_analog` key of each `[portN]` section of `config.ini`) and the Libretro
core. **The Android frontend builds its own `Pad`** — its on-screen controller and its gamepad do
not go through here —, and so the option does not reach it.

To measure without a window:

```sh
zeebx bench "<game.zip>" --seconds=40 --keys=15000:up:5000,20000:down:5000 \\
    --dpad-on-axes
```

The summary has the `axis:` line with the off-center reads, and the `input:` line with the calls.

### There is a third channel, and in it the list order is everything

The Data East ports do not stop at the axis: they call `GetButtonInfo` **sixteen times per
frame**, with indices 0 to 15, and store each one's state in a vector indexed by position. The
loop is at `0x1f2a8` in the Bad Dudes module, and it is literal — `mov r1, r4`, call,
`ldr r3, [sp, #4]`, `strb r3, [r5, #0x13c]`, `add r4, r4, #1`, `cmp r4, #0x10`.

For a game that reads this way, **the UID does not matter: the position does**. While the list
opened with `Button_2` and had `Right_Shoulder_Upper` right behind it, the controller's button 2
reached the arcade as the right trigger and 4 as 3 — and `b2` and `b3`, which were at positions
16 and 17, never got read. That is why the four face buttons open the list, in the order 1, 2, 3,
4, and what is not a device button (the `LeftThumb_X` the console file left in the middle, and the
second appearance of `Right_Shoulder_Upper`) went to the end, outside the range the arcade scans.

The D-pad follows the same logic: **it stays in UID order, up, left, down, right** (positions 12
to 15). The SDK samples' `GamepadMgr` reads the `id` from `GetNextButtonEvent`, which is the
position in the list, and stores the state in a vector indexed by it (`0x16b68` in Dragon Vs
Chicken). In the order up, down, left, right, the demo walked down with left and vice versa.
Tênis, Rolima and Bad Dudes do not use that loop.

`Z` and `RZ` had nothing feeding them until the right stick was wired to them. **The direction of
these two is an assumption**: the file names the axes without saying their direction, so we follow
the same convention as the left pair, and the settings screen has an "Invert" checkbox.

### The `type` of `AEEHIDDeviceInfo` is a UID

Before creating the device, some games check what `IHID::GetDeviceInfo` says about it. The first
field of the struct is the **device type UID** — the same joystick `0x0106c3fd` the game passes to
`GetConnectedDevices`, and `0x0106c3fc` for keyboard —, not a small number. We answered `1`.

**Bad Dudes vs. DragonNinja** compares that field with `0x0106c3fd`; with `1` it never called
`CreateDevice` and stayed stuck on the opening notice, seeing no button at all. It also skips the
device with product `3` and vendor `0x15a2`, which is the Boomerang receiver. The other Data East
ports do not check the type, and that is why they worked.

**And an empty connection-event queue answers `EFAILED`.** `GetNextConnectEvent` answered success
with zeroed fields, which for the game means "there was an event" — a device with identifier zero
connecting, on every query. While `type` was `1`, the Zeebo Extreme games discarded the event and
moved on; with the right UID they started handling it, and stayed in `GetNextConnectEvent` and
`GetDeviceInfo` forever, without arming a timer or drawing. The session then ended on its own,
for lack of anything to do, and the game "did not open". An empty queue is `EFAILED`, as in
`GetNextButtonEvent`.

## The button numbering is the device's

The controller's diamond is **not** numbered in the order the eyes read: **1 is at the bottom, 2
on the left, 3 at the top and 4 on the right** (checked against the controller's official
images). The mapping to the RetroPad preserves the **position of the hand**, not the number:

| device | where it is | RetroPad |
|---|---|---|
| Button 1 | bottom | `B` |
| Button 2 | left | `Y` |
| Button 3 | top | `X` |
| Button 4 | right | `A` |
| HOME | in the middle | `Select`; in the standalone, the host's `Start` also lands on it |
| ZL / ZR | shoulders | `L` / `R` |
| D-pad | cross | `D-Pad` |
| two sticks | — | left and right analogs |

**This was once wrong in two ways at the same time** (issue #41): the core's mapping screen
labeled `B` as Button 1 while the reading delivered `B` as Button 2 — two parallel lists that
diverged silently —, and the art map in `assets/controller-map.svg` numbered the diamond as 1 at
the bottom, 2 on the right, 3 on the left and 4 at the top. Now the core's table is **a single
one** (`RETROPAD_BUTTONS`: reading and labeling both draw from it), the art follows the device's
numbering, and a test pins both things — descriptors and reading can no longer disagree.

**Whoever already had a saved mapping is reached too.** `settings.json` overrides the default,
and an old mapping would keep delivering east on `b2` — the whole defect.
`Player::migrate_action_buttons` swaps the four action buttons for the new ones **when they are
still exactly the old ones**; whoever changed any of them keeps what they wrote, which is the same
rule as the axis convention migration. In RetroArch there is no migration to do: the default
mapping there is by physical button, and the fix applies as soon as the new core goes in.

## Configurable mapping

`input/bindings.rs`. The mapping is stored **by name** — the key name, the host controller's
button name, the Zeebo button's name — and not by index. Indices change when a table changes;
names survive, and that is what keeps a configuration file written today valid later.

- A Zeebo button accepts **more than one source**: that is what lets `Space` and `X` do the same
  thing, and the keyboard keep working with a controller connected.
- `Source::Axis { name, positive }` is "did the player push this way?", and serves to trigger a
  **button**.
- `AxisSource { name, invert }` carries the axis's **whole value**, which is what an analog is.

The stick does **not** press the D-pad. They are different controls, and wiring the two would make
both act together and neither alone. The analog value goes in **after** the buttons and only
outside the dead zone: that way it adds travel to what the D-pad wrote, instead of erasing it
when the stick is at rest.

`Y` is inverted because in HID the vertical axis grows downward and in the controller library up
is positive. Getting that sign wrong inverts the vertical axis of every game that reads it —
there is a test.

**Event notification is per device, not per event type.** The game registers the `ISignal` of
`RegisterForButtonEvent` and of `RegisterForPositionChange` **on the device object**, one per
connected controller. Storing the signal only by the registration name made the second one erase
the first: every change woke controller two's callback, the game asked the wrong device, found no
event and controller one did nothing. Treino Cerebral got stuck on "press button 1" as soon as
port two came in. The key is the pair `(registration, port)`.

**The port's `device_kind` is what the console enumerates, and the host controller is something
else.** There are two fields: `device_kind` says whether that port is a Z-Pad, a controller, a
Boomerang or a keyboard — and the joystick `GetConnectedDevices` only lists the first three —,
while `device` says which host device the port reads from. Marking the second
controller on a keyboard port left the port out of the games' count, and the two-player option
stayed greyed out with both ports connected. Choosing a controller in the list now adjusts the
device along with it, and an old configuration is corrected on load.

**The host controller is also stored by name, and two identical ones have the same name.**
Someone who connects two devices of the same model and marks the second on port 2 saved the same
string as port 1, and the lookup — which scans the system's list looking for the name — always
returned the first: port 2 responded to controller 1. The list numbers the repeats (` #2`, ` #3`)
and the lookup uses the same list, so it is still a name, which survives unplugging and
replugging a device.

## Configuration migration

`Controls::adopt` adjusts a file written by an earlier version: it gives axes to whoever has a
controller and did not have them yet, takes the stick off the D-pad buttons and gives the vertical
inversion back to whoever lost it — there was a version that saved all four axes straight, and
the default going back to being right does not fix a file already written.

It exists because **a new field's default is not always empty**. Without it, someone who already
had a controller configured was left with silent sticks and would have no way to guess why.

## Real controllers

`input/gamepads.rs`, on top of `gilrs`. A computer with no controller — or without permission to
read them — cannot stop the emulator from opening: the failure becomes "no controller" and the
keyboard carries on.

Buttons come before axes in capture: someone pressing the D-pad of a controller that also reports
it as an axis wants the button, which is the more specific one.

### The controller reconnected with the emulator open

A Pro Controller turned off and back on with the emulator open did not come back: only a restart
did it. `gilrs` only tries to open a device on the udev `add`, and with `joycond`, common on Arch
and derivatives, a Nintendo controller's node is born closed. Measured on 2026-10-11, with a udev
monitor that opened the node the way `gilrs` does:

| Instant | Event | Node | Open |
|---|---|---|---|
| 23.67 s | `add` (tag `joycond`) | `crw-------` | `EACCES` |
| 24.17 s | — | `crw-------` | `EACCES` |
| 24.81 s | `change` (tag `uaccess`) | `crw-rw-rw-` | opens |

`gilrs` ignores the `change`. When the emulator opens, the controller is already released, and
`gilrs`'s initial scan finds it: that is why restarting solved it.

On Linux, `Gamepads::poll` checks once per second which `event*` nodes in `/dev/input` the process
can open (`access`, which respects udev ACLs). A node that becomes openable is noted. If by the
next check `gilrs` has not connected any controller, it did not see that node, and it is created
again. **Only in that case**: recreating reorders the list, and a port with no chosen controller
takes the one in its position. A regular controller `gilrs` connects on its own on `add`, and the
list stays as it is. Measured with a virtual controller (`uinput`): the order does not change.

### What the log says about controllers

With session recording on, the log states the controllers `gilrs` sees on opening (name, system
name, VID/PID and whether there is an SDL mapping), each connection and disconnection, and the
first event of each controller with the raw code, including from a button the mapping does not
name. A capture that waits more than 5 s with no sign from any controller shows a hint on the tab
itself: the USB cable, Steam and PS Remote Play, which hold on to the controller, and pairing
again.

It is the diagnosis for issue #59, the PS4 controller on an M4 Mac that capture did not see.
Without a mapping, macOS `gilrs` still names the raw buttons 1 to 21 by position, so a DS4 that
reached the emulator would be captured, with the buttons swapped. Capture waiting forever means
nothing arrived: the controller did not open, or another program is holding it. The report is
what tells the two apart.

## The two ports

The Zeebo has **two USB ports**, and the console enumerates them: the Z-Wheel's
`GetConnectedDevices` passes capacity 2. Before this the emulator had one controller and that was
it — `input::PORTS` is now 2, and each port is configured separately.

Each port can have one of these devices:

| Choice | Device UID | VID:PID | What the game sees |
|---|---|---|---|
| `dragon` (`controle`) | `0x0106c3fd` | `1EAA:0135` | The Dragon, with the 18 buttons and the 4 axes |
| `zpad` | `0x0106c3fd` | `1A5C:3033` | The Z-Pad: the same buttons and axes, another identifier |
| `boomerang` | `0x0106c3fd` | `15A2:0003` | The Boomerang, with the motion report — see [20](20-boomerang-and-wii-remote.md) |
| `keyboard` (`teclado`) | `0x0106c3fc` | — | A USB keyboard |
| `none` (`nenhum`) | — | — | The port is not enumerated |

The UIDs and the pairs come from the console's `hid_devices.cfg`, like the rest of the table.
Z-Pad and Dragon differ only in the VID/PID pair, which the Boomerang Sports games use to choose
how to handle them.

On the command line: `--ports=dragon,keyboard`. In the interface, each port has its own mapping
tab. Only the first port starts connected — a configuration file written before ports existed
stays valid with the controller on port 1, which is what it described.

### The keyboard is a device, not a shortcut

This deserves the distinction because both things exist and are different:

- **The host keyboard simulating the controller** is the usual mapping, in `input/bindings.rs`.
  The game sees a controller.
- **The keyboard as a device** is a port occupied by a USB keyboard. The game enumerates it, and
  the Z-Wheel writes `Keyboard Connected.` to its log.

Anyone who wants both can have them: controller on port 1 with a keyboard mapping, a real
keyboard on port 2.

### The order matters

The Z-Wheel enumerates the HID devices during `EVT_APP_START`. Configuring the ports **after**
starting the game makes the console never see the keyboard, however right the configuration is —
that is what happened, and the symptom was "the keyboard does not work" with nothing in the
report. Hence `Session::start_with(path, ports)`: there is no path to start a game and another to
configure it.

## Real keys

Besides the controller, the emulator delivers `EVT_KEY` (`0x100`) with BREW's AVK codes, in the
`AEEVCodes.h` numbering that is in `input::avk`: `AVK_0..9 = 0xE021..`, `AVK_CLR = 0xE030`, the
four directions from `0xE031` to `0xE034` and `AVK_SELECT = 0xE035`.

**The release is `EVT_KEY_RELEASE`, `0x102`.** It went out as `0x101`, which in `AEE.h` is
`EVT_KEY_PRESS`. Quake reads only these two, and confirms the numbering: its handler (`0x3a8ec`)
treats `0x101` as a press and `0x102` as a release, and its own translation of the controller
into keys sends `0x101` on press (`0x49a24`). Button 2 becomes `AVK_CLR` in the emulator's
translation, and its release reached Quake as a press of `AVK_CLR`, that is, of the menu key.
Since the real release never came, the key stayed down forever, and every following press was
discarded as a repeat: after the first button 2, the menu no longer opened.

The press is still only `EVT_KEY`. With `EVT_KEY_PRESS` alongside, button 2 would open Quake's
menu in the middle of a level, and on the device it does not: in the longplay, the "Always run
off" from button 2 shows up with no menu at all, and the menu comes later, on its own.

**A key goes first to the widget handlers and only then to the applet.** That is BREW's order:
the interface gets the first chance, and whatever it does not consume goes up. Delivering straight
to the applet makes no menu respond.

## Two scripts, for testing without a window

They exist because without them the only way to know whether input works is to press the key and
look, and that does not fit in a test or in an automated run.

- `--keys=ms:button[:duration]` moves the **controller**. `--keys=3000:b1,6000:start`.
- `--key-names=ms:name` delivers AVK **keys**. `--key-names=1000:select,2000:down`.
- `--boomerang` and `--motion=ms:x:y:z` (in `session`) move a **Boomerang**; `--wiimote` uses
  the connected Wii Remote. See [20](20-boomerang-and-wii-remote.md).

The instant is the game's virtual clock, not the number of loop iterations: an iteration does not
always last the same. A button stays down if **any** step wants it down now — applying step by
step looked the same and was not, because every script that navigates a menu uses the same button
twice, and the second step undid the first one too early.

The report lists the presses delivered, with each one's port:

```text
presses:   6 delivered to the game
     3001 ms  port 1  press   b1
     3134 ms  port 1  release b1
```

## Turbo and the button shortcuts

The turbo key is one more entry in each player's mapping, `buttons["turbo"]`, which
`Pad::button_by_name` does not know and so presses nothing in the game. Which buttons pulse comes
from the player's mode; when they are down, from the phase of the game clock, in the session. The
window shortcuts that accept a button — fast-forward and rewind — store a `Source`, like the game
buttons, and a shortcut's button does not reach the game. See [24-speed.md](24-speed.md).
