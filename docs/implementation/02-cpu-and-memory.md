# 02 — CPU and memory

## The core

`CpuBackend` (`cpu/mod.rs`) is the boundary: read and write registers and memory, and a
`run(pc, budget) -> StopReason`. The implementation is **Dynarmic** (`cpu/dynarmic.rs`),
configured for ARMv6K/A32 and Thumb — the MSM7201A core is an ARM11, and asking for a compatible
architecture keeps the game from tripping over an instruction the console had.

The interface exists so the rest of the emulator doesn't depend on the concrete backend. It is thin
on purpose: all that goes through it is registers, blocks of bytes and a stop reason.

### Stop reasons

| `StopReason` | How it happens |
|---|---|
| `ApiCall { addr }` | The game jumped into the `0xf000_0000` range — it is an API call |
| `Returned` | The game came back to the sentinel address we put in `lr` |
| `MemoryFault` | Read or write outside the map |
| `Exception` | Invalid instruction |
| `Budget` | The instruction budget ran out |

None of them brings the emulator down. A memory fault is **information**: the report shows the
address, the `pc`, the `lr`, the registers and the stack. Aborting would hide exactly what
matters.

### Instruction count

It comes from the blocks recompiled by Dynarmic. It is the emulator's virtual clock: the time the
game sees comes from here, not from the host, so that two identical runs give the same result.

### Throughput

Measured, not estimated — the main bench is `zeebx bench <game>`:

| | |
|---|---|
| Instructions per second | varies by game and host; measure with the bench before claiming |
| Cost of entering the guest | amortized by the recompiled blocks |
| Instruction ceiling enabled | charged by the backend without a per-instruction hook |

These numbers exist to answer the question "is the emulator slow or does the game do a lot of
math?" without an investigation. **Every time a game seemed stuck, the core was never the
culprit** — it was always our side of the dispatch, and since then `--profile` also measures the
real time of each API method, which is where the answer usually is.

## The memory map

| Region | Base | Size | What for |
|---|---|---|---|
| null | `0x0000_0000` | up to the module | zeros that can be read, with no write or execute |
| extensions | `0x0800_0000` | 16 MB each, up to 8 | the package's extension modules |
| module | `0x0001_0000` | image + 1 MB | the loaded `.mod`, with a one-page prefix |
| heap | `0x1000_0000` | 64 MB | what the game's `MALLOC` consumes |
| stack | `0x2000_0000` | 1 MB | `sp` starts at the top; the ARM stack grows downward |
| objects | `0x3000_0000` | 4 MB | the objects we hand to the game |
| helpers | `0x3100_0000` | 1 KB | the BREW stdlib table |
| stubs | `0x3200_0000` | 4 KB | ARM code that **we** write |
| internal | `0x3400_0000` | 16 MB | what **the emulator** allocates for itself: thread stacks, buffers |
| surfaces | `0x4000_0000` | 8 MB | the pixels, in the guest's space |
| vtables | `0xe000_0000` | read-only | the pointers that lead to the trampoline |

Three decisions you can't deduce by looking at the table:

**The prefix before the module.** The stub that `elf2mod` puts at the start computes its own base
with PC-relative arithmetic and then reads **two words before it**. The AEE loader reserves that
prefix, and so do we — one page is more than enough slack.

**The null page can be read.** The console has no memory protection, and the low addresses are
readable: a game that reads through a null pointer gets some value and carries on. Aviãozinho, a
fan-made Quake port, depends on this when loading the first level: `start.bsp` is missing one of
its 19 textures, the engine puts the fallback texture `r_notexture_mip` in its place, and this port
never creates it. Its name is read from address zero, and stopping execution there left the game
stuck in the menu. Here the range below the module returns zeros. Writing to it is still a fault,
and so is executing — the region is marked no-execute, and a jump to address zero still shows up in
the report as before.

**64 MB of heap.** Quake measures free memory before loading the `.pak` files and gives up with
"Not enough free memory" if it is small. The console has 128 MB; the number here is our choice, it
only needs to be roomy enough for the game to accept the device.

**The surfaces live in the guest's space.** Because `IDIB` hands the game the buffer pointer so it
can draw directly — that is how commercial games write to the screen. Keeping the pixels only on
our side would make that impossible. The consequences of this are in
[05-video-2d.md](05-video-2d.md), and they are not small.

## The processor runs in user mode

A BREW applet is not privileged, and there is code that **checks this and changes path**. It was a
bug of ours while the `CPSR` wasn't written: whoever queried the mode took the wrong branch.

The Superscape 3D engine that Kingdom Hearts ships as an extension is the case, and it says
plainly which mode it expects:

```
0x0803aedc  mrs  r4, apsr
0x0803aee0  tst  r4, #0xf
0x0803aee4  beq  #0x803af74        <- user mode: skips all of this
0x0803aee8  mrc  p15, #0, r1, c2, c0, #0   <- TTBR0, the page table base
0x0803aef8  ldr  r5, [r7, r6, lsl #2]     <- and walks it
```

Privileged, it would walk the MMU page table to translate an address. Here there is no MMU,
`TTBR0` is zero, and it died reading `0x00000400` — a fault inside the extension, at an address
that said nothing about the cause. With the `CPSR` at `0x10`, it skips the whole stretch and the
game runs.

Eight games were checked after the change — Quake, Zeeboids, Crash, Peggle, Prey Evil, Zuma's
Revenge, Bejeweled Twist and Zenonia — and none changed behavior. Semihosting, which Peggle and
Zuma use for logging, still gets through: the `SVC` is served by the same interrupt hook, which
doesn't depend on the mode.

