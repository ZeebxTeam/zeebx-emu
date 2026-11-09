#!/usr/bin/env python3
"""Runs the ROM scan in batches, separating regressions from games known to be out.

It exists because the scan is the test that holds every game's behavior to account at once, and it
has two traps that have already cost an afternoon each:

1. **The extraction cache has to be cleared BEFORE each batch.** The game writes its own files
   (`udata/dooopt.sav`, `udata/options`, `default.opt`) **inside the extracted package**, and the
   cache survives between runs. With a dirty cache, the `open` that used to fail starts working,
   the report **loses** a pending item, and the baseline flags a regression where there was none.
   Measured: five games "changed" because of a save left behind.
2. **Not every test failure is a regression.** `the_given_rom_progresses` fails for two reasons —
   a category that does not pass (a game known to be off the list) and a baseline difference —,
   and only the second one matters. Without separating the two, six known games hide the answer
   one wants.

And the cache is cleared in batches because extracting all the ROMs at once does not fit on the
disk.

    python3 tools/batch_scan.py --roms <folder> [--batch 8] [--ms 6000]

The Portuguese option names (`--lote`, `--saida`) are still accepted.
"""
import argparse
import os
import pathlib
import re
import shutil
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
CACHE = pathlib.Path.home() / ".config/zeebx/cache"

# What the test prints, matched as data. The Portuguese spellings are the ones `src/scan.rs`
# printed before its translation; both are accepted so the script works on either side of it.
FAIL = r"(?:FAIL|FALHA)"
CHANGED_SINCE = ("changed since", "mudou desde")


def clear_the_cache():
    shutil.rmtree(CACHE, ignore_errors=True)
    CACHE.mkdir(parents=True, exist_ok=True)


def run_the_batch(roms, ms, out):
    """Runs one batch and returns the text of the test's output."""
    folder = pathlib.Path("/tmp/zeebx-lote")
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir(parents=True)
    for rom in roms:
        (folder / rom.name).symlink_to(rom)
    env = {
        **os.environ,
        "ZEEBX_ROM": str(folder),
        "ZEEBX_ROM_MS": str(ms),
        "ZEEBX_ROM_OUT": str(out),
        "ZEEBX_ROM_BASE": str(REPO / "docs/scan"),
        "CARGO_PROFILE_DEV_DEBUG": "0",
        "CARGO_PROFILE_TEST_DEBUG": "0",
        "CARGO_BUILD_JOBS": "2",
    }
    done = subprocess.run(
        ["cargo", "test", "--lib", "--locked", "--no-default-features",
         "scan::tests::the_given_rom_progresses", "--", "--nocapture"],
        cwd=REPO, env=env, capture_output=True, text=True,
    )
    return done.returncode, done.stdout + done.stderr


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--roms", required=True, help="folder with the ROMs' .zip files")
    parser.add_argument("--batch", "--lote", type=int, default=8, help="ROMs per batch (default 8)")
    parser.add_argument("--ms", type=int, default=6000, help="virtual milliseconds per ROM")
    parser.add_argument("--out", "--saida", default="/tmp/zeebx-varredura", help="where to write the reports")
    args = parser.parse_args()

    roms = sorted(pathlib.Path(args.roms).glob("*.zip"))
    if not roms:
        print(f"no ROM in {args.roms}", file=sys.stderr)
        return 1
    out = pathlib.Path(args.out)
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True, exist_ok=True)
    print(f"{len(roms)} ROMs, batches of {args.batch}", flush=True)

    differences, off_list = [], []
    for i in range(0, len(roms), args.batch):
        clear_the_cache()
        batch = roms[i:i + args.batch]
        _, text = run_the_batch(batch, args.ms, out)
        # Each failure starts with "FAIL " and goes up to the next line that is not a continuation.
        for block in re.findall(FAIL + r" (.+?)(?=\n(?:" + FAIL + r"|ok |\n|$))", text, re.S):
            target = differences if any(s in block for s in CHANGED_SINCE) else off_list
            target.append(block.strip().splitlines()[0])
        ready = len(list(out.glob("*.txt")))
        print(f"batch {i // args.batch + 1}: {len(batch)} ROMs, {ready} report(s)", flush=True)
    clear_the_cache()

    print()
    print(f"reports: {len(list(out.glob('*.txt')))} of {len(roms)}")
    print(f"baseline differences: {len(differences)}  <- only this is a regression")
    for d in differences:
        print("   ", d[:160])
    print(f"games off the list (expected): {len(off_list)}")
    for f in off_list:
        print("   ", f[:160])
    return 1 if differences else 0


if __name__ == "__main__":
    sys.exit(main())
