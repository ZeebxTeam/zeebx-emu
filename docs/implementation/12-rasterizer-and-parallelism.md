# 12 — Rasterizer: the granularity of parallelism

This document exists because the first version of the parallel rasterizer **worked and was useless**
for half the games, and the reason is not obvious from looking at the code. It is about where to
cut the work, not about how to fill a triangle.

## The mistake: parallelizing per draw call

The first version split the frame into horizontal bands **on every draw call**. One band per core,
each one walking the triangles of that call. It worked well:

| Game | before | after |
|---|---:|---:|
| Alpine Racer (15 virtual s) | 16.92 s | 5.67 s |
| Crash Nitro Kart | 6.15 s | 3.97 s |

And it did **nothing** for Quake, which stayed at 28% of the console's speed.

The measurement that explained it:

```
FILL serial=198000 parallel=2000 avg-cost=5104
```

**99% of Quake's fills went serial.** The average cost of one of its draw calls is 5,104 fragments
— about 15 µs of work. Waking eight threads costs between 200 and 400 µs. The 64 thousand fragment
threshold, which existed precisely to avoid paying that price for nothing, was right: there was
no way to parallelize that.

## Two ways of drawing a frame

What separates the games is not how much they draw, it is **in how many pieces**. Timing each
draw call:

```
Alpine Racer:  >1ms: 22518ms/1840x   200µs-1ms: 441ms/712x   the rest: 0ms/13x
Quake:         556,411 draw calls in 25 virtual seconds
```

Alpine concentrates 98% of the time in 1,840 calls of ~12 ms each — each one is plenty of work for
eight threads. Quake spreads the same kind of scene over half a million two-triangle calls. Both
draw the same number of pixels; only one of them can be split per draw call.

This is not a Quake oddity. It is what any engine that sorts by texture does: one call per
material, per sprite, per piece of scenery.

## The fix: accumulating the frame

Drawing is now **accumulated** and painted only once, at the end of the frame. Two hundred thousand
small serial fills become one large parallel one.

The draw call now has two phases:

- **`prepare`** projects the triangle, culls faces, computes the bounding box and the attributes
  divided by `w`. Everything that does not depend on the pixel.
- **`enqueue`** stores the triangles in the queue and records, alongside them, the OpenGL state
  in effect at that moment — because it changes between one call and the next.
- **`flush`** paints the whole queue, splitting the screen into bands.

```rust
struct Job {
    first: usize,          // its range in the triangle queue
    last: usize,
    texture: Option<u32>,  // the name, not the reference
    texture_env: u32,
    depth_test: bool, depth_mask: bool, depth_func: u32,
    blend: bool, blend_src: u32, blend_dst: u32,
    alpha_test: bool, alpha_func: u32, alpha_ref: f32,
}
```

Three decisions worth recording:

**One triangle queue, not one per draw call.** Each `Job` stores only the indices of its range. A
`Vec` per draw call would be two hundred thousand allocations per frame — trading one bottleneck
for another.

**The texture is kept by name, not by reference.** Between `enqueue` and `flush` the game binds
other textures, and what counts is the one that was bound when that batch was built. Storing
`&Texture` would also make it impossible to lend the texture map to the threads while the frame
is being written.

**The two vectors live across frames.** Giving back and requesting the same memory sixty times a
second has no reason to happen.

## Order is what guarantees the result

Each band is an **exclusive** piece of the frame — `chunks_mut` over `color` and `depth` — and
walks the whole queue in the order the game drew it. Since two bands never touch the same pixel,
and within a band the order is the original one, transparency stacks in the same sequence.

The result is not "similar": it is **identical**. Verified by comparing Quake's frame before and
after, byte by byte — 921,600 bytes, zero differences. The same already held for the per-draw-call
version, checked over 300 frames of Alpine Racer.

## When the queue has to be flushed

The rule: **before anything that reads the frame or changes something the queue mentions.**

