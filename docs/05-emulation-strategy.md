# 05 — Proposed emulation strategy

## HLE, not LLE

| | LLE (running the firmware) | **HLE (reimplementing BREW)** |
|---|---|---|
| Needs the console's NAND/AMSS | yes (a legal and distribution problem) | no |
| Needs to emulate MSM7201A, Adreno 130, QDSP-5, MDP, modem | yes | no |
| Needs to emulate the ARM11 | yes | yes (only the game code) |
| Effort | years | months until "something runs" |
| Accuracy | high | depends on how faithful the APIs are |

We go with **HLE**. The game only talks to BREW vtables; beneath them it is us. The NAND dumps
serve as a disassembly reference when an API's behavior is ambiguous, not as a runtime dependency.

## Emulator building blocks

```
      .mod / .mif / .sig  in roms/<Title>/
                │
       ┌────────▼─────────┐
       │  Module loader   │  BREW header parsing, relocation, module memory map
       └────────┬─────────┘
                │  entry = AEEMod_Load
       ┌────────▼─────────┐        ┌────────────────────────┐
       │  ARM core        │◄──────►│  Guest memory (flat)   │  heap, stack, module .text/.data
       │  (ARMv6/ARM11)   │        └────────────────────────┘
       └────────┬─────────┘
                │  call through a vtable pointer at a "magic" address
       ┌────────▼───────────────────────────────────────────┐
       │  AEE / HLE layer: interface+slot dispatch           │
       │  IShell IDisplay IGraphics IBitmap IFileMgr IFile   │
       │  ISound ISoundPlayer IMedia IHID ISignal IHeap ...  │
       │  + OpenGL ES 1.x / EGL extension                    │
       └────────┬───────────────┬───────────────┬────────────┘
                │               │               │
          video (GL/…)     audio (mixer)    input (SDL gamepad)
                │
        VFS: maps BREW paths ("fs:/~/…") to roms/<Title>/mod/<id>/
```

### 1. ARM core
Target: **ARMv6 (ARM1136), ARM + Thumb, no NEON, with VFP** (check whether the games use VFP or
only fixed point — the Developer Guide pushes fixed point).
Options: start with **our own interpreter** (easier to debug and to instrument call traces), with
`dynarmic` or `unicorn` as plan B/optimization. Infuse uses dynarmic.

### 1.5. The helper table (found in practice)

Besides the object vtables, dynamic modules call the **BREW stdlib** through a table of function
pointers that the loader hands over — `MALLOC`, `STRLEN` and friends are not interface methods.
The pointer to that table reaches the module through two words written **before** the loaded
image, which the `elf2mod` stub copies into the module. Practical consequence: the module cannot be
loaded at address 0, and the helper table needs the same trampolines as the vtables.

### 2. API trampoline — the central trick
BREW interfaces are structs with a pointer to a vtable. The usual technique:
allocate the vtables in an **invalid/sentinel** address region in the guest space; when the ARM
core tries to execute a `BLX` there, the emulator catches it, decodes *(interface, slot)* from the
address, reads the arguments from `r0..r3`/stack per the AAPCS, runs the native implementation and
returns the result in `r0`. Every new API becomes "one more entry in the table".
**Slot order is ABI** — use the BREW SDK headers (`AEE*.h`) and check against
`vendor/tripleoxygen/research/brew/vtbl.ods`, which has the console's real vtable.

### 3. Implementation order (shortest path to the first screen)
1. `.mod` loader + relocation + `AEEMod_Load` returning successfully.
2. Minimal `IShell`, `MALLOC/FREE/REALLOC`, `IModule`, the `EVT_APP_START` cycle.
3. `IFileMgr`/`IFile` on top of a VFS pointed at the module directory.
4. `IDisplay`/`IBitmap`/`IGraphics` → 640×480 RGB565 framebuffer in an SDL window.
5. Timers/callbacks (`ISHELL_SetTimer`, `AEECallback`) and the frame loop.
6. `IHID`/`ISignal` → gamepad.
7. EGL + OpenGL ES 1.x (via ANGLE or translation to GL 2+/gl4es).
8. Audio: PCM/ADPCM → MP3 → MIDI wavetable (the most annoying one; leave it for last).
9. Network: stubs that fail "gracefully" (`IWeb`, `INetMgr`) — and, later, a local server
   pretending to be TecToy's store for the Z-Wheel.

### 4. Testing strategy
- **`roms/Z-Wheel`** (App ID 274755) is the console's shell: it exercises 2D, fonts, audio, i18n,
  QXEngine and network. It is an ambitious target for the first boot, but an excellent *stress
  test*.
- An easier initial target: compile our own BREW apps with an open toolchain
  (`arm-none-eabi-gcc` + `elf2mod`, see `vendor/repos/openzeebo/brew/intro`) — that way you have
  **source + binary** and know exactly which API was called.
- Second target: the official SDK samples (`vendor/sdk/samples/unpacked/`), which cover GL ES in
  isolation (VBO, dot3, point sprites, matrix palette, ATITC…).
- **Trace-first**: before implementing, log every unimplemented call with
  `(interface, slot, args)` and let the app crash. The list of missing pieces becomes the backlog.

### 5. Choices still to make
- **Language**: C++17/20 or Rust. C++ makes it easier to reuse dynarmic; Rust makes it easier not
  to get the guest pointer arithmetic wrong.
- **Frontend**: SDL3 (window, input, audio) is the shortest path to cross-platform.
- **GL**: ANGLE (GL ES on top of Metal/D3D/Vulkan) solves macOS, where native GL ES does not exist.

## Real risks
1. **MIDI/wavetable audio** — the 512 kB table belongs to the firmware; reproducing the exact
   timbre requires extracting the wavetable from the dump or accepting a substitute soundfont
   (an audible difference).
2. **QXEngine** is a closed binary; if it touches obscure BREW APIs or makes assumptions about
   memory layout, it will hurt.
3. **The `.mod` format** is not publicly documented — the relocation table has to be inferred from
   `elf2mod` and validated with binaries of known source.
4. **Dead network/store layer**: several titles may check activation/DRM online.
5. Fixed-point fidelity in GL ES (`GLfixed`) — a subtle error here becomes crooked geometry.
