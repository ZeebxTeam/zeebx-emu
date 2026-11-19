# Libretro Plan — Zeebx

> Status: implementation under continuous validation. The Libretro core is already implemented and
> passes the ABI; the historical tables in this plan preserve the original decisions.
>
> **Status measured on 2026-09-22:** Linux AArch64 is the distribution target for R36S/R35S/RGB20S
> (ArkOS, AeolusUX, dArkOS and dArkOSen) and RG40XX-H (muOS Pixie or newer). On that architecture
> the core asks for GLES 3 for hardware rendering and falls back to software when the frontend does
> not offer the context. Versioned save states, `.7z` and hardware rendering are implemented. See
> `docs/libretro/HANDHELDS-ARM64.md`.
>
> Branch: `feat/libretro-core`, based on `upstream/development` at commit `6c74975` (`v0.2.1`).

## Goal

Ship Zeebx as a Libretro core for Linux **x86_64** and **AArch64**, initially for RetroArch. The
core receives a Zeebo game (`.mod`, `.zip`, `.7z`), runs the BREW applet, delivers video, audio
and input to the frontend, and keeps content, saves and "device" data in separate places.

The first target is a portable software core. Native window, host OpenGL, CPAL, host gamepads and
the desktop UI are not part of the core.

## Measured current state

The project is a single Rust binary (`src/main.rs`), with no `lib` target.

Pieces that can be reused:

- `src/session.rs::Session`: module/applet lifecycle, Dynarmic, timers, callbacks, framebuffer
  and input state;
- `src/input/mod.rs::Pad`: Z-Pad button/axis state;
- `src/audio/mod.rs::Mixer`: mixing of voices and streams; `Mixer::silent(rate)` serves hosts
  with no audio device;
- `src/video/display.rs::Framebuffer`: the current screen is 640×480 RGB565;
- `src/loader/archive.rs`: safe selection of the `.mod` in a ZIP and extraction of the whole tree;
- `src/brew/vfs.rs::Vfs`: BREW semantics of `fs:/`, `fs:/~/` and relative paths.

Couplings to remove from the Libretro path:

- `Session::start_with()` takes `Option<Arc<eframe::glow::Context>>`;
- `Machine::use_gpu()` depends on `eframe::glow`;
- `Session` uses `ui::library` and `ui::settings::ZWheel`;
- `loader::archive`, widgets and SQL use `ui::settings::config_dir()`;
- current desktop dependencies include `eframe`, `glutin`, `minifb`, `rfd`, `gilrs`, `cpal` and
  Discord.

`Session::step(Duration, speed_limit)` is **not** the right unit for `retro_run()` either: it
measures/limits real time with `Instant`, catches up on delay and stops on a wall-clock budget. A
Libretro core needs a deterministic virtual step.

## MVP decisions

| Subject | Decision |
|---|---|
| CPU | Dynarmic, like the current `Session` |
| Video | software RGB565, 640×480, 4:3 |
| Audio | stereo PCM16, 44,100 Hz; the amount per `retro_run` comes from the **virtual** time elapsed |
| Input | up to two RetroPad ports → Z-Pad |
| Initial content | `.mod` and `.zip` |
| `.7z` | done: built-in decoder, same limits as zip, and proven with a real game |
| HW rendering | outside the MVP |
| Save states | outside the MVP; the API returns unsupported |
| Core Options | none until there is a real working option |
| Persistent data | `zeebx/` folder under the frontend's saves directory |
| Guest network | off by default; no implicit connection from the frontend |

## Proposed architecture

```text
src/lib.rs                         shared engine (`zeebx` rlib)
src/main.rs                        CLI and desktop frontend
src/session.rs                     desktop Session + deterministic Libretro driver
src/storage.rs                     persistent roots, identity and content layout
src/loader/archive.rs              ZIP/7z, validation, cache and atomic extraction
src/brew/vfs.rs                    read-only content + saves overlay + device
frontends/libretro/Cargo.toml      independent `zeebx-libretro` package/target
frontends/libretro/src/lib.rs      core state and C ABI exports (`cdylib`)
frontends/libretro/src/bindings.rs generated, versioned bindings of libretro.h
frontends/libretro/include/        official `libretro.h` pinned to a known revision
frontends/libretro/zeebx_libretro.info
                                  distributed core metadata
docs/libretro/LIBRETRO_PLAN.md     this plan
```

The root becomes a Cargo workspace. The root package `zeebx` exposes the engine as an `rlib` and
keeps the desktop binary; `frontends/libretro` is another package, depends on `zeebx` by path and
is the only one that produces a `cdylib`. This keeps Libretro dependencies/exports from leaking
into the CLI and leaves the target visible in its own folder.

### Features and Cargo workspace

Sketch:

```toml
# root Cargo.toml
[workspace]
members = [".", "frontends/libretro"]
resolver = "2"

[package]
name = "zeebx"

[features]
default = ["desktop"]
desktop = ["dep:eframe", "dep:glutin", "dep:minifb", "dep:rfd", "dep:gilrs", "dep:cpal"]

# frontends/libretro/Cargo.toml
[package]
name = "zeebx-libretro"

[lib]
name = "zeebx_libretro"
crate-type = ["cdylib"]

[dependencies]
zeebx = { path = "../..", default-features = false }
```

The core build must work in isolation:

```bash
cargo build --locked --release -p zeebx-libretro
```

It is not enough for the core not to *use* desktop dependencies: they have to be optional in the
engine, and the modules that import them have to be behind `cfg(feature = "desktop")`.

## Deterministic frame driver

Create a dedicated path, for example:

```rust
pub fn Session::run_libretro_frame(&mut self) -> FrameResult;
```

It must:

1. not query `Instant` nor apply a wall-clock brake;
2. deliver `EVT_APP_START` on the first run, after audio and input are ready;
3. run the BREW loop: `advance`, signals, callbacks, threads, timers and presentations;
4. respect the emulator's virtual time/vblank;
5. keep the last screen and the intermediate screens from `IDISPLAY_Update`;
6. return a stop state without panicking;
7. report virtual-time progress to the audio adapter.

`Session::step()` stays for the desktop UI. Implementing `retro_run()` by calling
`step(Duration::from_millis(16), ...)` is forbidden: it would make timing, the amount of audio and
CPU progress depend on host performance.

Before implementing the ABI, create tests that run a fixed number of frames twice and compare the
virtual clock, input state and the framebuffer checksum.

## Libretro contract

Vendor the official `libretro.h` at a pinned revision. Generate the Rust bindings once and version
the generated file, so that the normal build does not depend on `libclang`. Validate
layouts/offsets on the x86_64 and AArch64 targets.

Every export must use the C ABI and not let a `panic` cross the boundary:

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_...()
```

Public entry points need a safe error/panic boundary and logging through the frontend when the log
interface is available.

### Required base exports

| Group | Functions |
|---|---|
| Callbacks | `retro_set_environment`, `retro_set_video_refresh`, `retro_set_audio_sample`, `retro_set_audio_sample_batch`, `retro_set_input_poll`, `retro_set_input_state` |
| Lifecycle | `retro_init`, `retro_deinit`, `retro_api_version`, `retro_get_system_info`, `retro_get_system_av_info` |
| Execution | `retro_set_controller_port_device`, `retro_reset`, `retro_run` |
| State | `retro_serialize_size`, `retro_serialize`, `retro_unserialize` |
| Cheats | `retro_cheat_reset`, `retro_cheat_set` |
| Content | `retro_load_game`, `retro_load_game_special`, `retro_unload_game` |
| Region/memory | `retro_get_region`, `retro_get_memory_data`, `retro_get_memory_size` |

Even APIs with no feature behind them have to exist.

### MVP behavior of APIs with no feature behind them

| API | Result |
|---|---|
| `retro_load_game_special` | `false`; there are no subsystems |
| `retro_cheat_reset`/`retro_cheat_set` | no-op |
| `retro_serialize_size` | `0` |
| `retro_serialize`/`retro_unserialize` | `false` |
| `retro_get_memory_data` | `NULL` |
| `retro_get_memory_size` | `0` |
| `retro_get_region` | `RETRO_REGION_NTSC` |

`retro_reset()` must not be a no-op. Until a safe internal reset exists, it recreates the session
from the original path and configuration, preserving saves and persistent data.

`retro_unload_game()` must destroy the session, mixer, buffers and temporary content.
`retro_deinit()` must also clear state if the frontend called an incomplete sequence.

### Lifecycle order

```text
retro_set_environment
retro_init
retro_set_video_refresh / audio / input setters
retro_load_game
retro_get_system_av_info
retro_run*
retro_unload_game
retro_deinit
```

`retro_set_environment()` is called before `retro_init()`. The pointers/strings of
`retro_get_system_info()` and the descriptors must have static lifetime. `retro_load_game()` must
refuse a null pointer or path and clear the previous session if the frontend violates the flow.

## System and content information

`retro_get_system_info()` must announce:

```text
library_name      = "Zeebx"
library_version   = Cargo version
valid_extensions  = "mod|zip|7z"
need_fullpath     = true
block_extract     = true
supports_no_game  = false
```

`.mif` is not bootable content. It is a side manifest that identifies the applet of a `.mod`.

`block_extract=true` is mandatory for ZIP/7z: the core has to receive the whole compressed file and
pick the right module, preserving the `.mif` and all neighboring resources. Do not use
`game->data`/`game->size` on the `need_fullpath=true` path.

`need_fullpath` does not authorize assuming that `game->path` is a POSIX path reachable by
`std::fs`: a frontend can provide a VFS/SAF path. The core has to obtain the VFS before the load
and read content, cache, saves and the system directory through the VFS adapter.

On load, negotiate `RETRO_ENVIRONMENT_SET_PIXEL_FORMAT` for `RETRO_PIXEL_FORMAT_RGB565`. Refuse to
load if the frontend does not accept the format.

## Video

Initial `retro_system_av_info` configuration:

```text
base_width   = 640
base_height  = 480
max_width    = 640
max_height   = 480
aspect_ratio = 4.0 / 3.0
fps          = 60.0
sample_rate  = 44100.0
```

Every `retro_run()` must call the video callback. If the game did not produce a new screen,
sending the last framebuffer is the simplest MVP rule. `NULL` to duplicate a frame must only be
used after explicitly negotiating that capability with the frontend.

For RGB565:

```text
width = 640
height = 480
pitch = 1280
```

If the resolution or geometry starts to vary, call `RETRO_ENVIRONMENT_SET_GEOMETRY`.

### Future hardware rendering

Do not use Glutin/Eframe in the core. A real GL variant will require:

- `RETRO_ENVIRONMENT_SET_HW_RENDER`;
- `context_reset` and `context_destroy` callbacks;
- recreating every GL resource after a context loss;
- functions through the frontend's `get_proc_address`;
- the frontend's framebuffer and `RETRO_HW_FRAME_BUFFER_VALID`.

This is a second video route, not a trivial option on top of the software MVP.

## Audio

Libretro expects interleaved stereo PCM16 in the batch callback. The current `Mixer` produces
`f32`; the adapter must clamp and convert to `i16`.

MVP configuration:

```text
44,100 Hz, stereo; on each retro_run the core delivers (virtual ms elapsed x 44.1) samples
```

Flow:

1. create `Mixer::silent(44_100)` in `retro_load_game()`;
2. connect it to the session before the first `EVT_APP_START`;
3. render the amount tied to the virtual step;
4. call `retro_audio_sample_batch`;
5. handle a short return in frames with a bounded queue and a documented drop policy;
6. never block `retro_run()` waiting for audio.

Do not use the environment's asynchronous audio callback. Video, audio and input must come out of
the core's normal synchronous thread.

## Input and controllers

Register:

```text
RETRO_ENVIRONMENT_SET_CONTROLLER_INFO
RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS
```

Libretro port 0 maps Zeebo port 0; port 1 maps Zeebo port 1. The device configured before the load
must go into `Session::start_software()`, because games can enumerate HID as early as
`EVT_APP_START`.

Initial mapping:

| RetroPad | Zeebo |
|---|---|
| D-pad | `input::DPAD` |
| Y | `b1` |
| B | `b2` |
| X | `b3` |
| A | `b4` |
| L | `zl` |
| R | `zr` |
| Start | `start` |
| Select | `back`/HOME |
| left analog X/Y | `Pad::axes[0]`/`[1]` |
| right analog X/Y | `Pad::axes[2]`/`[3]`, when applicable |

On each `retro_run()`:

1. call `input_poll()` at least once;
2. query the joypad and analogs of the active ports;
3. normalize the `i16` values to the travel of `Pad::set_axis()` (`-128..=128`);
4. apply both `Pad`s before running the guest.

Input bitmask is a future optimization. Individual polling is valid and simpler for the first
core.

### Zeebo devices per port

The emulator already distinguishes four devices the guest can enumerate over USB/HID:

| Zeebo device | Current identity/effect | Libretro source |
|---|---|---|
| Dragon (`Controle`) | joystick `1EAA:0135` | standard RetroPad |
| Z-Pad | joystick `1A5C:3033`; buttons/axes same as the Dragon | RetroPad subtype `Z-Pad` |
| Boomerang | shared receiver, limited buttons and accelerometer | RetroPad subtype `Boomerang` + sensor/fallback |
| USB keyboard | keyboard HID enumeration and AVK events | subtype `USB Keyboard` + RetroKeyboard |

Register `RETRO_ENVIRONMENT_SET_CONTROLLER_INFO` for both ports. Each list must contain
`Disconnected`, `Dragon`, `Z-Pad`, `Boomerang` and `USB Keyboard`. The first three game controllers
can be subclasses of `RETRO_DEVICE_JOYPAD` using `RETRO_DEVICE_SUBCLASS`; the core must, as the
spec asks, keep polling the base type (`RETRO_DEVICE_JOYPAD`/`RETRO_DEVICE_ANALOG`) and use the
subtype only to define the device the guest sees. `retro_set_controller_port_device` updates
`Machine::set_ports()` and re-sends the input descriptors; for titles that enumerate HID at boot,
document that a reset is required after changing the device.

The Boomerang is not a full Z-Pad. Expose only the D-pad, `b1`, `b2` and HOME in its descriptors,
because those are the bits the physical receiver delivers. The guest already receives a specific
report with a counter, player toggle and periodic position signal; preserve that protocol instead
of turning acceleration into Z-Pad axes.

### Motion and Boomerang

The standalone today reads the Wii Remote and controller IMUs directly from `/dev/input` on Linux.
The Libretro core **must not** reuse those readers: the frontend owns the host devices, and opening
a Wii Remote/IMU on the side fails in a sandbox, duplicates input and would only work on some of
the systems.

In `retro_load_game()`, when a port is configured as Boomerang, negotiate
`RETRO_ENVIRONMENT_GET_SENSOR_INTERFACE`. If it exists, call `set_sensor_state(port,
RETRO_SENSOR_ACCELEROMETER_ENABLE, 100)` and, on each `retro_run()`, read
`ACCELEROMETER_X/Y/Z` to feed `Session::set_port_motion(port, acceleration)`. The 100 Hz requested
match the pace of the emulated receiver; the machine can keep emitting packets at the virtual pace
even if the frontend only updates the sample at 60 Hz.

The Libretro sensor API is experimental, but the official convention is acceleration in **g**,
including gravity: at rest, roughly `[0, 0, 1]`; +X is right, +Y is up and +Z points at the user
seen from the front. The physical orientation of the controller and the Boomerang's frame of
reference remain distinct. So the adapter has to:

- keep per-port calibration in `<save-dir>/zeebx/input.json`;
- normalize the at-rest reading to `[0, 0, 1] g`;
- apply the known rotation of the Boomerang protocol and a configurable orientation, including axis
inversion;
- use the last valid value, not zero, between readings;
- disable the sensor in `retro_unload_game()`;
- report clearly when the frontend has no sensor or refuses to enable it.

A controller/Wii Remote connected to the host will only provide motion if the **Libretro
frontend** exposes it through this interface. This has to be tested in RetroArch per platform; it
is not right to promise universal Wii Remote support just because the Linux standalone reads it.

As a portable fallback, offer a real per-port option only after the MVP:

```text
zeebx_boomerang_p1_source = sensor | analog_right | static
zeebx_boomerang_p2_source = sensor | analog_right | static
```

`analog_right` converts the right analog into X/Y tilt and keeps Z gravity; `static` delivers
`[0, 0, 1]`. Do not enable this option until Boomerang games are calibrated and tested. A gyroscope
is not needed by the known protocol; do not ask for `RETRO_SENSOR_GYROSCOPE_*` without a
measurement that uses it.

### USB keyboard and BREW events

`IHID` does not cover every BREW input. The Z-Wheel, in particular, may expect `AVK_0` or
`AVK_CLR`; delivering only Z-Pad state can leave it with no navigation.

When the port is configured as `USB Keyboard`, register `RETRO_ENVIRONMENT_SET_KEYBOARD_CALLBACK`
and keep a queue of transitions. The callback does not run the guest directly; `retro_run()`
drains the queue and calls `Session::set_key(avk, pressed)`. This avoids reentrancy and keeps all
access to the emulator on the core's normal thread. Use the callback for transitions/text, without
confusing `keycode` with a Unicode character: one key can produce several characters, and
key-only or character-only events are valid. Also poll `RETRO_DEVICE_KEYBOARD` for sustained
state, such as held arrow keys.

Minimum mapping to test/document:

| Libretro key | BREW AVK |
|---|---|
| arrows | `AVK_UP`, `AVK_DOWN`, `AVK_LEFT`, `AVK_RIGHT` |
| Enter | `AVK_SELECT` |
| Backspace/Escape | `AVK_CLR` |
| `0`–`9` | `AVK_0`–`AVK_9` |
| `*`/`#` | `AVK_STAR`/`AVK_POUND` |

