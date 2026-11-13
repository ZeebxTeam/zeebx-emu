# 01 — Architecture

## The idea in one sentence

The game is an ARM binary that never talks to hardware: it calls vtables. The emulator runs that
binary on a real ARM core and **intercepts the calls**, servicing each one on the host. No console
firmware is involved at any point.

## The path of a call

```
  game (ARM)                      emulator (Rust)
      │
      │ ldr r12, [vtable + 0x30]
      │ blx r12
      ▼
  0xf000_7038  ──── unmapped address ────▶  the core stops with FETCH_UNMAPPED
                                                    │
                                     aee::decode(0xf000_7038)
                                                    │
                                        (Interface::Bitmap, slot 14)
                                                    │
                                          Machine::dispatch
                                                    │
                                     bitmap_call("SetTransparencyColor")
                                                    │
                                      r0 = result, pc = lr, continue
```

The address **does not exist**: it is an instruction-fetch fault treated as a call. That is what
makes any ARM glue code unnecessary — the details are in [03-api-dispatch.md](03-api-dispatch.md).

## The modules

The tree follows the subsystems: one folder per subject, and inside it one file per piece.

### Execution core

| File | Role |
|---|---|
| `cpu/mod.rs` | `CpuBackend`, the boundary with the ARM core: registers, memory, `run` |
| `cpu/dynarmic.rs` | The implementation on top of Dynarmic, configured for ARMv6K |
| `cpu/mem.rs` | The guest memory map, in named regions |
| `loader/mod.rs` | Sets up the module environment and calls `AEEMod_Load` |
| `loader/modfile.rs` | The `.mod` parser |
| `machine/mod.rs` | The loop: run, service the call, continue. Holds the `Machine` state and the dispatch |

The API implementation lives in one submodule per subject, each with its own
`impl<C: CpuBackend> Machine<C>` block. A submodule sees the `Machine`'s private fields because it
descends from `machine`; what it exposes back is `pub(super)`, visible in the module and its
siblings and nowhere else.

| File | Role |
|---|---|
| `machine/shell.rs` | `IShell`: resources, device information, applet startup |
| `machine/display.rs` | `IDisplay` and `IGraphics`: 2D drawing and text |
| `machine/bitmap.rs` | `IBitmap` and `IDIB`: surfaces, blit and syncing with the game's memory |
| `machine/image.rs` | `IImage` and `IImageDecoder` |
| `machine/widget.rs` | `IWidget`, `IControl`, `IForm` and painting the tree |
| `machine/egl.rs`, `machine/gl.rs` | EGL and OpenGL ES 1.1, and the bridge to the rasterizer |
| `machine/media.rs`, `machine/sound.rs` | `IMedia` and `ISound` |
| `machine/file.rs`, `machine/zip.rs` | `IFileMgr`/`IFile` and `IUnzipAStream` |
| `machine/net.rs` | `IWeb` and `ISource` |
| `machine/hid.rs` | `IHID`, the ports and the keys |
| `machine/sql.rs` | `AEECLSID_SQLMGR`, serviced by SQLite |
| `machine/cipher.rs` | `ICipher` and `IHash` |
| `machine/thread.rs` | `IThread`, `IQueue` and the BREW heap |
| `machine/signal.rs` | `ISignal` and callback delivery |
| `machine/time.rs` | The clock, vsync and skipping idle time |
| `machine/helper.rs` | The BREW stdlib on the `Machine` side: `printf`, `qsort`, conversion |
| `machine/misc.rs` | The single-call interfaces: configuration, SIM, power, license |
| `machine/probe.rs` | Probing an unknown class |
| `machine/diagnostics.rs` | Tracing, fault dumps and the counters the interface shows |

### BREW API

These files, and the `.mod`/`.mif`/`.bar` parsers, live in the
[`zeebx-core`](https://github.com/ZeebxTeam/zeebx-core) crate (the parsers under `formats/`).

| File | Role |
|---|---|
| `brew/aee.rs` | The trampoline: converts address ↔ (interface, slot) |
| `brew/aee_slots.rs` | The name of each method, in the order it occupies in the vtable |
| `brew/aee_helpers.rs` | The BREW stdlib table (`memcpy`, `malloc`, `sprintf`…) |
| `brew/cformat.rs`, `brew/fmath.rs` | The `printf` and the floating point the table points to |
| `brew/objects.rs` | The objects we hand to the game, with reference counting |
| `brew/heap.rs` | The heap the game's `malloc` consumes |
| `brew/vfs.rs`, `brew/sql.rs`, `brew/crypto.rs` | The AEE services: files, SQLite and cipher |

### Outputs

| File | Role |
|---|---|
| `video/display.rs` | The framebuffer and the 2D operations |
| `video/rasterizer.rs` | OpenGL ES 1.1 in software |
| `video/gles.rs`, `video/atc.rs`, `video/paltex.rs` | GL state and the compressed texture formats |
| `video/font.rs`, `video/icon.rs`, `video/gif.rs` | The text and the images that come inside the games |
| `audio/mod.rs`, `audio/wav.rs`, `audio/mp3.rs`, `audio/midi.rs` | Sound mixing and decoding |
| `input/` | Input: the console controller, the host gamepads and the button map |

### Outside the emulator

| File | Role |
|---|---|
| `session.rs` | A running game, from the file to the frames — what the interface uses |
| `ui/` | The interface: screen, library, preferences, saves and translation |
| `main.rs` | The command line, and the `launch()` that opens the interface when there are no arguments |
| `loader/archive.rs`, `loader/miffile.rs`, `loader/resfile.rs` | The `.zip`, the `.mif` and the `.bar` |

## The loop

`Machine::advance(budget)` is one turn. It:

1. **moves the clock forward** if the game is only waiting (`skip_idle_time`);
2. **fires the expired timers** — removing them from the list *before* calling the callback,
   because the callback typically re-arms the timer and the re-armed one must not fire in the same
   frame;
3. **delivers the pending callbacks and threads**.

One turn **is not one frame**. The game can run its entire main loop inside a callback, and each
turn returns only the slice of instructions that fit. In Peteca it takes a million and a half
turns for a few dozen frames. What drives the redraw is `eglSwapBuffers` (3D) or the real clock
(2D).

## The emulator runs on the interface thread

There is no separate thread. `Session::step` already hands control back at every slice of real
time — which is what keeps the window alive while the game runs. A thread would bring
synchronization and nothing in return.
