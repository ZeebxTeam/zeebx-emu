# Optimizing Zeebx — the 0.3.0 starting line

This document exists for a simple reason: **the performance numbers the project publishes no
longer describe the emulator in the tree.** They were measured with Unicorn, and Unicorn is gone.
Before optimizing anything we have to measure again, and before measuring we have to say out loud
what we know, what we assume and what we are going to check.

The target of this round is not the desktop. It is the handheld: **R36S** (Cortex-A35, ArkOS) and
**RG40XX-H** (Cortex-A53, muOS), through the Libretro core. The desktop comes in as the measuring
bench, because there it can be instrumented without an SD card in the way.

## 1. What is out of date

`ARCHITECTURE.md`, in the "Known limits" section, carries a cost table that came from the old
backend:

| published number | where | status |
|---|---|---|
| 52% of console speed in Quake | `ARCHITECTURE.md:243` | measured with Unicorn |
| ~68% ARM + dispatch, ~28% pixels, ~4% geometry | `ARCHITECTURE.md:243` | measured with Unicorn |
| 1.4 µs per API call | `ARCHITECTURE.md:264` | measured with Unicorn |
| 57 ns per `read_u32` against 0.3 ns in bulk | `ARCHITECTURE.md:252` | text says "unicorn FFI" |
| 218 M instr/s in the loop, ~86 M in game | `ARCHITECTURE.md:257` | measured with Unicorn |
| `cargo test --release instrucoes_por_segundo` | `ARCHITECTURE.md:258` | **the test no longer exists** |

The dates settle it:

```
8af069f  2026-09-11  Update the measured performance numbers
a359183  2026-09-23  Remove Unicorn and make Dynarmic the default backend
```

The measurement is twelve days older than the removal of the backend it measured. The document
also describes the tree with `cpu/unicorn.rs` and a `CpuBackend` "on top of unicorn"
(`ARCHITECTURE.md:59`, `:83`), a file that is no longer there.

**None of this means the conclusions were wrong.** It means they have to be redone before they
can serve as the basis for an optimization decision.

## 2. What is still true in today's tree

These points were checked in the code, not in the document:

- **API dispatch still stops the JIT.** The callback calls `halt(HaltReason::UserDefined1)`
  (`src/cpu/dynarmic.rs:126`), the method is served on the host, and the loop clears the reason
  and re-enters with `jit.run()` (`src/cpu/dynarmic.rs:587`). The round trip per call still
  exists with Dynarmic; what changed is its cost, which nobody has measured yet.
- **The page table is already a partial fastmem.** `config.page_table(...)` with
  `page_table_mask(0)` (`src/cpu/dynarmic.rs:450`), and the null entries fall into the callbacks:
  read-only regions, pages already executed, pages with a write watch and the last page of a
  region that does not fill it (`src/cpu/dynarmic.rs:314`).
- **There is a recent, useful number in the code itself:** without the page table, Need for Speed
  stayed at 176 million instructions per second, 69% of console speed
  (`src/cpu/dynarmic.rs:317`). That is a number from the page-table era, not from Unicorn.
- **The rasterizer is already parallel.** It splits the frame into bands and uses
  `std::thread::scope` (`src/video/rasterizer.rs:2341`). Whoever proposes "parallelizing the
  rasterizer" is proposing what already exists.
- **The per-API cost profile exists and is opt-in.** `Machine::enable_api_profile()`
  (`src/machine/diagnostics.rs:87`) and `Machine::api_profile()` (`:92`), turned on by the scan
  with `ZEEBX_ROM_PROFILE`.
- **The scan already measures performance.** `Performance` (`src/scan.rs:427`) carries
  `instructions`, `calls`, `frames`, `pixels`, `laps`, `virtual_ms`, `run_loop` and `speed()` as
  a percentage of console speed.
- **The release profile is already tight.** `lto = "thin"` and `codegen-units = 1`
  (`Cargo.toml:171`). Whoever suggests "turning on LTO" is suggesting what is already on; what is
  left is comparing `thin` with `fat`.

## 3. What the Libretro core already offers the handheld

Thirteen options, of which these directly affect cost:

| option | effect | worth it when |
|---|---|---|
| `zeebx_frameskip` | skips drawing, without touching the clock | rasterization dominates |
| `zeebx_limite_fps` | caps presentation at 60/30 | the device runs unthrottled and heats up |
| `zeebx_resolucao_interna` | supersampling **upward**, 1..4 | there is GPU to spare |
| `zeebx_antialias` | samples per pixel on the GPU | there is GPU to spare |
| `zeebx_rasterizador` | forces software when the driver lies | the GPU draws wrong |
| `zeebx_perfil` | Handheld: lower rate, voices and cache | the device is weak |
| `zeebx_soundfont_taxa`, `zeebx_midi_vozes`, `zeebx_cache_de_som_mb` | MIDI cost | the music is heavy |

Notice what does **not** exist: an internal resolution **below** 1. Today `scale` is an integer
multiplier (`src/video/gpu.rs:207`, `:727`, `:2270`) and only applies to the GPU rasterizer. For
`0.5x` and `0.25x` a fraction would be needed, and the bigger gain is probably on the side of the
software rasterizer, where every pixel is CPU.

## 4. The hypotheses to be tested

None of them is decided. They are hypotheses, and the measurement decides.

1. **The handheld's bottleneck is ARM + HLE, not fill.** If true, `0.5x`, GLES 2 and frameskip do
   not solve it; what solves it is the trampoline and bulk reads.
2. **The cost per API call dropped with Dynarmic.** The `1.4 µs` is from Unicorn. It may have
   dropped a lot, it may have dropped a little. Until it is measured, priority 1 is a bet.
3. **Reads from read-only pages still go through a callback.** If that weighs, extending fastmem
   to reads is cheap and pays off.
4. **SoundFont synthesis stalls the first song.** The cost is a load cost, not a frame cost, but a
   hitch of hundreds of ms is felt on the handheld.
5. **`0.5x` on the software rasterizer is worth more than `0.5x` on the GPU.** CPU fill scales
   with area; the Mali-G31 at 640x480 is probably not saturated.
6. **NEON pays off on the host, not in the guest.** The game's ARM code belongs to the guest and
   goes through the JIT; what can be vectorized is what the host does: fill, convert, copy and
   clear.

## 5. What will **not** be done in this round

Said now so it does not turn into an argument later:

- We will **not** copy SH4ZAM. It is Dreamcast SH-4 assembly, and the host here is ARM64 or
  x86-64. The usable idea in it is "a specialized routine per host architecture", and that is
  written with NEON, not with FSCA and XMTRX.
- We will **not** lower the requirement to GLES 2 for performance. GLES 2 is a **compatibility**
  matter — old Android, limited drivers —, and it would change VAO, `blit_framebuffer`,
  multisample and shader version (`src/video/gpu.rs:284`, `:533`, `:419`, `:1493`).
- We will **not** write a GLES 1 backend. It would be a second fixed-pipeline rasterizer.
- We will **not** create an audio output thread in the core. In Libretro the frontend pulls the
  audio; a core that creates an output thread is fighting the contract.
- We will **not** tune MIDI timbre or gain any further. That was already calibrated by
  measurement.

## 6. How to measure

### 6.1 On the bench (desktop)

The scan already does the job and needs no window:

```sh
ZEEBX_ROM="<game>.zip" \
ZEEBX_ROM_PROFILE=1 \
ZEEBX_ROM_MS=15000 \
ZEEBX_ROM_OUT=/tmp/perf \
cargo test --release scan -- --nocapture
```

What comes out of it: speed as a percentage of the console, instructions, calls, frames, pixels
and the list of the most expensive API methods, with a total.

Measurement rules, inherited from the mistake documented in `8af069f`:

- **measure time with the profile off.** The block profile cost 24% in the old measurement, and
  the price falls almost entirely on the ARM slice;
- use the same `ZEEBX_ROM_MS` in every comparison;
- always run in `--release`;
- compare proportions with the profile on, wall clock with it off.

**Be careful with what the API profile measures.** `api_time` only adds up the time of
`dispatch_inner` (`src/machine/mod.rs:3370`), that is, **the method body**. It does not include
the JIT `halt` nor the re-entry through `jit.run()`. The cost of the trampoline, which is exactly
the `1.4 µs` number we want to redo, **does not come out of here**: it needs a micro-test of its
own — a guest loop calling a cheap method, compared with the same loop without the call.
Confusing the two is the easy mistake of this round.

### 6.2 Games for the round

| game | why this one |
|---|---|
| Quake | historical worst case, half a million draw calls |
| Crash Bandicoot Nitro Kart 3D | uses `glReadPixels`, turns frameskip off |
| Need For Speed Carbon | 3D with a lot of geometry; gave the fastmem number |
| Zeebo Extreme Rolimã | has already shown `memset` as a cost |
| Double Dragon | 2D, and it is what gets played in the device tests |

### 6.3 On the device

What is missing is per-frame telemetry in the core. What we want to count, per second:

```
guest time (ARM)
time in gles_draw
number of draws and of vertices
number of glReadPixels
time in memset/copy
frame conversion time
audio time
skipped frames
```

Until that exists, the device only answers "how many FPS", and that does not say where the time
went.

## 7. The order of attack

The order holds **after** the measurement, and changes if the measurement disagrees.

| # | front | why now | risk |
|---|---|---|---|
| 1 | Re-measure and fix `ARCHITECTURE.md` | the document is guiding decisions with old numbers | none |
| 2 | API trampoline | it is the fixed cost per call, and the doc already points to it | high: touches the core |
| 3 | Bulk reads and read fastmem | the lesson "ask for the block, never the element" is already the project's | medium |
| 4 | NEON on the host: fill, convert, copy, clear | scales with pixels and with bytes | medium |
| 5 | `0.5x`/`0.25x`, software first | cuts area, and area is the cost of CPU fill | medium: viewport, scissor, readback |
| 6 | SoundFont synthesis without stalling the load | removes a hitch felt on the handheld | low |
| 7 | `lto = "fat"` measured against `thin` | it is cheap to test | low |

## 8. Acceptance criterion

An optimization goes in when:

1. there is a measurement **before and after**, on the same game, with the same `ZEEBX_ROM_MS`;
2. the scan of the 62 games does not regress behavior — performance is left out of the baseline
   comparison on purpose, and that is why the number lives here and not there;
3. the gain also shows up on the handheld, or it is stated to be a desktop-only gain;
4. `cargo test`, `cargo clippy` and the Libretro core stay green;
5. the architecture document is updated along with it, with the new number and the date.


## 10. External reference

A consultation session with DeepSeek about emulator optimization raised points worth reading,
and it also shows where an outside analysis goes wrong:

https://chat.deepseek.com/share/yjigu5xc76ahsri24s

What it got right: the API trampoline is the structural bottleneck; the rasterizer is not the
bottleneck; the lesson of asking for the block instead of the element; extending fastmem to
reads.

What it got wrong: it proposed turning on `lto` and `codegen-units = 1`, which are already on
(`Cargo.toml:171`), inferring the file from its size in bytes without opening it; it proposed
parallelizing the rasterizer, which is already parallel (`src/video/rasterizer.rs:2341`); it
proposed a dedicated audio thread, which goes against the Libretro contract; and it treated the
numbers in `ARCHITECTURE.md` as current, when they are from the Unicorn era — which is exactly the
reason this document exists.

