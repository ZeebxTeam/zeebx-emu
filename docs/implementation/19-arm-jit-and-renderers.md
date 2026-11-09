# 19 — ARM JIT and 3D renderers

## The symptom

Some 3D games stayed between 10 and 15 FPS even when the host's work was small. The case that
revealed the cause was the Kingdom Hearts intro: it loads `swv21brew.mod`, a Superscape extension
that implements its own RGB565/depth rasterizer in ARM. It does not go through our EGL/OpenGL ES;
every pixel of the scene is a loop of guest instructions.

The profile of the intro showed that the problem was not the BREW loop:

| Interval | Timers/turns | ARM instructions |
|---|---:|---:|
| ~2 virtual s | 50 timers, 150 turns | ~25 million |
| ~8 virtual s | 495 turns | ~762 million |

Timers, signals and callbacks did not have the volume to explain the drop. The CPU ran on
Unicorn's TCG, which preserves compatibility well but was not fast enough for that rasterizer
written in ARM.

## What was not changed

Trading performance for changing the game's timing would hide the defect and break physics,
animation and input. So the JIT uses the same `Machine` and the same session sequence:

```text
advance → expired timers → timer callback → signals → pending callbacks
```

The virtual clock also stays the same:

```text
utime = clock_us + instructions / 528
```

`528` is the frequency, in MHz, of the MSM7201A's ARM11. The idle skip is still limited to one
vblank; it only represents time in which the console would be waiting. The JIT reduces host wall
time, it does not create virtual time or fire extra callbacks.

## Dynarmic backend

`cpu/dynarmic.rs` implements the same `CpuBackend` as Unicorn, now with Dynarmic A32 ARMv6K. It is
the backend of the normal graphical session; `zeebx bench file.zip --seconds=N` keeps a windowless
bench to measure the whole ROM.

The contract with the rest of the emulator is preserved:

- registers, memory and the instruction counter are still accessed through `CpuBackend`;
- the BREW vtables (`0xf0000000..0xf0ffffff`) and the sentinel return are still addresses with no
  code; the JIT's fetch stops there so the Rust dispatcher can serve the API;
- the stop at the vtable does not count as an executed ARM instruction, and the PC goes back to
  the address Unicorn would expose;
- the CPSR starts in user mode (`0x10`), as in the Zeebo applet;
- `SVC #0xAB` serves `SYS_WRITEC` and `SYS_WRITE0`, keeping Peggle's and Zuma's semihosting.

## Code and surface coherence

The JIT may reuse a compiled block. When the guest or a host API writes a page that has already
been fetched as code, the block has to be invalidated before the next entry. The backend tracks
only executed pages and invalidates only those pages; invalidating on any write would be
disastrous, since an RGB565 renderer writes millions of pixels per frame.

The bitmap/EGL buffers stay in the guest's memory space. Dynarmic has the same write watch as
Unicorn (`watch_dirty`): the guest's write callback sets the range's flag, a host write does not.
An envelope with the smallest interval that contains all the ranges leaves an ordinary write — the
vast majority — at two comparisons.

For a while the watch always answered "dirty", which looked merely conservative and was not:

- **It cost the whole game.** Every drawing call imported every surface. In Pac-Mania, 100
  thousand `IIMAGE_Draw` added up to 22 s just reading untouched buffers; 8 virtual s did not
  finish in a minute. With the watch, they take 6.3 s.
- **It hid a correctness bug.** "Always dirty" is only safe if the guest's buffer is never behind
  the host's copy, and it was: a new bitmap at the address of a freed one inherited the buffer
  with the dead one's pixels (see `inherited_dibs` in the 2D video doc). Tekken 2's letters came
  out as blocks only on this engine.

## Interworking: bit 0 of the address

The dispatcher resumes the guest at `lr`, and the `lr` of a call made from Thumb code has bit 0
set. Unicorn handles this in `emu_start`; Dynarmic does not, and `run` has to translate: bit 0 set
sets the `T` of the `CPSR` and leaves the address. Without this Zenonia, which is Thumb, came back
from every API one byte ahead, and the core aborted with `Unhandled instruction 0xF8F9F5F0` — half
of a `bl` with half of the next one.

## Validation done

The first frame of Kingdom Hearts at 2 virtual s was produced by both cores, and the BMPs have the
same SHA-256:

```text
d5be7b5c0589e5d8af8bb2d0d8652d745449262d65b34675133384136566b4bc
```

