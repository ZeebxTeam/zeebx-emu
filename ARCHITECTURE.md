# Architecture

How Zeebx is built on the inside. Each subsystem has its own document in
[`docs/implementation/`](docs/implementation/README.md); this is the overall picture.

## The starting point

Emulating the Zeebo means reimplementing Qualcomm BREW 4.0.2. The game is an ARM binary that never
touches hardware: it talks to the system through tables of function pointers, `IShell`,
`IDisplay`, `IFileMgr`, `IHID`, OpenGL ES. There is no video register to write, no timer
interrupt to service, and no console firmware involved at any point.

That is why the emulation is high-level. The game's ARM code runs on an emulated core and the API
calls are intercepted and serviced on the host. What has to be faithful is the conversation, not
the silicon.

## The trampoline

The vtables we hand to the game point to an address range that is not mapped. When the game calls
a method, the core aborts the instruction fetch and we read the address: it encodes which
interface and which method were requested.

```
  game (ARM)                          emulator (Rust)
      │ ldr r12, [vtable + 0x30]
      │ blx r12
      ▼
  0xf000_7038  ──── unmapped ────▶  FETCH_UNMAPPED
                                        │  aee::decode → (Bitmap, slot 14)
                                        │  Machine::dispatch
                                        ▼  r0 = result, pc = lr, continue
```

There is no ARM glue code, no compiled stub and no jump table inside the guest. The address is
the call's identifier. Returning works the same way: `lr` receives a sentinel address that is also
unmapped, and reaching it means the module returned.

The cost of this has been measured, **but the measurement is from the Unicorn era**. The round
trip through the core — leaving the JIT and entering it again — cost **1.4 µs** per call even when
the method did nothing. With Dynarmic that number **was not redone**: today's instrument
(`ZEEBX_ROM_PROFILE`) times only the method body, not the `halt` plus the re-entry, which is where
the fixed cost lives. Until the micro-benchmark exists, the per-call cost is unknown, not 1.4 µs.

What remains depends on the method, and sometimes it is much more than that: in Pac-Mania the
average of 8 µs per call did not come from the trampoline, it came from two methods doing too much
work. See [`docs/OPTIMIZING_V0.3.0.md`](docs/OPTIMIZING_V0.3.0.md).

## Layers

```
  interface (Qt)          ─┐
  command line            ─┤
  frontends/android       ─┼─▶  Session  ─▶  Machine  ─▶  CpuBackend  ─▶  dynarmic (A32)
  frontends/libretro      ─┘                                            └─▶ interpreter (wasm32)
  frontends/headless      ─┘
                                              │
                                              ├─▶ software rasterizer (OpenGL ES 1.1)
                                              ├─▶ 2D framebuffer
                                              ├─▶ audio mixer ─▶ cpal
                                              ├─▶ VFS ─▶ module directory
                                              └─▶ heap, objects, timers
```

`CpuBackend` isolates the ARM core. On native the implementation is **Dynarmic** (A32, ARMv6K),
and on `wasm32` it is the interpreter: Dynarmic does not compile to WebAssembly. **Unicorn was
removed**; the backend swap this paragraph anticipated happened, and without touching the rest of
the code — which is exactly what the trait existed to guarantee.

The `Machine` is the emulator proper: the loop, the dispatch and the implementation of the APIs. It
does not know a window exists.

The `Session` is a running game, from the file to the frames. It is what the interface uses, and it
is where the speed limiter and the measurement live.

The frontends are packages of their own next to the core, each with its own window and event loop:
[`frontends/android`](frontends/android/README.md), on top of `NativeActivity`,
[`frontends/ios`](frontends/ios/README.md), on top of UIKit,
[`frontends/vita-standalone`](frontends/vita-standalone/README.md), the Vita VPK, and
[`frontends/headless`](frontends/headless/README.md), with no interface at all, for those who
already have a frontend of their own and want only the emulation from the command line with a
`config.ini`. No line of emulation knows which of them it is running in.

### The modules

The tree follows the subsystems: one folder per subject, and inside it one file per piece.

Execution:

| | |
|---|---|
| `cpu/mod.rs` | The `CpuBackend` trait: registers, memory, `run` |
| `cpu/dynarmic.rs` | The implementation on top of Dynarmic, with a direct page table |
| `cpu/interpreter.rs` | The `wasm32` backend, where Dynarmic does not exist |
| `cpu/mem.rs` | The guest memory map, in named regions |
| `loader/` | Loading the `.mod` and setting up the environment, plus the `.mif`, `.bar` and `.zip` formats |
| `machine/mod.rs` | The loop, the dispatch and the machine state |
| `machine/*.rs` | One submodule per BREW interface — see [`docs/implementation/01-architecture.md`](docs/implementation/01-architecture.md) |