RetroPad Select can produce `AVK_CLR` on the Z-Wheel route as a shortcut, but it does not replace a
real keyboard. The physical keyboard is global in Libretro; selecting `USB Keyboard` on the port
only decides what the guest enumerates. Supporting two distinct keyboard ports would require proof
that BREW and the Libretro ABI can tell them apart.

### USB mouse

Do not announce `RETRO_DEVICE_MOUSE`, `RETRO_DEVICE_POINTER` or a USB mouse in the MVP. The current
code does not implement a `Mouse` device, mouse HID enumeration, UIDs, buttons or the matching BREW
events; searching the engine only finds the desktop interface's mouse. There is also no
measurement in the corpus proving which USB device/BREW method Zeebo titles expect for a mouse.

Future mouse support first requires implementing and measuring the guest device. Only then can the
core map `RETRO_DEVICE_MOUSE` (X/Y delta, buttons and wheel) or `RETRO_DEVICE_POINTER` (absolute
position/touch) to that protocol and add it to `SET_CONTROLLER_INFO`.

## Guest network

The current `Machine` starts with the network on and can use `ureq` to serve `IWeb`. That default
is inappropriate in a core: opening a ROM must not create an unexpected external connection.

The Libretro core must start with the network **off**, without reading `ZEEBX_SERVER` or other
process variables. When there is a working, documented option, it can allow the network by
explicit user decision; the redirect host/port must be controlled by the core, not by the global
environment. The network must not be called from an auxiliary thread, and failures must go back to
the guest as a BREW error, without panicking.

## Storage, ROMs and savegames

### Libretro layout

In `retro_set_environment()`, before asking for any directories, negotiate
`RETRO_ENVIRONMENT_GET_VFS_INTERFACE` with required version 3. That version brings directory
creation and listing, which cache/overlay need. Keep only the interface lent by the frontend and
wrap the operations behind `StorageFs`; no path returned by the frontend may be passed directly to
`std::fs`/`fopen`.

`StorageFs` has to cover open/read/write/seek, stat, mkdir, listing, rename and removal. It also
has to represent `StoragePath` without assuming `PathBuf`: a `saf://` may not accept native
`parent()`/`join()`. Discovery of the `.mif`, sibling resources and extension must use the listing
and path joining provided by the adapter. ZIP/7z must receive a VFS reader; ZIP needs a
`Read + Seek` adapter. For ordinary files, the cache/overlay uses VFS operations and atomic rename
on SAF/Android paths too; full compatibility of those paths remains conditional on the SQLite
solution described below. Only materialize a local temporary file if the frontend authorizes a
native location and the file is removed on unload.

The profile uses **both** directories the frontend provides, with the same split PPSSPP makes
between `flash0` (system) and the memory stick (saves):

```text
<system-dir>/zeebx/            # what belongs to the machine or is disposable
├── aparelho/                  # fs:/ shared between titles (the console's NAND)
└── cache/<content>/           # package extraction: disposable, and what fills the disk

<save-dir>/zeebx/              # what the player wants to keep
├── saves/mod/<folder>/        # saves store common to all games
└── metadata/<content>.json    # origin and identity index
```

**The name `zeebx` is fixed and lowercase.** It must never come from the core's display name:
RetroArch already creates its folders with that name — `states/Zeebx`, `saves/Title.srm` — and on
a case-sensitive host (`saves/zeebx` and `saves/Zeebx` are different folders) the game would end
up with two profiles.

Without `GET_SYSTEM_DIRECTORY` everything falls into the saves directory, which is the minimum the
ABI guarantees. Do not change `HOME`, `XDG_CONFIG_HOME` or the current directory of the frontend
process.

### Full disk

The cache keeps the extraction of **every** title ever opened; with no limit, a 62-game collection
goes past 1.5 GB inside the frontend's directory and fills the disk of someone who only wanted to
play. Measured on this machine: 66 zips add up to 1.6 GB extracted.

Rule: the core prunes the cache on every load, keeping the extraction in use and deleting the
oldest ones until it fits in `CACHE_LIMIT_BYTES` (512 MB). The extraction in use is never removed,
even if on its own it blows the ceiling — deleting the running game would be worse than a full
disk.

A full disk must not freeze the frontend. Two code rules back this up:

1. the core **never** calls a frontend callback while holding a mutex of its own: the pointers are
   copied before the call. A frontend prompt ("disk full, do you want to save?") stopped freezing
   once `retro_run` started releasing the lock before video and audio;
2. a write failure in the VFS is a guest error, not a panic: the game gets `IFAILED` and carries
   on, and the reason goes into the report.

If `GET_SAVE_DIRECTORY` does not return a writable directory, `retro_load_game()` must fail with a
clear message. Do not use the ROM's folder as a fallback, and do not accept a run that promises
saves but loses them in a temporary directory.

### Three layers

| Layer | Where | Persistent | Shared | Contents |
|---|---|---:|---:|---|
| `cache/<content>/` | system | disposable | no | extracted ROM, `.mod`, `.mif`, assets |
| `saves/mod/<folder>/` | save | yes | no | what the games write to `/mod/` |
| `aparelho/` | system | yes | yes | `fs:/`, Z-Wheel, global data |

This preserves the console's measured behavior: Zeeboids can write to `fs:/zeeboiddata`, and
Zeebo F.C. can read that data in another session.

### Layered VFS

Replace the current single root with something equivalent to:

```rust
pub struct Vfs {
    content_root: PathBuf, // logically read-only
    save_root: PathBuf,    // per-game overlay
    device_root: PathBuf,  // shared fs:/
    boundary: PathBuf,     // safe limit for fs:/~/../
}
```

Rules:

- relative/`fs:/~/` read: overlay first, base content after;
- relative create/write: always in the overlay;
- `READWRITE`/`APPEND` of a file that only exists in the base: copy-on-write to the overlay;
- map `READWRITE`/`APPEND` to the VFS mode `UPDATE_EXISTING`, because opening for write without
  that flag truncates an existing file;
- removing a file from the base: create a tombstone in the overlay, so the file does not reappear
  on the next listing/open;
- listing: union of overlay + base minus tombstones; overlay wins, case-insensitive comparison and
  deterministic order;
- `fs:/...`: `device_root`;
- `fs:/mod/...`: keep the current fallback between device and installation;
- never change the ROM, the original ZIP/7z or the content cache;
- preserve the current VFS's limits on `..`, absolute paths and case-insensitivity.

Path selection must happen after the open mode is known. So `machine/file.rs` cannot pick
`vfs.resolve()` before knowing whether the operation is read, create, append or read-write.

### Critical limit: the engine's file APIs and SQLite

The adaptation is not just swapping `loader/archive.rs`. Today the core path uses
`std::fs`/`PathBuf` in `Session`, archive, `Vfs`, `machine/file.rs`, shell, media, fonts, widgets
and SQL; `Machine` keeps `std::fs::File` open. On top of that, `rusqlite` opens databases by native
path and can create a journal or WAL. A `saf://` path from the frontend is not valid for those
APIs.

Before promising Android/SAF, create a `StorageFs`/`GuestFile` layer used by **all** of the
engine's I/O, with a native backend for desktop and a `retro_vfs_interface` backend for the
frontend. Inventory every `std::fs` call that still enters the core path; UI, tests and CLI can
stay native behind the `desktop` feature.

SQLite requires an explicit decision:

1. implement a SQLite VFS on top of `retro_vfs_interface`; or
2. materialize a transactional copy of the database in an authorized native directory, open it
   with SQLite and sync back through the VFS, including journal/WAL and commit failures.

Option 2 is only valid where the frontend provides an authorized native location; it does not
serve to make `SAF` magically compatible. Until the option exists, the MVP must declare support
for the **native** VFS (Linux x86_64/AArch64) and not promise loading through SAF/Android. The
Android criterion comes in after tests of SQLite, VFS, ZIP and real saves on that platform.

### Identity

The current fingerprint (`name + size + mtime`) is not enough. For ZIP/7z:

```text
content_hash = BLAKE3(bytes of the compressed file)
```

For a direct `.mod`, use at least the bytes of the `.mod`, the associated `.mif` and the origin
context.

Human-readable directory:

```text
<normalized-title>-<classid>-<short-hash>
```

The full hash stays in the metadata. A ROM update gets its own save for safety; the device's
global data stays shared.

## ZIP and 7z extraction

Extracting the `.mod` and `.mif` is not enough. Resources, databases, fonts and audio can be opened
after boot. Extract the whole validated tree.

ZIP already has a base in `loader/archive.rs`. For 7z, use a built-in/Rust decoder or a portable
library. Do not call an external `7z` binary: that breaks AArch64, Android, sandboxing and portable
distribution.

Before announcing `7z`, validate with real files. Encrypted archives without a password must be
refused with a clear message.

Security requirements for both formats:

- refuse absolute paths and `..`;
- refuse symlinks, hardlinks and special nodes;
- limit the number of entries;
- limit the entry size and the total uncompressed size;
- do not recursively extract archives inside archives;
- extract to `<hash>.partial`;
- validate the `.mod`/`.mif` and the manifest;
- atomically rename to `<hash>` on success;
- per-hash lock for concurrent instances.

### Cache or `/tmp`

The cache lives in `<system-dir>/zeebx/cache/`, persistent across sessions, so the package is not
decompressed on every open — and **outside** the saves directory, which is what the frontend
syncs. It is pruned to 512 MB on every load (`CACHE_LIMIT_BYTES`), preserving the extraction in
use.

Optional future mode:

```text
zeebx_extract_mode = cache | temporary
```

In `temporary` mode, content can go to a private temporary directory and be removed on unload, but
saves and `aparelho` always stay under `<save-dir>/zeebx/`. Never write saves to `/tmp`.

### Concurrency and integrity

`aparelho/` represents a single virtual console and is shared between titles. Two simultaneous
instances can corrupt the SQLite database, the configuration or global saves. The current VFS ABI
does not offer a portable exclusive lock, so the core **cannot promise** it will block a second
instance on SAF/sandbox just by creating `<save-dir>/zeebx/.lock>`.

Implement a profile lock as a best-effort improvement on the native backend, and treat concurrency
as unsupported/documented on VFS backends without an atomic lock. The per-hash cache lock stays
separate: it only coordinates extraction when the backend allows exclusive creation. Do not use a
plain "file exists" as a guarantee of mutual exclusion.

Writes of metadata, manifests, tombstones and saves produced by the core must use a provisional
file + atomic rename when the VFS backend guarantees that operation. On unload, close and sync the
files the guest still keeps open. SQLite and other writers need their own commit/journal policy;
atomically renaming external files does not protect their open databases.

## Useful environment callbacks

| Callback | Use |
|---|---|
| `SET_PIXEL_FORMAT` | RGB565, mandatory in the MVP |
| `GET_SAVE_DIRECTORY` | root of `zeebx/` |
| `GET_SYSTEM_DIRECTORY` | future firmware/data |
| `GET_LOG_INTERFACE` | load/VFS/error logs |
| `SET_CONTROLLER_INFO` | port devices |
| `SET_INPUT_DESCRIPTORS` | RetroPad labels |
| `SET_GEOMETRY` | only when the geometry varies |
| `GET_CORE_OPTIONS_VERSION` | only when there are real options |
| `SET_CORE_OPTIONS_V2` | future options |
| `GET_VARIABLE_UPDATE` | re-read future options |
| `GET_VFS_INTERFACE` | mandatory before directories/load; all of the core's I/O goes through it |
| `GET_VFS_AUTHORIZED_LOCATIONS` | when available, validate the VFS roots where cache/files can exist |

Do not register an option that is cosmetic or that cannot be applied safely to a live session.

## Core Options: mechanism and design for Zeebx

The **Core Options** menu is not fed by the core's `.info`. The core registers the options through
the `retro_set_environment` callback, using the `libretro.h` ABI:

1. call `RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION` (`52`);
2. if the version is at least 2, register a static table with
   `RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2` (`67`), using `retro_core_options_v2`, categories and
   `retro_core_option_v2_definition`;
3. for old frontends, fall back to `RETRO_ENVIRONMENT_SET_VARIABLES` (`16`), with
   `retro_variable { key, "Name; value1|value2" }`;
4. read each value with `RETRO_ENVIRONMENT_GET_VARIABLE` (`15`), using keys prefixed with
   `zeebx_`;
5. query `RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE` (`17`) in the loop when there is an option that
   can change during the session.

The frontend keeps a copy of the strings and tables. So descriptions, values and terminators need
static lifetime; do not point at a local `String`. The `key` must be stable and the same across
versions so RetroArch preserves the choices. `retro_get_system_info` still describes only the core
and the extensions (`mod|zip|7z`); there is no user option there.

**Status: implemented.** `retro_set_environment` registers controllers, input descriptors and the
core options, through `SET_CORE_OPTIONS_V2` when the frontend announces version 2, and through
`SET_VARIABLES` when it does not. `retro_run` queries `GET_VARIABLE_UPDATE` (17) once per frame and
re-reads **only** what can be applied without recreating the session.

### Options registered today

```text
key:      zeebx_midi_backend
category: audio
values:   auto | timbres | soundfont
default:  auto
applies:  on content reload (the label says "(restart)")

key:      zeebx_volume
category: audio
values:   100 | 90 | 80 | 70 | 60 | 50 | 40 | 30 | 20 | 10 | 0
default:  100
applies:  right away

key:      zeebx_rasterizador
category: video
values:   auto | software
default:  auto
applies:  on content reload

key:      zeebx_resolucao_interna
category: video
values:   1 | 2 | 3 | 4
default:  1
applies:  on the next frame

key:      zeebx_antialias
category: video
values:   1 | 2 | 4 | 8
default:  1
applies:  right away

key:      zeebx_filtro_anisotropico
category: video
values:   1 | 2 | 4 | 8
default:  1
applies:  right away

key:      zeebx_neblina
category: video
values:   enabled | disabled
default:  enabled
applies:  right away
```

**`zeebx_rasterizador` is the only way out for a GL driver that accepts the context and draws
wrong.** When the driver really fails, the core already falls back on its own; when it accepts
everything and delivers a black image, from the core's point of view nothing failed, and without
this option the remedy would be to change frontend or device.

**Internal resolution is supersampling here, not a bigger image.** The frame delivered to the
frontend is still 640×480 — that is what `retro_get_system_av_info` declares as
`max_width`/`max_height`, and delivering more than that would be a defect. Drawing happens at
scale and the readback reduces it again, which smooths polygon edges. In the desktop GUI the
**same** engine call has another visible effect, because there the big frame goes to the window
through `Session::gpu_frame`, a path the core does not use. Copying the description from there to
here would promise something that does not happen here.

```text
key:      zeebx_soundfont_taxa
category: audio
values:   44100 | 22050
default:  44100
applies:  from the next song on

key:      zeebx_midi_vozes
category: audio
values:   128 | 96 | 64 | 48
default:  128
applies:  from the next song on

key:      zeebx_cache_de_som_mb
category: audio
values:   24 | 48 | 16 | 8 | 4
default:  24
applies:  from the next eviction on
```