On the same development machine, the Dynarmic bench ran:

| Load | Virtual time | Real time | Speed |
|---|---:|---:|---:|
| Kingdom Hearts | 2.001 s | 0.305–0.331 s | 604–657% |
| Kingdom Hearts | 8.004 s | 3.555 s | 225% |

The 8 s stretch was about 762 million instructions, at 214.5 MIPS. The headroom above 100% is
intentional: the session applies the speed limiter against the real clock, as it did before.

Besides the CPU unit tests (execution, stop at an API, return and semihosting), the full suite must
pass with:

```bash
cargo test --release
```

## How to investigate a visual regression

1. Reproduce at the same virtual instant, preferably with `--keys` and a BMP dump.
2. Compare Unicorn and Dynarmic, not just FPS: an identical hash proves the pixel state at that
   instant; a different hash calls for capturing the PC/routine that writes the surface.
3. If the difference only appears after switching menus, first check code invalidation in RAM and
   reads of memory updated by the host; do not compensate by touching timers or `utime`.
4. Keep the equivalence before optimizing surface synchronization.

The next piece of work is to extend these tests to interactive scripts for CNK3D, Tekken and
Kingdom Hearts, especially menus and option screens, where code and mutable tables are more common.

## 3D internal resolution (upscale)

With the GPU rasterizer on, the graphics tab offers **internal resolution from 1x to 6x**
(`graphics.resolucao_interna`). The game still sees 640×480; only the color and depth attachment
of the `GpuState` grows.