BREW API (`brew/`), which comes from the [`zeebx-core`](https://github.com/ZeebxTeam/zeebx-core) crate together with the `.mod`/`.mif`/`.bar` parsers (`formats/`), the memory map (`layout`) and the save state format:

| | |
|---|---|
| `brew/aee.rs` | The trampoline: address ↔ (interface, slot) |
| `brew/aee_slots.rs` | The name of each method, in vtable order |
| `brew/aee_helpers.rs` | The BREW stdlib: `memcpy`, `malloc`, `sprintf` and friends |
| `brew/objects.rs`, `brew/heap.rs` | Reference-counted objects, and the guest heap |
| `brew/cformat.rs`, `brew/fmath.rs` | The guest's `printf` and floating point |
| `brew/crypto.rs` | AES and MD5, for `ICipher1` and `IHash` |
| `brew/sql.rs` | The `ISQLMgr` SQLite databases, on top of `rusqlite` |
| `brew/vfs.rs` | The guest paths, confined to the module directory |

Output:

| | |
|---|---|
| `video/rasterizer.rs` | OpenGL ES 1.1 in software |
| `video/gles.rs`, `video/atc.rs`, `video/paltex.rs` | GL state and the compressed textures |
| `video/display.rs` | Framebuffer and 2D operations |
| `video/font.rs` | The `IDISPLAY_DrawText` text, with the font the game ships |
| `video/icon.rs`, `video/gif.rs` | The images that come inside the games |
| `audio/` | Sound mixing, and the WAV, MP3 and MIDI formats |
| `input/` | The Zeebo controller, the host gamepads and the button map |

Outside the emulator:

| | |
|---|---|
| `session.rs` | A running game |
| `ui/` | The interface: screen, library, preferences, saves and translation |
| `main.rs` | Command line and opening the interface |

## The loop

`Machine::advance` is one turn. It moves the clock forward if the game is only waiting, fires the
expired timers and delivers the pending callbacks and threads.

One turn does not correspond to one frame. The game can run its entire main loop inside a
callback, and each turn returns only the slice of instructions that fit; in Zeebo Sports Peteca it
takes a million and a half turns for a few dozen frames. What drives the redraw is
`eglSwapBuffers` in 3D and the real clock in 2D.

The emulator runs on the same thread as the interface. Dynarmic does not cross threads — the `Jit`
is tied to the thread it was born on —, and `Session::step` already hands control back at every
slice of real time, which is what keeps the window alive while the game runs. A separate thread
would bring synchronization and nothing in return. It is the same reason the Libretro core
delivers one frame per `retro_run`, and the core's log is drained there and not from inside the
drawing.

## What the project keeps on purpose

The frame is bit-for-bit reproducible: two identical runs draw the same pixels. That is what made
it possible to prove that the rasterizer parallelization and the per-frame dump changed nothing,
with 921,600 bytes compared and zero differences. A real GPU backend would cost that guarantee,
and that is one of the reasons it is not at the front of the queue.

**One caveat, measured on 2026-09-24: reproduction requires the same extraction-cache state.**
Three runs with the extraction already in the cache give **760,815 instructions, digit for
digit**; the same ROM with a freshly extracted cache gives **760,921**, with one extra
`CreateInstance` and the heap shifted by 64 bytes. The emulator is deterministic — what changes is
what the guest observes of the host, and the cause is not settled yet: the `.zeebx-pacote`
manifest hypothesis was tested and **rejected**. See
[`docs/OPTIMIZING_V0.3.0.md`](docs/OPTIMIZING_V0.3.0.md), sections 32 and 33. Anyone comparing two
runs needs to warm the cache first, or they will measure this instead of what they wanted.

The clock belongs to the guest, not the host. The time the game measures comes from the executed
instructions, and idle time is skipped forward instead of spent. Without that, a game that waits in
a loop spends the wait for real; with it, `--seconds=N` means the same thing on any machine.

A guess stays marked as a guess. An API serviced without documentation goes into the `assumptions`
set and comes out in the report. The `IHash` slot order is an example: SDK 4.0.2 does not ship the
header, so the order is a hypothesis, and that is stated in the code and in the program's output.

## Dependencies

Image decoding, sound, rasterization, `printf`, AES, MD5 and resource inflate are written here. The
dependencies cover what belongs to the host or what would be reinventing badly.

`dynarmic` is the ARM core, behind `CpuBackend`, and only exists on native: `wasm32` uses the
interpreter in `cpu/interpreter.rs`. The interface is in Qt Quick, through `cxx-qt`
(`frontends/qt-standalone`), and `minifb` is the window for the command line's `--window` mode,
which is separate from it. `egui` stays in the core without any window: it provides the key names
stored in the configuration and the frame painter that Android and headless use. `cpal` only puts
sound on the sound card; the decoding is ours. `gilrs` provides real controllers on all three
systems.

`ab_glyph` rasterizes the `IDISPLAY_DrawText` glyphs: writing a TrueType interpreter would be
reinventing badly. The font itself is never ours: it is the one the game ships.

`rusqlite`, with the bundled build, services `AEECLSID_SQLMGR`. It is not convenience: the console
really used SQLite — the `tt_prefs.db` the Z-Wheel carries in its package starts with
`SQLite format 3`, and the statements the module loads as text use `INSERT OR REPLACE`,
`COLLATE NOCASE` and JOIN. Writing an engine for that would be reinventing badly.

`flate2` does the inflate for BREW's `IUnzipAStream`. `zip` is used only for reading and only with
`deflate`, which is what the packagers use. `png`, `zune-jpeg` and `resvg` cover the formats that
the `.mif` files and the resources carry. `rfd` is only the folder picker, through `xdg-portal` so
it does not depend on GTK on Linux. `serde` and `serde_json` store the configuration.

The root `build.rs` existed for a single reason: linking the `libatomic` that `unicorn-engine-sys`
needed, and that reason **left the tree with Unicorn**. What remains of it is the
`cargo:rerun-if-changed` and the note that the `.exe` icon is compiled by the packages that produce
an executable: `frontends/qt-standalone` and `frontends/cli`. The one with a `build.rs` doing real
work today is the Libretro core, and for another reason: the compatibility shim for glibc older
than 2.34
([`frontends/libretro/src/r36s_compat.c`](frontends/libretro/src/r36s_compat.c)), which defines
`__libc_single_threaded` and the `__dso_handle`/`__cxa_finalize` pair as weak symbols.

## Debugging tools

Almost every hard bug in this project was found by one of them, and not by reading the code.

`--trace[=fragment]` shows every API call with arguments and return value. `--watch=ADDRESS` says
who wrote and who read a field, which is how you answer "who should have filled this in?".
`--code=START:END` lists the instructions executed in a range.

`--profile` shows where time is spent on **both sides**: in the guest, per translation block
instead of per instruction, so as not to change what is being measured; and on the host, the real
time of each API method the emulator serviced. The second exists because the average lies — "8 µs
per API call" hid that Pac-Mania's `IIMAGE_Draw` and `IIMAGE_SetParm` alone were 99% of its
dispatch. `--wall=SECONDS` stops on real time,
even from inside a slice, and it is what makes it possible to profile a game that never ends.

Outside the emulator there is `tools/disassemble.py`, which disassembles a stretch of the game
module, resolving the literals and the strings. It answers the question none of the inside tools
answer — **why** the game branched —, and the difference is large: "the app dies at `0x78b70`" and
"`0x78b70` is the error branch that only prints the message" are indistinguishable without it.

`--probe=0xCLSID` services a class we do not know with an observation object, instead of refusing
it, and says at the end which slots the game called, with which arguments and with which text. That
is how you find out what interface a class is without having the header — the console's `ISQLMgr`
came out of it whole, in a single run. The procedure is in
[docs/implementation/13-unknown-classes.md](docs/implementation/13-unknown-classes.md).

`--keys=ms:key` scripts the input in virtual time, so the same sequence happens on every run.
`--dump-gl=DIR` writes one `.bmp` per presented frame. `--seconds` and `--frames` limit how long to
run, in virtual time or in loop turns.

The main instrument, though, is the report that `run` prints at the end: where it stopped, with
registers and stack; the classes the game asked for and we do not have; the files it did not find;
the APIs serviced by hypothesis; and the game's own log, which comes out through `DBGPRINTF` and
through ARM semihosting.

## Tests

There are 308 test functions, all next to the code they test. The commands are in
[docs/implementation/17-tests.md](docs/implementation/17-tests.md).

One part checks algorithms against the specification: the RFC 1321 vectors for MD5, the FIPS-197
ones for AES. Another checks the format parsers against the real files, the `.mod`, the `.mif` and
the `.bar`.

The third part is the most useful day to day: each test carries in its comment the bug that gave
rise to it. The `%02d` that made Resident Evil 4 look for `3d_stg02_0.h2z` when the file is
`3d_stg02_00.h2z`. The ClassID outside Qualcomm's range that kept Zenonia from opening. The
instruction budget that has to be checked before adding the block, not after.

On top of that there is the compatibility scan, which runs the real ROMs and classifies each one.
It lives in [`src/scan.rs`](src/scan.rs) and is **environment-driven**: without `ZEEBX_ROM` the
tests warn and pass, because no ROM is in the tree. With it, each game advances in virtual time and
a report comes out — where it stopped, how much it cost, which API and which class were missing,
which file was not found, and the registers of whoever crashed.

What makes this a test, and not a report, is the **baseline**: each game's summary is compared
with what was already known about it, and whatever changed shows up as `+`/`-` in the failure.
Performance is left out of the comparison on purpose — a machine's numbers are not emulator
behavior. The procedure is in
[docs/implementation/11-compatibility.md](docs/implementation/11-compatibility.md).

## Known limits

**The numbers below have dates, and the date matters.** Everything marked as *Unicorn era* was
measured before Dynarmic became the backend, and **does not describe today's emulator**. When
Unicorn left the tree nobody redid the measurements; redoing them is item 1 of
[`docs/OPTIMIZING_V0.3.0.md`](docs/OPTIMIZING_V0.3.0.md), where the current numbers live.

### What is known today (Dynarmic)

Measured on an x86-64 bench machine, `--release`, without the profile on, 15,016 virtual ms of
Quake:

| | |
|---|---|
| speed | **170%** of the console's |
| core throughput | **195 million** instructions per second |

On both numbers, 2.3 times what Unicorn delivered. **The caveat is in the document:** the scene is
not the same as in the old measurement, so what compares directly is the core throughput, not the
wall clock of two different scenes.

What the long run showed, with 63 ROMs and 60 virtual s each, **with the profile on** — which makes
the clock about 43% more expensive and is therefore good for ranking, not for publishing: Action
Hero 3D 91%, Quake 93%, Ultimate Chess 3D 114%, Need for Speed Carbon 123%. Pac-Mania does not
complete 60 s even in 240 s of real time: it spends 32 million `GetClipRect` and 16 million
`SetClipRect` on the drawing path, and it is the clearest optimization candidate the tree has
today.

### What remains from the Unicorn era

It is here because it is still true as a **lesson**, not as a current measurement.

**Always measure without `--profile`.** It cost 24% in the old measurement — 36 s versus 29 s on
the same work —, because the block profile does one table insertion per translation block. The
cost fell almost entirely on the ARM slice, so with it on the emulation looked bigger than it was.
The same care applies today, for another reason: `ZEEBX_ROM_PROFILE` times every API method.

**Ask for the block, never the element.** Geometry was 14% of the time in an old measurement and
dropped when `read_attribute` stopped asking for one `read_u32` per component: each request crossed
Unicorn's FFI, which looked up the region before copying four bytes — **57 ns**, versus **0.3 ns**
when the same bytes came from a one-kilobyte `read_mem`. With 6.65 million vertices in 15 seconds,
that was 3.2 s of the 3.6 s the draw calls cost, versus 400 ms for the actual drawing. The lesson
still holds for all data the guest hands over in an array, and it is what backs today's block
read.

**The per-API-call cost is unknown, not 1.4 µs.** That number measured the round trip through
Unicorn's `emu_start`. The current instrument does not measure the `halt` plus the JIT re-entry: it
measures the method body. Until the micro-benchmark exists, treating 1.4 µs as fact is treating a
measurement of a removed backend as if it belonged to the current one — and that was exactly the
mistake the external review made.

**The rasterizer *is* a bottleneck on the software path.** The sentence "it is not a bottleneck
today" is also from the Unicorn era. With Dynarmic, the ARM slice shrank and presentation took
over: in the five profiled games it stayed between 54% and 85% of API method time, and
`eglSwapBuffers` carries rasterization, conversion to RGB565 and copying, because that is where the
triangle queue becomes pixels (`frame_rgb565` calls `flush`). On the GPU path, a different set of
costs is in charge — and that is what telemetry on the handheld still needs to show.

### What nobody has measured yet

- the real cost of an API call with Dynarmic, trampoline included;
- how much of the handheld's time is ARM, how much is rasterization and how much is
  `glReadPixels`;
- how many code+data pages are mixed together and make SMC expensive.

The first number comes from a micro-benchmark; the other two, from per-frame telemetry in the core.
