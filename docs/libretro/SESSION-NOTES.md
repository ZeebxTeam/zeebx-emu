# Session notes — 2026-09-21

What was measured, what changed and what was left to do. Written for whoever carries on: every
finding brings the symptom **and** the evidence, and none of them is a deduction.

## Double Dragon's "Memory is insufficient"

**Measured cause:** the game opens `./udata/ddz.sav` with `OFM_CREATE` **without calling `MkDir`
first**, and the `udata/` directory did not exist. A refactor in this session (commit `748525c`,
which extracted the file block into `machine/file.rs`) lost, in the move, the parent directory's
`create_dir_all` that existed in the create branch. The open failed with `No such file or
directory`, and the game — which does not tell "I could not open it" apart from "memory ran out" —
showed the out-of-memory screen.

This explains what the team saw: in the stock `zeebx` (upstream) the `create_dir_all` is there,
which is why Kaio never had the problem; Tênis do João worked because it does not create a file in
a new folder at startup.

**How it was measured:** the scan report started recording the `IFileMgr` calls with path,
arguments and return value, and the host path with the system error when the open fails. The line
that solved it:

```text
OpenFile "./udata/ddz.sav"  (0x5e078 0x4 0xf0003008) -> 0
open failed: <cache>/…/mod/274754/udata/ddz.sav (No such file or directory)
```

After the fix: `OpenFile "sound.ggz" -> 805307728`, the `.sav` is created, and the screen drawn
becomes `CARREGANDO`.

## Three CI traps, all measured

1. **Push cancels push.** Four runs in a row of the `ci` workflow ended `cancelled`, because each
   new push brought down the six-target matrix that was halfway through — and the standalone CI
   had **never** run to the end. Fixed: `cancel-in-progress` only on PRs.
2. **`continue-on-error: ${{ matrix.experimental }}` makes GitHub reject the whole file.** The
   symptom is a run **with no jobs at all** (`gh api .../runs/<id>/jobs` returns `total_count: 0`).
   Neither `actionlint` nor a YAML reader flags it. Rule: an empty run is the file, not the machine.
3. **The tests never compiled the binary target**: `main.rs` imported `zeebx::scan` under
   `#[cfg(test)]`, and in that target the library is compiled **without** the `cfg`. Running
   `cargo test --lib` hid this.

Two more, platform ones:

- **macOS**: two VFS tests compared the exact case of the path, and APFS **is not
  case-sensitive** — there are no two spellings to compare. The same kind of defect showed up on
  Windows x86_64, in a test that looked for `/zeebx/` in the text (Windows uses `\`).
- **Windows ARM64**: `unicorn` (QEMU in C) **does not compile** there — QEMU assembles
  `setjmp-wrapper-win32.asm` with MSVC's `ml`, which only exists for x86 and x64. `unicorn-engine`
  2.1.5 is the last published version. Solution: a target-conditional dependency, and `dynarmic`
  becomes the default backend on that target, with unicorn's debugging hooks implemented as an
  explicit refusal.

## Two games were mute, and what silenced them

The report lists the rejected sounds. Making the rejection **state the format and the first
bytes** cost one line and solved both cases:

```text
Turma da Mônica   rejected (unknown format, 86 sounds, 4f 67 67 53 …)   ← "OggS"
Peggle            rejected (audio/mpeg, 7 sounds, 49 44 33 03 00 00 00 00 …)   ← "ID3"
```

| Game | Before | After |
|---|---|---|
| Peggle | 7 rejections, peak **0.000** | 0 rejections, peak **0.356** |
| Turma da Mônica | 86 rejections, peak **0.000** | 0 rejections, peak **0.586** |

- **Ogg/Vorbis**: `symphonia` already knew how to decode it; the feature just had to be turned
  on.
- **MP3 with an ID3 tag that lies**: the tag declares a size of **zero bytes** and has thirty
  thousand of content. Whoever skips by the declared field looks for the frame in the wrong place
  and rejects the whole file. Now the search is by **frame sync**.

**A lesson that holds for the rest of the engine:** a rejection that does not say *what* arrived
costs a whole investigation inside the game.

## Performance, measured

- **Rolima**: 6.8 million API calls in six seconds, in a wait loop the idle shortcut did not
  recognize (clock, `memset`, controller query). After recognizing it: **79% → 284%**,
  instructions from 2.46 billion to 886 million. 51 games got faster, and **no state changed** in
  four scans of the corpus.
- **Where the time goes** (`ZEEBX_ROM_PROFILE=1`): in Crash Nitro Kart, `eglSwapBuffers` is
  **89.8%** of the API time — it is the software rasterizer drawing the scene. Even so the game
  runs at 1039%.
- **GL gap**: the "GL served as a no-op" section pointed at **one** call across the 62 ROMs,
  `glPixelStorei` (texture row alignment), in six games. Implemented; afterwards, **zero** games
  ignore any GL call.

## What the scan caught today, and what was fixed

The scan with a committed baseline stopped being a report and became a test. On the same day it
started enforcing, it found four defects — and three were fixed:

| Defect | Fix | Measured result |
|---|---|---|
| `glPixelStorei` ignored (six games) | texture row alignment | zero games ignore GL |
| Two mute games | Ogg/Vorbis and the MPEG frame by sync | peak 0.000 → 0.356 and 0.586 |
| `AEECLSID_JPEGDecoderBREW` rejected | one line in the factory | Zuma gets past one gate |
| The decoder only tried PNG | dispatch by signature | **Zuma runs**, 18,268 colors |

Also in: a black screen that passed as "runs" (Prey Evil), and the audio step that cut off at once
became a 1.5 ms ramp-down — without changing the measurement of Peteca's 18 jumps, which turned out
to be the effects' **attacks**, not pops.

## The state of hardware rendering

No code is missing. What exists, measured:

- the GPU rasterizer draws **the same frame** as the software one (identical in Double Dragon and
  Crash; 2.6% rounding in Peteca);
- the engine can draw into a framebuffer **from outside** (a test that reads the pixels of the
  foreign framebuffer);
- the `gl` and `gpu` features are separate, and the core uses only the first: **0 host
  dependencies**, measured in CI on the six targets;
- the core asks for the context, builds the `glow::Context`, draws into the frontend's FBO, and
  **falls back to software** if any step fails — including a panic, which would bring RetroArch
  down;
- the `retro_hw_render_callback` offsets match `libretro.h`, and a test enforces them.

### Verified in real RetroArch, and two defects that only showed up there

Running the core in this machine's RetroArch 1.20 (Wayland/EGL, AMD renoir, Mesa 25.0.7) found two
defects that no headless test caught, and both only revealed themselves with the frontend
presenting the frame:

1. **The context profile was wrong.** The core asked for `RETRO_HW_CONTEXT_OPENGL`
   (compatibility) with version 3.3. A request for version 3.2+ **without a profile mask** makes
   EGL return a *core* context, and RetroArch's `gl` driver is a compatibility one — it broke
   before running a single frame:

   ```text
   [INFO] [GL]: Version: 4.6 (Core Profile) Mesa 25.0.7
   [ERROR] [GL]: GL: Invalid enum.
   [ERROR] [Video]: Cannot open video driver.. Exiting..
   ```

   The standalone emulator asks for GL 3.3 and `glutin` delivers **core**, and that is where the
   engine was measured. Now the core asks for `RETRO_HW_CONTEXT_OPENGL_CORE` and RetroArch uses
   the `glcore` path.

2. **The draw target was rebound to our framebuffer every frame.** The choice of target was
   written in two places in `dest()`, and the two disagreed: the path that **creates** the
   destination respected the frontend's framebuffer, and the short path — destination already
   built, same parameters, which is what runs every frame after the first — rebound ours. Result:
   we drew into our framebuffer, RetroArch presented its own and the screen stayed black while the
   GPU worked just as hard. Measured by reading 4×4 pixels of the frontend's framebuffer:

   ```text
   before: [0, 0, 0, 254] × 4     ← black, and the capture with 1 color
   after:  [254, 254, 254, 0] × 4 ← drawn, and the capture with 1282 colors
   ```

   The choice now lives in `draw_target()`, so the two branches cannot disagree again, and the
   outside-framebuffer test gained a **fence**: it paints the framebuffer green midway and
   requires the following frames to overwrite it. Without it the test passed with the defect,
   because it only proved that *some* frame had gone there. With the fence, the defect
   reintroduced on purpose leaves 100% of the pixels green and the test fails.

### Measuring color in RetroArch: watch out for the shader

The color count of a **RetroArch capture** measures the frame **after** the shader — on this
machine there is a CRT filter active. It does not serve to judge what the core drew. What counts is
reading the pixels of the framebuffer the core received (`glReadPixels` on the frontend's FBO), as
in the measurement above: there, no shader has run yet.

### The rounding between the two rasterizers

The test that compares the two rasterizers compared in RGB565 channel steps with a tolerance of
60% of the pixels. Running with Crash, 98.33% of the pixels differ — but by **1 or 2 steps**,
mean 1.93, worst 2, and **0.00% above two steps**: the software rasterizer truncates (`>> 3`) and
`glReadPixels` in `UNSIGNED_SHORT_5_6_5` rounds. The check now separates the two things: a
different image is off by much more than two steps. Aligning the two converters is a decision that
needs the console in hand, and it is declared here.

## What is missing

**Rewritten on 2026-09-22 with what is measured today.** What was here before said that the scan
could not exercise the wheel and that save states did not exist — both things fell, and the second
fell by measurement, not by opinion.

- **Item 5** (hardware rendering): the GPU rasterizer is verified against the software one
  (identical in Double Dragon and Crash; 2.6% rounding in Peteca) and the plumbing with the
  frontend exists. What is missing is the **physical test on the device** — a hardware block,
  declared.
- **Item 6** (save states): **done, and proven through the ABI**. `retro_serialize_size` returns
  the real size of the session file, and the test `the_save_state_crosses_the_abi` saves, dirties
  the state, loads and **saves again byte for byte identical** — which is what a save state
  promises. What stays forbidden is the **partial** state: the gate only opens when the whole
  session can be described.
- **Item 8** (Z-Wheel cycle): **the launch is proven in the headless harness**, scripted, with
  `Session::take_launch_request` as the outcome — `requested launch: 0x0108e356`, with the library
  grid drawn before. The **core** path gained two measured pieces (translating the controller into
  console keys, without which the wheel ignored the RetroPad, and the local library catalog), but
  **it still does not request the launch**: the difference is in how the core drives the session,
  not in the key. Validation **on the device** stays blocked by hardware.
- **Item 9** (covers and No-Intro): ready locally and measured — a playlist with 62 entries and
  **none without a file**, 58 covers in the format and with the name that the Libretro thumbnails
  repository looks for (`Mobile - Zeebo`), a No-Intro DAT of 57 games. **Publishing depends on an
  account**, and it is Rafael's decision; nothing was published and no external repository was
  created.

**Blocked by hardware, no workaround:** item 5 (physical test) and the on-device validation of
item 8. The core on the muOS card, which was postponed at this point in the history, was later
installed on SD1; the physical validation is still pending.

## What the headless test already answers about the Z-Wheel

I had written that the wheel **does not respond** without a frontend. That was wrong, and the
mistake was the test's: it stopped at the first button that did not change the image and concluded
"nothing responds". Measuring button by button, with the test that counts distinct images:

```text
button 3 (START):  1 image
button 8:          1 image
button 0:          1 image
button 2 (SELECT): 2 images   ← the wheel reacted
stick x and y:     1 image    (did not move)
```

**The wheel responds — and this was measured with enough time.** With 1200 frames of waiting
(twenty virtual seconds) before sending input:

```text
button 3 (START):   1 image
button 8:          12 images   ← reacted
stick x=+32767:    26 images   ← reacted
```

That is: **the wheel becomes interactive at around twenty seconds**, and its input path works
through the core. The two earlier measurements that said "does not respond" were measuring
**loading** — the same trap as the F.C. titles, for the third time that day. The test now waits
before sending input, and says which input changed the screen.

**What has not come out yet is the launch of a game.** With the wheel interactive and navigating,
the test tries the stick (four directions) with each face button and Start, and waits 180 frames
after the confirm — the wheel's transition is animated. The request did not come. What is already
ruled out, by measurement: the input (it arrives), the loading time (twenty seconds is enough), the
time after the button (180 frames), and an empty device tree (the test was run with
`ZEEBX_CORE_SYSTEM` pointing to the real folder, with the 63 games installed). What is left is the
**sequence** — which gesture the wheel expects in order to confirm.

The test now covers, and none of it opened a game: the stick in the four directions; each of the
four face buttons **and** Start; the stick pushed with the button held without releasing **and**
released before pressing; 180 frames of waiting after the confirm (the transition is animated); and
the real device tree, with the 63 games installed. All with `LAST_OPENED` as the instrument — it
says whether the request went out, and it did not.

**And the values it reads are right, checked in the SDK.** I came to suspect a defect in
`GetPositionState` — the first field is written as zero, and I read the button state there.
`AEEIHIDDevice.h` says otherwise: the first field is `bRelativeAxes`, and zero there means
**absolute axis**, which is correct. The four axes are in words 1, 2, 3 and 6, exactly as the
engine writes them (`AXIS_SLOTS`). The code got it right; the quick reading got it wrong.

## Where the Z-Wheel gesture stops, measured

Once the key script existed, the question became answerable: **does the wheel ever request the
launch?** Tracing `IShell` in the sequence that advances the screen (`kselect` → `kdown` →
`kselect`, and the variant with `kright`), the trace shows 39 `CreateInstance`, `SendEvent` and
`SetTimer` — and **no** `CanStartApplet` or `StartApplet`.

And before that, an instrument on key delivery answered what was left to know: with `AVK_SELECT`
arriving, there are **zero handlers on screen and no current form**. The Z-Wheel **does not use the
engine's `IWidget`** — it draws directly —, so the key falls into the `if !handled` branch and goes
to the applet. And the applet reacts: the screen changes.

So the input path is proven end to end:

```text
key script -> core/scan -> set_key -> flush_keys -> applet -> the screen changes
```

What is missing is **the gesture**, and it is inside the wheel's own logic: it receives the key,
changes screen and does not request a launch. Closing this is reverse engineering of its module —
the stretch that decides when to call the shell —, or a session with the controller in hand. It is
not a lack of instruments: it is not knowing which press it expects.

### The trigger, read in the disassembly, with an address

Reading the wheel's handler (`0x7b310`, through the disassembly with `tools/disassemble.py`), the
launch trigger shows up in two lines:

```asm
0x07b4bc  cmp  r0, #0x7000      ; the EVENT
0x07b4c0  beq  #0x7b51c
0x07b51c  sub  ip, r6, #0x400
0x07b520  subs ip, ip, #0xea    ; and wParam == 0x4ea
0x07b524  bne  #0x7b574
0x07b53c  ldr  r0, [r5, #0x580] ; the chosen item
0x07b544  beq  #0x7b640         ; no chosen item, no launch
0x07b55c  ldr  r3, [r1, #0x14]  ; IShell::StartApplet
```

Measured, with the event injected by the script (`ZEEBX_ROM_KEYS="…:e0x7000=0x4ea"`): the event
**reaches the applet** and `StartApplet` **is not called**, because the wheel has no chosen item —
key navigation changes the screen without filling its `+0x580` field.

Two things stayed as tooling: the script can deliver a widget event, and there is a test from the
**text** to the command (`the_script_text_becomes_an_event`) and from the command to the delivery
(`the_script_delivers_a_widget_event`). Whoever continues from here can inject the trigger and
observe what the wheel does, without repeating the reading of the disassembly.

### The block, at the component level

**What sends the `0x7000` on the console is the list widget**, and the wheel **creates** that
widget: among the classes it asks for is `0x01028e19`, which belongs to the engine's widget family
(`WIDGET_FAMILY`). What does not exist is its **behavior**: our implementation answers
`Interface::Widget` and stops there. On the console, it is this widget that fills in the chosen
item and sends the event when activated.

That closes the explanation of the whole cycle:

```text
stick/key -> list widget (0x01028e19) picks the item and writes [wheel+0x580]
          -> widget sends event 0x7000 with wParam 0x4ea
          -> the wheel calls IShell::StartApplet
          -> the shell switches session, and the core already knows how to come back
```

The only missing link is the second. Without it the wheel navigates, changes screen and **never**
reaches `StartApplet` — which is exactly what the measurements showed, one by one.

**The way to close it**: implement the behavior of this widget family from the SDK, not by trial
and error. What already exists is the safety net: the script injects the event, and the report
says whether the launch was requested.

## The defect that invalidated the whole gesture investigation

`ZEEBX_ROM_KEYS` did not reach the game combined. The script's pad was recreated at every step —
and, after a first fix, at every **frame** —, so:

```text
script "18000:x=128,20000:b1"
   before: 0x10006b50  00 00 00 00 | 80 00 00 00   ← stick at CENTER at the moment of the press
   after:  0x10006b50  00 00 00 00 | ff 00 00 00   ← stick at MAXIMUM, and the press does not release it
```

The address `0x10006b50` is where the Z-Wheel keeps what `GetPositionState` answered: the dump
shows what the game **actually read**. Holding the stick and pressing a button — the gesture a menu
asks for to open something — never reached the guest whole. **Every earlier conclusion that "the
gesture does not exist" was invalid**: the instrument was broken, and the defect was silent because
the pad worked for **one** step.

Two things stayed in place: `due_steps` keeps the contract written down, and with tracing on each
script step comes out in the report (`script 20003 ms: step 1 -> x=255 y=128 buttons=0x0`). A
script that does not reach the game can now be told apart from a game that ignores it.

With the instrument working, the wheel **reacts to the stick** — the screen goes from 2001 to 1904
colors, and with the stick held for twelve seconds it goes to 4098. But the launch request **does
not come**:

```text
script "20000:x=128" (stick at maximum until the end, 32 s of execution)
  state: runs
  screen: 4098 color(s), dominant 0xd69a
  requested launch: (none)
```

The report now says explicitly when the shell requests another applet
(`Session::take_launch_request`, the same hook the core uses to switch sessions), so "did not
request" is a measured answer and not an absence of clues. It navigates by **position** — it
registers `RegisterForPositionChange`, reads `GetPositionState` and never asks for a button event
—, so the gesture that confirms is in its logic, and not in the input path. Finding it is reverse
engineering work on the module, or a session with the controller in hand.

This is where the headless investigation stops, and for a practical reason: **the instrument has
already answered what it had to answer** (the input arrives, the wheel reacts, the request does not
go out), and what is missing is the meaning of the gesture — which only the frontend with a
controller in hand, or a reverse engineering session on the wheel itself, resolves. It is written
down for whoever picks it up: the three ruled-out causes above save repeating three measurements,
and the list of attempts is half the way back.

## The Z-Wheel navigates by KEY, not by the controller

This was measured, and it explains a whole investigation that was chasing the wrong gesture. The
engine already had the clue written down — *"the widget gets the key first, not the
application"* —, and the wheel confirms it:

| Input | Effect on screen |
|---|---|
| `AVK_SELECT` (`Return`) | goes from 1904 to 6427 colors: changes screen |
| right arrow + `SELECT` | 1892 colors |
| down arrow + `SELECT` | 86 colors, dominant white (loading screen) |
| `avk::CONFIRM` (`0xe064`) | 214 colors |
| stick, controller buttons, `AVK_0`, `AVK_CLR` | **nothing** |

The stick and the buttons do not move a pixel. It is not a missing gesture: the list is a
**widget**, and in BREW a widget receives `EVT_KEY`. Registered with `IHID` the wheel only *reads
the position* — which led to a wrong conclusion for quite a while, because the test instrument
could not press a key.

With the instrument fixed (`ZEEBX_ROM_KEYS` accepts `k0`, `kclr`, `kselect`, `kright`,
`kconfirma`), the navigation showed up in the first batch.

### What the wheel opens, and what it does not

It walks `assets/stage_slides/5008X/slidebanner.qxt` — which is a folder **of the wheel itself**
(`mod/274755`), not of a game — and calls `IShell::CreateInstance` 46 times, building the list. The
launch request (`StartApplet`) **did not come** in any of the twelve measured runs. That is: the
wheel advances, reacts and draws, but has not chosen a game yet — the cycle did not close.

### A real defect on the way

```
tectoymain.c:1668  ERROR: Unable to create instance of AEECLSID_LCT_SIMCARDCTL, cannot do SIM check
```

The interface `0x01006c01` already existed and was already served; what was missing was the
**factory** knowing the class, as it already knew its neighbor `0x01006c02`. The constant was
defined and never used. After the fix the class leaves the list of missing ones and the wheel's log
stops reporting the error — and the same sequence that broke with `invalid access at 0x0` now
ends in `runs`.

Of the classes, only `0x01000000` is still missing, which the SDK does not list as a class (it is
`QVERSION`) and which the wheel asks for once without complaining.

## How to close item 8 (the Z-Wheel cycle)

It is the only thing no test here reaches: the wheel **does not draw** on the scan path (measured:
twelve seconds with and without the stick differ by a thousand instructions out of 133 million), so
only RetroArch answers. The script, and what each result means:

1. open the **Z-Wheel** as content (the package's `.zip`, or the `.mod` inside it);
2. wait for the wheel to appear — the core finds **63 games** next to the content (measured), so it
   cannot open empty;
3. **navigate with the stick** (the wheel is navigated with the analog stick, not the buttons) to a
   game;
4. **confirm** with the action button — the shell then requests the launch, and the core switches
   session on its own;
5. play for a few seconds and **exit with Select** on the RetroPad (it counts as `AVK_CLR`, the
   console's "back") — the return is to the wheel, as on the device.

What to watch for, and what each thing means:

| Symptom | Likely reading |
|---|---|
| the wheel opens and navigates, but no game opens | the launch request did not arrive, or the ClassID is not in the games folder |
| the game opens and Select does not go back | the `AVK_CLR` shortcut or the return through the previous session |
| the wheel opens **empty** | it is not discovery: the core sees the 63 (measured) — it is drawing or installation |
| the wheel draws nothing | same family as Prey Evil: some graphics interface is missing |

The log line that matters in each case:

```text
Zeebx: the shell asked for {class}; opening {path}
The shell asked for {class}, which is not in the games folder
```

## The steps that only run on tag day

Packaging and publishing only happen when someone pushes a tag, so those paths were never
exercised — and three defects showed up when I started rehearsing them by hand:

1. **`zip` does not exist on the Windows runner.** The core packaging step used `zip` in a
   `shell: bash`, and would have failed on two of the six targets. Each system now uses the tool it
   has (`Compress-Archive` on Windows).
2. **The artifact names could collide.** The release job downloads everything into a single
   directory and publishes `pacotes/*`: six identical `zeebx_libretro.so` would bring the
   publication down. The per-target zips avoid this, and I checked that there are **11 artifacts
   with distinct names**.
3. **The `.deb` did not pull in the windowing libraries.** `eframe` is declared with `x11` and
   `wayland`, so the binary links `libX11`, `libXcursor`, `libXrandr`, `libXi` and
   `libwayland-client` — and the dependency list had none of them. On a minimal system install the
   emulator would install and not open.

And the rehearsal of the core packaging confirmed what matters to the user: the `.zip` comes out
with the `.so` and the `.info` of the same name, and the library **loads after being extracted**
(`api_version` 1).

**The lesson that stays:** a path that only runs on release day is an untested path. Rehearsing it
by hand cost minutes and caught three defects.

## The `.rdb` format is not SQLite (and we almost "fixed" a good file)

Recorded because it costs an investigation: RetroArch's `.rdb` **looks like** a database and the
extension invites opening it with `sqlite3` — which answers `file is not a database`. It is not a
defect: the format is RetroArch's own, with the signature `RARCHDB\0` followed by **msgpack**. Our
file opens like this:

```text
magic: RARCHDB\0  ·  blocks: 119  ·  distinct titles: 62
keys of each block: crc, description, name, rom_name, size
the five titles missing from the DAT are there, with the right name
```

`Mobile - Zeebo.lpl` also already carries the 62, with the five named. That is: whoever opens
RetroArch now sees the titles with name and cover, without depending on anything from the database.

## Before opening RetroArch, nothing to clean

Worth recording because it is a common scare: RetroArch keeps a `core_info.cache` with the cores'
information, and an old cache makes it **refuse** extensions the `.info` already declares — which
looks like our defect. Checked on this machine:

```text
~/.config/retroarch/cores/zeebx_libretro.info   332 bytes, 21:09
  supported_extensions = "mod|zip|7z"
  database = "Mobile - Zeebo"
core_info.cache: does not exist
diff against the repository: identical
```

That is: there is no cache to clean, and the installed core is byte for byte the repository's. If
the `.info` ever changes and RetroArch insists on the old version, deleting `core_info.cache` solves
it.

## How to capture what the core says

The core writes to two places, and both serve to diagnose without opening a debugger:

- **on screen**, through `RETRO_ENVIRONMENT_SET_MESSAGE`: hardware rendering refusal, failure to
  open a game, collection warnings. Three seconds, time to read without getting in the way of
  whoever is playing;
- **in the RetroArch log**, through `RETRO_ENVIRONMENT_GET_LOG_INTERFACE`: the same information
  and more (how many games it found, whether it draws on the GPU, which applet the shell asked for).

To see the log without touching the preferences:

```bash
retroarch --verbose 2>&1 | grep -i zeebx
```

What to look for, in order of importance for item 5:

```text
Zeebx: the frontend accepted hardware rendering (OpenGL 3.3); drawing now happens on the GPU
Zeebx: drawing on the GPU
```

or, if the attempt fails — and then the game keeps running on the processor:

```text
Zeebx: hardware rendering failed (…); continuing on the processor
```

Either of the two lines is a result: the first says the fit closed, the second says where it did
not close.

## Tools born here

```bash
# scan with a report per game, comparing against the baseline
ZEEBX_ROM=roms ZEEBX_ROM_MS=6000 ZEEBX_ROM_OUT=output ZEEBX_ROM_BASE=docs/scan \
  cargo test --release scan -- --nocapture

# controller script on the virtual clock (buttons and stick)
ZEEBX_ROM_KEYS="4000:x=-128,10000:b1,11000:" …

# cost profile per API method
ZEEBX_ROM_PROFILE=1 …

# the two rasterizers, side by side, without a window
cargo run --release -p zeebx-cli -- session "roms/Crash.zip" --seconds=3 --dump=software.bmp
cargo run --release -p zeebx-cli -- session "roms/Crash.zip" --seconds=3 --gpu --dump=gpu.bmp

# the core through its own ABI, with a real ROM
ZEEBX_CORE_ROM="roms/Z-Wheel.zip" cargo test -p zeebx-libretro -- --nocapture
```

## MIDI audio, from diagnosis to fix

A summary of what the MIDI audio front closed, with the numbers, and of what was left open. The
measured detail is in [07 — Audio](../implementation/07-audio.md); here is only the map.

**What was wrong, in three layers:**

1. **The timbre table** mapped each family of eight programs to a fixed waveform, and 29
   (*overdrive*), 30 (distortion), 42 (cello) and 48 (strings) came out with the **same wave** —
   23.2% of Double Dragon's notes among the first three. Total centroid error against the
   soundfont: **7,635 Hz**, down to **701 Hz** after the fix.
2. **Sample material was missing.** A bank synthesizer went in (`rustysynth`, MIT, pure Rust),
   **optional**: the bank is not embedded (32 MB), it is looked for in the device folder, and
   without it MIDI falls back to the table. In Double Dragon's twelve songs, the centroid error
   against the reference is **11.3%** with the bank and **40.3%** with the table.
3. **The cost of synthesizing was absurd.** The synthesis summed `sin()` per harmonic **on every
   sample**, inside the API dispatch: **6.25 s** per song, meaning the game froze for six seconds
   every time a track started. With the wave precomputed per voice: **183 ms** (34×), with the
   timbre intact.

**What is validated:** a scan of the 62 ROMs with **0 differences** from the baseline; four
feature combinations green (456/461/467/472 + 5 from the core); CI **6/6** in both workflows, with
`rustysynth` inside — and `readelf` on the ARM artifact confirms the dependencies are still only
libstdc++, libgcc, libm and libc.

**What was pending at that closing:** installing the core on the muOS card. That was done later on
SD1 with the AArch64 core, the `.info` and `GeneralUser-GS.sf2`; the hashes and the delay observed
on the first MIDI are recorded in the next section. The command is still the reference for a clean
install:

```bash
python3 tools/install_core.py --muos /media/$USER/ROOTFS --soundfont GeneralUser-GS.sf2
```

The bank decision is still open and it is not technical: GeneralUser GS reaches 11.3% and allows
redistribution, but the license text admits an unknown origin for part of the samples. The
candidates, with size, license and presets measured, are in the table in `07-audio.md`.

## SoundFont on the RG40XX-H: the first song takes a while, but it plays

**Physical observation of 2026-09-22:** on the RG40XX-H, the new core with `GeneralUser-GS.sf2`
took more than **2 minutes** before playing Double Dragon's menu theme; then it came up and the
music was heard on the console. This is not a format or path failure: the installed bank was
checked with SHA-256 `9575028c7a1f589f5770fccc8cff273456af40cd26ed836944e9a5152688cfe`, and the
new core with `rustysynth` support is on SD1.

**Measurement on the laptop:** `SoundFont::new` took **66 ms**; the 47.5 s track took **279 ms**
with the default `rustysynth` (internal block 64, chorus/reverb on). With block 1024 and effects
off, it took **149 ms**. Double Dragon's twelve songs rendered in about **7.2 s** on the laptop.
This separates the cost of opening the 32 MB from the cost of rendering the voices. In the current
code, `Machine::new` loads the bank synchronously and `decode_sound` renders the whole score before
handing it to the mixer; the cache is per path and avoids reloading the bank in the same process,
but does not avoid the first synthesis of each MIDI.

**Working hypothesis:** the delay on the H700 is in the cost of synchronous rendering (possibly
several tracks requested at startup), not in copying or parsing the `.sf2`. The AArch64 core was
compiled with `-C target-cpu=cortex-a35`, the portable configuration used for RK3326/H700. The
per-track time on the device still has to be measured. The planned A/B is to temporarily remove
the `.sf2`: the timbre table should go back to the fast startup and confirm the regression of the
sample path.

**Candidate optimizations, not applied yet:** internal block 1024; optional chorus/reverb on the
handheld; a persistent PCM cache; or incremental/asynchronous rendering instead of synthesizing the
whole song in the `Play` dispatch. The local measurement shows that block/effects alone give about
1.9x, so they are not yet a complete explanation for the more than two minutes.

## Item 8, from "does not react" to "opens and runs" — what changed in this session

The Z-Wheel cycle was considered blocked on a windowless path. It opens, chooses, **opens the game
and runs it**, on both paths (the scan and the Libretro core), with the launch request proven by
`Session::take_launch_request` — and **four real defects** were knocked down on the way, none of
them the one being looked for:

1. **The `IDownload` class (`0x01000000`) was refused.** The SDK's `AEEClassIDs.h` says, in three
   consecutive lines, that `AEECLSID_PRIV` is `QVERSION` and that `AEECLSID_DOWNLOAD` is
   `AEECLSID_PRIV`; the earlier reading stopped at the first. Refused, the Z-Wheel aborted the
   game library.
2. **`SystemCtl::slot5` had no name.** Read in the firmware (it is `DefinirModo` with one extra
   argument), the confirm stopped aborting the scan.
3. **The SIM card class was being offered.** The long comment on `Interface::SimCardCtl` already
   said that `Unable to create instance of AEECLSID_LCT_SIMCARDCTL` **is the right path**; a
   commit of 09-21 put it in the factory to silence the log. A/B: 474,692 instructions and a frozen
   frame, against **750 million** and five distinct frames.
4. **The core did not consume the intermediate screens of `Update`.** The window and `run` consume
   them; the core did not — and with the queue full `advance_once` returned "presented" forever,
   without running the guest and without draining the key queue. Measured: clock stopped at
   37,012 ms and **one** key delivered, against 78,983 ms and the script's eight, one by one.

With that, on the RetroArch path: `launch 0x0108e356` (Alien Breaker), the session switched to the
game and 61,814 virtual ms executed.

**What is missing, with the number:** the **return to the wheel** — the core hands control back to
the shell when the game ends, and that was not proven. The limit is measured in five attempts: the
launch only happens with the catalog full, and there the focus lands on a game that does not exit
on its own; with few titles the grid does not offer the focus the script reaches. What is missing
is reading the grid itself (the accessor census and the widget tree already exist on both paths).

**Instruments that stayed in the repository**, useful for this and for the device: serial capture
in the core (`ZEEBX_CORE_SERIAL`), the line for the **delivery of each key** and the one for the
**queue exit**, the accessor census per widget class (`ZEEBX_ROM_SELECTORS`), and in the core test
the class that is running (`CURRENT_CLASS`), the virtual clock (`CLOCK`), the instructions
(`INSTRUCTIONS`) and the configurable script (`ZEEBX_CORE_KEYS=ms:id`).

**Blocked by hardware, declared:** physical test on R36S/RG40XX-H (item 5) and validation on the
device (item 8). The core, the `.info` and the MIDI bank have already been installed on SD1; the
first test on the RG40XX-H played, but took more than two minutes to start the first song.