**These three are engine globals, not construction parameters.** The synthesizer policy
(`zeebx_midi_backend`) is a parameter because it changes what the machine **is** when it is born:
the bank is opened at construction. Rate, voices and cache ceiling apply to the next song and the
next eviction — a song already synthesized does not change rate. One more parameter in five public
signatures for a value nobody needs at birth is cost with nothing in return, and the bank module
already keeps a global for the same reason: the cache of open banks.

**The label says when the effect arrives.** None of the three asks for a restart, but none applies
right away like the volume. Leaving that out would make the user conclude the option does not work.

```text
key:      zeebx_perfil
category: sistema
values:   padrao | portatil
default:  padrao
applies:  MIDI synthesizer on reload; the rest without reloading
```

**"Handheld" exists because ten loose buttons are not what the RG40XX-H needs.** It bundles in one
go what the investigation on weak ARM measured as the most expensive: timbre table instead of
SoundFont, reduced MIDI rate and voices, smaller sound cache, no supersampling in 3D.

**Deliberately left out:** volume and fog, because they are a matter of the player's taste and not
processor cost; and the rasterizer, because forcing the processor would take the GPU away from
those who have a capable one — the separate option is still the way out for whoever needs it. A
preset that sacrifices things unrelated to performance is the classic defect of this kind of
button.

While active, the profile **wins** over the individual options it covers — they stay visible in the
menu, but have no effect. It is the only way for a preset to work without the API having a way to
hide an option.

```text
key:      zeebx_frameskip
category: video
values:   desligado | automatico | 1 | 2 | 3 | 4 | 5 | 6
default:  desligado
applies:  right away

key:      zeebx_limite_fps
category: sistema
values:   60 | 30 | desligado
default:  60
applies:  right away
```

### Frameskip and the speed limit are not the same thing

**`zeebx_frameskip` saves rasterization, not game time.** `gles_draw` and `Clear` are called
during API dispatch by the CPU — there is no separate GPU queue as in Flycast. When a frame is
skipped, the ARM logic carries on; what goes away is reading vertices from guest memory and filling
pixels. Fixed skips `N` frames out of every `N+1`; Automatic uses the official
`SET_AUDIO_BUFFER_STATUS_CALLBACK` mechanism (`libretro.h` says to try skipping when
`underrun_likely=true`) and asks for 96 ms of audio headroom, six frames at 60 Hz.

`glReadPixels` is a safety exception: Crash Nitro Kart reads the framebuffer back into guest
memory. After the first readback, frameskip turns itself off for the rest of the session and warns
— returning the previous frame would stop being a visual loss and could change logic. The first
`ReadPixels` can still land after a skipped frame, because there is no knowing it will come before
serving the previous `Draw*` calls; the protection avoids repeated corruption, it does not promise
to guess the future.

**`zeebx_limite_fps` limits real speed, it does not touch the virtual clock.** It exists because
the Libretro core used `Session::run_frame`, which did not have the `ahead_ms()` brake the desktop
already used: Crash Nitro Kart was measured at 1039% and Zeebo Extreme Rolima at 267% of console
speed when the host has headroom. At 60, before advancing the next frame, the core waits for the
real clock to catch up with the virtual one; at 30, it keeps the same logic at 1x but duplicates
the presentation every two frames. So 30 is not "advance 33 ms per call" — that could speed games
up, exactly the defect the option tries to avoid. Off preserves a deliberate boost, useful for
Need for Speed.

**Audio:** there is no time-stretching/pitch-shifting. The mixer resamples each voice to the
card's rate, fades on underrun and drops the excess so delay does not accumulate; the core delivers
samples by the difference of the virtual clock. With the normal limiter, the sleep comes **before**
advancing the next frame: the previous frame's audio plays while the core waits, so there is no
need to stretch audio to hold 1x. An anomalous clock jump never blocks more than 50 ms in one call,
so as not to freeze the frontend while loading.

### Defect found in this phase: the "video" category was never registered

The five video options of the previous phase declared `category_key: c"video"`, but the
`CATEGORIES` array only had `"audio"`. The frontend does not freeze on a category that does not
match — the option just lacks the right grouping in the menu — and that is why the defect did not
show up in any test. Fixed together with the profile's `"sistema"` category.

### How a new option should be born

Every option that can be applied without recreating the session goes into `apply_hot_options`, and
**not** into its own `if`. The `GET_VARIABLE_UPDATE` notice is consumed on the first query: one
option per `if` would make the first one eat the notice of the others, and the defect would show
up as "sometimes it does not take".

The same function runs when the session is born and when it is switched, so there are not two
copies of the application logic — the one that runs less is the one that goes wrong without anyone
seeing.

A missing option or one with a broken value **keeps what was there**, instead of going back to the
default: an old frontend that does not know the key must not undo the choice of whoever configured
it.

**Values are tokens, and the human text goes in the `label`.** The value is what gets saved in the
user's `.opt` and what `FromStr` parses again: `Tabela de timbres`, with an accent and spaces, is
fragile at both ends. In the corpus of the twelve libretro cores installed on this machine, a value
is always a stable token (`enabled`, `disabled`, `auto`, `scph5500.bin`). The `FromStr` of
`MidiBackend` also accepts the old spellings, so an `.opt` already saved does not break.

**The restart is marked in the label** because the API has no field for it — that is the
ecosystem's convention, where `(Restart)` is the most common mark.

`Auto` preserves the current behavior: it uses an `.sf2` when one is found and falls back to the
table when there is no bank or the bank is invalid. `Timbre table` avoids the long wait on the
RG40XX-H. `SoundFont` silently refuses the fallback only after warning that there is no bank, or
keeps the current fallback if compatibility is preferred. The option text must say that changing
it requires reloading the content.

The option cannot be merely decorative: today `Machine::new` calls `device_sound_bank` while
building the session and keeps `sound_bank`; `Session::start_*` and `Machine::new_with_storage` do
not receive an audio policy. The implementation has to load the option in `retro_load_game`, pass
it through the creation of the `Session`/`Machine` and also apply it in `switch_to`, which recreates
the session when the Z-Wheel opens a game or returns to it. Reading the option in `retro_run`
without rebuilding the session would leave the menu saying one thing and the already-loaded bank
doing another.

### Performance options, only after measurement

Do not register controls for `block_size` or chorus/reverb yet, before a real implementation
exists. The candidates are:

```text
zeebx_midi_effects: On | Off
zeebx_midi_block:  64 | 1024
```

The local measurement of 2026-09-22 showed 279 ms for the 47.5 s track with block 64 and effects
on, and 149 ms with block 1024 and effects off. That is about 1.9x, but on its own it does not
explain the more than two minutes observed on the H700; that is why the first option must be the
MIDI backend, and not exposing performance internals as if they were the solution.

## Catalog, identification and covers

The collection has two layers of identity, and both exist today:

| Identity | What it is for | Where |
|---|---|---|
| BLAKE3 of the package bytes | cache and saves overlay: the same ROM opens in the same place, a swapped ROM does not inherit the save | `StoragePaths::content_id` |
| No-Intro (CRC32/MD5/SHA1 of a file inside the tape) | telling **which game it is**, with the official name, and finding the cover | `tools/catalog.py` |

### What the Zeebo No-Intro hashes

The official DAT is `Mobile - Zeebo` (libretro-database, `metadat/no-intro/`). It hashes **one file
inside `mod/<folder>/`**, and the name in the DAT is the internal path without the slashes:

```text
mod274754sound.ggz                              -> mod/274754/sound.ggz
mod280386resources.pakz                         -> mod/280386/resources.pakz
modnfsresourcestracksworld_3401.viv             -> mod/nfs/resources/tracks/world_3401.viv
```

The module folder **is not always numeric** (`mod/nfs/...`); comparing by prefix gets Need For
Speed wrong. The comparison is on the whole path without separators.

### Measurement against the user's collection

```text
packages: 62
verified by No-Intro: 57 of 57 in the DAT (0 mismatched)
outside the DAT: 5
```

The five outside the DAT are the four Data East ports and one homebrew:

- `Bad Dudes vs. DragonNinja`
- `Caveman Ninja`
- `Dark Seal`
- `Karnov's Revenge`
- `Kingdom Hearts V CAST (Zeebo Homebrew)`

### What the tool writes

`tools/catalog.py --roms DIR --out DIR [--icons]`:

- `Mobile - Zeebo.lpl` — RetroArch playlist, `label` with the No-Intro name, `crc32` of the package
  and `db_name` of the database;
- `thumbnails/Mobile - Zeebo/{Named_Boxarts,Named_Snaps,Named_Titles,Named_Logos}/` — the four
  folders RetroArch looks in, with each title's exact name;
- `catalogo.json` — hashes, verification and the path of each package's `.mod`/`.mif`.

### The official covers come inside the Z-Wheel itself

The Z-Wheel package carries the store's covers and the database that links them to the games:

```text
mod/274755/tt_game_info                     SQLite
  GAMEINFO(game_id, class_id, boxart_path, ...)   -> class_id is the applet's ClassID
  TITLETEXT(game_id, lang_id, titletext)          -> official name per language
mod/274755/assets/games/<game_id>/boxartlg.jpg    170x220, the largest published
mod/274755/assets/games/<game_id>/boxart.bmp      160x227
```

The join key is the **applet's ClassID**, which the emulator already reads from the `.mif` — not a
file or folder name. Measured against the collection:

```text
official covers written: 58 of 62
no cover: Kingdom Hearts (homebrew, no applet ClassID in the .mif)
          Z-Wheel, Zeebo App and Zenonia (they have no store entry)
```

`--zwheel PACKAGE` turns that path on; `--covers-beside` copies the cover next to the `.zip`, which
is where the **standalone** frontend looks (`library::cover` reads `<game>.png|jpg|bmp` next to the
file). With `--icons`, the `.mif` icon goes into `Named_Titles` as a stopgap — a menu icon, 65×42
at most, not a cover.

What is still missing for a "published fullset":

1. covers at a higher resolution than the store's (170×220 is what the package has);
2. ~~Zeebo RDB~~ — **done**, and RetroArch's "Scan Content" matches by hash;
3. submitting the covers to the Libretro thumbnails repository, with the system name equal to the
   database's (`Mobile - Zeebo`), which is the directory RetroArch looks in — the covers **already
   are** in the right format and with the right names, ready to upload.

### RDB

**Solved.** The Zeebo RDB did not exist anywhere (135 RDBs installed, none for Zeebo) and now it
does, generated by `tools/rdb.py` from RetroArch's own format (`libretro-db/libretrodb.c` and
`database_info.c`), checked against the official RDBs:

```text
16 bytes     "RARCHDB\0" + uint64 **big-endian** with the metadata offset
records      msgpack maps in sequence, one per game
1 byte       0xC0, end sentinel
metadata     msgpack map { "count": N }
```

Fields of each record, the same as in the official RDBs: `name`, `description`, `rom_name`,
`size`, `crc` (4-byte binary, big-endian), `md5`, `sha1`.

Each game goes in **twice**: with the CRC of the `.zip` and with the CRC of the file No-Intro
hashes inside it. The scanner queries `crc:or(b"<inner file>", b"<package>")`, so the collection
matches by the package and also by the inner ROM — the latter survives recompressing the zip.

Measured, with the core installed:

```text
[Scanner]: Add "Double Dragon (Brazil) (Es,Pt)" to "Mobile - Zeebo.lpl"
[Scanner]: Add "Zeebo Sports Peteca (Brazil) (Es,Pt)" to "Mobile - Zeebo.lpl"
[Scanner]: Add "Caveman Ninja (Brazil) (Es,Pt)" to "Mobile - Zeebo.lpl"
[Scanner]: Add "Zenonia (Brazil) (Es,Pt)" to "Mobile - Zeebo.lpl"
```

The last two lines are not in No-Intro and have no store entry: they match through the **CRC of
the `.zip`** record, which is generated for all 62 packages. In other words, the database names the
whole collection, not only the part verified by the DAT.

Two conditions for the scan to find the database, and both are easy to forget:

1. the database has to be `<database name>.rdb` inside `content_database_path` (in the user's
   config: `~/.config/retroarch/database/rdb`), with the name equal to the `database` field of the
   `.info`;
2. the default scanner requires the content to already match an **installed core**
   (`scan_without_core_match = "false"`). Without the core in the cores folder, it does not even
   enter the database phase and marks everything `??`. With the core installed it works; if it
   still does not match, turn on `scan_without_core_match`.

### Explicit destinations, and the `.info` next to the core

Two lessons from this stage, both measured on the machine:

1. **the core and the `.info` go together.** An old `.info`, without the `database` line, installed
   next to a new core, breaks the scan from the menu: RetroArch associates no database with that
   core, and all content comes out as `??`. That is exactly what happened here — the `.info` had
   been copied before the line existed;
2. **no artifact may land in the frontend's folder by accident.** `--out` was used
   for the playlist, catalog and DAT cache, and pointing it at the RetroArch configuration dropped
   `Mobile - Zeebo.lpl`, `catalogo.json` and `Mobile - Zeebo.dat` at its root. Now the playlist,
   catalog and DAT have their own destinations (`--playlists`, `--catalog` and the DAT next to the
   catalog).

### Playlist

RetroArch 1.20 writes a playlist as **one JSON document** with a header and `items` — not the old
one-JSON-per-line format. The `crc32` carries the `|crc` suffix and the `db_name` carries `.lpl`.
The tool copies the format of whoever reads it, instead of inventing one.

## How a game ends

On the console, leaving a game hands control back to the Z-Wheel, which is just another installed
applet. In Libretro this has two possible forms, and they are not equivalent:

| Path | How it works | Status |
|---|---|---|
| Frontend closes the content | the core calls `RETRO_ENVIRONMENT_SHUTDOWN` (7) | **implemented** |
| Core loads the Z-Wheel | the core itself starts another `Session` with the Z-Wheel's `.mod` and keeps presenting | **implemented** |

The engine already knows the shell asked for another applet: `Machine::pending_launch`, exposed by
`Session::take_launch_request()`, is what the desktop UI uses to go back to the Z-Wheel. What does
**not** exist in the ABI is a "load this other content" from the core to the frontend — no command
makes the frontend switch games at the core's request. So going back to the Z-Wheel can only
happen if the core loads it internally.

### Wiring done in the core

The engine already had the pieces; the core just had to use them.

| Piece | How it ended up |
|---|---|
| Buttons | the four face buttons follow the device: **1 at the bottom** (RetroPad `B`, PlayStation `X`, SNES `B`) and the numbering goes up clockwise — 2 on the left (`Y`), 3 on top (`X`), 4 on the right (`A`). The mapping labels point to the same `id` the core reads |
| Video | **1x and native aspect ratio, always**: `set_internal_resolution(1)` and `set_aspect_ratio(None)` on load and on session switch. A frame outside 640×480 is warned about once in the log, instead of showing up as a skewed image with no explanation |
| Keyboard | `SET_KEYBOARD_CALLBACK` receives the keys and **queues** them; `retro_run` delivers the `AVK`s. Mapped: arrows, Enter, Backspace/Esc, digits, `*` and `#`. RetroPad Select counts as `AVK_CLR` |
| Library | `library::scan` on the content folder, deduplicated by ClassID, feeds `set_installed_applets` — without it the Z-Wheel opens empty |
| Session switch | the launch request ends the current session and starts the new one with the same `StoragePaths`; the frontend does not even notice |
| Back to the Z-Wheel | when the game ends, if the session was the Z-Wheel or was opened by it, the core goes back to it instead of asking for `SHUTDOWN` |
| System font | `system_font_in(cache, device)` copies the package's `tectoy.ttf` to the root of the frontend's device. If the collection only has the extracted Z-Wheel, `install_package_font` takes it from next to the module |

Measured, headless, with the Z-Wheel as content:

```text
Zeebx: 63 game(s) installed from /media/.../zeebo/ROMs
Zeebx: system font in .../system/zeebx/aparelho/shared/fonts/tectoy.ttf
screen: 640x480, 398 distinct colors
```

The 63 are 62 titles plus the Z-Wheel itself; before deduplication by ClassID there were 124,
because the ROMs folder has the `.zip`s **and** an already-extracted copy.

Current behavior:

1. the outcome is reported **once**, with the virtual clock (`stopped at N virtual ms`), followed by
   the last lines of the game's own log and the heap usage;
2. if the shell asked for an applet and it is in the games folder, the session switches; if it is
   not, the ClassID goes to the log saying it was not found;