## 11. Measurement log

| date | game | backend | speed | instr/s | API calls | note |
|---|---|---|---|---|---|---|
| 2026-09-11 | Quake | Unicorn | 52% | 86 M | — | `ARCHITECTURE.md:243`, gameplay scene |
| 2026-09-24 | Quake | Dynarmic | **170%** | **195 M** | 1,323,900 | 15,016 virtual ms in 8.8 real s, 583 frames, software rasterizer |

### The bench

A performance number without the machine that produced it is worth nothing. Every bench
measurement in this document comes from here, until said otherwise:

| item | value |
|---|---|
| device | Lenovo IdeaPad 3 |
| processor | AMD Ryzen 7 5700U (Zen 2, 8 cores / 16 threads, up to 4.37 GHz) |
| graphics | integrated Radeon (Vega), Mesa |
| memory | 9 GiB |
| system | Debian GNU/Linux 13 (trixie), kernel 6.19.5-x64v3-xanmod1 |
| compiler | rustc 1.98.1 |
| profile | `--release`, `lto = "thin"`, `codegen-units = 1` |

This is an office laptop, not a measurement bench: the clock varies with temperature and with the
governor, and the measurement of the same work fluctuates between runs. It is good for comparing
**before and after on the same device**, in the same session, and not for publishing an absolute
number.

And the difference from the target is huge, which is exactly the point: the Ryzen 5700U has 16
threads of execution and more than 4 GHz, while the R36S has a Cortex-A35 and the RG40XX-H a
Cortex-A53. Quake's 170% does **not** carry over to the handheld, and any conclusion about a
device bottleneck has to be measured on the device.

**The first measurement with Dynarmic refutes the pessimistic reading.** Quake went from 52% to
170% of console speed, and the core from 86 M to 195 M instructions per second — 2.3 times. The
emulator `ARCHITECTURE.md` describes is not the one in the tree.

An honest caveat, so the measurement does not turn into advertising: **the scene is not the
same.** The old measurement described the gameplay scene with half a million draw calls; this one
ran 15 seconds from the opening, with 278,297 `DrawArrays`. What is directly comparable is the
pace of the core (instructions per second), not the wall clock of different scenes. To compare
wall clock we will need a key script (`ZEEBX_ROM_KEYS`) that reaches the same point in the game.

The call profile of the same report shows where the conversation with the guest concentrates:

| calls | method |
|---:|---|
| 283,982 | `IGLES11::VertexPointer` |
| 282,717 | `IGLES11::TexCoordPointer` |
| 278,297 | `IGLES11::DrawArrays` |
| 137,720 | `AEEHelpers::strcmp` |
| 61,143 | `AEEHelpers::aee_GetRand` |

The first three add up to 844,996 of the 1,323,900 calls — **64% of all API calls are the three
geometry entry points**, and they travel together: one `VertexPointer`, one `TexCoordPointer` and
one `DrawArrays` per batch. If the trampoline cost still weighs, this is where it weighs, and this
is where batching pays.

## 12. Where the time went, in the five games (2026-09-24)

Run with `ZEEBX_ROM_PROFILE=1`, 15 virtual s, software rasterizer, on the bench described above.
**The profile is expensive**: Quake goes from 8.8 s (170%) without the profile to 12.6 s (119%)
with it, about 43%. That is why the table below is for **proportion**, and the absolute clock
comes from the run without the profile.

| game | real | speed | instr/s | API calls | time in API | presentation |
|---|---:|---:|---:|---:|---:|---:|
| Crash Nitro Kart 3D | 1.5 s | 1009% | 207 M | 44,559 | 1,145 ms | 975 ms (85%) |
| Double Dragon | 2.6 s | 569% | 22 M | 37,808 | 2,010 ms | 1,643 ms (82%) |
| NFS Carbon | 6.3 s | 238% | 51 M | 252,324 | 4,809 ms | 4,062 ms (84%) |
| Quake | 12.6 s | 119% | 136 M | 1,323,900 | 8,109 ms | 5,139 ms (63%) |
| Zeebo Extreme Rolimã | 3.8 s | 396% | 240 M | 176,080 | 2,316 ms | 1,242 ms (54%) |

"Presentation" is `eglSwapBuffers`/`SwapBuffers`, and we have to say what it **is** on the inside,
or the number misleads:

```rust
"SwapBuffers" => {
    self.sync_egl_color_from_guest()?;
    self.egl_swaps += 1;
    self.present_gl();
    self.wait_for_vsync();
    (2, gles::EGL_TRUE)
}
```
`src/machine/egl.rs:311`

`present_gl` calls `frame_rgb565` (`src/machine/gl.rs:1051`), and the software `frame_rgb565`
starts with `self.flush()` (`src/video/rasterizer.rs:2451`) — **that is where the triangle queue
becomes pixels**. In other words: the time of `SwapBuffers` is rasterization, RGBA→RGB565
conversion and copying, not "swapping buffers". `wait_for_vsync` only touches the virtual clock
and does not sleep (`src/machine/time.rs:115`).

### What this changes

**The sentence "the rasterizer is not a bottleneck" in `ARCHITECTURE.md:271` no longer holds in
this configuration.** It was written when ARM cost twice as much; with Dynarmic, the ARM slice
shrank and fill came to dominate: 54% to 85% of API time in all five games, and somewhere between
41% and 65% of wall-clock time.

Other targets the profile shows:

- **Geometry, in Quake:** `DrawArrays` 876 ms (10.8%), `TexCoordPointer` 431 ms (5.3%),
  `VertexPointer` 427 ms (5.3%) — 1.73 s, 21% of API time. The trio is not expensive only because
  it stops the JIT; it costs on the inside too.
- **`memset`, in Rolimã:** 728 ms, 31.4% of API time. It confirms the old suspicion.
- **`strcmp`, in Quake:** 203 ms, 2.5%. Smaller than the number of calls suggested — 137 thousand
  calls, but `read_cbytes` already reads in 64-byte blocks.
- **Crash Nitro Kart:** 44 thousand calls in 15 s and 1009% speed. The game that motivated the
  `glReadPixels` protection is, on the bench, the one with the most headroom of the five.

### Consequence for the plan

Fractional internal scale (`0.5x`, `0.25x`) **goes up** in priority, but with a different target
than we thought: the **software** path, where the per-pixel cost is CPU and `set_scale` is today
ignored on purpose (`src/video/rasterizer.rs:756`).

The trampoline goes neither up nor down: it is still unmeasured, because `api_time` only counts
the method body.

## 13. PDCA 1 — reusing temporary indices (rejected)

### Plan

`DrawArrays` created a new `Vec<u32>` per batch:

```rust
let indices: Vec<u32> = (0..count).map(|i| first + i).collect();
```

Quake measured 278,297 calls in 15 s. The hypothesis was that reusing the vector's capacity
would reduce allocations and improve the time.

### Do

A `gl_indices_scratch` in `Machine` was implemented locally, reused in `DrawArrays` and
`DrawElements`. The patch did not change indices, ABI, rasterization or semantics.

### Check

The comparison was controlled, without `ZEEBX_ROM_PROFILE`, same Ryzen 7 5700U, same Quake, same
`ZEEBX_ROM_MS=15000`, and with the release profile:

| version | real time | speed |
|---|---:|---:|
| baseline | 8.8 s | 171% |
| scratch | 8.8 s | 169% |

The variation is bench noise. The patch produced no measurable gain.

### Act

Patch **rejected and removed**. It will not be committed as an optimization. The conclusion is
useful: allocating the index vector is not the dominant cost in this case; rasterization,
conversion and copying in `SwapBuffers` are bigger. The next patch should attack that path or
eliminate the index materializer entirely with a contiguous `DrawArrays`, not just reuse the
capacity.

The scan of 66 games was interrupted so as not to contaminate the A/B. It got as far as
Pac-Mania, which reached 52,830 virtual ms in 240 real s (22%); it is not a complete baseline and
must not be treated as a final result.

## 14. PDCA 2 — direct export in RGB565 words (accepted with a small gain)

### Plan

`present_gl` made the rasterizer generate RGB565 bytes and then the surface converted each pair of
bytes back to `u16`. On the software path, this was duplicated work and traffic.

### Do

`frame_rgb565_words()` was added to the rasterizer contract, implemented in the software and GPU
paths. `present_gl` now updates the surface with `load_rgb565_words()`. The last-frame vector
(`gl_last_frame_words`) also started storing RGB565 words; conversion to bytes stays only in the
paths that ask for bytes, such as `eglGetColorBuffer`.

### Check

Sequential A/B, no profile, same bench, Quake, 15,016 virtual ms per run:

| run | baseline | RGB565 words |
|---:|---:|---:|
| 1 | 9.1 s / 165% | 9.0 s / 167% |
| 2 | 9.0 s / 167% | 8.8 s / 170% |
| 3 | 8.8 s / 171% | 8.8 s / 170% |
| average | **8.97 s / 168%** | **8.87 s / 169%** |

Average gain: roughly **1.1% on the clock** and **0.6% in speed**. It is small and close to the
bench noise; it is not presented as a big leap. The core's release tests passed.

### Act

Patch kept because it removes a structurally unnecessary conversion, not because Quake promised a
big gain. The next confirmation should be on the handhelds' GPU rasterizer and in different
games. The dominant bottleneck is still `flush`/fill, not this isolated loop.

## 15. PDCA 3 — `memset` straight into guest memory (accepted)

### Plan

Rolimã had already shown `memset` as 31% of the profiled time. The previous path filled a 4 KiB
stack buffer and called `write_mem` for each block. That repeated borrow, validation and code
invalidation.

### Do

`GuestMemory::fill` now fills the region slices directly, and `DynarmicCpu::fill_mem` uses that
path. Code invalidation still happens once over the whole range. Read-only regions and unmapped
addresses still fail; tests were added.

### Check

Sequential A/B on the Ryzen 7 5700U, Rolimã, no profile, 15,048 virtual ms:

| run | baseline | `GuestMemory::fill` |
|---:|---:|---:|
| 1 | 3.7 s / 408% | 2.8 s / 534% |
| 2 | 3.2 s / 469% | 3.0 s / 495% |
| average | **3.45 s / 439%** | **2.9 s / 515%** |

Average gain of roughly **16% on the clock**. The bench variation is large, but both runs point
in the same direction and the first gain was 24%.

### Act

Patch kept. The path is covered by the memory tests and by the core's release suite. NFS, Quake
and the 66 games still need measuring to know how much the gain shows up outside Rolimã.

## 16. The GPU path on the handhelds: Mali is not AMD

### Correct hardware

- **R36S:** Rockchip **RK3326** (not RK3356), Cortex-A35, Mali-G31 MP2.
- **RG40XX-H:** Allwinner H700/A133, Cortex-A53, Mali-G31.

Both advertise GLES 3.2 in the hardware/libMali. That does not guarantee every image with
Mesa/Panfrost exposes 3.2: there are G31 drivers that expose only GLES 3.1. The core asks for
GLES 3.2 and falls back to software if the frontend refuses.

### Actual current path

In the Libretro core, the hardware flow is:

```text
retro_run
  -> context_reset / glow::Context
  -> GpuState
  -> gles_draw (batches on the host)
  -> eglSwapBuffers
       -> flush()
       -> glReadPixels (GLES reads RGBA8)
       -> conversion to RGB565
       -> CPU screen
  -> retro_video_refresh(HW_FRAME_BUFFER_VALID)
```

The last step hands the frontend's framebuffer to RetroArch, but the path before it still does a
readback to the CPU surface. On Mali, that readback can force the tiled GPU to finish the frame
and block the CPU. The laptop's AMD can hide that cost with much larger cache/bandwidth;
therefore the software benchmark on the Ryzen does not estimate the Mali cost.

Patch `64ceaa5` removed the intermediate bytes→`u16` conversion, but did **not** remove
`glReadPixels`. The next big GPU patch should be lazy readback/zero-copy:

1. in a pure 3D scene, present the frontend's FBO without reading the GPU back to the CPU;
2. only do a readback when there is 2D composition, `GetColorBuffer`, `glReadPixels`, a dump or a
   fallback;
3. preserve the 640×480 screen when the game mixes `IDisplay` with GL;
4. keep Crash Nitro Kart on the safe path, because it reads pixels back;
5. measure before/after on the R36S's libMali and on the H700 driver.

It is not safe to simply delete `present_gl`: games that draw 2D after GL need the base to be up
to date. The implementation should have a `gl_readback_pending` state and a single point that
materializes the screen before any 2D drawing/read.

### Verification still missing on the RG40XX-H

The core now logs, in `context_reset`:

```text
GL_VENDOR
GL_RENDERER
GL_VERSION
GLSL_VERSION
```

The R36S already had `Mali-G31`/GLES 3.2 observed in the physical test. On the RG40XX-H, muOS's
`video_driver="gl"` proves the kind of path, but it does not replace the log of the driver
actually loaded. The next install should capture that line and compare vendor/renderer/version
before any conclusion about Panfrost or libMali.

### First safe step on the Mali path

Before full zero-copy, the core already avoids a useless cost in GPU mode: `retro_run` does not
call `write_rgb565_into()` nor compute the CPU signature when there is an active hardware FBO.
RetroArch receives `HW_FRAME_BUFFER_VALID` and ignores the pixel pointer in that mode. This
removes the per-frame CPU copy/scan without changing 2D composition or the fallback.

This does **not** yet remove the `glReadPixels` that happens in `present_gl`; that is the next
patch, and it needs lazy readback so as not to break games that mix GL, `IDisplay` and pixel
reads.

## 17. Reference: ParaLLEl-N64 and Mupen64Plus-Next

Sources reviewed without copying code:

- `libretro/parallel-n64` at `6e4c44c`;
- `libretro/mupen64plus-libretro-nx` at `6752836`.

The N64 guest CPU is MIPS and the Zeebo is ARM, so the dynarecs' lowering is of no use. The
architecture is: fastmem by pages, block linking, granular invalidation and a differential corpus.
Zeebx already delegates that to Dynarmic; swapping it for an adapted MIPS JIT would be a project
regression.

The useful graphics lessons are more direct:

1. **The normal frame stays on the GPU.** The cores hand over `RETRO_HW_FRAME_BUFFER_VALID`;
   readback exists for guest memory, screenshots or compatibility, not for presenting every frame.
2. **On-demand readback.** GLideN64 marks the framebuffer dirty, reads only when the guest CPU
   looks, can crop to a page/band and downscales to native resolution before copying.
3. **PBO ring.** When readback is unavoidable, it uses a double/triple PBO and consumes the
   previous one; the GLES2 fallback is synchronous `glReadPixels`. Zeebx should mark valid slots
   and use a fence — the reference code is not safe to copy literally.
4. **Shadow state.** It caches enable, FBO, buffer, texture, viewport, scissor, blend, depth,
   program, attributes and uniforms. **Program/VAO/VBO done**: the three are created once and never
   change, and `submit_with` rebound them on every batch and **unbound** them at the end of each
   one — five driver calls per batch to reassert what was already in effect, and unbinding program
   and VAO is what makes a thin ARM driver revalidate more. Quake reaches 370 batches per frame.
   Now they go in once (`GpuState::enabled`), and only a recreated context invalidates them. What
   is **missing** from shadow state is the rest: `apply` still re-emits
   enable/blend/depth/texture per batch.
5. **Batch by key.** It accumulates triangles until FBO/program/textures/blend/depth/scissor
   changes or a barrier shows up. Zeebx already groups fans/strips of the same `State`, so it
   should improve the state cache before building another batcher.
6. **VBO streaming.** It uses a persistent ring when there is `bufferStorage`; otherwise an
   unsynchronized map. In Zeebx, it is only worth it with segments and fences, so as not to
   overwrite data in use on Mali.
7. **SIMD with a scalar oracle.** Angrylion keeps SSE2/NEON and scalar bit-exact. It is the right
   pattern for RGB565, textures and rasterization; it does not justify importing the RDP/RSP
   specific code.
8. **The GL thread is optional.** Mupen warns that it improves some drivers and adds latency. The
   Libretro context and the lifetime of the arguments make this a high-risk project, not a quick
   win.

There is no ready-made Mali recipe in these cores. They use capability probing and measured
quirks. That led to two immediate local fixes:

- the AArch64 handheld now asks for **GLES 3.0**, enough for VAO/FBO blit/MSAA/GLSL 300, instead
  of refusing Panfrost 3.1 for requiring 3.2;
- `GL_DEPTH_CLAMP` is no longer emitted on GLES, avoiding `GL_INVALID_ENUM` and useless per-batch
  validation in the Mali driver.

Resulting priority: lazy readback/zero-copy, shadow-state/uniform cache, PBO only for unavoidable
readback, and then NEON in the measured kernels. Vulkan/ParaLLEl-RDP and the MIPS dynarec stay
out of scope.

## 18. Later audit: fixes and a 63-ROM run

The review of the patches found three limits that the first implementation did not record:

1. `GuestMemory::fill` could change the first region and fail on the second before the JIT
   invalidation. Now the whole range is validated before any write, with an atomicity test.
2. `GL_DEPTH_CLAMP` stopped being enabled on GLES in `apply`, but was still being disabled in
   `hand_back_context`, producing the same `GL_INVALID_ENUM`. Both sides are guarded.
3. With the external Libretro FBO, drawing went to the frontend's framebuffer, but
   `bind_for_read` bound the internal FBO. `glReadPixels` and CPU composition could read a stale
   frame. At 1x without MSAA, the read now uses the same external FBO that received the drawing.

Supersampling, MSAA and wide aspect ratio together with the external FBO still need explicit
composition into the frontend's FBO; they are not declared solved by this fix.

The long run finished: 63 recognized ROMs, 60 virtual s, profile on, 1,504 real s. Slowest cases
among those that progressed: Action Hero 91%, Quake 93%, Ultimate Chess 114%, NFS Carbon 123%; the
profile makes the clock much more expensive, so these percentages are not normal-use FPS. Six
cases need triage: Bejeweled, Kingdom Hearts homebrew, Pac-Mania (22% and the 240 s cap), Prey
Evil, Ridge Racer and Opera Mini. They must not be called regressions without comparison against
the baseline and equivalent scripts.

## 19. Reference: PCSX-ReARMed, DuckStation and Flycast

Sources reviewed without copying code:

- `libretro/pcsx_rearmed` at `ff81ed1`;
- `stenzek/duckstation` at `7326f90`;
- `flyinghead/flycast` at `869038f`.

### The useful consensus

1. **The framebuffer stays on the GPU.** Flycast keeps RTT as a texture; DuckStation downloads
   only the requested rectangle; PCSX-ReARMed writes straight into the software framebuffer
   offered by Libretro. Zeebx should not read back every normal frame.
2. **Readback is a guest operation, not presentation.** When unavoidable: bounding rect/tiles,
   staging/PBO ring, fence, and waiting only when the guest CPU actually consumes. DuckStation even
   keeps a parallel software rasterizer to avoid round trips in read-heavy games.
3. **Shadow state before more batching.** DuckStation and Flycast avoid repeated program,
   texture, blend, depth, stencil, scissor and parameters. Zeebx's `GpuState::apply` still
   re-emits almost everything on every flush.
4. **Batch/arena by compatible state.** DuckStation accumulates until texture/blend/mask changes
   or a hazard; Flycast sends vertices/indices/uniforms in a per-frame arena. Zeebx already groups
   primitives of the same `State`; it should add dirty bits and safe streaming, not rewrite from
   scratch.
5. **The video thread has a bounded queue.** DuckStation uses an SPSC FIFO and at most two
   frames; Flycast applies back-pressure/deduplicates and drops stale presentation. An unbounded
   queue trades FPS for latency and RAM. In Libretro/GL, context ownership makes this high risk.
6. **Software raster in specialized blocks.** PCSX-ReARMed separates setup/texture/shade/blend
   and processes blocks of pixels with NEON; DuckStation compiles the same oracle as scalar and
   SIMD. It is the model for Zeebx's `fill_band`: correct scalar, kernels per common state, a
   differential test.
7. **Fastmem/JIT is not for copying.** DuckStation/Flycast use a virtual arena, fault-and-patch,
   block linking and adaptive invalidation. Zeebx already gets the ARM JIT and page table from
   Dynarmic; they are only worth it as a checklist and telemetry, not as a replacement.
8. **Pacing in the Libretro core does not sleep.** PCSX-ReARMed runs up to the emulated boundary
   and returns; the frontend controls the wall clock. Flycast and DuckStation use deadlines/queues
   in their own frontends. Zeebx's current `sleep` inside `retro_run` deserves a separate PDCA
   because of the risk of double throttling.

### Specific findings

- **PCSX-ReARMed:** `GET_CURRENT_SOFTWARE_FRAMEBUFFER` can remove a copy in the software
  fallback; it requires respecting pitch/format/lifetime. Its GLES GPU is legacy and is no use as
  a modern backend.
- **DuckStation:** dirty rectangles, download textures/PBO with a fence, pipeline/shader cache and
  a robust video thread. Its AVX2 rasterization path is disabled because of defects: a warning
  against assuming SIMD is automatically correct.
- **Flycast:** it really does have Mali quirks. It tests `glBlitFramebuffer` and falls back to a
  quad if the driver advertises it but fails; it uses `DEPTH24_STENCIL8` on Mali; on Vulkan it
  adds ARM-specific barriers. Zeebx already uses `DEPTH24_STENCIL8`, but it should test blit in
  the real context before trusting MSAA and internal resolution.
- **Flycast frameskip:** it reacts separately to a slow CPU and a busy GPU queue. Zeebx today
  reacts to Libretro's audio warning; backlog/GPU time telemetry would allow a better decision.

### The order that results from this comparison

1. lazy readback/zero-copy;
2. shadow state + uniform cache;
3. a real FBO blit test on the driver;
4. PBO with a fence for unavoidable reads;
5. `GET_CURRENT_SOFTWARE_FRAMEBUFFER` in the fallback;
6. NEON/SSE in the software rasterizer, in blocks;
7. a bounded video queue, only later;
8. a PDCA for the `sleep` inside `retro_run`.