| Point | Why |
|---|---|
| `present` | it is the delivery of the frame; whoever asks for the result has no reason to know that drawing is accumulated |
| `clear` | clearing is an operation on the frame and goes into the same order: it would erase what has not even been painted yet |
| `glTexImage2D` and `glCompressedTexImage2D` | changing the contents of a texture the queue will still read would change the past |
| `glDeleteTextures` | likewise, for the same reason |
| `set_surface` | what was drawn at the old size has to become pixels before the switch |

`present` flushes by itself, and that is why it takes `&mut self`. It was a deliberate signature
change: with `&self` it was possible to read a half-finished frame, and the compiler had no way to
warn.

## What it gained

> **The numbers in this section are from the Unicorn era and do not describe today's emulator.**
> They were measured with the backend that left the tree, and the comparison between the three
> rows below remains valid as *a history of what gained what* — the proportion between the slices
> does not. The current measurements are in
> [`../OPTIMIZING_V0.3.0.md`](../OPTIMIZING_V0.3.0.md). In particular, the sentence "even for free
> Quake would be at ~52%" **no longer holds**: with Dynarmic Quake goes above 100%.

Quake, 25 virtual seconds, measured three times:

| | wall time | speed |
|---|---:|---:|
| before | 89.8 s | 28% |
| reusing the vertex buffer | 81.0 s | 31% |
| accumulating the frame | 58.5 s | 43% |

Where the time went, before (measured by turning off each stage):

| | time | slice |
|---|---:|---:|
| pixel filling | 37.1 s | 38% |
| geometry (transform and clip) | 13.5 s | 14% |
| ARM emulation + API dispatch | 48.3 s | 49% |

Two corrections after this measurement:

- **Geometry dropped from 14% to ~4%.** `read_attribute` issued one `read_u32` per component, and
  each request crosses unicorn's FFI: 57 ns to fetch four bytes, against 0.3 ns when they come
  from a one-kilobyte `read_mem`. Of the 3.6 s the draw calls cost in 15 virtual seconds, 3.2 s
  were crossing and 400 ms were drawing. Today it is one block per array per draw call.
- **The table above was taken with `--profile`, which costs 24%** — and the price falls almost
  entirely on the ARM slice, because the block profile does one table insertion per translation
  block. The proportion between the three slices is usable; the absolute clock is not. Measure
  time without it.

## Finding out that nothing changed cost more than drawing

The game can write directly to the surface that EGL exposes — that is how the Z-Wheel composes
the 2D over the 3D stage. To not lose those writes, `sync_egl_color_from_guest` read the whole
surface back from the guest and compared it byte by byte with our copy. It is called on **every**
`Draw*`, `Clear` and `ReadPixels`.

Measured on the Z-Wheel, thirteen virtual seconds: **93,750 calls**, each one reading 400 KB and
comparing 400 KB — close to 37 GB of traffic just to find out that there was almost never a
change. That was 3,324 ms of reading and 2,732 ms of comparison, and the cost showed up where
nobody would look: in `glDrawElements`, with 2,883 ms.

The fix is a **write watchpoint** (`CpuBackend::watch_dirty`/`take_dirty`): the backend's hook
sets a `bool` when the guest writes to the range, and `sync` returns in O(1) while it is clean.
The mechanism was born with Unicorn's hooks and **survived the switch**: what changed was who sets
the flag, not the idea. After: reading 105 ms, comparison 39 ms, `glDrawElements` **275 ms** — ten
times less —, and the API total dropped from 16,345 ms to somewhere between 12,550 and 13,500 ms.

Two things this measurement taught, which hold beyond this case:

- **The host's writes do not go through the backend's hooks.** The API implementations write
  directly into guest memory, and that is how the 2D reaches the surface. The first version of
  the flag ignored this: the cost vanished and the wheel's covers stopped appearing, with the
  difference confined to the cylinder's band. Whoever arms a watchpoint has to mark it in the
  emulator's own `write_mem` too — the debugging watchpoint already did that, and that is where
  the clue came from.
