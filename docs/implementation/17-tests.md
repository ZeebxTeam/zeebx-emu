# 17 — Tests

How to check that the emulator does what we say it does, and which commands to use for it.

The project's goal is to reproduce the behavior of BREW 4.0.2 as the Zeebo presented it. That is
a claim about **observable behavior**, not about pretty code — so the only way to know we are
still right is to measure. And measuring once is not enough: what matters is knowing **in the
commit that caused it** that something stopped working.

Hence three layers, with very different costs:

| Layer | Cost | What it answers |
|---|---|---|
| Tests next to the code | seconds | is an algorithm, a parser, an API method right? |
| Opening scan | ~15 s for the whole library | does the ROM open, or blow up before initializing? |
| Execution scan | seconds to minutes per game | does the game move forward, what does it cost, and what is still missing for it? |

## The basics

```bash
# everything
cargo test

# a module, or a name
cargo test rasterizer
cargo test clip

# the ones left out by default: they measure time, or depend on the machine running them
cargo test --release -- --ignored --nocapture
```

There are hundreds of test functions, all next to the code they test, and some marked with
`#[ignore]`. They are left out because **time is not a reproducible result** or because they look
at the real file system's save cache.

`--nocapture` shows what the tests print. Without it, `cargo test` swallows the output of the ones
that passed — and in the scans the output is exactly what matters.

## Why there is no `tests/`

Rust has two layers of tests: integration tests, in `tests/` at the root, each file compiled as a
separate binary that only sees the crate's **public API**; and unit tests, next to the code in
`#[cfg(test)] mod tests`, which can touch private fields.

Only the second exists here, and not by oversight: **the crate is binary only**, it has no
`src/lib.rs`. A file in `tests/` could not even `use zeebx::…`, because there is no library to
import.

That fits what these tests need to do. A test that checks the clipping of a blit needs to build a
`Machine`, call a method by slot and read the surface from inside — none of that is, nor should
be, the emulator's public interface. Having to make public what the tests use would make the code's
boundary worse in order to improve no test boundary at all.

The price is that the ROM scan also lives in `src/`, under `#[cfg(test)]` — that is, it does not
exist in the release binary. It is the same effect as development-only tests in other languages,
achieved by conditional compilation instead of by directory.

## The ROM scan

`src/scan.rs`. It is the compatibility survey from [11-compatibility.md](11-compatibility.md)
turned into a test. It is **driven by the environment**: no ROM is in the tree, so without
`ZEEBX_ROM` both tests warn and pass. That is on purpose — `cargo test` cannot turn into a scan of
sixty-three games because someone cloned the repository.

| Variable | What it does |
|---|---|
| `ZEEBX_ROM` | a `.mod`/`.zip`, a comma-separated list, or a directory |
| `ZEEBX_ROM_MS` | how much **virtual** time to run, in ms (default 6000) |
| `ZEEBX_ROM_CAP` | **real** time cap per game, in seconds (default 90) |
| `ZEEBX_ROM_OUT` | directory where the full report of each game is written |
| `ZEEBX_ROM_BASE` | baseline directory: what is missing is written, what exists is enforced |
| `ZEEBX_ROM_KEYS` | controller script on the **virtual clock**: comma-separated `ms:button`, and `ms:axis=value` for the stick |
| `ZEEBX_ROM_PROFILE` | turns on the cost profile per API method (it makes execution more expensive: do not use it to measure speed) |

### Controller script

There were questions that could only be answered by someone pressing the controller — whether the
input reaches the guest, whether a menu accepts the choice. With the script, the scan answers on
its own:

```bash
# pushes the stick to the left at 4 s, returns it to rest at 6 s, confirms at 10 s
ZEEBX_ROM="roms/Z-Wheel.zip" ZEEBX_ROM_MS=12000 \
  ZEEBX_ROM_KEYS="4000:x=-128,6000:x=0,7000:y=-128,9000:y=0,10000:b1,11000:" \
  cargo test --release scan -- --nocapture
```

The button names are those of the device's table (`b1`…`b4`, `start`, `up`, `left`, `down`,
`right`), and the axes are `x`, `y`, `z` and `rz` over the `-128..=128` travel. **The step is on
the virtual clock**, so the same script works the same with the machine at 30% or at 300% of the
console's speed. A step with a colon and nothing after it (`11000:`) releases everything.