3. the final screen stays up for **2 seconds**; then the core goes back to the Z-Wheel, if it is
   available, or asks for `SHUTDOWN` so the frontend goes back to its menu;
4. the unload request is made **outside** the state mutex, like every call to the frontend.

The core-loads-the-Z-Wheel path requires resolving the ClassID to an installed package — the core
does not have a library scan yet — and deciding the "next content" policy with the frontend. It
stays as a feature, with the engine already ready to start a new session.

### Installation and CI

- `tools/install_core.py` copies the `.so` **and** the `.info` **together**. The two go as a pair:
  an old `.info` next to a new core breaks the scan from the interface, silently — it has already
  happened here. The script warns when the destination `.info` was different from the one going
  in;
- `.github/workflows/libretro.yml` builds both targets — **x86_64** and **AArch64 on a native ARM64
  runner**, because cross-building `unicorn`/`dynarmic` would require a toolchain and sysroot that
  nobody wants to maintain just for packaging — and checks three things, none of them "it
  compiled":
  1. the 25 `retro_*` symbols are exported (`nm -D`);
  2. the `.so` does not pull in `libX11`, Wayland, EGL, ALSA, udev or GTK (`ldd`);
  3. the published package carries the `.info` with the `database` field.

### Player notices and button reading

- `SET_MESSAGE` shows notices on the frontend's screen ("no `tectoy.ttf`", "the shell asked for a
  class that is not in the folder"), besides writing them to the log — not every frontend shows
  messages, and a notice nobody sees is useless;
- buttons are read by **bitmask** when the frontend announces support (`GET_INPUT_BITMASKS`), which
  cuts twelve queries per frame down to one. **RetroArch 1.20 returns false**, so in practice it
  uses individual queries; the bitmask path is there for frontends that announce it. The log says
  which of the two is in use, instead of leaving it invisible.

## What the Data East ports are missing: the `IFont` interface

Investigating Double Dragon's white frame, the **underlying** cause showed up: it asks for three
classes we answer as unknown.

```text
classes the game requested and we do not have:
  0x0102f679   AEECLSID_FONT_STANDARD11
  0x01030852   AEECLSID_FONT_STANDARD15
  0x0102f681   AEECLSID_FONT_STANDARD36
```

They are **BREW's standard bitmap fonts**, and the SDK's `AEEFontsStandard.bid` confirms what each
one is (documented ascent/descent) and which interface they implement:

```c
#define AEECLSID_FONT_STANDARD11  0x0102f679   /* IFont */
#define AEECLSID_FONT_STANDARD15  0x01030852   /* IFont */
#define AEECLSID_FONT_STANDARD36  0x0102f681   /* IFont */
```

**`IFont`, not `ITypeface`** — that is the difference that matters, and it is why the mapping could
not just be "point at our TrueType font". `AEEFont.h` lists the six methods, and a reference
emulator already on disk (`projects/zeebo-emulator/.../zeemu/brew/BrewFont.cpp`) gives the vtable
order and the metrics table of all the classes:

```c
AddRef(0)  Release(1)  QueryInterface(2)  DrawText(3)  GetInfo(4)  MeasureText(5)
```

The whole family is 30 classes: `FONTSYS*` (6), `FONT_STANDARD*` (11), `FONT_BASIC*` (10),
`FONT_FIXED4X6`, `AEECLSID_FONT` and `BITFONTFOUNDRY`. The signatures come from the SDK's own
examples:

```c
IFont_GetInfo(pFont, &info, sizeof(info))
IFont_MeasureText(pFont, pszBuf, WSTRLEN(pszBuf), IFONT_MAXWIDTH, &nChars, &extent.width)
```

What to do, in the order the project follows — **measure before inventing**:

1. use the existing probe (`--probe=0x0102f679`) on a ROM that asks for the class, to record which
   slot the game calls and with what arguments. `DrawText` is the only one whose layout is not
   measured yet;
2. implement `Interface::Font` with the six slots, `GetInfo` writing the four metrics and
   `MeasureText` returning the table's width and height;
3. map the 30 classes to that interface, with `DrawText` drawing on the current destination — the
   same path `IDISPLAY_DrawText` already uses.

Without this, every title that draws with the system fonts ends up in the same place: Double
Dragon, Resident Evil 4 (`0x0102f681`) and the Data East ports.

## Save states

Save states require a pointer-free, versioned snapshot of:

- the Dynarmic CPU;
- guest memory;
- the heap and BREW objects;
- callbacks, signals, threads and timers;
- the relevant VFS;
- audio;
- the rasterizer and graphics resources;
- extensions and applet state.

When implemented, `retro_serialize_size()` must not grow between `retro_load_game()` and
`retro_unload_game()`. Validate:

```text
save → run N frames → restore → run N frames
```

Compare clock, framebuffer, memory and audio as applicable.

A save state must not pretend to be a rollback of persistent files: `saves/` and `aparelho/` stay
out of the snapshot and keep the last confirmed write on the host. This rule must appear in the
feature's documentation once it exists. Any state that holds a pointer, file descriptor or host
resource has to be rebuilt from a serialized representation, never copied raw.

## Distribution and builds

Initial targets:

```text
x86_64-unknown-linux-gnu
aarch64-unknown-linux-gnu
```

Distributed artifact:

```text
zeebx_libretro.so
```

The current host only has the x86_64 Rust target. AArch64 requires a consistent Rust target,
linker, C/C++20, CMake, Ninja and sysroot. `unicorn-engine` compiles C/QEMU; Dynarmic compiles
C++20/CMake; Linux also links `libatomic` according to the current `build.rs`.

The local `cargo check --locked` build reached the Dynarmic build, but failed for lack of disk
space while CMake/Ninja built hundreds of targets — not because of a source error. CI and
cross-builds need adequate disk space and a dependency cache.

Recommendation:

1. validate x86_64 locally;
2. use a native AArch64 runner first;
3. add cross-builds after the toolchain/sysroot is proven;
4. test real loading in RetroArch, not just compilation/dlopen.

CI must check the separate package:

```text
cargo build --locked --release -p zeebx-libretro --target x86_64-unknown-linux-gnu
cargo build --locked --release -p zeebx-libretro --target aarch64-unknown-linux-gnu
readelf -h
nm -D / check of retro_* symbols
ldd
RetroArch smoke test
```

The release collects only `target/<triple>/release/libzeebx_libretro.so` and
`frontends/libretro/zeebx_libretro.info`; it must not include the desktop binary, UI settings or
artifacts from other workspace members.

## The `.info` file

Distribute `zeebx_libretro.info` next to the `.so`, for example:

```ini
display_name = "Zeebx"
authors = "ZeebxTeam"
supported_extensions = "mod|zip"
categories = "Emulator"
systemname = "Zeebo"
manufacturer = "TecToy"
licenses = "GPLv2+"
firmware_count = 0
supports_no_game = "false"
```

Add `7z` only after real support.

## Validation performed

### Tool