- **A single run measures nothing here.** Three runs of the same binary gave 13,903, 14,221 and
  15,845 ms of API: a 14% spread. Only a large delta counts — and that is why the drop in
  `glDrawElements` serves as proof, while the fast import path (which is algorithmically better
  and pixel for pixel identical) is left without a number: its effect does not rise above the
  noise.

## The ceiling that remains

> This whole section is from the Unicorn era: the two conclusions below — the cost per API call
> and the core's ceiling — remain the right targets, but **the numbers do not**, and the second
> has already been partially solved (`dynarmic` came in, and Quake went from 52% to 170%).

It is worth being clear so as not to expect from the rasterizer what it cannot give: **even for
free**, Quake would be at ~52%. The 48 s of emulation plus dispatch for 25 virtual s are already
twice the clock. That is 3.4 billion guest instructions per 25 virtual seconds, and the core
delivers 218 million per second in a tight loop that does not touch memory — but about 86 million
in the real game, where there is memory traffic through the softmmu and an `emu_start` round trip
per API call.

The next two bottlenecks, in order:

1. **The cost per API call**, measured at 1.4 µs round trip, because every call is an `emu_stop`
   followed by an `emu_start` from unicorn. Quake makes 2.5 million of them in 25 virtual seconds.
   Serving inside a hook, without stopping emulation, is a redesign of the trampoline — and it is
   the largest thing left **in the mechanism**. Before it comes what each method does inside:
   `--profile`'s API profile measures that, and the three times a game seemed stuck in dispatch
   the cause was there, not in the trampoline.
2. **The core itself.** At ~86 effective MIPS, a game that uses a quarter of the capacity of the
   console's ARM11 already consumes a good part of our clock just executing instructions. **This
   is where the `dynarmic` backend came in** — `CpuBackend` existed exactly for that, and the
   switch was worth 2.3 times in the core's pace. Today what remains on this front is not
   switching backends, but SMC: pages that mix code and data leave the direct table and go
   through a callback on every read and write.

## Calibration numbers

Bands, measured on Alpine Racer on a 24-core machine: 4 → 10.89 s, 6 → 8.64 s, 12 → 5.59 s,
16 → 5.74 s. It stays at one band per core, with a floor of rows per band (`MIN_BAND_ROWS`).

**The floor was 40 rows, and that was the wrong part of the calibration.** It is not tuning: it
is the real ceiling of parallelism when the surface is short. The Z-Wheel stage is 640x330, and
`330 / 40` gives eight bands — on a 24-core machine, two thirds of it sat idle during the whole
fill. The number came from measuring only 480-row scenes, where `480 / 40 = 12` was already close
to what the machine would give, and the defect stayed invisible precisely because of that.

Lowering it to 8 rows, `flush` time on the Z-Wheel over thirteen virtual seconds: **2474 ms with
40, 1938 with 16, 1868 with 8**. And on the Crash track, over thirty virtual seconds, the API total
went from **4637 to 3842 and 3622 ms** at the same cuts. The fear that thin bands would be
expensive at 480 rows was not confirmed, and the reason is arithmetic: there `480 / 16` and
`480 / 8` hit the number of cores before hitting this constant, so both cuts describe the same
split.

Splitting finer does not change a single pixel — the nine dumped surfaces of the Z-Wheel come out
byte for byte identical —, because each band is still an exclusive region and walks the queue in
the original order.

The cost threshold (`PARALLEL_COST`, 64 thousand fragments) was calibrated when the split was per
draw call, and there it made no difference between 16 thousand and 256 thousand. With the
accumulated frame it almost never holds anything back — it remained as protection for the case of
the tiny frame, where waking threads still does not pay off.

## The lesson, short

Parallelizing is not choosing between serial and parallel: it is choosing **the size of the
piece**. The same code, with the same number of threads and the same threshold, ranged between
"3× faster" and "does nothing" depending only on where the cut was made. And the only way to
find that out was to count how many times each path was taken — the code looked right in both
versions.