### Licenses

PCSX-ReARMed and Flycast are GPL-2.0 or later/compatible per file, but dependencies vary. Current
DuckStation uses **CC-BY-NC-ND-4.0**: no code is copied or ported from it into the distributed
Zeebx. Only facts and ideas come from it, with an independent implementation. No code from these
three was copied in this round.

## 20. Instrumentation: the core's log in five levels

The plan for this round started with "measure before optimizing", and the first obstacle was that
the core **had no diagnostics channel**: the desktop wrote through scattered `eprintln!`, the game
had a separate log of its own, the Libretro core had a `log()` with a fixed level, and the scan
printed into its report. There was no way to ask for "the debug level of the CPU subsystem".

Now there is. `src/logging.rs` provides five levels — `Debug` (the "verbose/debug"), `Info`,
`Warning`, `Error`, `Fatal` — with a target (the subsystem) and a 300-line ring that the frontend
drains.

### Decisions worth recording

- **The default is `Warning`, and the filter costs one atomic read.** Whoever touches nothing
  keeps the silence they had before. The macro checks the level **before** building the text: a
  filtered debug message does not even pay for the `format!`.
- **The core stores, the frontend delivers.** The frontend's log is a variadic C callback;
  calling it from deep inside a draw would mean running frontend code in the middle of our own
  state. The ring is what separates the two — the dump happens in `retro_run`.
- **"Off" is not "I did not understand".** The first design returned the same `None` for both,
  and a `ZEEBX_LOG=banana` would silently turn logging off. Today there is a `Threshold` that
  tells them apart, and the invalid token **warns**.
- **A single vocabulary** for the core option, `config.ini`, `settings.json` and the environment
  variable. Four tables would diverge; one conversion function does not.
- **Precedence:** the environment variable is the starting point and the explicit option wins
  over it, in all three frontends.

### Where to turn it on

| Frontend | Where |
|---|---|
| Libretro core | option `zeebx_log` (System category), applies right away |
| Headless | `[system] log = warn` in `config.ini` |
| Desktop | `debug.nivel_de_log` in `settings.json` |
| Any of them | variable `ZEEBX_LOG` |

### What can already be seen

Real proof, Double Dragon, 60 frames, `ZEEBX_LOG=debug`:

```text
DEBUG session: content ...Double Dragon (Brazil) (Es,Pt).zip resolved to .../ddragonz.mod
DEBUG session: 462748 bytes read; parsing the module
DEBUG cpu: page table: 20082 of 1048576 with direct access (9 region(s))
INFO session: opened Double Dragon (Brazil) (Es,Pt).zip (class 0x0102f789) with the CPU rasterizer and 0 applet(s) installed
INFO midi: timbre table: 15051 bytes of SMF -> 47.6s of audio synthesized in 187.6ms
```

The two numbers that matter to the plan:

- **20,082 pages of 1,048,576 with direct access.** Dynarmic's table covers 1.9% of the address
  space, which is what is expected — only the mapped regions —, but it is the first number that
  says how much of the guest's access goes direct and how much goes through Rust. It serves as a
  baseline for discussing SMC and hot pages.
- **187 ms to synthesize 47.6 s of music.** It confirms, on the real-use path, what the bench
  measurement said: the MIDI cost is a load cost, and it is what explains the slow first song on
  the handheld, not the mixer.

### Side finding

The Libretro core's `log()` sends everything at level `3` (`RETRO_LOG_ERROR`), including
informational messages such as "drawing on the GPU". It predates this round and **was not
changed**: touching it changes what shows up in the log for whoever filters by severity, and it
is a separate decision. The new path does not depend on it — the core's lines go out with the
right level.

### Pending