RetroArch 1.20.0 with an isolated profile in `/tmp/zeebx-ra` (never the user's profile), drivers
`video = null`, `audio = null`, `input = null`, `--max-frames=N` and a final screenshot:

```bash
XDG_CONFIG_HOME=/tmp/zeebx-ra retroarch -c /tmp/zeebx-ra/ra.cfg \
  -L /tmp/zeebx-ra/cores/zeebx_libretro.so --max-frames=900 \
  --max-frames-ss --max-frames-ss-path=out.png "'ROM.zip'"
```

### What the frontend confirmed

| Step | Result |
|---|---|
| Exported symbols | 25 `retro_*` functions |
| `ldd` of the `.so` | only `libc`, `libm`, `libstdc++`, `libgcc` — no X11, Wayland, EGL, ALSA, udev, GTK |
| `cargo tree -p zeebx-libretro` | no eframe, cpal, gilrs, glutin, minifb, rfd, Discord |
| `retro_api_version` | 1 |
| `retro_get_system_av_info` | 640×480, aspect 1.333, 60 Hz, 44,100 Hz |
| `SET_CONTROLLER_INFO` / `SET_INPUT_DESCRIPTORS` | accepted |
| `SAVE_DIRECTORY` / `SYSTEM_DIRECTORY` | used; profile created in `<save>/Zeebx/zeebx/` |
| `SET_PIXEL_FORMAT` | RGB565 accepted |
| Extraction | `cache/<title>-<BLAKE3 hash>/mod/<id>/<name>.mod` |

### Per game

| Title | Frames | Time reported by the frontend | Frame delivered |
|---|---:|---:|---|
| Zeebo Sports Peteca | 900 | 14 s | 2,734 distinct colors |
| Crash Bandicoot Nitro Kart 3D | 900 | 14 s | 5,959 distinct colors |
| Double Dragon | 3,600 | 59 s | uniform white |

**Double Dragon's white frame: solved, and it was three stacked defects.**

1. `system_font()` looked in the **desktop** cache instead of the cache the frontend provided;
2. `console_font()` — the lookup at draw time — also used the desktop path, so the font installed
   in `<device root>/shared/fonts/tectoy.ttf` was never found;
3. **and the order was wrong**: the font was installed **after** the session existed. The engine
   reads the font when the machine is built, so the whole session went without any font.

The game's own report already said what was missing:

```text
text we could not draw:
  Application is finished
  Memory is insufficient.
  Please delete
  by pushing the button.
  some files.
```

Double Dragon's screen is **white with that message in black**. Without a font, it came out pure
white — which looked like "draws nothing" and was "draws invisible text". Measured, before and
after:

```text
before: 1 color  (pure white)
after:  2 colors (white + black) — the text shows up
```

What the message reveals is something else, and that one remains: Double Dragon asks for three
classes we do not have (`0x0102f679`, `0x0102f681`, `0x01030852`) and does not find
`./udata/ddz.sav`, and that is why it shows "Memory is insufficient". The emulator now **shows**
the problem instead of hiding it.

Peteca and Crash prove real video through the Libretro path. Double Dragon loads, runs 59 virtual
seconds without error and does not crash, but delivers a white frame.

**Open investigation, not an ABI defect.** The table in
`docs/implementation/11-compatibility.md` was measured with the `zeebx run` path (**Unicorn**,
`src/main.rs::run_frames`), while the core uses `Session` (**Dynarmic**). Before blaming the core,
the same ROM has to be run on both paths and compared; the document itself lists Double Dragon
with "pointer refused, text without a font".

### What the tests on Wayland found

Running Crash Nitro Kart in the user's RetroArch (Wayland + Vulkan, real video driver), the core's
log showed the most expensive defect of this stage:

```text
Zeebx: stopped at 19640 virtual ms: the game ended
```

The game ran for 19.6 s and the session ended on its own. Cause: `Machine::is_idle()` decided the
machine was idle by looking at timers, callbacks, threads and blits — but **not** at
`pending_signals` nor at the fact that the guest had registered interest in input. Crash's intro
ends exactly when it starts waiting for the player: with no timer armed and the input callback
registered, the emulator concluded "it's over". After the fix the same game goes past 27.7 s, and a
title that sits waiting for a button keeps running, as on the console.

Second finding, still on Wayland: with the machine's disk full, RetroArch opened the save prompt
and the emulator froze. The core called `video_refresh` and `audio_sample_batch` while holding the
state mutex, and also called the environment callback while holding the mutex of the frontend
pointers. Now the pointers are copied and the locks released before any call to the frontend, and
the frame and audio buffers leave the state before video/audio and come back after.

### Test hygiene

The suite left garbage behind: the SQL and saves tests created `/tmp/zeebx-sql-*` and
`/tmp/zeebx-saves-*` and cleaned up **at the start** — in case the previous run had failed — and
never at the end. A session with several runs piled up ten directories. Now there is
`src/scratch.rs::TempDir`, which deletes the folder when the test goes out of scope, whether it
passes or not. Verified: after the whole suite, `/tmp` has no `zeebx-*`.

### Automated tests

```text
cargo test --lib --locked
→ 442 passed, 0 failed, 9 ignored (with the desktop features)

cargo test --lib --locked --no-default-features
→ 384 passed, 0 failed, 5 ignored (engine only, which is what the core uses)

cargo test --lib --locked brew::vfs      → 14 passed
cargo test --lib --locked loader::archive → 6 passed
cargo test --lib --locked storage::      → 3 passed
```

### How to repeat it

```bash
CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=1 cargo build -p zeebx-libretro
cp target/debug/libzeebx_libretro.so  <cores folder>/zeebx_libretro.so
cp frontends/libretro/zeebx_libretro.info <cores folder>/
```

`DEBUG=0` is not a detail: with debuginfo the tree goes past 5 GiB and, on this machine, the disk
ran out in the middle of the build more than once.

## References and verified patterns

This section records external sources used for architecture decisions. Do not copy C/C++ code
without reviewing license, security and adaptation to Rust.

### Official documentation/API

- [Libretro Input API](https://docs.libretro.com/development/input-api/) — semantics of RetroPad,
  keyboard, mouse and pointer;
- [canonical libretro.h](https://github.com/libretro/libretro-common/blob/master/include/libretro.h)
  — ABI, `RETRO_DEVICE_SUBCLASS`, sensor, VFS, directories and lifecycle;
- [libretro-common 7z archive](https://github.com/libretro/libretro-common/blob/master/file/archive_file_7z.c)
  — proof that the official infrastructure handles 7z; it does not remove Zeebx's security limits.

### Official cores examined

| Core | Reference | Practice adopted |
|---|---|---|
| Flycast | [`core/libretro/libretro.cpp`](https://github.com/libretro/flycast/blob/master/core/libretro/libretro.cpp) | joystick subclasses, `SET_CONTROLLER_INFO`, keyboard callback, VFS requested early, device/descriptor refresh after a port change |
| Dolphin | [`DolphinLibretro/Input.cpp`](https://github.com/libretro/dolphin/blob/master/Source/Core/DolphinLibretro/Input.cpp) | per-port sensor: enable, accept only a success return, transform orientation, use per frame and disable on shutdown |
| Dolphin | [`Main.cpp`](https://github.com/libretro/dolphin/blob/master/Source/Core/DolphinLibretro/Main.cpp), [`Boot.cpp`](https://github.com/libretro/dolphin/blob/master/Source/Core/DolphinLibretro/Boot.cpp), [`VFile.cpp`](https://github.com/libretro/dolphin/blob/master/Source/Core/DolphinLibretro/Common/VFile.cpp) | `need_fullpath`+`block_extract`, user layout, negotiated VFS and explicit open/append/truncate modes |
| ScummVM | [`libretro-core.cpp`](https://github.com/libretro/scummvm/blob/master/backends/platform/libretro/src/libretro-core.cpp) | accept only supported input types/ports, keyboard by callback, VFS/authorized locations and save states declared absent |
| DOSBox Pure | [`dosbox_pure_libretro.cpp`](https://github.com/schellingb/dosbox-pure/blob/main/dosbox_pure_libretro.cpp) | explicit subclass decoding, separate keyboard/mouse/pointer, saves in the frontend's directory and fixed-size/validated serialization |
| PPSSPP | [`libretro.cpp`](https://github.com/libretro/ppsspp/blob/master/libretro/libretro.cpp) | split of system/save directory; its fallback to the content directory will **not** be copied, because Zeebx must not write next to the ROM |

Consequences built into this plan:

- the VFS stops being a later phase: it is an architecture requirement from the first load; full
  support for non-native paths also depends on adapting SQLite and all of the engine's I/O;
- a subtype is not a poll type; Zeebx polls the base device;
- the sensor only becomes a Boomerang source if `set_sensor_state()` returns success;
- the keyboard uses callback and poll, with a queue so as not to re-enter the guest;
- mouse/pointer only come in once there is a measured BREW device;
- 7z announced only after a safe parser and corpus tests.

## Build matrix

Six targets per artifact — the core and the standalone —, three systems × two architectures,
**each on its native runner**:

| | x86_64 | AArch64 |
|---|---|---|
| Linux | `ubuntu-24.04` | `ubuntu-24.04-arm` |
| Windows | `windows-latest` | `windows-11-arm` (experimental) |
| macOS | `macos-15-intel` | `macos-latest` (Apple Silicon) |

Nothing here is cross-compilation, and that is on purpose: `unicorn` compiles QEMU in C and
`dynarmic` compiles C++20, exactly the combination where cross-building requires a C++ toolchain
and sysroot. There is a native ARM64 runner for all three systems, and it comes out cheaper than
maintaining cross builds.

The core is published as `<target>/zeebx_libretro.{so,dll,dylib}` together with the `.info`, and
the standalone as the target's binary. Each artifact goes through two checks before upload:

1. **The library loads and the ABI responds** (`tools/verify_core.py`): opens the library with the
   `dlopen`/`LoadLibrary` behind `ctypes` — the way the frontend opens it —, checks the 25 symbols,
   requires `retro_api_version` equal to 1 and requires `retro_get_system_info` to fill in `Zeebx`,
   `mod|zip` and the two flags. Reading symbols with `nm` would not catch a library that exists
   and **does not load** because of a dependency missing from the link; loading it does.
2. **No interface dependency** (Linux): `ldd` refuses `libX11`, Wayland, EGL, ALSA, udev and GTK.
   Video, audio and input belong to the frontend.

### Windows ARM64 is experimental, and the cause is measured

The target exists in the matrix, runs, and does **not** bring the run down. It does not go green
because of a third-party limitation:

```text
FAILED: CMakeFiles/unicorn-common.dir/qemu/util/setjmp-wrapper-win32.asm.obj
  ml  -I...unicorn-engine-sys-2.1.5\msvc -I...
  CreateProcess failed: The system cannot find the file specified.
```

QEMU assembles that `.asm` with MSVC's `ml`, which **only exists for x86 and x64** — there is no
MASM assembler for ARM64. `unicorn-engine` 2.1.5 is the latest published version, and QEMU does not
support Windows ARM64 as a host. When that changes — a new unicorn version, or `unicorn` becoming
optional on this platform, with `dynarmic` alone —, it is enough to drop the experimental mark from
the target.

### Linux AArch64: the runner's `glibc` is not every handheld's

**Measured on 2026-09-23, comparing the CI artifact (`core-linux-aarch64`, `ubuntu-24.04-arm`)
against two real physical cards.**

`ubuntu-24.04-arm` builds natively, and "native" here means: the binary comes out bound to the
`glibc` the runner has — not to the `glibc` of whoever is going to run the core. That matters
because this project's real target is handheld devices with firmware that does not update itself,
and their `glibc` can be much older than that of a CI image that follows the latest Ubuntu.

| device / origin | `glibc` | does today's `core-linux-aarch64` core load? |
|---|---|---|
| CI `ubuntu-24.04-arm` (the builder) | requires up to **2.34** in what it produces | — |
| RG40XX-H / muOS 2508 (Buildroot) | 2.38 | yes, barely (2.38 ≥ 2.34) |
| original R36S / ArkOS AeUX (CFW `11072025`) | **2.30** | **no** — `GLIBC_2.34` missing |

The `GLIBC_2.34` requirement does not come from a new function. It comes from `pthread_create`,
`pthread_join`, `pthread_mutex_trylock`, `dlopen`, `dlsym` and others — old functions, but ones that
**changed symbol version** when `libpthread` was merged into `libc` itself, in `glibc` 2.34. Every
binary linked on a host with `glibc` ≥ 2.34 inherits that requirement for those functions, even
though they have existed since long before.

**The fix is where it is built, not the code.** `cargo zigbuild`, using `zig` as the *linker*,
lets you choose the minimum `glibc` explicitly — `aarch64-unknown-linux-gnu.2.28`, for example —
and the binary comes out bound to that version **regardless of the runner's `glibc`**. 2.28 is
below the R36S's 2.30, with a margin for even older devices. See the `core-linux-aarch64` job of
`libretro.yml` for the change.

### Android AArch64: the matrix's `linux-aarch64` is not the same target

**Found on 2026-09-23, while investigating whether CI was missing for the Libretro core on
Android.**

The build matrix has `linux-aarch64` (`aarch64-unknown-linux-gnu`, `glibc`) and the `android.yml`
workflow builds `frontends/android` (`aarch64-linux-android`, Bionic) — but neither of them is "the
Libretro core for Android". They are **different** C runtime targets: Bionic is not `glibc`, the
*dynamic linker* is another one (`/system/bin/linker64`), and a `linux-aarch64` `.so` does not get
past `dlopen` in a real Android RetroArch.

`libretro.yml` has no Android target in the matrix — only `linux-x86_64`, `linux-aarch64`,
`windows-x86_64/aarch64`, `macos-x86_64/arm64`. Whoever wants the Zeebx core **inside RetroArch on
Android** (phone, Retroid, tablet, or an Android handheld) has no CI artifact at all today.
`android.yml` solves that only for the Zeebx standalone app, which is another frontend
(`frontends/android`), not the Libretro core (`frontends/libretro`).

**An `android-aarch64` target is missing from `libretro.yml`**, building only `-p zeebx-libretro
--target aarch64-linux-android` (no APK, the raw `.so`), reusing the same NDK infrastructure
`android.yml` already uses — same pinned NDK version, same `cargo-ndk`. Tested by hand on this
machine (NDK r27c): the build works, but it requires a workaround — the `cmake` of
`unicorn`/`dynarmic` does not find `clang-scan-deps` by its plain name, and it is solved by
creating `<architecture>-clang-scan-deps` symlinks pointing to the NDK's `clang-scan-deps`, for the
four Android architectures. The `android.yml` run in CI (`ubuntu-22.04`, NDK via `sdkmanager`)
did **not** hit this problem — the workaround may be specific to this manual NDK install, and it is
worth confirming before porting it to the new job.

### Field note: original R36S card investigated (2026-09-23)

CFW `ArkOS AeUX` (`CFW_VERSION=11072025`), Ubuntu 19.10 base, Cortex-A35, 1 GB RAM, `glibc 2.30`.
RetroArch 1.21.0, dual install (`retroarch` AArch64 + `retroarch32` armhf), 148 AArch64 cores
installed. **Zeebo is not integrated**: there are ready-made theme icons (Ozone, XMB) for a "Zeebo -
Zeebo" system, but there is **no** `<system>` entry for `zeebo` in `es_systems.cfg` (126 systems
registered, none of them Zeebo) nor an installed core. Context: the card will be replaced by
another CFW soon, so the full integration (ES entry, core, RDB) waits until the device has its
final CFW — but the `glibc` finding above holds regardless of the specific CFW.

### Audit of the ES integration (2026-09-23): what works, and a real risk

After inserting `<system>zeebo</system>` into `es_systems.cfg` (same pattern as a simple
single-core system, e.g. `advision`), copying a test ROM to `/roms/zeebo/` and pointing the
`<command>` at `~/.config/retroarch/cores/zeebx_libretro.so`, the audit found:

**No risk, checked:**
- passwordless `sudo` for the `ark` user (`ark ALL=NOPASSWD: ALL`, read with the host password) —
  the `sudo perfmax ...; sudo perfnorm` pattern every `<command>` uses works the same as for the
  other 126 systems already on the card.
- The `<system>zeebo</system>` block is well-formed XML on its own.
- `HiddenSystems` (`alg;wolf;easyrpg`) does not include `zeebo` — it shows up in the menu normally.
- The "broken XML" a strict validation flags (`2>&1` without escaping `&`, in the already existing
  `options` entry) is an **original ArkOS defect**, not from this change — confirmed by an identical
  md5 against a never-touched backup (`es_systems.cfg.rk3326`). Their EmulationStation uses a
  tolerant parser, and that is why it works in practice.

**Real risk, not fixed — it belongs to the system, not to Zeebx:** there are three dated backups
(`es_systems.cfg.update04302025.bak`, `.update05152025.bak`, `.update06302025.bak`) and an
`es_systems.cfg.rk3326` **byte-for-byte identical** (same md5) to `/usr/local/bin/es_systems.cfg`.
That proves ArkOS has an update mechanism that **replaces the whole file** with a per-device-model
template. Running `Update.sh` (or any update) before switching CFW deletes the `zeebo` entry — the
file goes back to the factory version. Since the card will be replaced by `dArkOSen` anyway, the
practical risk is low, but it is on record: **do not update the ArkOS system on this card before
the reinstall**, or the integration has to be redone.

### Final audit of the core installation (2026-09-23)

After installing `zeebx_libretro.so` (`glibc 2.28`) and `zeebx_libretro.info` in
`~/.config/retroarch/cores/` (owner `ark:ark`, permissions `755`/`644`), the whole launch path was
checked, from the ES `<command>` down to the real binary:

1. `/usr/local/bin/retroarch` **is not the binary** — it is a script that resolves
   `emulator=$(basename "$0")` (it becomes `"retroarch"`, because that is how the `<command>`
   calls it) and ends by invoking
   `/opt/retroarch/bin/retroarch -c /home/ark/.config/retroarch/retroarch.cfg "$@"`. The final
   command that zeebo fires is:
   ```
   /opt/retroarch/bin/retroarch -c /home/ark/.config/retroarch/retroarch.cfg \
     -L /home/ark/.config/retroarch/cores/zeebx_libretro.so <rom>
   ```
2. The same script extracts the core name by regex (`cores/(.*)_libretro.so`) to check a netplay
   blocklist — `zeebx` is not on the list, it goes straight through.
3. `nice -n -19` (high priority) runs **without** `sudo` in front, but it is the same pattern as
   the other 126 already working systems: whatever allows it for them allows it for zeebo, no new
   risk.
4. The `.info` follows the `libretro-super` form (checked in the file's own comment, against the
   installed `desmume_libretro.info`) — no field incompatibility.

Nothing broken in that chain. The only verification gap is the lack of a dynamic test (real
`dlopen` through QEMU), because of the tight disk at the time — see the recipe section, above.

### Update (2026-09-23): the R36S card is going to switch CFW, and that changes the priority

**ArkOS is discontinued.** The `AeolusUX/ArkOS-R3XS` repository itself (the one on the card, build
`11072025`) says: *"dArkOS has replaced ArkOS. ArkOS will no longer be maintained effective
immediately."* The user is going to install `djparentx/dArkOSen-R36S` in its place.

**`dArkOSen` is `dArkOS` — and `dArkOS` is Debian, not Ubuntu 19.10.** dArkOSen's own README says
`Built from dArkOS_RG351MP_trixie_07262026` — **Debian 13 "trixie"**, the same distro as this
project's host. The `dArkOS` README confirms: *"this OS is based on the latest stable version of
Debian"*, with builds via `chroot` (`make <device_name>`) and 64- and 32-bit userspace support
(`BUILD_ARMHF`), like the old ArkOS.

That changes the conclusion of the previous section: **the `GLIBC_2.34` problem goes away as soon
as the card switches CFW.** The host's Debian trixie already showed `glibc` up to **2.41**
available (`objdump -T /usr/aarch64-linux-gnu/lib/libc.so.6`); a real `dArkOSen` on the same base
covers CI's 2.34 with room to spare. **There is no longer a reason to chase the build with an old
`glibc` (`cargo zigbuild` + cross `g++`) for this specific device** — the effort was paused after
running into a PCH/C++20 modules problem in `dynarmic` with a hand-assembled cross `g++`, and it is
not worth solving that for a target that is about to disappear.

| CFW | base | measured/estimated `glibc` | does today's `core-linux-aarch64` core load? |
|---|---|---|---|
| ArkOS AeUX `11072025` (on the card, will be replaced) | Ubuntu 19.10 | 2.30 | no |
| **dArkOSen** (`dArkOS` Debian trixie, next CFW) | Debian 13 | ~2.40–2.41 | **yes, with room to spare** |
| RG40XX-H / muOS 2508 | Buildroot | 2.38 | yes, barely |

**Update (same session, later): the build with an old `glibc` was completed successfully.** See
the section below, "Working recipe: `cargo zigbuild` with a pinned minimum `glibc`". The core is
already installed on this card, with `glibc 2.28` — below the R36S's 2.30, with room to spare.

## Working recipe: `cargo zigbuild` with a pinned minimum `glibc`

**Measured on 2026-09-23.** After pausing the effort (see above), the user asked to keep at it.
The final recipe that worked is simpler than the path to get there: **it needed neither the cross
`g++` nor any compiler patch** — only `cargo-zigbuild` and two tweaks to `dynarmic`'s vendored
`build.rs`, applied through a **local** `[patch.crates-io]`, without touching the project's
`Cargo.toml`.

```bash
# zig via pip (ziglang), with a one-line wrapper to expose `zig` on the PATH
uv pip install ziglang
printf '#!/bin/bash\nexec <venv>/bin/python-zig "$@"\n' > /tmp/zigbin/zig && chmod +x /tmp/zigbin/zig

cargo install cargo-zigbuild --locked
rustup target add aarch64-unknown-linux-gnu

# local copy of dynarmic (crates.io), with two extra defines in build.rs:
#   .define("DYNARMIC_USE_PRECOMPILED_HEADERS", "OFF")
#   .define("FMT_MODULE", "OFF")
# and the three vendored libs linked as explicit static libs:
#   cargo:rustc-link-lib=static=dynarmic / static=fmt / static=mcl

PATH=/tmp/zigbin:$PATH cargo zigbuild --release -p zeebx-libretro \
  --target aarch64-unknown-linux-gnu.2.28 \
  --config 'patch.crates-io.dynarmic.path="/tmp/dynarmic-patched"'
```

**The two problems the recipe works around, and why:**

1. **`DYNARMIC_USE_PRECOMPILED_HEADERS` (default `ON`) and `FMT_MODULE` (auto-detected `ON` with
   CMake+Ninja+a recent GCC)** make CMake try to scan C++20 module dependencies
   (`-x c++-module`) to build a precompiled header. `zig c++` does not recognize that invocation.
   Neither option is needed for `dynarmic` to work — they are build optimizations, not a feature
   in use.
2. **`cargo:rustc-link-lib=dynarmic/fmt/mcl` without the `static=` prefix** makes `zig`'s linker
   (which, unlike the normal `ld`, does not fall back to `.a` when it does not find a `.so`)
   refuse the link — the three only exist as static libraries, because that is how `cmake-rs`
   builds them by default when they are embedded in a Rust crate.

**Result, checked with `objdump`/`readelf`:**

| check | result |
|---|---|
| maximum `GLIBC` required | **2.28** (below the R36S's 2.30) |
| libraries depended on | `libc`, `libm`, `libpthread`, `libdl` — the pre-2.34-merge layout, consistent with the target |
| `libstdc++` | linked **statically** (`-static-libstdc++ -static-libgcc`), zero dependency on the system's version |
| `retro_*` symbols | the 25 ABI exports, all present |
| *features* of the `zeebx` crate | `default-features = false, features = ["gl", "soundfont"]` — **pinned in the `Cargo.toml` of `frontends/libretro`**, the same CI uses; does not depend on a command-line flag |

**What was left unverified, and why:** there was no real load test (`dlopen`) of the aarch64 binary
on this x86_64 host — it would require user-mode QEMU, and the disk had 1.3 GB free at the time,
too tight to risk it. The definitive proof waits for booting on the real device.

**Generalization:** this recipe solves the same problem for any `linux-aarch64` target whose real
`glibc` is older than the CI runner's — it is not specific to the R36S. Porting it to
`libretro.yml` means swapping `cargo build` for `cargo zigbuild --target
aarch64-unknown-linux-gnu.<version>` in the `core-linux-aarch64` job, with the `dynarmic`
`[patch]` applied through the repository's `.cargo/config.toml` (no longer through `--config` on
the command line).

## What is ready and does not need redoing

The `<system>zeebo</system>` entry in `es_systems.cfg`, the `/roms/zeebo/` folder with a test ROM,
and now **the core itself installed and working** (`glibc 2.28`) at
`~/.config/retroarch/cores/zeebx_libretro.so` — all of that survives the CFW switch, because
`dArkOSen` is a light fork of `dArkOS` and keeps the same EmulationStation + RetroArch structure.

## Status of the items, with the proof for each

| Item | Status | Proof |
|---|---|---|
| Scan of the 62 ROMs | done | 56 run (the doc's baseline: 50), 0 states got worse — rechecked after all of this session's changes. The 6 outside changed names: **Zuma left** (it started running) and **Prey Evil entered** the count, because before it showed up as "runs" with a black screen |
| CI for the standalone's six targets | **done** | `ci 77d5b02`: linux x86_64 and AArch64, macOS Intel and Apple Silicon, Windows x86_64 and ARM64 — **all six green**, and the core too |
| `IFont` and the `DrawText` layout | done | metrics transcribed from `AEEFontsStandard.BID`, with a test that checks the eleven classes |
| Audio and performance | done | the scan measures peak/rms/continuous/jump per game; Rolima 79% → 284%, 51 games faster; and **through the core path**: Peggle delivers 229,080 stereo samples in 120 frames |

### The scan caught a "regression" that was a truth

After the six extensions and the joystick, a check scan showed **one more game outside "runs"**: 7
instead of 6. The game was **Prey Evil**, and the easy reading would be "the extensions broke
something" — they did not.

It was **equally broken in both cases**. Before, the report said `runs`, because the game executed,
answered the API and presented frames: what it did not do was draw, and the scan back then did not
look at the screen. Now it says `crashed in the frame loop`, which is what it always was — and the
six extensions are what took it to the point where the crash happens, instead of stopping at the
first closed gate.

The other six games outside "runs" are exactly the same as before, with no state change in any of
them. The check was worth it for two reasons: it confirmed that today's changes did not touch any
game, and it showed that **the colors metric swapped a false label for a true one** — which is its
job.

### Zuma: the decoder only knew PNG

The `AEECLSID_JPEGDecoderBREW` class went into the factory (one line), and the game **moved past
one gate** — the list of missing classes became empty and the failure moved from `0x1a6b8` to
`0x1a72c`. The trace, turned on right after, delivered the cause of the second one:

```text
IImageDecoder::QueryInterface  → 0    (SUCCESS)
IForceFeed::Reset / Write      → 0    (the game fed the decoder, with 16 KB of data)
IImageDecoder::GetBitmap       → 0x1  ← FAILED, and the game carries on with a null bitmap
```

And the reason is in `machine/image.rs`: `decoded_bitmap` calls **`decode_png`** and, when the bytes
are not PNG, records the hypothesis "a decoder received data that is not a PNG" and returns 0. The
game asked for the **JPEG** decoder and fed it a JPEG.

**The fix is short and the dispatcher already exists**: `video::icon::decode(&bytes)` picks the
format **by signature** — PNG, BMP or JPEG — and it is the same one the emulator uses for the
modules' icons. What is missing is wiring that path to the guest's decoder, converting the image to
the structure `decoded_bitmap` publishes (the `DecodedImage`, with the 24- or 32-bit DIB that
`publish_png_dib` already knows how to build). It is the next piece, and it is the same recipe that
closed seven gates today: measure, narrow the list, fix what is missing — and the scan says whether
it moved.

### Peteca's 18 pops were not from the mixer

Item 4 asks for a noise review, and the audio measurement flagged 18 jumps above half travel in six
seconds of Peteca, the largest at 0.947. There was a clear suspect in the code: the `Voice`
**vanished all at once** when the sound ended — the level could be at 0.947 and the next sample was
zero. The `Stream` path, in the same file, already avoided that ("holds the last value instead of
popping to zero"); the voice did not.

Implemented the fade-out over [`FADE_OUT_FRAMES`] frames (1.5 ms), with a test that checks the
monotonic ramp. And the measurement, afterwards:

```text
before: largest jump 0.947 · 18 above 0.50
after:  largest jump 0.947 · 18 above 0.50     ← identical
```

**The jumps are the attacks of the sound effects**, which start far from zero in the game's own
data — and a legitimate percussive attack is indistinguishable from a pop by the metric, which only
sees the mixed output. The honest conclusion of item 4 is twofold: **the mixer was not the cause**
of these 18, and the voice cut that was a possible cause no longer exists.

### The screen the scan did not look at

The scan counted **writes** to the screen, and a game that paints 307,200 pixels black counts
307,200 writes: it recorded "runs" with the screen off. Now the report counts **distinct colors and
the dominant one**, and the line goes into the comparison with the baseline — a regression that
blanks the screen without breaking execution now shows up in the commit that caused it.

**The whole scan matches the committed baseline, and keeps matching.** After all of the day's
changes — the six extensions, the joystick, the JPEG decoder class, dispatch by signature and the
voice fade-out in the mixer —, with the 62 ROMs run again:

```text
62 reports · 0 differences · 0 baselines written
```

No game changed its summary without the change being intentional and recorded, and the tree came
out clean.

**The whole scan matches the committed baseline.** With the 62 ROMs run after all of the day's
changes — the six extensions, the joystick, the JPEG decoder class and dispatch by signature —, the
result was **zero differences** and **zero baselines written**: no game changed its summary without
the change being intentional and recorded (Zuma, which started running), and no other one moved.
That is what item 1 of the plan asks for, and now it is automatic: the `docs/scan/` folder is in
the repository with the 62 lines.

**The count goes into the baseline, so it had to be stable** — a metric that varies between runs
would flag a false regression on every scan, and a test that always flags is never read. Measured
with Pac-Mania, three runs in a row:

```text
run 1: screen: 695 color(s), dominant 0x0004
run 2: screen: 695 color(s), dominant 0x0004
run 3: screen: 695 color(s), dominant 0x0004
```

That is what is expected — the final frame is a function of **virtual** time, which the scan
controls — and it is what lets the baseline check the whole line, not just the state.

In the first scan with the metric, two black screens among those that "run", and giving more
virtual time separated the two possible cases:

| Game | 6 s | 20 s | Reading |
|---|---:|---:|---|
| Zeebo F.C. Foot Camp | 1 color | **2410 colors** | it was loading: six seconds is too little |
| Zeebo F.C. Super League | 6 colors | **4133 colors** | same |
| **Prey Evil** | 1 color | **1 color** | **draws nothing, and runs at 3287%** |

The first two are calibration: **six seconds is not enough for a game that loads before drawing**,
and the number of colors is what gives that away without anyone looking at the screen. The third
is a defect: the game executes, answers the API and does not put a pixel on the screen — and nobody
knows for how many sessions it went unnoticed. **Investigated in the same session, and the cause is
identified.** Prey Evil does not call any drawing at all: the report lists eleven GL methods and
**no** `DrawArrays`, `DrawElements`, `glClear` or `eglSwapBuffers`. It prepares matrices and
textures, and stops — and the black screen is the consequence: there is nothing to show.

The reason is in the list of classes it asks for and we do not have, and **all six are GL/EGL
extensions**:

| Class | Name | Header |
|---|---|---|
| `0x0103d8de` | `AEEIID_GLES10EXT` | `AEEGLES10Ext.h` |
| `0x0103d8eb` | `AEEIID_GLES11EXT` | `AEEGLES11Ext.h` |
| `0x0103def1` | `AEEIID_GLES11EXTPAK` | `AEEGLES11ExtPak.h` |
| `0x0103d8ef` | `AEEIID_EGLGETCOLORBUFFER` | `AEEEGLGetColorBuffer.h` |
| `0x0103d8f0` | `AEEIID_EGLGETPOWERLEVEL` | `AEEEGLGetPowerLevel.h` |
| `0x010426e3` | `AEEIID_EGLOESSWAPINTERVAL` | `AEEEGLOESSWAPInterval.h` |

The game partitions its drawing path by what exists: without the extensions, it does not draw.

**And the fix is smaller than it looks.** The engine **already implements** those functions —
`eglGetColorBufferQUALCOMM`, `SwapIntervalOES` and the other `EGL_QUALCOMM` ones live in
`machine/egl.rs` —, but exposes them through `eglGetProcAddress`. The game asks for them through
`CreateInstance`, with the **ClassID**, and gets null. What is missing is registering the six
ClassIDs as interfaces, with the slots in the order of the headers above — the same path `IFont`
and `IGraphics` already took, and with the same check: after registering, **Prey Evil's color
count goes above one**.

The `IGLES11Ext` table is already read, and it is **15 slots**: the three of `IQueryInterface`
(`AddRef`, `Release`, `QueryInterface`) and, in the order of `AEEGLES11Ext.h`,
`CurrentPaletteMatrixOES`, `LoadPaletteFromModelViewMatrixOES`, `MatrixIndexPointerOES`,
`WeightPointerOES`, `DrawTexsOES`, `DrawTexiOES`, `DrawTexxOES`, `DrawTexsvOES`, `DrawTexivOES`,
`DrawTexxvOES`, `DrawTexfOES`, `DrawTexfvOES`. The `DrawTex*` ones are what matters to a game that
builds the frame in a texture — which is the case of Prey Evil, with 16,746 `BindTexture` calls and
no drawing.

**Done, and measured — the mechanism is confirmed.** `IGLES11Ext` was registered in the factory
(`shell_create_instance`) and in the `QueryInterface` **of the EGL object**, which is where the
game asks (the first attempt, only in the factory, changed nothing, and it was the report that said
so: the class was still on the list). After the registration:

```text
before: missing classes: 0x0103d8de 0x0103d8eb 0x0103d8ef 0x0103d8f0 0x0103def1 0x010426e3
        eleven GL methods, no drawing, single-color screen, "runs" at 3287%

after:  0x0103d8eb **left the list**
        the game starts doing sprintf/strlen/strncmp/malloc in volume — another phase
        and crashes at the **next** gate: invalid access to 0x0, from 0x161d8
```

In other words: **delivering one interface moves the game one gate forward**, and what it calls
next is the null pointer of one of the other five.

And that is what happened, one by one, each one measured:

| Gate | Interface | Slots | After it |
|---|---|---:|---|
| 1 | `IGLES11Ext` | 15 | leaves the list; the game changes phase (`sprintf`/`malloc` in volume) |
| 2 | `IGLES10Ext` | 4 | leaves the list |
| 3 | `IEGLGetPowerLevel` | 4 | leaves the list |
| 4 | `IEGLOESSwapInterval` | 5 | leaves the list |
| 5 | `IEGLGetColorBuffer` | 4 | leaves the list — **one left** |

The check is always the same, and it is what makes this a procedure and not a bet: the class
leaves the "requested and we do not have" list, and the game crashes somewhere else — at the same
PC while it is the same gate, and at a new PC when the gate changes.

| 6 | `IGLES11ExtPak` | 30 | **the list of missing classes became empty** |

All six were delivered, and Prey Evil's report **no longer has any missing class** — the per-class
survey, which is what says "the game asked for something we do not have", is clean.

**And what is left is already identified.** The game still crashes at the same point — `0x161d8`
calling `0x0` —, and now it is not a missing class: it is a null pointer it stored earlier. What
gave the clue was the **game's own log**, in the last lines before the failure:

```text
*dbgprintf-4* ..\..\..\common\sharedgl\gamepadmgr.cpp:277
Creating USB Joystick interface
*dbgprintf-4* ..\..\..\common\sharedgl\gamepadmgr.cpp:319
1 Joysticks connected
```

Qualcomm's joystick manager asks `CreateInstance` for the `IJoystick` interface, gets null, stores
it — and the first `Read` falls into the void. And the interface is **small**, with the slots in
`sdk/inc/AEEJoystick.h`:

```c
AEECLSID_IJOYSTICK1 = 0x01021c2b        // and IJOYSTICK2 = 0x01021dac
  INHERIT_IQueryInterface(IJoystick);
  int (*SetParm)(IJoystick *po, int16 nParmID, int32 p1, int32 p2);
  int (*GetParm)(IJoystick *po, int16 nParmID, int32 *pP1);
  int (*Read)(IJoystick *po, int16 *px, int16 *py);
```

Six slots, and `Read` has a **real** implementation available: it is the same Z-Pad state that
`IHIDDevice::GetPositionState` already delivers.

**Implemented — and this was not the failure.** `IJoystick` is in the engine, with the six slots
and `Read` delivering the Z-Pad axis in the console's range, and Prey Evil crashes **at the same
point**, with the same log. What this teaches is on record: the log said "1 Joysticks connected",
and that is not proof that the interface was missing — the manager says that **after** enumerating
the HID, which we serve. The clue was two lines away and I read the wrong one.

**The current clue, this one for real, came from the report itself:**

```text
open failed: <cache>/…/mod/276154/udata/save.dat (No such file or directory)
OpenFile "udata/save.dat"  (0x1002bcb4 0x1 0xf0003008) -> 0
```

The game opens the **save** with mode `OFM_READ` (1) on a first run, when the file does not exist
yet, gets zero and carries on. It is the same pattern as Double Dragon, one step further: there it
was `OFM_CREATE` that failed for lack of a directory; here the file does not exist because it is
the first save, and the question is what the console returns in that case — and what the game does
with the answer. **Measured, and our behavior is right.** `AEEFile.h` says what the console does:

```text
_OFM_READWRITE and _OFM_APPEND will not create a file.
_OFM_CREATE will only create a file if it did not exist prior to the IFILEMGR_OpenFile call.
```

In other words: the console **also** returns null for an `OFM_READ` on a file that does not exist.
It is not our defect — it is the game carrying on with the empty pointer. The question left is
**where** it uses that null, and for that the report was not enough: it is the trace.

**The scan gained `ZEEBX_ROM_TRACE`** — `1` keeps the last calls, or a text filter
(`ZEEBX_ROM_TRACE=IFile`). Without this option the trace existed in the engine and nobody turned it
on in a scan, which is where the investigation happens.

**Both work, and the filter is the most useful.** I had noted here that the per-family filter gave
no result — **and the mistake was mine, in the reading**: I used `sed` from the header to the end of
the file and a `tail -3`, which caught the end of the **log**, not of the trace. With `awk`
delimiting the right section, the filter delivers exactly what you want.

And what it delivered was the **final sequence before the crash**, filtered by `IHID`:

```text
IHIDDevice::RegisterForButtonEvent     (r1=0x30000650)
IHIDDevice::RegisterForPositionChange  (r1=0x300006d0)
IHIDDevice::GetMinPositionInfo         (r1=0x1002bad8)
IHIDDevice::GetMaxPositionInfo         (r1=0x1002bb3c)
IHIDDevice::GetAxesInfo                (r1=0x1002bba0)
IHIDDevice::GetPositionState           (r1=0x200fff3c)
```

It is the controller calibration, done by the game's joystick manager, and **all of them answer
SUCCESS with the structures filled in** (`write_axis_range`, `write_position_info` with
`POSITION_INFO_WORDS` and `AXIS_SLOTS`). The failure, then, is in what the game does **after** that
— the table of 28-byte entries the unfiltered trace showed. This is where the next session picks
up: those six calls are the game's last contact with the emulator before it crashes.

**And the scan gained the instrument that was missing to look at the table.**
`ZEEBX_ROM_DUMP=0xADDR:BYTES` dumps memory into the report, in hexadecimal, with the address of
each line:

```bash
ZEEBX_ROM=… ZEEBX_ROM_DUMP=0x10038600:128 cargo test --release scan -- --nocapture
```

The address changes from run to run (it is guest heap), so the dump is read **at the end**, with
the game already stopped — that is when the table is ready.

Read, it shows what the trace promised: **regular 28-byte** records, and an `ff ff` marker that
shows up now and then, in the middle of the sequence:

```text
0x10038600  21 0f 18 fe ff 00 0f 28 0f 28 00 58 0f 28 00 27
0x10038660  0f 78 00 39 0f 18 0f 78 ff ff 00 27 0f 78 00 39
```

The bytes do not look like a function pointer (no zero, no module address), so the null the game
calls **does not come straight from here**.

**It comes from a field at `+0x274` of an object, and that is decoded.** The memory dump also
serves to read **code**: with `ZEEBX_ROM_DUMP=0x161b0:64` out comes the instruction that crashes,
and the four words around it can be read by hand:

```text
0x161c8   LDR r0, [r6, #8]        ; the object
0x161cc   LDR r1, [r0, #0x274]    ; the function pointer in field +0x274  ← came back null
0x161d0   MOV r0, sp
0x161d4   BLX r1                  ; ← the call that crashes
0x161d8   (return)
```

The chain is short and points to a single place: **the object at `[r6+8]` has field `+0x274`
null**.

**Measured, and the null is there.** With `r6 = 0x1000110c` at the moment of the failure, the
object is `0x10001114`, and the dump shows both ends:

```text
0x10001114   d8 19 00 10  …      → vtable 0x100019d8, **in the module**: it is the game's own structure
0x10001388   00 00 … (32 bytes)  → field +0x274, all zero: this is what the BLX calls
```

**And crossing it with the trace says which field it is.** The last thing the game did before
crashing was the controller calibration, including `RegisterForPositionChange` — the registration
of the position callback. Field `+0x274` is that callback, and it never got written.

**The reentrancy hypothesis was raised and knocked down in the same session — and it is worth
recording why.** The idea was: `RegisterForPositionChange` raises the signal during the
registration call itself, and the callback could run before the game stored the pointer. Reading
the scheduling refutes it: `raise_input_signal` only **queues** (`pending_signals`), and the queue
is drained at the start of the **next loop** (`pump_pcm_streams` and company, in
`machine/signal.rs`), after the API call has returned completely. There is no reentrancy, and the
early notice — which exists because it makes Ridge Racer calibrate without anyone touching the
analog stick — is not to blame.

So field `+0x274` stays null **because of the game's own logic**, or it belongs to an object other
than the one the dump caught. What is left is reverse engineering of the game, with five measured
starting points: the PC (`0x161d8`), the stack (`0x43494 0x4f300 0x3af54 0x43d5c`), the object
(`0x10001114`, vtable `0x100019d8`), the field (`0x10001388`) and the 28-byte table
(`0x10038600`).

And what it showed was the exact pattern before the crash: a **table being built**, 28-byte
entries (`malloc 0x1c` followed by `memmove 0x1c`), in a loop, with the sources 28 bytes apart
(`0x1003869a`, `0x100386b6`, `0x100386d2`, …) — and **no API call after that**. The failure is in
the game's own code, calling a pointer it built (or read from a table), not a pointer of ours.

That is where this session stopped: what is left is reverse engineering of the game, with the
trace in hand and the PC (`0x161d8`), the stack (`0x43494 0x4f300 0x3af54 0x43d5c`) and the table
sources (`0x1003869a`) as the starting point.

`IGLES11ExtPak` answers with the same treatment as the other graphics extensions — "success" for
what does not change the drawing, identifier `1` for the `Gen*OES` calls (there is only one target
here) and `GL_FRAMEBUFFER_COMPLETE` (0x8CD5) for the completeness query.

**And the class number already shows up in the engine through another path.** In BREW, the ClassID
and the IID are the same value: `AEEIID_GLES_IMAGEON_EXT` (`0x01058546`) is already served in
`machine/egl.rs` through the **function** route (`eglGetProcAddress`), and Prey Evil asks for its
extensions through the **object** route (`ISHELL_CreateInstance`). What is missing is the
registration in the factory, in `shell_create_instance`, with the slot table above — and the
function-route handler serves as a reference for what each slot answers.

### Two silent games, and what silenced them

The scan report lists the sounds the decoder refused. Two games showed up there, and the audio
measurement — peak, rms, continuous and jump between samples — showed **peak 0.000 in both**:
absolute silence, not "no sound for now".

Investigating started by making the refusal **say the format and the first bytes**. Two lines
solved both cases:

```text
Turma da Mônica   rejected (unknown format, 86 sounds, 4f 67 67 53 ...)   ← "OggS"
Peggle            rejected (audio/mpeg, 7 sounds, 49 44 33 03 00 00 00 00 ...)   ← "ID3"
```

- **Ogg/Vorbis**: `symphonia` already knew how to decode it; the feature just had to be turned on.
  86 sounds became music — peak 0.586 at thirty virtual seconds, against 0.000 before.
- **MP3 with an ID3 tag that lies**: Peggle delivers a tag that says its size is **zero bytes**
  and thirty thousand of content. Whoever skips by the declared field looks for the frame in the
  wrong place and refuses the whole file. Now the search is by **frame sync** (`0xFF` and three
  high bits), which is what the format guarantees — seven tracks came back: peak 0.356 against
  0.000.

The lesson holds for next time: a refusal that does not say **what** arrived costs a whole
investigation inside the game. By saying the format and the bytes, it was two lines of diagnosis
and twenty of fix.

In the whole corpus, after the two fixes: **zero sounds refused** (there were two) and sixteen
games with sound measured in the scan's six seconds. Turma da Mônica only enters that count after
thirty virtual seconds — the short scan would leave it out, and that is why its number was measured
separately.

### Where the time goes, measured

With `ZEEBX_ROM_PROFILE=1`, the scan records the real cost per API method. In the two heaviest
games of the collection:

| Game | Time in API calls | What dominates |
|---|---:|---|
| Crash Nitro Kart 3D | 462 ms | `eglSwapBuffers` 415 ms = **89.8%** |
| Zeebo Extreme Rolima | 958 ms | `memset` 520 ms, `eglSwapBuffers` 257 ms |

Crash's 415 ms are **3.5 ms per presented frame** (118 frames), and inside that is the software
rasterizer drawing the scene — swapping the buffer is the same as drawing. Even so the game runs at
**1039%** of console speed: the rasterizer is the largest single cost and **is not a bottleneck**
for what exists today. What hardware rendering changes is **fidelity** (GL state, compressed
textures, blend), not the speed of this collection.

Rolima's `memset` cost 731 ms because `helpers` **allocated a `Vec` of the requested size on every
call**. With block-wise clearing from a stack buffer: 520 ms, and the game went from 247% to 267%.
| Hardware rendering (`SET_HW_RENDER`) | **wired in, proven as far as possible without a frontend** | rasterizer verified against the software one; the engine draws into the frontend's framebuffer; the `gl`/`gpu` features split (0 host dependencies, measured in CI on the 6 targets); the core asks for the context, builds the `glow::Context`, draws into the FBO and falls back to software if it fails. The drawing itself depends on RetroArch |

### The GPU rasterizer draws the same frame, measured

Before turning on `SET_HW_RENDER` in the core, we needed to know whether the GPU path draws **the
same image** as the software one — otherwise the switch would mean leaving a measured path for one
nobody had looked at. The test `both_rasterizers_draw_the_same_frame` runs the same content for the
same virtual time on both and compares pixel by pixel, **without a window** (the offscreen context
exists precisely for that: measuring).

```bash
ZEEBX_TEST_ROM="roms/game.zip" ZEEBX_TEST_MS=3000   cargo test --features gpu both_rasterizers -- --nocapture
```

| Game | Differing pixels | Mean difference | Worst pixel |
|---|---:|---:|---:|
| Double Dragon | **0.00%** | 0.000 | 0 |
| Crash Nitro Kart 3D | **0.00%** | 0.000 | 0 |
| Zeebo Sports Peteca | 2.62% | 0.034 | 3 of 63 |

Two games come out **identical**, and the third differs only in half-level rounding — what the
comparison checks is that they draw the same image, not that they are bit-for-bit equal. With this
math done, what is left in item 5 is the plumbing: negotiating the context with the frontend through
`RETRO_ENVIRONMENT_SET_HW_RENDER` and handing over the `glow::Context` the engine already accepts.

While the plumbing does not exist, you can **see** the two rasterizers side by side from the
command line, with no interface — the same two commands, changing only the GPU flag:

```bash
cargo run --release -p zeebx-cli -- session "roms/Crash.zip" --seconds=3 --dump=software.bmp
cargo run --release -p zeebx-cli -- session "roms/Crash.zip" --seconds=3 --gpu --dump=gpu.bmp
```

The two `.bmp` files are the same virtual instant of the same game, one drawn on the processor and
the other on the GPU. It is the check anyone can do without writing code, and it is what the test
above automated.

### What has already gone through the core path

The test `the_core_abi_runs_a_rom` exercises the core the way RetroArch exercises it —
`retro_init`, `retro_load_game`, frames, `retro_unload_game` —, and counts what reaches the
frontend. Measured today:

| Content | Frames | Stereo samples | Games alongside |
|---|---:|---:|---:|
| Crash Nitro Kart 3D (`.zip`) | 360 | 267,668 | 63 |
| Double Dragon (`.zip`) | 360 | 268,729 | 63 |
| Double Dragon (`.7z`) | 300 | 224,669 | — |
| Peggle (`.zip`) | 300 | 229,080 | — |
| Z-Wheel (`.zip`) | 300 | 220,300 | 63 |

```bash
ZEEBX_CORE_ROM="roms/Crash.zip" ZEEBX_CORE_FRAMES=180 \
  cargo test -p zeebx-libretro -- --nocapture
```

Each line is something that stopped being an assumption: 3D delivers frames and sound through the
core, `.7z` opens through the core, and the Z-Wheel sees the 63 games through the core. What is
**not** in this table is drawing on the GPU: for that a real frontend is needed, and that is what
is missing in item 5.

### The GL gap the survey found

The scan report has a **"GL served without doing anything"** section: calls the rasterizer accepts
successfully and ignores. Across the 62 ROMs it pointed to **only one** call, in six games —
`glPixelStorei`. It is not noise: GL aligns each texture row to a multiple of the requested
alignment (4 by default), and what is left over is padding. Ignoring the call, a texture whose width
**in bytes** is not a multiple of four arrives with shifted rows — the image comes out scrambled
diagonally, without a single line of warning.

Implemented: the alignment lives in `Machine` (it is a property of guest memory, not of the
rasterizer), reading the texels packs the rows in a single access, and `round_up_to` has a test.
Checked afterwards: Double Dragon and Galaxy on Fire **stop showing up in the section**, and keep
running. It is the kind of defect hardware rendering would solve for free — and that here cost
twenty lines, without swapping rasterizers.
| Save states | **missing** | the core **declares** it has none (`size` 0, `serialize` false), which is the acceptance criterion |
| `.7z` | done | `/tmp/dd.7z` → `state: runs`; same limits as zip; and **through the core path**: 300 frames delivered to the frontend |
| Z-Wheel cycle in RetroArch | **partial** | the engine is measured by the scan; the core loop that switches sessions has no automated test |
| Covers and No-Intro | **missing** | depends on an account and on an external submission |

### What the CI run taught

Three defects that only showed up once the tests started running to the end, and that hold for next
time:

1. `cargo test` had **never** compiled the binary's test target: `main.rs` imported `zeebx::scan`
   under `#[cfg(test)]`, and there the library is compiled without that `cfg`. Running
   `cargo test --lib` hid this.
2. Two VFS tests compared the exact case of the path, and macOS's APFS **is case-insensitive**:
   the requested file already exists, and there are no two spellings to compare.
3. `continue-on-error: ${{ matrix.experimental }}` makes GitHub **reject the whole workflow file**
   — the run shows up as a failure **with no jobs at all**. `actionlint` and a YAML reader do not
   flag it: the diagnosis is the empty run.

## Publishing the core

The core is published together with the installers, under the same tag. The `core` job of
`release.yml` builds and checks the six targets (the same ABI check as `libretro.yml`) and packages
**one `.zip` per target**, with the library and the `.info` together — RetroArch wants both with
the same name in the cores folder, and six files with the same name do not fit loose in a release.

```text
zeebx_libretro-linux-x86_64.zip      zeebx_libretro-windows-x86_64.zip
zeebx_libretro-linux-aarch64.zip     zeebx_libretro-windows-aarch64.zip
zeebx_libretro-macos-x86_64.zip      zeebx_libretro-macos-arm64.zip
```

Each system packages with the tool it has: `zip` on Linux and macOS, `Compress-Archive` on Windows
— `zip` **does not exist** on the Windows runner, and a `shell: bash` with `zip` would fail on a
third of the targets on tag day.

### Save state: the format exists, the content is what is missing

Item 6 asks for "serialize/unserialize with a versioned format". The **format** is done and tested
(`src/save_state.rs`): `ZBXS` signature, version, size, `crc32` of the content and **named**
sections — an unknown section is skipped, which is what lets an old engine read a state written by a
new one. Eight tests, including the ones that refuse a truncated file, a flipped byte, a wrong
signature and a version from the future (saying which).

The **content** is what is missing, and now it is measured instead of estimated: `Machine` has
**190 fields**, of which ~180 are real state. By type:

```text
 50 HashMap   ·  32 u32   ·  19 Vec   ·  16 Option   ·  13 BTreeSet
  8 u64       ·   8 bool  ·   7 VecDeque  ·  5 ArrayPointer  ·  3 BTreeMap  ·  2 Heap
```

The largest are tables indexed by **guest pointer**: `objects`, `collections`, `fonts`,
`databases`, `open_files`, `decoders`, `images`, `sounds`, `ciphers`, `widgets`, `streams`,
`threads`, `timers`, `signals`… plus the `heap` and `objects` (address counters) and guest memory,
which lives in the CPU backend.

**The good news for the design:** guest pointers are absolute addresses, so restoring memory at the
same addresses keeps **every** pointer stored in those tables valid — there is no remapping. The
work is enumerating the sections and covering each one with a round trip, not inventing a graph
format.

**Until it is complete, the core keeps answering `retro_serialize_size = 0`**, and there is a test
that checks it: zero is how the frontend understands "this core does not save". Partial state is
what the criterion forbids, and it would not slip through by carelessness because that is the gate.

### What already goes into the state, and what is missing

Went in, with a tested round trip:

- **registers** (16, in the section's fixed order — writing "all in the order of the `enum`" would
  leave the format hostage to someone reordering the enumeration);
- **memory of every writable region**, read from the core (that is where memory lives after
  `reset`, not in the loader's map);
- **the heap ledger** (base, end, first never-used address, free and in use), with two checks on
  the way back: the heap has to be this machine's, and an address cannot be free and in use at the
  same time.

Two decisions worth recording, because I wrote one of them wrong first:

1. **The heap cut comes from the state itself**, and not from the current machine. Whoever loads a
   save state loads the heap that was there — I had treated "the state's heap is larger" as an
   error, and it is the normal case.
2. **The objects region and the surfaces region still go whole.** Truncating them at their
   allocators' `next` would be cheaper, but their ledgers do not go into the format yet, and a size
   the way back cannot check is worse than a bigger size.

### The inventory of what already goes in, and what is missing

`Machine` has **190 fields**. The state that already goes and comes back, each with a round-trip
test:

```text
cpu              registers, CPSR, virtual clock, memory of every writable region
ledgers          heap, objects (with the encoded interface), surfaces
input            key and button queues, device on each port, device notices
schedule         timers, registered and pending callbacks
tables           ~20 maps and numeric tables (dib, streams, sounds, images, transparency…)
content          preferences, parameters, IConfig, fonts, pages, decrypted text
pixels           surfaces, screen, decoded images
text             installed modules, enumerations, open file paths
stop             the point where the engine stopped, variant by variant
rest             GL (client vectors), IGraphics, keyboards, threads, Update frames
widgets          the three maps, text, coordinates and the three function pairs
cipher/zip/peek  the library state that has a closed form
```

**Deliberately left out**, with the reason written in the code: the instrument and diagnostic
fields (`api_time`, `profiling_api`, `tracing`, `fault_*`, `ignored_gl`, `*_log`, `missing_*`,
`bad_pointers`), the host `Mixer`, `clock_us` (derived from the instruction counter), and the
**caches of what is on disk** (`vfs`, `resources`, `interned`, `MediaPayload`) — writing those
would duplicate the package and the file inside the save state.

**Three pieces** are missing, and none is mechanical:

| Piece | Size | Why it has not gone in yet |
|---|---|---|
| `gl` — the guest's GL state machine | 57 fields | see the classification below; it is the riskiest piece of the item |
| `decoders` | per object | an image decoder in the middle of a stream: expose the inner one, or accept losing what was in progress |
| `databases` | per object | an open SQL database — the same decision |

### The GL state machine, classified

The 57 fields of `GlState` **are not one block**: they are four groups, and each one calls for a
different decision. This was surveyed field by field, with the type alongside, so the next pass is
mechanical instead of archaeology.

**A — write (48 fields).** The state the game's GL calls change and the next draw reads: the three
matrix stacks (`modelview`, `projection`, `texture_matrix`, 16 floats each), the matrix mode, the
viewport and the scissor (raw and enabled), `surface` and `stretched`, the clear colors and the
current one, the bound texture target and the two units, the test flags and the functions
(`depth_*`, `blend_*`, `alpha_*`, `cull_*`, `stencil_*`, with the three `stencil_op`), fog, the
depth range, the color mask, and lighting (`lighting`, `color_material`, `lights`, `material`,
`light_model_ambient`, `shade_model`). They are numbers and fixed-size vectors — the same shape as
the tables that already went in.

**B — write with its own format (1 field).** `textures`, with each one's mipmap chain. It is what
gives the group its volume and what demands care: a missing mipmap level does not cause an error,
it causes stripes in the image — that is how the defect showed up when the chain started being
used.

**C — decide whether to write (3 fields).** `color`, `depth` and `stencil`, the three 640×480
buffers. The color one is the most interesting candidate: it is **written from the frame and read
to present**, and the screen (`screen`) already goes into the state — if the relation between the
two is one of derivation, writing both duplicates 1.2 MB. The depth and stencil ones have no
duplicate: without them, the depth test and stencil marking start over, and the Z-Wheel stage loses
its reflection. **This is the decision still to be made, and it is made by measuring who writes to
whom.**

**D — do not write, and check they are empty (4 fields).** `batch`, `pending`, `transformed` and
`dirty` are accumulators of a draw **in progress**. The frontend calls serialize **between
frames**, never inside a `retro_run`, so at the instant of the save they are empty — and that is
what makes them dispensable. The right thing is not to rely on that: it is to **refuse** the save
if any of them is not empty, and have the test cover both sides.

### The save state is up and running

The three pieces went in, and with them the **complete state**. The core stopped answering zero:

```text
retro_serialize_size   the size of the state, measured now (7,084,619 bytes with Double Dragon)
retro_serialize        delivers what `_size` measured, and refuses if it does not fit the buffer
retro_unserialize      puts it back, and refuses with the reason when some section does not match
```

Two fitting decisions worth recording:

- **the measured size keeps the bytes.** The ABI calls the size and the write in sequence, and the
  size varies with what the game has in memory — recomputing on the write would give a state
  **different** from the one measured, and the frontend would have allocated the buffer by the
  wrong number;
- **the engine drains the draw queue before checking.** A batch of triangles waiting its turn is not
  a half-done draw — it is work that was going to be done on the next frame. Refusing the save
  because of it would block the player over something the engine resolves on its own. After
  draining, what is left is a truly interrupted draw, and then the refusal is honest.

**The end-to-end proof** is the test `the_save_state_crosses_the_abi`, and it takes the path
RetroArch takes: measures the size, saves, runs more frames for the state to **change**, loads,
saves again and requires **the same file byte for byte**. Then it corrupts a byte and requires that
the refusal does not touch the machine. Comparing fields would be weaker: what the player sees is
the game continuing from the same point.

**What stays out, by declared decision:** the instrument and diagnostic fields, the host `Mixer`,
`clock_us` (derived from the instruction counter), and the caches of what is on disk (`vfs`,
`resources`, `interned`, `media_payloads` — the latter **with a reload path**, which is why it is
not a gap).

### The objects ledger, and the encoding of the interfaces

The `enum Interface` has **62 variants, all without payload, with an explicit discriminant from 0
to 61** — so the number in the file is the discriminant itself, and not a parallel table someone
has to maintain. The way back (`from_code`) is written by hand because Rust does not undo an
`as u32`, and there is a test that checks **all 62**: a new interface without an entry turns the
test red before a save state becomes unreadable. It is the kind of thing that needs to hurt on the
day someone touches it, not on the day someone loads a state.

With the objects ledger in the format, their region and the surfaces region started being **cut at
the first never-used address**, like the heap — and the cut is only honest because each one's
ledger goes into the file, which is where the expected size comes from on the way back.

| | before | now |
|---|---|---|
| state of a minimal machine | 14.07 MB | **2.07 MB** |
| mapped memory | 84 MB | 84 MB |

A trap that showed up along the way and is worth recording: the machine has **two** `Heap`s (the
game's memory and the surfaces region). With a fixed section name, the second would overwrite the
first in the file, and the defect would show up as "the surfaces came back in place of the heap".
The sections now have a prefix.

Missing, to close it: the `ObjectStore` (depends on a stable encoding of `enum Interface`), the ~50
per-object state tables, the instruction counter (it is the virtual clock, and the core's trait has
no way to receive it back yet) and the CPU **flags** — `CpuBackend` exposes 16 registers and the
Thumb mode, and not the `CPSR`.

**Local rehearsal of the packaging step, because it only runs on a tag.** The workflow commands,
run by hand:

```text
zeebx_libretro-linux-x86_64.zip
  zeebx_libretro.info    332 bytes
  zeebx_libretro.so      15,637,608 bytes

extracted: the library loads and answers api_version 1
```

Two files with the **same name**, and the library working after coming out of the `.zip` — which
is exactly what RetroArch needs to accept the core.

**And the artifact names do not collide**, which was the other risk of that step: the release job
downloads everything into a single directory and publishes `pacotes/*`, so two files with the same
name would bring the publication down. Checked: there are **11 artifacts** — five installers (two
`.deb`/AppImage on Linux, one `-setup.exe`, two `.dmg`) and six core `.zip`s, one per target, all
with distinct names. Without the per-target zips, there would be six identical
`zeebx_libretro.so`.

## What is missing, with the map of each item

### Item 5 — hardware rendering: only the plumbing

**What already exists:** the GPU rasterizer, verified against the software one (identical in Double
Dragon and Crash; 2.6% rounding in Peteca); the offscreen context to measure it without a window;
`Session::start_with(..., gpu, Some(Arc<glow::Context>), ...)`, which already accepts the
frontend's context.

**What is missing, in order:**

### The GPU struct, verified by offset

Before asking Rafael for the test, I checked the assumption that would make hardware rendering fail
**silently**: the `retro_hw_render_callback` the core fills in. If my struct had one field too many
or too few, `get_current_framebuffer` and `get_proc_address` would be read from the wrong place —
and the symptom would be "it doesn't work", with nothing in the log.

The repository's `libretro.h` settled the question: in this revision `context_destroy` comes
**after** `cache_context`, and not in third place as in others. My struct's offsets match it, and
now there is a test: `the_gpu_struct_offsets_match_libretro_h` checks 8, 16, 24, 32, 36, 40, 44 and
48 for the fields we use.

The check also found a real gap: I was **not** filling in `context_destroy`. Now it exists and has
an effect — when the frontend swaps the video context (changing driver, for example), the core
discards the `glow::Context`, **recreates the session in software** and warns on screen. Without
that, the session would carry on in the GPU rasterizer calling GL functions that no longer exist.

0. ~~Separate the GPU backend from context creation.~~ **Done**: the engine has two features —
   `gl` (the drawing, which **only** needs `glow`, with nothing from the host) and `gpu` (opening a
   context of our own, which brings in `glutin` and links EGL/GLX). Measured:
   `cargo tree --features gl` has **0** glutin and `glow` present; `--features gpu` has 3. That was
   what was missing for the core to be able to use the GPU: a core cannot link a host interface
   library, and that is what the `ldd` check in `libretro.yml` enforces.
1. ~~Turn on the `gl` feature, build the context and negotiate `SET_HW_RENDER`.~~ **Written and
   compiling**, with the safety valve: if the frontend accepts and does not deliver, or if the GPU
   session fails, the core **falls back to software** and says why in the log. `switch_to` is also
   born on the GPU, so the Z-Wheel opening a game does not hand drawing back to the processor.
2. ~~Make the engine draw into the frontend's framebuffer.~~ **Done and verified**: the engine has
   `draw_to_fbo`, the test `the_engine_draws_into_the_frontend_framebuffer` creates a framebuffer
   of its own, tells the engine to draw into it and reads **its** pixels — in the two games
   measured, **0 of 307,200 pixels** kept their initial color, that is, the drawing all went to the
   outside framebuffer, and not to the internal one.
3. Replace the per-frame `retro_video_refresh` with `RETRO_HW_FRAME_BUFFER_VALID` — without it
   RetroArch receives pixels that are not the FBO's.

The engine side is already ready to receive: `Session::draw_to_fbo(Option<u32>)` is public, and
`Some(0)` means the frontend's default framebuffer. What is left in the core is connecting the
wires.

**An ordering trap, so as not to spend a session finding out:** `libretro` only delivers a
**valid** GL context after calling the `context_reset` of the struct the core filled in. Whoever
calls `SET_HW_RENDER` is the core (inside `retro_load_game`, before creating the session), and
whoever announces "the context is ready" is the frontend, **afterwards**. In other words: the GPU
session cannot be created together with the negotiation — it has to wait for `context_reset`, or
the first `retro_run`. The two possible ways out are creating the session inside `context_reset`,
or keeping the request and creating it on the first `retro_run`, when the context is already
current. And `get_current_framebuffer` is queried **per frame**, in `retro_run`, because the
framebuffer can change.

**How to verify:** the test `both_rasterizers_draw_the_same_frame` still serves as the yardstick;
what changes is where the frame comes out.

### Item 6 — save states: why not yet

The plan's acceptance criterion is "the absence of save states is declared correctly, **without
partial state**", and that is what the core does (`retro_serialize_size` returns 0,
`retro_serialize` is false). A partial state would be worse than none: guest memory and registers
would come back, and the emulator's object table would not — the game would resume calling APIs
with identifiers that no longer exist.

**What a complete state would require, counted:** `Machine` has **183 fields**. Classified by
name, they are:

| group | fields | what to do |
|---|---:|---|
| instruments (`bad_pointers`, `calls`, `api_time`, `assumptions`, `debug_*`, refusal counters) | 13 | **nothing** — they are not state |
| object tables (`bitmaps`, `canvases`, `databases`, `decoders`, `collections`, `egl_surfaces`, …) | 18 | describe each live object and recreate it on load |
| scheduling (`*_pending`, `current_thread`, `delivered`) | 13 | serialize the queue |
| scalars and counters (`clock_us`, `epoch_seconds`, series) | 10 | one `u64` each |
| the rest (drawing state, EGL, configuration in effect, diagnostic lists) | ~129 | separate what the game sees from what belongs to the emulator |

**The criterion that shrinks the scope: an instrument is not state.** Everything that exists for
the report — refused pointers, call counts, time per method, hypotheses — stays out of the save
state, otherwise loading a state would also load another session's debugging history, and the scan
baseline would start depending on when the game was saved.

**And the criterion that grows the scope:** the 18 object tables need a **description** that is
enough to recreate each object — open file and offset, surface and format, media stream and
position, SQL query and cursors. That is where the work is, and that is where a partial state would
lie.

### Item 8 — Z-Wheel cycle: what is already measured

The scan **cannot** exercise it: twelve seconds with and without the stick differ by a thousand
instructions out of 133 million, and the wheel draws nothing on that path (zero frames, zero text).
The practical conclusion is that **only RetroArch can answer** — it is not worth spending another
session trying it this way.

The **precondition**, on the other hand, is measured through the core path: when opening the
Z-Wheel, the core finds **63 games** next to the content (the ClassID survey, with deduplication
between the `.zip` and the extracted copy). That separates two symptoms that look alike: **"the
wheel opens empty" is not a discovery failure** — it knew about all 63.

**And input reaches the guest, measured.** The test `retropad_input_reaches_the_guest` presses a
RetroPad button through the core path and counts the distinct images the game returns:

```text
Pac-Mania:   START button →  2 distinct images in 60 frames (the game responded)
Peteca:      START button → 28 distinct images in 60 frames
```

That closes the other hypothesis that was open: when the Z-Wheel did not react to the stick in the
headless test, I did not know whether it was the core's input path or the wheel's state. **It is the
wheel's state** — input demonstrably reaches the guest, in two games. What the Z-Wheel is missing is
a real frontend, which is what item 8 asks for.

### Item 9 — covers and No-Intro: depends on an account

The local part is done, and re-measured on 2026-09-22:

```text
playlist  Mobile - Zeebo.lpl        62 entries, 0 without a file
covers    Named_Boxarts            116 files (58 titles, in .jpg and .png)
          Named_Titles              44 files (.mif icon, stopgap)
database  Mobile-Zeebo-No-Intro-libretro.dat   12,372 bytes, 57 games, version 2026.08.01
RDB       Mobile - Zeebo.rdb       installed, and what makes "Scan Content" match by hash
```

What is missing is publishing to the Libretro thumbnails repository (the directory has to be called
`Mobile - Zeebo`, which is the database's name — the covers are already in that format and with
that name) and submitting the five titles outside No-Intro. **Both need an account and human
review**, and it is Rafael's decision: nothing was published and no external repository was
created.

**The format was wrong for the destination, and that was fixed.** RetroArch's thumbnail
repositories accept **only PNG**, and the covers the Z-Wheel delivers are JPEG — the fifty-eight
files would be refused on submission day. The catalog gained `--png`, which converts at write time
(with Pillow; without it, it writes the original and warns), and the generation was checked:

```bash
python3 tools/catalog.py --roms ROMS --out out --png --zwheel "Z-Wheel.zip"
# official covers: 58 of 62  ·  png: 58  ·  jpg: 0
# Action Hero 3D ….png: PNG image data, 170 x 220, 8-bit/color RGB
```

**And the titles outside No-Intro come out with the hashes ready.** What the proposal asks for is
the name, size, CRC32, MD5 and SHA1 of the file the DAT hashes — and for a title that is not in the
DAT, that file is the submitter's decision. The tool writes `<out>/fora-do-dat.txt` with **all**
the files of `mod/<id>/` and the three hashes of each, and says in the header that the choice is a
human one: guessing here would get the proposal refused, and the work would be doubled.

```text
## Bad Dudes vs. DragonNinja (Brazil) (Es,Pt)
archive: Bad Dudes vs. DragonNinja (Brazil) (Es,Pt).zip  (1369145 bytes)
archive crc32: 56020280   sha1: 2E E9 …
  mod/279888/baddudes.eng
    size: 1456  crc32: AD9831B6  md5: 82A42D24…  sha1: E4DCEE01…
```

And the same content comes out in **DAT format**, which is what the proposal really asks for — five
`game (` blocks, with the file name in the database's convention (the module folder and the file,
without a slash):

```text
game (
	name "Bad Dudes vs. DragonNinja (Brazil) (Es,Pt)"
	region "Brazil"
	rom ( name "mod279888baddudes.mod" size 3034964 crc DE1F72C0 md5 3567F3EE… sha1 BB5781BE… )
	… (all the module's files)
)
```

A `game` with several `rom` entries is valid in the format, and it is the right thing here: **which
of the files is the dump is the database maintainer's decision**. The proposal carries all of them,
with the hashes, instead of betting on one — and a wrong bet, in this case, costs a whole round
trip.

## Implementation order

1. Inventory all core I/O and define `StorageFs`/`GuestFile`; decide on a SQLite VFS or
   transactional staging before promising non-native paths.
2. Create `StoragePaths`, `ContentIdentity` and `ContentLayout` on top of that abstraction.
3. Move configuration and `.mif` reading into neutral modules.
4. Refactor archive, machine, SQL, widgets, fonts, media, shell and VFS to receive explicit
   storage; keep `std::fs` only in desktop CLI/UI/tests.
5. Implement the VFS with a read-only base, writable overlay, tombstones and shared device.
6. Extract a deterministic virtual frame driver from `Session`.
7. Create `src/lib.rs`, features and an engine without desktop dependencies; turn the root into a
   workspace.
8. Create `frontends/libretro/` as a `cdylib` package, vendor `libretro.h`, generate bindings and
   implement all the base exports, including VFS v3.
9. Implement `.mod`/ZIP, RGB565, PCM16 audio, Dragon/Z-Pad and RetroPad.
10. Add `SET_CONTROLLER_INFO`, USB keyboard with an AVK queue and static Boomerang.
11. Integrate the Libretro sensor and Boomerang calibration; validate the analog fallback later.
12. Add deterministic, VFS, COW, malicious ZIP and cross-persistence tests.
13. Integrate and test RetroArch x86_64.
14. Add native AArch64/CI.
15. Implement 7z with a real corpus and security limits.
16. Only afterwards: Core Options, full SAF/Android support, measured guest mouse, HW render and
    save states.

## MVP acceptance criteria

- `frontends/libretro/` is an independent package and does not bring in the desktop UI as a
  dependency;
- the x86_64 core loads in RetroArch;
- `.mod` and `.zip` start the applet and display the framebuffer;
- Dragon, Z-Pad, Boomerang and USB keyboard show up to the guest according to the selected device;
- RetroPad input reaches the Z-Pad;
- the Libretro keyboard delivers AVK press and release without running the guest inside the
  callback;
- the Boomerang receives accelerometer samples from the frontend, or clearly reports
  fallback/absence;
- stereo audio reaches the frontend;
- relative saves survive unload/reload;
- `fs:/` is shared between titles;
- clearing `cache/` does not delete saves or `aparelho`;
- the ROM and the content cache are not modified by a run;
- a malicious ZIP does not write outside `zeebx/`;
- content, saves and cache go through `StorageFs`/VFS; native Linux is validated in the MVP;
- non-POSIX paths (SAF/Android) are only announced after the SQLite/VFS solution and real tests;
- a missing/non-writable `GET_SAVE_DIRECTORY` fails without creating a misleading temporary save;
- concurrency on the same `aparelho/` is refused on the native backend when a lock exists, and
  documented as unsupported where the VFS offers no mutual exclusion;
- the Z-Wheel can receive at least `AVK_CLR`/`AVK_SELECT` through the keyboard route;
- the guest network stays off without an explicit choice;
- all the base exports exist;
- the absence of save states is declared correctly, without partial state;
- the AArch64 build and loading are validated before a release.
