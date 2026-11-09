# 04 — Time, timers and callbacks

## The clock is virtual

```
now_us = clock_us + instructions_executed / 528
```

528 is the clock of the MSM7201A's ARM11 in MHz. The game's time, then, **moves with the work it
does** — not with the host's clock. That makes execution repeatable: the same game, with the same
input, produces the same sequence of frames, whether the host is busy or not.

`clock_us` is the part we **advance**, and it exists for two reasons.

## Uptime is not a calendar

BREW has two time questions, and they are not the same:

- `GETUPTIMEMS` — how long since the device was switched on.
- `GETTIMESECONDS` and `GETJULIANDATE` — **what day it is today**, in seconds since January 6,
  1980 GMT (the GPS epoch, not the Unix one).

We answered the virtual clock to both, which amounts to saying that today is the day the console
launched. A game doesn't notice; an application does. Today the calendar comes from the host's
clock, captured **once** when the machine is created, and from then on it moves with the virtual
clock — a game that asks for the date gets one that exists, and the time it *measures* is still
the virtual one, without depending on how long the emulator took.

## Advancing idle time

`skip_idle_time` — when there is no pending callback, thread, blit or signal, the clock jumps to
the deadline of the next timer. That is what the console does: with no pending work it sleeps and
wakes up on time. Emulating the sleep instruction by instruction would be emulating nothing.

## Skipping the busy wait

Some games don't arm a timer for the next frame: they sit in a loop reading the clock and yielding
until the deadline arrives. Zeebo Sports Peteca does **69 thousand reads per frame**, 14 million
instructions spent just waiting. On the console this costs nothing, because time passes by itself
while the ARM spins; here each pass is really emulated, and the game ran six times slower than the
device.

`note_spin` recognizes the pattern. It counts consecutive calls that are **only** clock queries
(`aee_GetTimeMS`, `aee_GetUpTimeMS`, `aee_GetSeconds`) or yields (`IThread::Suspend`,
`IShell::Resume`). Any other call is a sign of work and **resets the count**. After 64 of these,
each following query advances the clock by 250 µs.

The criterion matters: a frame that reads the clock in the middle of what it does still costs what
it costs. Only whoever *is not making progress* gets time for free.

Result in Peteca: 101 s real → 12.4 s, for the same 428 frames of 17 virtual seconds.

## Sleeping

`MSLEEP` — the stdlib's `sleep(msecs)` — **advances the clock**. On the console the ARM really
stops and time passes by itself; here the clock moves with the instructions, so whoever sleeps
without the clock advancing sleeps forever. Magical Drop 3 called `sleep` a hundred million times
in a loop that never ran out.

There is a ceiling: an absurd value, requested by mistake, can't drag the clock along with it.

## Timers

`ISHELL_SetTimer` stores `(deadline, callback)`. On each pass, `advance` collects the expired ones
and calls each of them.

The detail that isn't obvious: the expired ones leave the list **before** running. The callback
typically rearms the timer, and the rearmed one must not fire in the same frame — that would be a
loop that never gives control back.

## Re-entering the guest

Several points need to call a function **of the game** from inside an API call: a timer callback,
a `qsort` comparator, the `PFNIMAGEINFO` of an image that is ready.

This works because the dispatch happens **outside** `CpuBackend::run` — the core has already
stopped when we serve it. Re-entering is safe as long as:

- the live registers are saved and restored (`SAVED_REGS`);
- there is a nesting ceiling (`MAX_NESTING`), otherwise a cycle would trap the emulator;
- the re-entry happens at a **call boundary**, never in the middle of one.

`run_pending_callbacks` runs at the boundary between two API calls — the only point where you can
enter the guest without interrupting anything halfway. `CALLBACK_ROUNDS` limits how many rounds to
deliver, because one callback can queue another.

## The instruction ceiling yields, and the stretch continues afterwards

The instruction ceiling exists so the frame loop can deliver input and check the clock: a game
that never yields would own the emulator. But cutting must not mean **losing**.

**Zeebo Extreme Rolima** runs its entire loading inside `EVT_APP_START`, without calling
`IThread::Suspend` or returning between frames — its siblings yield, it doesn't. The ceiling cut
the stretch, the event loop found no timer or callback at all, and the session ended by itself at
3.8 seconds, as if the game had finished.

Today the cut stores where to continue, and the next pass resumes before anything else —
delivering a timer on top of a half-done frame would deliver it at the wrong time. Two subtleties:

- **The mode goes along.** The resume address carries bit 0 set when the guest was in Thumb, which
  is how ARM says "continue in Thumb". Without it, a Thumb game comes back decoded as ARM and jumps
  to address zero.
- **So do the registers.** Between the cut and the resume the emulator still delivers that pass's
  signals and callbacks, and entering the guest for that overwrites `r0`-`r3` and `lr`. The
  context is stored at the cut and restored at the resume.

Only the **outermost** stretch is resumed: a nested stretch — a callback called from inside the
dispatch of an API — has someone waiting for it on our side, and that frame is already gone when
the pass ends. When nested, the ceiling is still only a request to yield.

## Cooperative threads

BREW's `IThread` has no preemption: the guest only loses control when it calls `Suspend`. That is
exactly where we save the registers, and resuming is restoring them and continuing from where
`Suspend` would return. It is the reason this is feasible in a single-core emulator.

## `qsort`

BREW's stdlib has `qsort`, and the comparator is game code. We sort **in the guest's memory**,
really swapping the elements, so that the pointers the comparator receives are the real addresses
inside the array — which is what C's `qsort` does and what a comparator can observe.

The algorithm is heapsort: it sorts in place and makes `n log n` comparisons. Since **each
comparison costs an entry into the guest**, their number is what matters; an insertion sort would
be simple and quadratic, and a large array would be expensive.

## Above and below the console's speed

The brake that compares the virtual clock with the real-world one accepts a ratio: fast-forward is
the same brake at 3x, and the limit turned off is no brake at all. Changing the ratio re-anchors
the comparison — without that, releasing fast-forward left the game "ahead" by everything it had
gained. The save state also records `clock_us`, the skipped time, which used to be left out and
made the restored clock start at "now". See [24-speed.md](24-speed.md).