## Stubs: our code, executed by the game

Some helpers always return the same thing. Serving one of them through the trampoline costs
stopping and restarting the ARM core — and `GetAppInstance` alone accounts for **more than half**
of Quake's API calls, because BREW games keep their globals inside the applet and every access to a
global goes through it.

For these, `loader/mod.rs` writes a few ARM instructions at `0x3200_0000` and points the table
there. The call doesn't even leave the CPU.

## Heap and objects

`brew/heap.rs` is a simple allocator: first fit on the free list and, when nothing fits, a pointer
that moves forward. All the bookkeeping stays on the host — no header is written into the game's
memory.

**But after each `MALLOC` block there is at least one byte, and it is a line break**
(`BLOCK_PADDING`, in `machine/thread.rs`). Um Jogo de Ovos reads each cutscene into a block of
the file's exact size, and its parser, before measuring a token, looks at the byte right after the
end of the file: if it is zero, it rejects the token. With the blocks packed together and the
padding zeroed, it was zero, and the boss 1 cutscene printed "Error loading cutscene" once for each
of the 747 characters of `b1i.cut`; the boss level never started. The game's tokenizer, run in
isolation over the eleven real cutscenes, rejects all of them with zero there and reads all of
them with any other byte.

The value is not irrelevant. Alpine Racer also reads past the end of a text buffer, up to the
first separator: with `0xaa` in the slack, it appended two `ª` to a field; with a line break (or a
space), its call sequence is the same as when there was a zero there. A line break also stops
whoever reads to the end of the line. On the device Ovos works, so there that byte is not zero;
the format of Qualcomm's heap was not checked.

**A returned block merges with its free neighbors, and this is not a refinement.** Without
merging, each `FREE` becomes an isolated hole and fragmentation shows up quickly in a long session:
Treino Cerebral switches screens hundreds of times, none of the pieces ever forms a large region
again, and the megabyte the next stage asks for finds nowhere to fit with the pointer already at
64 MB. The game doesn't check for the null that `MALLOC` returns and dies calling a method at
zero. With merging, the same playthrough stays at **one** megabyte of use instead of sixty-six.
The block that touches the top doesn't go to the list: the pointer moves back and it becomes fresh
space again.

That is why `used()` is not "where the pointer got to": what is on the free list has already come
back, and counting it would make `GetRAMFree` lie to whoever asks before allocating.

**There is memory the game hands over and doesn't give back, because the interface is what gives
it back.** `IMEMASTREAM_Set` passes the buffer to the stream, and it is the stream that frees it
when it is destroyed or receives another one. Action Hero 3D builds 7,616-byte bitmaps, several per
frame, puts them in a stream, decodes them into an image and lets go of both: with the stream
forgetting the buffer, the level ate 1.9 MB per second, and in a little over three minutes
`MALLOC` returned null and the game copied to address zero. `SetEx` is left out, because there it
is a `pfnFree` of the game that frees.

In `realloc`, running out of space leaves the old block intact and returns null — freeing it along
the way took from the game the data it still had. Size zero is `FREE`: the block goes and the
pointer becomes null, and `ERR_REALLOC(0, &p)` leaves `p` null as the game expects.

**The heap is the game's alone.** What the emulator needs in the guest's space — the stack of each
`IThread`, the `IPeek` line buffer, the `AEEImageInfo`, the `AEEAppStart`, the `eglQueryString`
strings, the output pointers passed to the game's callbacks — comes from the internal region,
through `alloc_internal`. Interleaved with what the game asks for, these blocks made the address
of each of the game's structures depend on the Zeebx version, and RetroAchievements reads the heap
by address: an achievement can't break because the emulator started allocating one more block.
What the game asks for through an API (`STRDUP`, `LoadResData`, `IHeap_Malloc`) stays in the
heap, because the size and the timing come from the game. Every `free` goes through
`Machine::free`, which picks the allocator by address.

What **we** allocate in the guest's space also has an owner. The `IPeek` line buffer, the size of
the whole file, goes away with the reader; the `AEEImageInfo` of the image notification is one
block per image, reused and returned on `Release`, not a new block for each notification.

**What only grows on our side also has a ceiling.** The `DBGPRINTF` log groups repetitions by
index and keeps up to 2,000 distinct lines — a line with fps or time is new on every call and grew
for the whole session; past the ceiling, the oldest half goes. Semihosting keeps up to 256 KB. The
`GetNextButtonEvent` queue keeps 64 events per port: a game that reads the controller some other
way never emptied it.

**An object's address gets reused, and the old one's state must not carry over to the new one.**
The `Release` of `IHash` and of the ciphers forgets the state (a new hash continued the previous
one's MD5), the input device's forgets the port, `ISignal`'s removes it from the input signals (a
touch would fire the callback of the signal born in its place) and `IImage`'s forgets the
`Notify`. The transparent color is forgotten when the object is **born**, not when it dies: a
bitmap's `Framebuffer` is still queried after `Release` by whoever kept the address without a
reference, and touching that is a separate job.

`brew/objects.rs` hands out addresses inside the objects region, each one starting with the vtable
pointer — which is what a COM object is. It keeps **reference counts**, and they are what decides
when the associated state (a bitmap, an open file, an enumeration) can be discarded.

`adopt` exists for the reverse case: an object that the **game** created and hands to us.
Bejeweled Twist implements its own `IBitmap`, with the vtable embedded in the object — and to draw
on it you have to ask it where the pixels are.