- Android still uses Rust's `log` on its own and does not drain the log;
- the scan does not drain it (its report already carries the game's log);
- the subsystems instrumented in this first pass are session, loader, CPU, audio and MIDI. The
  rasterizer, the network and storage do not have their own lines yet.

## 21. Closing the emulator review: the addenda

Sections 17 and 19 carry the **five main reports**. This section closes what was left out:
**twelve addenda and corrections** that arrived later, and that in several cases **change the
conclusion** of the main report. Without them the review is only half done — which is how it
stayed until now.

### Corrections that change the conclusion

1. **Do not build a new fastmem, nor prioritize it.** `src/cpu/dynarmic.rs` already hands
   Dynarmic a direct table of 2²⁰ page pointers, a bitmap of executed pages and per-range
   invalidation. The second addendum is explicit: the current table **already captures the main
   gain**, and what is left is **adaptive SMC** — a page that mixes code and data leaves the table
   and goes through the callback on every read and write. If it is ever done for real, the route
   is Dynarmic's own API (`Config::fastmem`), never a signal handler of ours: the handler cannot
   allocate, lock or enter a `RefCell`.
2. **Do not create a presentation-only thread.** DuckStation **removed** dedicated presentation
   in `1c1b82ed` with the justification "Worse frame pacing" and unified render and present on the
   same thread. If Zeebx parallelizes, it is **one** thread that owns context, drawing and
   presentation, with a backpressure of 0 or 1 frame to begin with.
3. **The low-risk batching we are missing is joining adjacent strips.** Flycast merges neighboring
   *strips* of equivalent state (`PolyParam::equivalentIgnoreCullDirection`,
   `makePrimRestartIndex` with restart index `~0` and winding correction) and still emits one draw
   per group — there is no multidraw. Sorting globally by state breaks blending, translucency and
   multipass. Our batch today closes on a change of `State`, which is the same family of key; what
   we do not have is *primitive restart* to join what is adjacent.
4. **Flycast's SIMD transfers poorly.** The NEON branch of the TA parser falls back to four `u64`
   in the ARM64 build, and `FIPR`/`FTRV` in NEON is **commented out in the code itself for
   precision**. The only real use is `shop_frswap`, with `Ld4/St4`. Confirmed: the external cores'
   SIMD does **not** justify priority; it is still only worth it as a working pattern
   (scalar oracle, a kernel per measured hotspot).
5. **Our `sleep` in `retro_run` is the deviation.** PCSX-ReARMed's `pl_frame_limit`, on the
   Libretro path, **only marks the boundary**: it delivers exactly one frame and does not sleep,
   because the wall clock belongs to the frontend. Zeebx sleeps (`src/session.rs`), which adds our
   brake to the frontend's. It is not a correctness bug, it is a risk of double throttling and
   judder — and it became its own item in the pending list.
6. **`GL_ARM_shader_framebuffer_fetch` is detected and inactive** in the cores (the code itself
   forces `ext_fetch = false`). Do not count it as a gain that already exists.
7. **The binary shader cache is disabled precisely on GLES** (`EnableShadersStorage` lives under
   `#if !defined(HAVE_OPENGLES)`), so the compilation hitch is still there in the cores. For us
   this is **harmless**: the engine has **one** shader and compiles it once.

### Confirmations in our code

- **The batch guard exists.** The addendum warned that Mupen joins 256 vertices and 1024 indices
  with no apparent guard. Checked: ours closes the batch at a cap (`BATCH_VERTICES`, 16,384
  vertices) before inserting. The defect is not there.
- **Partial audio consumption is handled.** PCSX ignores the partial return of
  `audio_batch_cb`; ours keeps what was left in `pending_audio` and delivers it on the next call.
- **Audio latency is constant, not derived.** PCSX recomputes the minimum latency only when the
  frameskip type changes, and it can go stale when the interval or NTSC/PAL changes. Ours asks for
  96 ms once, and the constant does not depend on the frame policy — without the stale-value
  defect, but with the same fragility if it ever becomes a function of the policy.

### Risks that only matter if we touch these fronts

- **`libretro-common`'s GLSM**: ~64 MiB of BSS with a direct index by (program, location) and
  overflow above 1024. Do not copy; a uniform cache, if there is one, with a sparse map and a
  validity bit.
- **Mupen's `AHardwareBuffer`**: it creates with `CPU_READ_OFTEN|GPU_SAMPLED_IMAGE`, uses it as a
  render target **without** `GPU_COLOR_OUTPUT` and ignores the return of `allocate()`. If we ever
  go through `EGLImage`, ask for `GPU_COLOR_OUTPUT`, validate the allocation and fall back to PBO
  on any failure.
- **GLideN64's threaded wrapper**: grows from 10 MiB up to 200 MiB. An optional driver gain at a
  cost in RAM and latency; it is not the default for a handheld.
- **"Compatible" `N64DepthCompare`**: `glMemoryBarrier` plus one draw per triangle. It is
  correctness at an extreme cost on a tiled GPU — it serves as a warning against "per triangle"
  solutions on Mali.
- **W^X and batched I-cache flush** (`start_tcache_write`/`do_clear_cache`) only matter when the
  JIT is ours; today it is Dynarmic's.

### Mali findings we do not use yet

- **Test `glBlitFramebuffer` at runtime.** Flycast does not trust the driver's advertisement: it
  runs a real test and, when the driver lies, draws a quad. Zeebx assumes blit works — and uses it
  in the MSAA `resolve` and in reading the big frame.
- **Discard depth and stencil after the last pass.** Flycast's GLES does **not** call
  `glInvalidateFramebuffer` or `glDiscardFramebufferEXT`: this would be a **new** experiment of
  ours, not a copied recipe. On a tiled GPU this is where it can pay off.
- **`DEPTH24_STENCIL8` on Mali** — Flycast switches format because of a quirk; we already use that
  format, so it is a confirmation, not a pending item.
- **Vulkan's tile memory model**: `eTransientAttachment`, `LAZILY_ALLOCATED`, storeOp
  `eDontCare` and `eByRegion` dependencies. It is a reference for when there is Vulkan, not for
  today.
- **Do not port the Dreamcast "tile renderer"**: Flycast does not re-execute tile by tile — it
  turns the clipping into `scissor` and a shader test. It would be an enormous rewrite.
- **`SwapIntervalDetector`**: measures the vblank interval with a moving average and forces
  interval 1 when the rate is unstable, so as not to fall into an accidental 30 FPS. It is an idea
  usable in our limiter and in the frameskip decision.
- **Flycast's frameskip separates "slow CPU" from "full GPU queue".** Ours only reacts to the
  frontend's audio warning; GPU telemetry would allow the second decision.

### What does not apply

- The dynarecs' `ARMv5_ONLY` path (immediates, pools, no `IDIV`, conditional VFP) is a model of
  ARM11/v6 constraints. Our **guest** is ARM and the **host** is ARM64; the JIT is not ours.
- Lightrec is LGPL-2.1+ and `rsp_jit`'s GNU Lightning is LGPL-3.0+: importing them would raise the
  project's license set, which is `GPL-2.0-or-later`.

### Licenses, settled

| source | license | use |
|---|---|---|
| PCSX-ReARMed, Flycast | GPL-2.0-or-later in the cited files | compatible with ours; with notices preserved |
| Mupen64Plus-Next, parallel-n64 | aggregate: GPL-2.0 (core), LGPLv3 (gles2n64), MIT/LGPLv3 (rsp) | check per file before any copying |
| DuckStation | **CC-BY-NC-ND-4.0** | **no code**; only facts and ideas, with an independent implementation |
| vendored dependencies (VIXL, Swappy, GNU Lightning) | their own | some raise the set |

**Not one line of code from these emulators was copied.** What entered the project were facts,
function names for cross-checking and the engineering decisions listed above.

### The revised order

| change | front | why |
|---|---|---|
| up | GL state cache | low-risk gain, pointed out by two reviews |
| up | join adjacent batches with restart | batching we do not have yet, and low risk |
| up | test blit and discard on the real driver | without it, MSAA and reads may be lying |
| up | measured SMC | it is what is left that is real on the memory front |
| new | PDCA for the `sleep` in `retro_run` | a deviation from the Libretro contract, measured against PCSX |
| down | new fastmem | Dynarmic already delivers it |
| down | the cores' SIMD | poor transferability confirmed |
| down | presentation thread | DuckStation removed it for making pacing worse |

## 22. Splitting the clock: how much stays outside the JIT

**The number that had been missing since the start of this round.** `ARCHITECTURE.md` published
1.4 µs per API call, measured with Unicorn; the instrument of the time (`ZEEBX_ROM_PROFILE`)
measures only the method body. Now the scan report splits the clock in two halves.

### How it is measured

Dynarmic's `run` was already the place everything goes through: each API call is an exit and a
re-entry. The clock around it gives the time **inside** the JIT — guest execution plus the memory
callbacks the JIT itself calls. Everything left over in the loop is dispatch: the trampoline, the
method body, the callbacks delivered at the boundary and the session's bookkeeping.

### The mistake the first version made, and what it taught

With the clock on **every** entry, Quake went from 8.8 s to 12.3 s: **40% slower**. `Instant::now`
on this machine is a system call, not the fast `vDSO` path — 1.3 µs per read, 2.6 µs per entry,
in a game with 1.3 million entries.

The fix is to read the clock on **one entry in every 64** and multiply the sampled average by the
counter. And since the cost dropped to about 0.6%, the measurement stays **always on**: every scan
report now carries the split, without depending on someone remembering to turn it on.

### The proof that it costs nothing

Five runs of the same Quake, 15,016 virtual ms, alternating the two builds:

| build | run | real time | speed |
|---|---:|---:|---:|
| with the split | 1 | 8.9 s | 168% |
| with the split | 2 | 8.8 s | 170% |
| without the split | 1 | 8.9 s | 169% |
| without the split | 2 | 8.8 s | 171% |
| with the split | 3 | 8.9 s | 169% |

The two builds are indistinguishable. The instrument goes in.

### What it says

```text
performance:
  opened in 0.1 s, ran 8.9 real s for 15016 virtual ms (169% of console speed)
  1091 lap(s), 583 frame(s) (38 virtual fps)
  1712755541 instructions (193 million/s), 1323900 API call(s)
  5434 ms inside the JIT and 3458 ms outside (38% of the loop in dispatch); 1324767 entry(ies)
  in 20700 sample(s), 2612 ns outside the JIT per API call
```

Three readings, and all three matter:

1. **38% of the clock is outside the JIT.** Dispatch is not a detail: it is more than a third of
   Quake's work, and it is the biggest single target the tree has today.
2. **2.6 µs per call, outside the JIT.** It is twice the 1.4 µs the document published — and
   twice a number that belonged to another backend.
3. **Inside the JIT it is 317 million instructions per second**, against 193 million overall. The
   recompiler is fast; what costs is crossing the boundary.

**The caveat, stated in full:** 2.6 µs is an **upper bound** on the trampoline. Inside those
3458 ms are also the body of each method, the callbacks delivered at the boundary and the work of
the measuring setup itself. What separates body from trampoline is the API profile — and it
confirms the number by another route: with the profile on, "outside the JIT" rose to 7272 ms, and
the cost of the profile itself (1.32 million calls × about 2.6 µs) accounts for some 3.4 s of
that, which gives back the same ~3.9 s. The two instruments agree.

### What this changes in the order

**The trampoline rises to first place**, ahead of the GL state cache. It costs 38% of the clock in
the worst case in the tree; the GL state cache attacks a handful of calls per draw batch. And there
is a route to a gain that does not require redesigning the trampoline: **reducing the number of
boundaries**. Quake crosses 1.32 million times in fifteen seconds, and 845 thousand of those are
the three geometry methods — `VertexPointer`, `TexCoordPointer` and `DrawArrays`, a trio per
batch. As long as the trio remains three calls, the fixed cost is paid three times per batch.

### Pending on this front

- measure the split **on the handheld**: the desktop says 38%, and the device has another balance
  between core and memory. The instrument now travels in the report from there.
- the same report on Pac-Mania, which is the 48-million-call case: if its split is even larger,
  it stops being "the slow game" and becomes the measure of the fixed cost.

## 23. The API profile was measuring its own clock

**This is the most important correction of the round, and it invalidates earlier readings in this
very document.**

### What was wrong

The API profile times each call with an `Instant::now()`/`elapsed()` pair. On Linux this is
usually the fast `vDSO` path — tens of nanoseconds —, and that is why the measurement looked
acceptable for so long.

**On this machine it is not.** Measured, with proof in the `how_much_the_clock_costs` test:

```text
clock: 1318 ns per read, 2634 ns per pair (now+elapsed); map by (interface, slot): 15 ns per accumulation
```

**1,318 nanoseconds per clock read** — it is a system call, not `vDSO`. And the profile reads the
clock **twice per call**. So every method that showed up costing around 1.4 µs in fact cost
**zero**: the number was the instrument.

That is how Pac-Mania's `GetClipRect` showed up with 45 s for 32 million calls (1.39 µs each) and
became "the clearest hotspot in the tree". It was not. Quake's `VertexPointer`, `TexCoordPointer`
and `strcmp` were on the same list, all with ~1.5 µs of "body".

### The two fixes

1. **Sampling:** the clock is read on one call in every 64, and the average is multiplied by the
   count. The call count stays exact; only the time is estimated.
2. **Subtracting the instrument:** when the profile opens, the cost of one clock read is measured
   on this machine (4,096 reads) and subtracted from the average before estimating. A method
   cheaper than the instrument comes out at zero, which is the right answer.

With both, the profile stopped making the run more expensive: **8.9 s against 8.7 s** for the
same Quake, within the noise — before, it cost 42%.

### The true picture (Quake, 15 virtual s, software)

```text
where the time went (5463 ms in API calls):
     4951.8 ms   90.6%  IEGL11::SwapBuffers
      183.6 ms    3.4%  IGLES11::Clear
      125.1 ms    2.3%  IGLES11::DrawElements
      108.8 ms    2.0%  IGLES11::DrawArrays
       47.0 ms    0.9%  AEEHelpers::malloc
```

And the three that dominated the old list **disappeared**: `TexCoordPointer`, `VertexPointer`
and `strcmp` have no measurable body.

### What this says, and what it does not

**It says:** the real cost is in **one method**, `SwapBuffers`, and it is the presentation path —
`flush` of the triangle queue, the CPU reading the frame and conversion to RGB565. This confirms,
with the instrument fixed, what the first reading already suspected by another route: on the
software path, what dominates is rasterization, not method dispatch.

**It also says:** the cost of the three geometry methods **is not the body, it is the boundary**.
They add up to 845 thousand of Quake's 1.32 million JIT entries, at ~2.6 µs each. In other words,
the trio costs the price of the trampoline, not the price of what it does — which reaffirms the
split from section 22 and explains why the two instruments disagreed.

**It does not say:** that these absolute values add up. The buckets overlap — there are API
methods that re-enter the JIT from the inside —, so the sum (5.46 s) even exceeds the time outside
the JIT (3.4 s). **The order holds, not the absolute value.** For absolute values, what to use is
the split from section 22, which is measured with nothing inside the loop.

### Consequences for the plan

- `SwapBuffers` (presentation: `flush` + read + conversion) becomes **the** target of the software
  path, and it is where lazy readback and internal scale attack;
- the geometry trio does **not** call for body optimization: it calls for a cheaper boundary;
- Pac-Mania leaves the list of method hotspots and goes back to being what it is: a game with 48
  million calls, that is, **48 million boundaries**;
- and the rule "always measure without the profile" can be revisited: with sampling, the profile is
  cheap. What still holds is not mixing the two instruments in the same number.

## 24. Lazy readback: the GPU frame only goes to the CPU when someone needs it

Front 1 of the thirteen, and the first of all the external reviews. Done in commit `a5acf46`.

### What it was

`eglSwapBuffers` did two things at once, and they are not the same thing:

1. **paint the triangle queue** — rasterizing the frame, work the game asked for;
2. **read the frame back into CPU memory** — `glReadPixels`, conversion to RGB565 and a copy to
   the console screen.

The second exists for two consumers: **2D drawing on top** (HUD through `IDisplay`, a message box,
the Z-Wheel compositing) and whoever **asks for the pixels** (the game's `glReadPixels`, frame
recording). A pure 3D game does neither — and paid for the read on every buffer swap. On a tiled
GPU, like the Mali of both handhelds, that forces the GPU to finish and hand back the frame.

### What it is now

`present_gl` now paints the queue and marks the frame as **pending**; whoever needs the pixels
calls `materialize_gl_frame`, which does the read only once:

| who | materializes? |
|---|---|
| `IDisplay`, `IGraphics`, `IBitmap` (any 2D) | **yes**, before writing — otherwise the 2D would erase the scene |
| the game's `glReadPixels` | no: it reads the OpenGL side, which is already correct |
| window/core presenting the **GPU texture** | **no** — that is the gain |
| window presenting the **CPU screen** | yes, before converting to bytes |
| frame dump, `gl_frame`, outside frontend | yes |
| `eglGetColorBufferQUALCOMM` | no: it reads the OpenGL side |

**And only the GPU defers.** In the CPU rasterizer the "read" is an in-memory conversion: there is
no wait to save, and the frontend reads the screen every frame — deferring there would only
create the chance of it presenting a stale frame. The trait gained `frame_waits_for_gpu` for this.

### The measurement

The `both_rasterizers_draw_the_same_frame` test runs the same game on both paths and compares the
307,200 pixels. I added to it the count of buffer swaps against reads:

| game | buffer swaps | reads (CPU) | reads (GPU) |
|---|---:|---:|---:|
| Crash Bandicoot Nitro Kart 3D | 78 | 78 | **1** |
| Need for Speed Carbon | 184 | 184 | **1** |

One read on the GPU path, and it is the test's own, asking for the pixels at the end. **From 78
and 184 frame reads down to one.** Both games draw everything through OpenGL — the HUD included
—, which is the case where deferring pays in full.

### The mistake I made along the way, and how it showed up

The first version deferred **in both** rasterizers and did not fix all the consumers. The
comparison test passed with "0 of 307,200 pixels different" — and that pass was **empty**: the
software path no longer updated the screen, so it was comparing two stale screens.

The fix was twofold:

- defer only where the read costs a GPU wait (which also gives software back its old behavior,
  without touching it);
- **a guard against an empty pass** in the test: it now requires the screen to have content
  (more than 1% of pixels lit) before comparing. A test that compares two blank screens guards
  nothing, and it took a real 3D game for the defect to show up.

### Side finding: the test was already failing on Need for Speed, and nobody knew

With NFS, the two rasterizers really diverge — 11.54% of pixels beyond two channel steps, worst
case 88 steps —, and the test **fails**. I checked with an A/B: the numbers are **identical** with
and without this change, so the divergence predates it and has nothing to do with readback.

It did not show up because the suite runs without a ROM: without `ZEEBX_TEST_ROM` the test
declares itself skipped. On Crash, the same test gives 0.00% beyond two steps — the same image,
with the known rounding of the two converters. **NFS is the only one of the three with a real
divergence**, and that matches what issue #36 reports about it. It is recorded as a front of its
own: either the GPU rasterizer is drawing differently, or the software one is.

## 25. Rejected PDCA: taking the context hand-back out of the per-batch path

Front 2 of the thirteen (GL state cache). **Rejected**, and the record is worth more than the
code.

### The hypothesis, and why it looked good

`GpuState::submit_with` — which runs once per draw batch — ended by calling `hand_back_context`,
which makes **twelve GL calls** to undo what the next batch redoes: default framebuffer, null
texture, five capabilities turned off, depth mask and range, stencil mask and color mask. The
trait's own documentation already said it should happen **once per frame**, and not on every
draw.

The arithmetic looked big: with ~370 batches per frame in Quake, removing twelve calls per batch
is more than **four thousand GL calls per frame**.

### What was tried, and what each attempt measured

| attempt | where the hand-back started happening | measured |
|---|---|---|
| 1 | end of the guest pass (`Machine::end_pass`) | **2,923** hand-backs for **78** frames (Crash) |
| 2 | only when there was drawing, plus the presented frame | **0** hand-backs |

The first attempt **made it worse**: the end of the pass is more frequent than the batch — 2,923
guest passes for 78 frames, that is 37 per frame. Swapping "twelve calls per batch" for "twelve
calls per pass" with 37 passes per frame is paying more.

The second attempt could not even get the counter to light up, even though the path was complete
on paper: the mark is set at the end of `submit_with`, the hand-back is called in `present_gl`
after painting the queue, and `borrowed` is true in the test. I did not find the cause within
this round's budget, and **without understanding why the counter stays at zero I do not keep the
change**.

### The decision

**Reverted.** The code went back to what it was before this PDCA, keeping everything else. What
remains is the measurement of the two extremes — one hand-back per batch is waste, and one per
pass is worse — and the middle path (per presented frame) remains **not demonstrated**.

### What this PDCA taught

1. **"Once per frame" needs a place that happens once per frame**, and the obvious candidate (the
   end of the guest pass) is not that place: the guest pass has no relation to the presented
   frame. Measured, not assumed.
2. **A counter that does not light up is information, not silence.** Attempt 2 looked right and
   did not move the number; keeping it that way would be trading behavior for nothing, with risk
   for the desktop, where it is this hand-back that keeps the `egui` interface from disappearing.
3. **Failing with one instrument does not invalidate the target.** The GL state cache is still the
   best risk/gain pointed out by the reviews — what failed here was the *place* of the hand-back,
   not the idea of not repeating what is already on the GPU. The next attempt should attack the
   eighteen calls in `apply` with a mirror of the state, which is a different and more direct job.

## Where the round stands

| # | front | status |
|---|---|---|
| 1 | lazy readback | **done** — 78 and 184 reads became 1 |
| 2 | GL state cache | tried and **reverted**; the target remains open |
| 3 | `GET_CURRENT_SOFTWARE_FRAMEBUFFER` | pending |
| 4 | Dynarmic fastmem | do not prioritize (addenda) |
| 5 | proof of the driver's blit | **done** |
| 6 | PBO + fence | no reason after front 1 |
| 7 | `0.5x`/`0.25x` | pending |
| 8 | NEON/SSE | deprioritized |
| 9 | JIT × dispatch split | **done** — 38% outside the JIT, 2.6 µs per call |
| 10 | PDCA for the `sleep` in `retro_run` | pending |
| 11 | `lto = "fat"` | pending (needs disk space) |
| 12 | `glInvalidateFramebuffer` | pending |
| 13 | Android draining the log | pending, mechanical |

Beyond the thirteen: the **API profile** was fixed (it was measuring its own clock),
`ARCHITECTURE.md` left the Unicorn era, and the five-level log went into all three frontends.

## 26. State mirror: 80% of the state calls are gone

Front 2 of the thirteen, after the rejected PDCA of section 25. Commit `f323467`.

### What it is

A mirror, inside `GpuState`, of what is already on the GPU. **Every field starts at `None`, and
`None` means "unknown"** — that is what makes invalidation trivial: forgetting is going back to
the default, and from there everything is sent again once.

It covers the eighteen calls of [`GpuState::apply`]: viewport, scissor and its on/off, depth
clamp, depth test, function, mask and range, blending and its function, color mask, culling and
its mode, front face, stencil test, function, mask and operations.

### The mistake the first version made

The first version forgot the mirror **entirely** inside `hand_back_context` — which runs on every
batch. A mirror zeroed on every batch answers "changed" eighteen times, always. Measured:

```text
GPU: 1602 state(s) sent and 0 spared by the mirror
```

**Zero.** The mirror existed and spared nothing.

The fix is the difference between forgetting and **learning**: instead of zeroing, the hand-back
records the state **that it itself leaves** — scissor off, depth test, blending, culling and
stencil off, depth mask on, range (0,1), full stencil mask, color mask fully on. The keys it does
not touch stay **unknown**, because the other user of the context may have changed them — and
that is why forgetting everything would be safe, just useless.

The counter is what caught this: without it, the change would have looked right and been worth
nothing.

### The measurement

| game | sends before | sends now | spared |
|---|---:|---:|---:|
| Crash Bandicoot Nitro Kart 3D | 1,602 | **315** | 1,287 (**80.3%**) |
| Need for Speed Carbon | 5,593 | **1,339** | 4,254 (**76.1%**) |

The number is of **calls**, not time: `Session::state_sent_and_spared()` returns the pair, and the
comparison test of the two rasterizers prints it. It is what can be proven on the desktop, where
the driver already caches state and the clock would not separate anything.

### Where invalidation lives

Four points touch this state **outside** `apply`, and each one tells the mirror:

| point | what it does | how it tells |
|---|---|---|
| `hand_back_context` | turns capabilities off and touches masks | **learns** the state it leaves |
| `dest` (new destination) | viewport, masks, scissor off | forgets everything |
| `resolve` | turns the scissor off for the MSAA blit | forgets the scissor |
| `bind_for_read` | same, for downscaling the big frame | forgets the scissor |
| `clear` | turns the scissor off before `glClear` | forgets the scissor |

### What was left out, and why

- **Program, VAO, buffers and textures**: `submit_with` already binds them once per batch and the
  cost is lower than that of the state above. A mirror of them would require tracking the active
  texture unit, which changes within the batch itself.
- **The context hand-back**: still per batch, as it was. Moving it would require a place that
  happens once per frame, and section 25 shows why the obvious candidate does not work.

### What is left for the handheld

The count is from the desktop. **On Mali the same call costs driver validation**, and that is
where the 80% should show up on the clock. The counter travels in the report:
`Session::state_sent_and_spared()`.

## 27. Front 7 (`0.5x`/`0.25x`): the design, measured before writing

Not implemented in this round. The design below comes from reading the code, and it exists so the
next pickup starts knowing where the difficulty is — which is **not** where I assumed.

### What I assumed, and what it is

I assumed it would be necessary to touch viewport, scissor, attachment, readback and
presentation. It is less: **the software rasterizer already knows how to draw on a smaller surface
and scale up at presentation** — it is the path that exists for `EGL_QUALCOMM_surface_scale`, in
which the game declares a small surface and the device stretches it.

What makes this work:

- `GlState::surface()` returns the declared surface, clamped to the frame, and `frame_rgb565`
  **resamples** from it to the output size. With a 320×240 surface and 640×480 output the
  resampler is nearest-neighbor with step 2 — exact, and already written;
- `GlState::set_viewport` **infers the surface from the largest viewport**:
  `surface = max(x + width)`. In other words, shrinking the viewport shrinks the inferred surface
  on its own, with no other change.

### What is missing, then

1. `GlState`: a `downscale: usize` field (1, 2, 4) and a `set_downscale`;
2. `set_viewport` and `set_scissor`: divide the coordinate by `downscale` before storing — and the
   inferred surface comes out reduced along with it;
3. `read_rect` (`glReadPixels`): the game's request comes in console pixels and the surface is
   reduced; the rectangle has to be mapped and each reduced pixel **replicated**, because the
   caller writes `width * height` pixels into guest memory;
4. the core option: today `zeebx_resolucao_interna` is `1|2|3|4` and only applies on the GPU. It
   needs to accept `0.5` and `0.25`, and send them to the **software** path — on the GPU another
   mechanism does the reducing, and the GPU ignores it;
5. `clear`: check whether it clears the whole frame or only the surface. Apparently the whole
   frame, which is harmless — presentation only reads the surface region.

### Where to measure

**On the desktop, without depending on the handheld**: the scan and the API profile run on the
**software** rasterizer. With the API profile fixed (section 23), the cost of `SwapBuffers` — which
in software is the queue `flush`, that is, rasterization — comes out directly. Reducing the area to
a quarter should show up there, in the same instrument that already showed `SwapBuffers` with
90.6% of method time.

### The risk

HUD and alignment: any drawing the game does **outside** the declared viewport (a HUD in screen
coordinates without its own `glViewport`) would fall outside the inferred surface. It is the same
risk that already exists today for `surface_scale`, and the mitigation is the same: the reduction
is opt-in, and the default stays 1×.

### The order among the remaining fronts

| front | why in this order |
|---|---|
| 7 `0.5x`/`0.25x` | measured on the desktop, attacks `SwapBuffers`, which is 90% of method time |
| 10 PDCA for the `sleep` | measured on the desktop, Libretro contract |
| 12 `glInvalidateFramebuffer` | only pays off on Mali, and is not measured here |
| 3 `GET_CURRENT_SOFTWARE_FRAMEBUFFER` | only pays off in a software-framebuffer frontend |
| 13 Android draining the log | mechanical, and the Android build is not verified on this machine |
| 11 `lto = "fat"` | needs disk space: 3.4 GB free and fat LTO recompiles the dependencies |

## 28. Fractional internal scale: `0.5x` and `0.25x` in the software rasterizer

Front 7, done in the commit following this section. The design in section 27 got the diagnosis
right: the path already existed, and little was missing.

### What was done

1. `GlState` gained `downscale` (1, 2 or 4) and `set_downscale`, which **forgets the surface** and
   resets viewport and scissor — the stored coordinates were for the old size.
2. `set_viewport` and `set_scissor` divide the coordinate by `downscale` before storing. Since the
   surface is **inferred from the largest viewport**, it comes out reduced on its own: there was no
   second change to keep up to date.
3. `read_rect` (the game's `glReadPixels`) maps the console rectangle onto the reduced surface. The
   caller writes `width × height` pixels into guest memory, and sampling by division already
   delivers exactly that — each pixel read stands for `downscale` console pixels.
4. The `zeebx_resolucao_interna` option now accepts `0.5` and `0.25`, besides `1..4`. **One number,
   two mechanisms:** below 1 the processor does the reducing, above 1 the GPU does the enlarging,
   and each uses what suits it.

### The measurement

Quake, 15,016 virtual ms, software rasterizer, with the API profile on (which is now cheap and
honest — see section 23):

| reduction | real time | speed | `SwapBuffers` |
|---|---:|---:|---:|
| 1× (native) | 8.9 s | 168% | 4,867 ms |
| **1/2** (320×240) | **6.9 s** | **216%** | 3,176 ms |
| **1/4** (160×120) | **6.0 s** | **249%** | 2,447 ms |

- `1/2`: **22% less real time**, 29% more speed;
- `1/4`: **33% less real time**, 48% more speed.

`SwapBuffers` does not fall in proportion to the area (1/4 of the area, 50% of the cost) because
it also carries reading the frame and converting to RGB565, which stay at 640×480. In other
words: what is left after the reduction is, in good part, the presentation path — the same one
front 1 attacked on the GPU side.

### Checking the image

The drawing stays the same, only smaller. Comparing the 4 s report at both ends:

```text
native:  screen: 177 color(s), dominant 0x220a
1/4:     screen: 177 color(s), dominant 0x220a
```

Same color count and same dominant color. A reduction that broke the drawing — HUD out of place,
surface not discovered, a skewed read — would show up here as a drop in colors or a single-color
screen.

### What is on record

- **Only the processor reduces.** On the GPU, 640×480 does not saturate the Mali, and reducing
  would spoil the image without gaining anything. The option says so in its labels, and the code
  silently refuses on the GPU side.
- **The reduction is the most direct handheld lever the round found**: on a weak device, 33% of
  real time is the difference between 30 and 45 frames per second. It is the first thing to turn
  on there — and the `Handheld` profile should start offering it.
- The default stays 1×: whoever asks for nothing loses no sharpness.

## 29. Front 10 closed without changing code: the speed brake delivers 1×

The front asked for a PDCA of the `sleep` inside `retro_run`, because the PCSX-ReARMed review
showed that, on the Libretro path, **the wall clock belongs to the frontend** — its
`pl_frame_limit` only marks the emulated boundary and does not sleep. The suspicion was double
braking: our `sleep` added to the frontend's.

### How it was measured

The `the_core_abi_runs_a_rom` test runs `retro_run` in a tight loop, with a fake frontend that
**waits for neither vsync nor audio**. There our `sleep` is the only brake there is — which is
exactly what we want to isolate. I added a pace measurement to the test:

```text
pace: 120 frame(s) — virtual 1983 ms in real 1985 ms (100% of speed),
       per frame: median 16.59 ms, p95 17.62 ms, worst 18.60 ms
```

### What the number says

1. **100% of speed.** 1,983 ms of virtual time in 1,985 ms of real time, without the frontend
   helping at all. The brake hits the target.
2. **Median 16.59 ms** per frame, against the 16.67 ms of a frame at 60 Hz — a tenth of a
   millisecond of systematic error.
3. **p95 17.62 ms and worst 18.60 ms.** The irregularity stays below two milliseconds, or 12% of
   a frame. It is not a source of judder by itself.

### Why double braking does not happen

The `sleep` is a **floor, not an extra brake**. It only sleeps for as much as the virtual clock is
ahead of the real one (`ahead_ms`), and that lead is *net*: if the frontend already held the call
for 16 ms, virtual time advanced along with it and the lead is negative — the `sleep` does not
happen. A frontend that waits for vsync is not punished twice; one that does not wait is held back
by us.

### Conclusion

**Nothing was changed.** The front closes with the measurement the front itself asked for, and the
test keeps a pace proof that did not exist before: any future change in the core's loop that
spoils the cadence shows up there as a speed deviation or as a p95 above two milliseconds.

## 30. Front 3: the frame goes straight into the buffer the frontend lends

A front that seemed to depend on a device and does not:
`RETRO_ENVIRONMENT_GET_CURRENT_SOFTWARE_FRAMEBUFFER` can be verified on the desktop, because the
core's test has a fake frontend, and it is the one that lends the buffer.

### What changed

Before, the core assembled the frame in a vector of its own and the frontend copied it — 600 KB
per frame. Now the core **asks for the buffer** and writes where the frame is going to live,
handing the lent pointer to `retro_video_refresh`, as `libretro.h` requires (with no offset).

Three precautions, all checked:

1. **The row pitch is respected.** `Framebuffer::write_rgb565_with_pitch` writes row by row at the
   pitch the frontend gave, and whatever is left at the end of each row stays as it was.
2. **Format and size are checked before writing.** `libretro.h` says the frontend may return
   another format; writing RGB565 into an XRGB8888 buffer would give a plausible and false image.
   Any refusal falls back to the usual path.
3. **The pointer is only valid inside the call**, and that is why the request and the use stay in
   the same place, without keeping anything between frames.

### The proof

The suite's fake frontend now lends a buffer with **pitch slack** (64 bytes) — on purpose: with a
pitch equal to the width, code that ignored `pitch` would get it right by luck, and the slack is
what makes the image come out skewed if it is ignored. The test checks that the buffer **did not
stay blank**, that is, that the core drew into it. And the pace measurement stays in the same
test:

```text
pace: 120 frame(s) — virtual 1983 ms in real 1984 ms (100% of speed),
       per frame: median 16.86 ms, p95 17.87 ms, worst 17.97 ms
```

### What this pays

One frame copy fewer per frame, on the software path. It is not the path of the two handhelds
(there it is the GPU), but it is the path of every frontend that uses a software framebuffer —
RetroArch with a software video driver, other frontends, and the suite itself.

### A defect of mine that the suite caught

The test of the log's drop counter (`the_drop_counter_shows_up_and_resets`) failed when running
the whole suite: it wrote at level `Info` without pinning the active level, and depended on what
the previous test had left. It passed until the order of the tests changed — which is what
happened when this round added tests. Fixed by pinning the level.

## 31. End of the round: the thirteen fronts, each with its outcome

### Front 13 — Android drains the log (done and verified)

The destination was missing: the desktop prints to `stderr`, the core sends through `retro_log`,
headless prints, and Android had nowhere to go. Now it drains into `log`, which `android_logger`
already installs, with the severity preserved — a warning from the core goes out as `WARN` in
`logcat`, which makes `adb logcat *:W` show what matters.

**The verification was the fork's Android CI**, triggered by hand (`gh workflow run android.yml`).
It does not run on its own on push: the repository's only automatic trigger is the tag. It is what
verified this front — and, before that, **it caught a regression of mine that this host has no way
of seeing**.

### The regression the Android CI caught

`DebugView` stopped being `Copy` when it gained the text field for the log level, and the Android
frontend does `let debug = self.settings.debug;` — a move out of a type that is no longer `Copy`.
The `zeebx-android` package **does not compile outside an Android target** (`ndk-sys` refuses:
*"only supports compiling for Android"*), so `cargo check`, `cargo test` and `cargo build` on the
desktop do not see that error. It stayed in the repository since the instrumentation and only
showed up when someone compiled for Android.

Fix: `clone()` at the point of the move — the same thing that had already been done on the
desktop, and on the same kind of struct (half a dozen fields, once per draw of the debug panel).

**The lesson holds for the whole round:** the one frontend the local suite does not reach is the
one that most needs CI, and that CI is manual (`workflow_dispatch`). Whoever touches
`ui::settings` has to trigger `android.yml` by hand — nothing warns.

### Front 12 — tile discard (delivered as an experiment, **not measured**)

New option, `zeebx_descarte_de_tiles`, **off by default**. When on, the GPU rasterizer tells the
driver that depth and stencil can be thrown away after the frame — which on a tiled GPU avoids
writing both attachments back to memory.

**It is not a measured gain, and it is not counted as one.** Two reasons for shipping it off:

1. a game that does **not** clear depth from one frame to the next counts on it — the console is a
   real framebuffer, and there depth persists;
2. the gain is Mali's, where I cannot measure.

What the delivery does is **make the test possible**: the option exists, it is labeled as
experimental, and the first thing to do on the device is turn it on and look at the image and the
clock.

### Front 11 — `lto = "fat"` (closed by decision, with the reason)

It is not worth the price, and the reason can be measured without running anything:

- the hot code **is not Rust from another crate**. The JIT is C++ (Dynarmic, beyond the reach of
  Rust's LTO) and the software rasterizer lives in `zeebx` itself;
- `codegen-units = 1` is already on, so inlining within the crate is already at its maximum;
- `thin` already inlines what is small across crates;
- the cost is a fat-LTO rebuild, which recompiles **all** dependencies, with 3.4 GB free.

It is recorded as the thing to test when there is disk space — and not as a forgotten pending item.

### Front 6 — PBO with a fence (no reason after front 1)

The PBO exists to hide the latency of `glReadPixels`. After front 1, the GPU path does **one** read
per session, not one per frame: there is no latency to hide. Closed.

### Extras from the studies, implemented in this round

- **`GET_CURRENT_SOFTWARE_FRAMEBUFFER`** (from PCSX-ReARMed) — front 3, done and proven.
- **Handheld profile turning on `0.5x`** — the performance item the profile was missing. It is the
  recommendation from section 28, now as a factory default: whoever picks Handheld gets the
  reduction that measured 22% less real time, and whoever does not like the blockier image picks
  the option by hand.
- **Driver blit test** (from Flycast) — front 5.
- **State mirror** (from DuckStation and Flycast) — front 2.
- **Lazy readback** (from Flycast and DuckStation) — front 1.

### The scoreboard of the thirteen

| outcome | fronts |
|---|---|
| done and verified | 1, 2, 3, 5, 7, 9, 10, 13 |
| delivered as an experiment, not measured | 12 |
| closed by decision, with a reason | 11 |
| no reason after another front | 6 |
| deprioritized by a recorded decision | 4, 8 |

**Eight of the thirteen with a measurement.** The remaining five have a written outcome, and were
not left open by forgetfulness: three depend on a device or on disk space, and two were
deprioritized by the reviews' own addenda.

### What is really still pending

1. **Measure on the device** — `0.5x` in the profile, tile discard, the JIT × dispatch split and
   the frame-read count. The instrument travels in all of them (`Session::gl_frame_readbacks`,
   `Session::state_sent_and_spared`, and the scan report).
2. **Triage of the six cases from the scan** — still not done.
3. **PR from `perf-v0.3.0` to `development`** — the branch is on the fork and there is no open PR.
4. **`lto = "fat"`** when there is disk space.

## 32. The scan is not deterministic — and the acceptance criterion rests on it

Acceptance criterion 2 of this round says: *"the scan of the 62 games does not regress
behavior"*. When running that criterion for the first time with the patches applied, the result
was **32 of 63 ROMs with a difference** — and before attributing that to the patches, I went to
check whether the scan is reproducible.

**It is not.**

### The measurement

The same binary, the same ROM (Action Hero 3D), two reports in a row:

| | laps | instructions | API calls | virtual time |
|---|---:|---:|---:|---:|
| run A | 216 | 1,052,233 | 46,145 | 6,002 ms |
| run B | 216 | **1,052,127** | **46,137** | 6,002 ms |

**Same number of laps, same virtual time, and 106 instructions and 8 calls of difference.** The
full report differs in **48 lines**, and the pattern of the differences is unmistakable: the same
events with heap addresses shifted by 64 bytes —

```text
-   OpenFile "a3d_sound_effect_04spf.wav"  (0x8c0d8 0x1 0xf0003008) -> 805311000
+   OpenFile "a3d_sound_effect_04spf.wav"  (0x8c0d8 0x1 0xf0003008) -> 805310936
```

One allocation fewer at the start shifts everything that comes after. Against the baseline, this
becomes 814 lines removed and 740 added — which the test reads as a behavior regression.

### What this means

1. **Criterion 2 cannot be enforced today.** It flags noise as a regression, and a test that goes
   red on its own guards nothing — the same lesson as `both_rasterizers_draw_the_same_frame`, which
   was already failing on Need for Speed and nobody knew because CI has no ROMs.
2. **The "bit-for-bit reproducible" frame has an undocumented exception.** `ARCHITECTURE.md`
   states that two identical runs draw the same pixels — and they do, because the drawing is a
   function of the state; but the **guest state** is no longer the same across two runs, and that
   is what the extra instructions show.

### What has already been ruled out as a cause

- **`aee_GetRand`**: it is an LCG with a fixed seed (`0x1234_5678`) and goes into the save state.
  Deterministic.
- **`aee_GetTimeMS` / `aee_GetUpTimeMS` / `aee_GetSeconds`**: they read the **virtual** clock,
  which comes from the instructions. Deterministic.
- **Host `HashMap` order reaching the guest**: `timers` and `pending_calls` are `Vec` (insertion
  order); `threads` and `resume_callbacks` are only accessed by key. `call_log` iterates a
  `HashMap`, but it is sorted before going into the report.

### The next step, concretely

The divergence is **106 instructions in 1.05 million** with the same 216 laps. The path is a
binary search: compare, between two runs, the *number of instructions per lap* and find the first
lap where the two numbers part; from there back, the event that decided differently. The
instrument exists — `Machine::advance` already counts laps, and `instructions()` already counts
instructions.

A natural suspect, and not checked: **clock advance on waiting** (`note_spin`, in
`machine/time.rs`), which decides *when* a loop lap is a wait and advances the clock. It is the
only piece of the emulator that looks at a loop of calls instead of at a single call, and a
threshold that depended on something outside the guest state would explain exactly a small, rare
deviation.

### What was decided

Criterion 2 is **suspended**, not met: not because the patches regressed — nothing indicates
that, and the differences are in addresses — but because the ruler does not measure what it
promises. Before demanding "did not regress" of a scan, it has to be reproducible, or the
comparison has to ignore the fields that carry addresses.

What **can** be enforced today, and was checked in this round: the **status** of each ROM matches
the baseline — 63 ROMs, and the categories are the same (56 run, 3 do not create the applet, 2
crash in the loop, 2 finish on their own). The difference is in the event log, not in the outcome.

## 33. The scan is deterministic — what is not is the extraction cache

A continuation of section 32, and the result is better than it said. **The emulator is
deterministic.** What changes the guest's execution is the **state of the extraction cache**:
cold (extracting now) versus warm (reusing the extraction that is already there).

### The measurements

| run | cache | instructions | API calls |
|---|---|---:|---:|
| warm 1 | reused | 760,815 | 46,137 |
| warm 2 | reused | **760,815** | 46,137 |
| warm 3 | reused | **760,815** | 46,137 |
| cold | freshly extracted | **760,921** | 46,145 |

Three runs with a warm cache give **the same number of instructions, digit for digit** — 760,815
in all three. The cold cache gives 106 more instructions, one more `CreateInstance` call and the
heap addresses shifted by 64 bytes.

**Determinism, then, stands:** the same cache state gives the same result. What was not in place
was control of the variable.

### The two time lines

Between two warm runs, the report differs in **six lines** — and all six are from the
performance block:

```text
-  ran 0.8 real s for 6002 virtual ms (754% of console speed)
+  ran 0.8 real s for 6002 virtual ms (757% of console speed)
-  1052127 instructions (1323430/s), 46137 API call(s)
+  1052127 instructions (1328443/s), 46137 API call(s)
```

They are wall clock, and have no business in a file that exists to record behavior. The compared
summary (`Report::summary`) already excludes them — what keeps them is the full report, which goes
to `ZEEBX_ROM_OUT` and is **not** the one compared. It is right as it is; what was missing was
control of the cache.

### Why the baseline flagged 32 of 63

The corpus run was done with a **mixed** cache: 37 entries already existed (warm, from earlier
runs) and the rest was extracted on the spot (cold). Each ROM was measured in one of the two
states, and the baseline was recorded in one of them. The result is 814 lines removed and 740
added — which the test reads as a regression, and it is not: it is the same execution with the
heap shifted.

### What to do before trusting the comparison

1. **Warm the cache before comparing.** Run the corpus once without `ZEEBX_ROM_BASE` (which also
   records what is missing), and only then run against the baseline. The first pass is the
   warm-up; the second is the measurement.
2. **Always record the baseline under the same condition**, and say which one. The one in
   `docs/scan/` was recorded warm or cold — it is not stated, and that is what is unknown today.

It is recorded in the scan procedure, which is where whoever repeats it will look.

### The fidelity defect that remains, and that is real

**The first game opened behaves differently from the following ones.** This is not measurement
noise: it is the guest executing 106 more instructions because of something that only exists on
the host. The suspects, in order:

1. **`.zeebx-pacote`, the manifest, inside the game's root.** It is a file of *ours*, written in
   the folder the guest sees as its root, and directory enumeration **does not hide it**
   (`machine/file.rs` lists, sorts and deduplicates, with no filter). A game that enumerates its
   own folder sees a file that does not exist on the console. If it is present in one condition and
   absent in the other, the difference of 106 instructions and one `CreateInstance` is explained.
2. **Extraction write order**: it would change the order of `read_dir`, but the enumeration sorts
   before handing over — so it cannot be this.

The way to close it: open the same game twice with the cache forced into each state and compare
the trace of the first calls (`ZEEBX_ROM_TRACE`), which shows where the two runs part. And,
independently of that, **hiding the manifest from the guest is an overdue fix**: it is a host file
in a folder the game considers its own.

### The acceptance criterion, with the corrected verdict

It is not suspended because the emulator is non-deterministic — it is not. It is **pending
control of the variable**: running the corpus with the cache in a single state. With that, the
comparison can be enforced again, and the differences that remain will be real regressions.

### The manifest hypothesis was tested — and rejected

`.zeebx-pacote` was suspect number one: a file of ours in the folder the guest sees as its root,
with no filter in directory enumeration. It was hidden from the guest (`machine/file.rs` now
filters it out) and the cache experiment was repeated:

| run | cache | instructions |
|---|---|---:|
| cold | freshly extracted | **760,921** |
| warm 1 | reused | 760,815 |
| warm 2 | reused | 760,815 |

**The difference is still the same — 106 instructions, 54 lines.** The manifest is not the cause.

The filter **stays**, because it is an overdue fix for another reason: the game must not see in its
own folder a file the console does not have. But it does not explain what this section is
investigating.

### The new suspect, and what supports it

**Lazy reading from the package.** There are two possible paths for the guest to read a file: from
the extracted tree (warm cache) or straight from the `.zip`, through the `IUnzipAStream` route —
the collection itself has that route, and there is code that extracts **only the requested file**
instead of the whole package. If the guest receives a stream of a different type depending on
whether the cache is cold or warm, the path the game takes changes — and that is exactly what 106
instructions and one extra `CreateInstance` look like.

The way to close it: open the same game with `ZEEBX_ROM_TRACE=1` in both cache states and compare
the trace of the first calls — the point where the two part names the mechanism.

### The criterion, revised again

The part that **depends on a fix to the procedure** still holds and is already in
`docs/implementation/11-compatibility.md`: warm the cache before comparing. The part that depends
on a fix to the **code** remains open, and now with a suspect of a different nature — not a visible
file, but a **different read path**.

## 34. The extras from the studies: one by one, with the outcome

So as to leave no extra in ambiguity, the outcome of each one.

| extra | coming from | outcome |
|---|---|---|
| lazy readback | Flycast, DuckStation | **done** — 78 and 184 reads became 1 |
| state mirror | DuckStation, Flycast | **done** — 80% and 76% of the state calls |
| buffer lent by the frontend | PCSX-ReARMed | **done** — front 3 |
| proof of the driver's blit | Flycast | **done** — front 5 |
| `0.5x` scale in the Handheld profile | Flycast/DuckStation (resolution) | **done** — and measured at 22% |
| coalescing adjacent strips (primitive restart) | Flycast | **no gain for us**: our batch already joins **consecutive draws of the same state** into a single call. Flycast's trick joins strips **within** a draw, and the Zeebo guest emits one `DrawArrays` per strip — which already lands in the same batch. There is nothing to join |
| frameskip reacting to a "full GPU queue" | Flycast | **needs GPU telemetry**, which we do not have. It stays as a lead |
| presentation on a separate thread | DuckStation (and its rollback) | **do not do it** — the Libretro core delivers one frame per `retro_run`, and the desktop paints after our pass; the thread would cost synchronization without taking work off the path |
| PBO with a fence | DuckStation | **closed** — no reason after front 1 |
| binary shader cache | Mupen64Plus-Next | **does not apply** — the engine has one shader |
| separate GLES 2 path | PCSX-ReARMed | **compatibility, not performance**; left for when there is a device that needs it |
| `AHardwareBuffer`/`EGLImage` | Mupen64Plus-Next | **Android and memory coherence**; today we have no zero-copy path that justifies it |
| NEON in the wide loops | PCSX-ReARMed, Flycast | **deprioritized by the addenda** — poor transferability confirmed |
| our own fastmem / dynarec | Flycast, DuckStation, PCSX | **deprioritized by the addenda** — Dynarmic already provides the page table, and what is left is measured SMC |

### What the RG40XX-H (muOS) logs have already proven

Measured on the device, on 2026-09-24, with the core log at `depuracao`:

```text
Zeebx: GL real vendor=ARM; renderer=Mali-G31; version=OpenGL ES 3.2 v1.r20p0-01rel0.…
       GLSL=OpenGL ES GLSL ES 3.20
Zeebx: the frontend accepted hardware rendering (OpenGL 3.0); drawing now happens on the GPU
Geometry: 640x480, Aspect: 1.333
Set video size to: 640x480
```

1. **The driver is libMali r20p0, not Panfrost** — that is the verification this section asked
   for and that did not exist. With it, the conclusions below stop being assumptions.
2. **The blit proof passed.** There is no `glBlitFramebuffer` warning in the log: on this driver,
   antialiasing and internal resolution were **not** turned off by front 5.
3. **The context that reaches the core is GL 3.0**, even though the driver advertises ES 3.2. It
   is noted as a cost suspect: a lower context tends to close off the driver's fast paths.
4. **The device's screen is 640×480**, and RetroArch presents at that size: there is no scaling
   involved, so what is measured there is pure rasterization.

And what the user measured by hand: **halving the resolution and drawing on the processor was
faster than the GPU path** with the same options. That is the difference the program/VAO/VBO cache
above came to attack, and the comparison on the device is set up (two `.so` files in the `ports`
folder, and `test_zeebx.sh` accepts which one to use).