It is worth knowing a detail that has already cost an investigation: **the Z-Wheel is navigated
with the stick**, not with the buttons (it registers `RegisterForPositionChange`). A script of
buttons only does not move the wheel a pixel — and the symptom is a still image, which looks like
an input defect.

### What the scan **cannot** verify, measured

An attempt to exercise the Z-Wheel's cycle this way, with the script above: **it does not work**,
and it is worth knowing why before trying again. Twelve virtual seconds, with and without stick:

| | with script | without script |
|---|---:|---:|
| laps | 1547 | 1543 |
| instructions | 133,567,092 | 133,566,074 |
| API calls | 264,953 | 264,931 |

The difference is a thousand instructions in 133 million: the wheel **does not react** and draws
nothing on this path (zero frames presented, zero text drawn in both cases). The Z-Wheel builds
its interface through the board path, which the scan loop does not exercise; practical conclusion:
**its cycle can only be verified in RetroArch, with a controller in hand** — which is what the plan
already said in item 8.

### One game, with the whole report

```bash
ZEEBX_ROM="roms/Quake.zip" cargo test --release scan -- --nocapture
```

Out comes the state, the performance, the APIs that were missing, the classes the game asked for
and we do not have, the files it did not find, the rejected pointers, the APIs served by
assumption, the registers and stack of whoever crashed, and the tail of the game's own log.

### Just the opening, across the whole library

```bash
ZEEBX_ROM=roms cargo test --release scan::tests::the_given_rom_opens -- --nocapture
```

The cheapest question you can ask of a game: it loads the `.mod`, runs `AEEMod_Load`, reads the
`.mif` and calls `CreateInstance` — and stops there. Sixty-three ROMs in about fifteen seconds.
Whoever does not create an applet shows up right away, without spending the six virtual seconds
it would never get to run.

The test does not stop at the first error: it notes each one and goes on, and only fails at the
end with the whole list. In a scan what you want to see is the scoreboard, not the first give-up.

### The scoreboard, to check a tweak

```bash
ZEEBX_ROM=roms cargo test --release scoreboard -- --nocapture
```

Each ROM runs a few virtual seconds and becomes a table row: state, virtual time achieved, drawing
and the reason for whoever stopped. It is the quick round-trip test after touching the emulator —
the question is "who still opens", not each game's report.

```
| Game | State | Virtual time | Drawing | Speed | Reason |
|---|---|---|---|---|---|
| Disney All Star Cards | runs | 2500 ms | 382200 pixel(s) | 905% |  |
| Bejeweled Twist | crashed in the frame loop | 11 ms | black screen | 24% | invalid access to 0x00000024 |
| Zeeboids | runs | 3148 ms | 41 frame(s) | 80% |  |
```

The drawing column counts whatever there is: frames presented when the game presents, writes to
the screen when it draws in 2D without presenting, and "black screen" when there was neither one
nor the other — which is the symptom that does not show up in the state.

| Variable | What it does |
|---|---|
| `ZEEBX_SCOREBOARD_MS` | **virtual** time per ROM, in ms (default 5000) |
| `ZEEBX_SCOREBOARD_CAP` | **real** time cap per ROM, in seconds (default 30) |
| `ZEEBX_SCOREBOARD_OUT` | file where the table is written |

**This test does not fail on a broken game.** The library has games known to be incompatible, and
failing on them would erase the table, which is what matters. What enforces regressions is the
baseline, below.

### The execution, with a baseline

```bash
ZEEBX_ROM=roms ZEEBX_ROM_OUT=output ZEEBX_ROM_BASE=docs/scan \
  cargo test --release scan -- --nocapture
```

`ZEEBX_ROM_BASE` is what makes this a test and not a report. The first time, each game's summary
is **written**; on the following runs, it is **enforced**, and what changed comes out as `+`/`-`
in the failure:

```
FAIL  Quake     runs     6016 virtual ms in 4.1 s, 41 frame(s), 146%, 13 finding(s)
  changed since docs/scan/quake.txt:
  - files not found:
  +   0x0103d8de
```

Writing what is missing, instead of requiring someone to write it by hand, is what makes it
possible to adopt a new game's baseline in one run — and the difference is visible in `git diff`,
which is where it should be reviewed, game by game.

### What the baseline keeps, and what it does not