- **Drawing.** `dest()` creates the attachment with `size × scale`, and `apply()` multiplies the
  viewport by the factor, after converting it to counted from the top (`top_down_viewport()`; the
  `y` of `glViewport` counts from the bottom, see [06](06-video-3d.md#the-surface)). Everything the game passes in pixels — viewport, `draw_texture`, the quad of
  `import_rgb565_changes` — stays in console pixels and is scaled only when it goes to the GPU.
- **Reading.** `bind_for_read()` reduces the large frame on the GPU (`glBlitFramebuffer` with a
  linear filter) to a console-sized framebuffer before any read: `frame_rgb565` (the
  `GetColorBufferQUALCOMM` and the copy to the screen) and `read_rect` (`glReadPixels`). Reading
  the large frame would mean moving the square of the factor in bytes only to throw almost all of
  it away.
- **Window.** `Machine::gpu_frame()` returns the large texture **only when the screen is exactly
  what the last `eglSwapBuffers` put there**: the screen's write count is stored in `present_gl`,
  and any 2D drawing after that changes it. In that case — a HUD through `IDisplay`, a message box,
  the Z-Wheel, which composes the 3D on the CPU — the window gets the 640×480 screen. The
  `GlPainter` draws the rasterizer's texture directly, in the same context egui uses, with the
  surface's clip.
- **Ceiling.** The factor is limited by the GPU's `GL_MAX_TEXTURE_SIZE`/`GL_MAX_RENDERBUFFER_SIZE`.
- **Without a window.** `zeebx session <zip> --gpu --scale=N --dump=…` also writes `*.grande.bmp`
  and says whether the window would show the large frame. Measured on Crash Nitro Kart: 20 virtual
  s in ~2.9 real s at scales 1 and 3, and the models' outlines come out smooth at 1920×1440.

The software rasterizer ignores the option: the cost would grow with the square of the factor. 2D
games gain nothing, because the image already comes out ready at 640×480.

### The front: the presented frame

The GPU destination is a single buffer, and the window read it live — as the large texture, or
through the CPU screen on the deferred read. But a window turn can end in the middle of the next
frame, when its deadline expires: the window took the destination after the `glClear` and before
the draws. Ultimate Chess 3D, which re-sends texture rows (`glTexSubImage2D`) every frame and so
rarely ends the turn at the swap, flickered between the scene and black at 3x, and at 1x, where the
CPU screen reads the frame, it stayed black. Measured with the shader painting green and the clear
red: right after each draw, the destination was all green, and the screen read came back red.

On the device `eglSwapBuffers` swaps the front with the back. Here, the first write after a swap —
`glClear`, a batch, the imported 2D — first copies the destination to the **front**
(`GpuState::protect_front`), and until the next swap whoever presents reads the front: the large
texture, the copy to the CPU screen and the screenshots' large frame. The game's `glReadPixels`
still reads the destination, which is the back. The copy is on demand: nothing is copied at the
swap, because the destination is the presented frame, and a turn that ends at the swap pays no
copy at all.

The swap is marked in `Rasterizer::end_frame`, which only `eglSwapBuffers` calls
(`GpuState::swap`), and not in `flush_draw`. The latter is also called by the save state, which
rewind takes every second in the middle of the frame. Marked there, the swap counted as done right
after the `glClear`: the front was dropped, the window showed the freshly cleared destination, and
the chess flickered black once a second, every 61 frames. With antialias, the `resolve` at that
moment also brought the half-drawn frame to the window's texture. Found with the same probe inside
the Qt window: the swap presented 0 lit pixels, and the clear stayed separate from the draws that
came after it.

## Wide aspect ratio, experimental

`graphics.proporcao` (Native, 16:9, 16:10, the window's), only on the GPU rasterizer. It does not
stretch: it **renders** the scene in a wider perspective, like Dolphin's widescreen hack.

- **The attachment gains columns on the sides** (`GpuState::extra`, 106 per side at 16:9 with 480
  rows), only when the surface goes to the whole screen — a smaller pbuffer has no sides to open.
- **The stretched surface opens too.** Quake draws at 320×400 and declares it through
  `EGL_QUALCOMM_surface_scale`, which the device enlarges to 640×480. `GlState` keeps that
  difference (`stretched_surface`, set by `SetSurfaceScale` and cleared by a pbuffer), the extra
  columns are counted in surface pixels (53 per side at 16:9) and the aspect ratio handed to the
  window is the screen's. Before, a surface smaller than the frame opened nothing, and with the
  internal resolution above 1 the frame went to the window at the surface's aspect ratio — 0.8,
  narrow and smaller, in both modes.
- **Perspective opens, the rest shifts.** A batch with a perspective projection
  (`GlState::is_perspective_projection`: `p[11] ≠ 0` and `p[15] = 0`) has its clip `x` multiplied
  by `k = 640 / (640 + 2·extra)` and the viewport widened by the inverse ratio, around the same
  shifted center. What was on screen lands on the same pixel as before, and what the clip cut
  appears on the sides. A HUD in orthographic, `draw_texture` and the 2D composition only go to the
  center.
- **The game still sees 640×480.** The read (`bind_for_read`) copies only the center, so
  `glReadPixels` and the copy to the screen come out identical to native — checked on Crash Nitro
  Kart.
- **The window** paints the wide frame at its aspect ratio (`GpuFrame::ratio`) when the screen is
  pure 3D; with 2D on top, it goes back to 4:3.
- Without a window: `zeebx session <zip> --gpu --aspect=16:9 --dump=…` writes the wide
  `*.grande.bmp`.

Expected problems, and that is why it is experimental: objects popping in at the edges (the game
does not draw what it thinks is off screen), the image alternating between wide and 4:3 when there
is 2D on top, and games that draw the HUD in perspective, which get distorted along with the scene.

## Antialias (MSAA) and anisotropic filtering

Also only on the GPU rasterizer, in the graphics tab, off by default.

- **MSAA 2x/4x/8x** (`graphics.antialias`). With samples, `dest()` sets up a second drawing
  framebuffer with multisampled color and depth; the `color` texture becomes only the result.
  `resolve()` copies the samples to it before any read (`bind_for_read`, the large frame) — and the
  window, which paints `color`, gets it resolved because `present_gl` reads the frame on every
  `eglSwapBuffers`. The number of samples is limited by `GL_MAX_SAMPLES`. It combines with the
  internal resolution. Transparency cut out by an alpha test is not smoothed.
- **Anisotropic 2x–16x** (`graphics.anisotropico`), through `GL_TEXTURE_MAX_ANISOTROPY` from the
  `EXT/ARB_texture_filter_anisotropic` extension, applied in `parameters()` and reapplied to every
  texture when it changes. Without the extension, it stays off.
- Without a window: `zeebx session <zip> --gpu --msaa=4 --aniso=16`.

Measured: on Crash Nitro Kart, MSAA 4x visibly smooths the models' outlines. On Need for Speed at
640×480 anisotropic barely changes the image (mean difference of 0.2 levels): its textures come
without mipmaps, and without a chain the filter has little to do.

## Why there is no "overclock" of the emulated CPU

The virtual clock advances with executed instructions (528 per microsecond, the console's ARM11),
and drawing in GL costs no virtual time. Measured on Need for Speed, in the race: ~60 frames per
virtual second at 100% and at 200% CPU — the game already hits the ceiling of the 60 Hz refresh,
because what made it slow on the device was the GPU, which is not emulated here. A faster CPU
option was tried and removed for changing nothing. What limits smoothness in the emulator is the
host being able to keep real speed (50 virtual s in ~44 real s in `zeebx session`, without a
window).

## Performance: Quake in slow motion

Quake executes 132 million instructions per virtual second, a quarter of the console's CPU, and
makes about 370 draw calls per frame: one fan `glDrawArrays` for each face. Measured with stack
samples through `gdb` in the level, half the time was the JIT, a third sending drawing to the GPU
and a sixth reading the frame back. Without a window the game ran at 0.88× the console, and three
things changed:

- **Vertex ring.** Each draw redefined the buffer (`buffer_data`) and the attribute pointers, 4 µs
  per call. Now the buffer is a ring (`GpuState::ring`): the draw writes to the next range with
  `buffer_sub_data` and draws from it through `first`; the pointers stay in the VAO.
- **Batches.** Consecutive draws with the same `fill` and the same perspective become loose
  triangles in a batch (`GpuState::batch`) that goes to the GPU in a single call. Fans and strips
  are taken apart in OpenGL order, which keeps the winding and face culling. The batch is flushed
  with the state in which it was gathered whenever the `fill` changes and before everything that
  touches the GPU outside drawing — clearing, uploading or deleting a texture, changing a texture
  parameter, reading the frame, switching destination, scale, aspect ratio or surface.
- **Cached uniforms.** Each uniform's location is looked up once and the value is only re-sent
  when it changes (`Uniforms`). It saves close to a microsecond per draw.

With that Quake went to 1.25× without a window, with the same image — also checked on Dragon Vs
Chicken and at 16:9.

**And the window ran one game frame per window frame.** `Session::step` returned at the first
`eglSwapBuffers`, and with the window below 60 frames per second the game moved in the same
proportion: smooth, without stutter, in slow motion. More than a frame behind the world clock, it
now runs up to four frames per turn and only the last one goes to the screen. A lag above 250 ms is
forgiven instead of recovered — pause, loading and a dragged window stop the virtual clock, and
catching up on that afterwards would make the game race.

## Performance: the Need for Speed race below console speed

Measured with `zeebx session <zip> --gpu --scale=3 --profile=45000`, which turns on the API profile
only from the given instant and counts the ARM instructions in the interval. In the race, from 45
s to 59 virtual s:

| | speed | real time | instructions/real s | frame swap |
|---|---|---|---|---|
| before | 69% of console | 20.3 s | 132 million | 4.9 ms/frame |
| RGB565 read | — | — | — | 2.1 ms/frame |
| + page table | **108%** | 12.9 s | 207 million | 2.6 ms/frame |

The game executes 192 million instructions per virtual second in the race — 36% of the console's
CPU; what made it slow on the device was the GPU. The same 2,682 million instructions and the same
frames in both measurements: the behavior did not change, only the cost.

- **Frame read in RGB565.** `eglSwapBuffers` copies the 3D to the console screen. The read in RGBA
  with conversion on the CPU cost 1.8 ms per frame, six times the read itself; now the GPU delivers
  `GL_RGB`/`GL_UNSIGNED_SHORT_5_6_5` (on GLES, where this is not guaranteed, RGBA stays), the same
  measurement becomes a copy, and `present_gl` reuses the previous frame's buffer.
- **Dynarmic page table.** Every read and write of the recompiled code went out to a Rust callback
  with a `RefCell` and a region lookup. The table (`DynarmicCpu::table`, 2^20 pointers) points
  directly at the pages of writable regions. These stay on the callbacks, with a null entry:
  read-only regions (the vtables, which is how the JIT hands control back at the APIs), pages
  already executed (nulled at the first code fetch, so a write to them invalidates the block),
  pages with a write watch (nulled in `watch_dirty` and restored in `unwatch_dirty`) and the partial
  page at the end of a region. ARM execution went from ~176 to ~350 million instructions per
  second. Crash Nitro Kart came out identical pixel for pixel, and the Z-Wheel still launches
  games.
- **Window without `gpu_present`.** The egui texture is uploaded only when the screen changes
  (serial and writes), and in a single pass; before there were two calls to `to_argb` and two copies
  per repaint.