It keeps **state and findings**. It keeps **nothing about performance** — no time, no
instructions, no frames, no file path, no registers from the crash.

The reason is practical: if it did, switching machines — or running in debug instead of
`--release` — would flag a regression in every game at once, and a test that always flags is
never read. There is a test just for this, `the_summary_does_not_change_with_performance`.

That is why `--release` is not decoration either: in debug the emulated core runs an order of
magnitude slower, and the real time cap would classify a good game as "too slow".

### The states, and what each one means

| State | Where it stopped | Does the test fail? |
|---|---|---|
| runs | reached the requested virtual time, still standing | no |
| finished on its own | no timer armed and no pending work | no |
| does not load | it is not a `.mod` we know how to read | yes |
| crashed before creating the applet | in `AEEMod_Load` or inside `CreateInstance` | yes |
| does not create the applet | no `.mif`, or `CreateInstance` refusing | yes |
| crashed at EVT_APP_START | the applet was born and died in the initial event | yes |
| crashed in the frame loop | already inside the loop | yes |
| too slow | did not reach the virtual time within the real cap | yes |

**A finding is not a failure.** A game that runs asking for the six classes of the extensions group
keeps passing, because that is how it behaves on the console too: it gets `ECLASSNOTSUPPORT` and
takes the alternative path. What enforces findings is the baseline, and only when it exists.

The separation between "crashed at `EVT_APP_START`" and "crashed in the frame loop" cost a change
in `session.rs` — the start became a step of its own, `Session::part`. Without it the two fell in
the same lap and a game that dies in the loop showed up as dead at the start, pointing at the wrong
stretch of code.

## The other measurements

```bash
# throughput of the ARM core and cost of entering the guest
cargo run --release -p zeebx-cli -- bench <game.mod>

# a game without a window, with the full report and the call summary
cargo run --release -p zeebx-cli -- run "roms/Quake.zip" --seconds=6

# only the calls of one interface
cargo run --release -p zeebx-cli -- run "roms/Quake.zip" --seconds=6 --trace=Bitmap

# one frame per file, to compare two neighbors
cargo run --release -p zeebx-cli -- run "roms/Quake.zip" --seconds=6 --dump-gl=frames
```

The `run` report and the scan report say the same thing, by construction: both come from the same
`Machine` counters. The `run` one is for investigating a game; the scan one is for enforcing what
was already known about it.

## How to write a test here

Two rules, and both come from a defect that slipped through.

**The test's comment carries the defect that gave rise to it.** Not "tests the formatter", but:
the `%02d` that made Resident Evil 4 look for `3d_stg02_0.h2z` when the file is
`3d_stg02_00.h2z`. A test without that sentence is a test someone will delete when it gets in the
way, without knowing what was being protected.

**The test measures behavior, not implementation.** Call the method by slot, as the game calls it,
and check what came out on the surface or in the return register — not check that an internal
function was called. That is what lets you rewrite the internals without rewriting the test.

## The scan needs a clean cache, and the reason is not obvious

The scan of the 62 ROMs compares each game's summary with the committed baseline. It failed in six
batches out of eight after an audio change, and the scare was false: **the game writes its own
files inside the extracted package** — `udata/dooopt.sav`, `udata/options`, `default.opt` — and
the extraction cache survives between runs.

The effect is exactly the opposite of what it looks like: with the file already written by an
earlier run, the `open` that used to fail starts working, the report **loses** a finding, and the
baseline flags a difference. Five games "changed" because of a save left behind:

```text
Iron Sight        - udata/dooopt.sav
Karnov's Revenge  - open failed: .../udata/options
                  - open failed: .../default.opt
```

Clearing the cache **before** each batch gives back the same result as always: no difference.

Two things stay from this measurement:

1. The scan script clears the cache at both ends of the batch, not just at the end.
2. Not every test failure is a regression. The test fails for two reasons — a category that does
   not pass (a game known to be off the list) and a baseline difference —, and only the second
   matters. The script's summary separates the two, otherwise six known games hide the answer you
   want.

The script is `tools/batch_scan.py`, and it carries both lessons:

```bash
python3 tools/batch_scan.py --roms /path/to/roms --ms 6000
```

`--ms` has to be the same the baseline was written with (6000). Measured: with 3000 the same game
shows up as a baseline difference — the summary has the screen and the findings of the instant the
measurement stopped, and stopping at another instant changes both.
