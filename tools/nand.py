#!/usr/bin/env python3
"""Reads the Zeebo NAND: partition table, extraction and what is known about EFS2.

The dump is published at `tripleoxygen.net/files/devices/zeebo/dump/nand/1.1.2/`.

**The extension modules are not in it.** The firmware carries the paths — `fs:/mod/widgets/
widgets.mod`, `fs:/mif/widgets.mif`, `fs:/mod/htmlwidget/htmlwidget.mod` — as constants, and the
files do not exist in any partition: the string `widgets` shows up thirteen times in the 128 MB and
all of them inside `1.1.2_APPS.bin`. `EFS2APPS` has two modules, `reksio.mod` and `tectoy.mod`.

What one gets from here, then, is the console's file system — the font at
`fs:/shared/fonts/tectoy.ttf`, the two modules — and the confirmation of what is **not** in the
material. The interface classes the Z-Wheel asks for are studied through the firmware's own call
sites.

Usage:
    python3 tools/nand.py partitions PATH/1.1.2.bin
    python3 tools/nand.py extract    PATH/1.1.2.bin EFS2APPS out.bin
    python3 tools/nand.py directory  PATH/part_EFS2APPS.bin 0x1340800
    python3 tools/nand.py names      PATH/part_EFS2APPS.bin

The Portuguese command names (`particoes`, `extrair`, `montar`, `nomes`, `diretorio`) are still
accepted.
"""

import pathlib
import struct
import sys

# The erase block size comes from the table itself: with 128 KB, the computed APPS partition
# matches byte for byte the `1.1.2_APPS.bin` published separately. It is the check that validates
# everything this file does.
BLOCK = 0x20000

# The dump comes in two files. `1.1.2.bin` is only the data area; `1.1.2_spare.bin` is the same
# thing **interleaved** with the out-of-band area, in sectors of 512 plus 16. Checked: the first 512
# bytes of each sector of the interleaved one match the data file.
#
# The OOB, however, has only ECC: the last six bytes of each sector are `0xff`. There is no file
# number nor logical page there, so the EFS2 map is internal, and not the NAND's.
SECTOR, OUT_OF_BAND = 512, 16

# The EFS2 logical page. Files are split into it, and the map is a table of `u32` with the physical
# page of each piece.
PAGE = 0x800


def partitions(data):
    """The partitions, read from the MIBIB. Returns `name -> (offset, size)`."""
    start = data.find(b"0:MIBIB")
    if start < 0:
        return {}
    # The entries are contiguous: name in 16 bytes, then the first block and the count.
    out, at = {}, start - 2
    while at + 32 <= len(data):
        name = data[at + 2 : at + 18].split(b"\0")[0]
        if not name[:2].isdigit() and not (len(name) > 2 and name[1:2] == b":"):
            break
        first, blocks = struct.unpack("<2I", data[at + 18 : at + 26])
        if first > 0x10000:
            break
        size = None if blocks == 0xFFFFFFFF else blocks * BLOCK
        out[name.decode()[2:]] = (first * BLOCK, size)
        at += 28
    return out


def directory_entries(data, at, count=200):
    """Walks an EFS2 directory node starting at `at`.

    The format came from reading the dump, not from documentation:

        <u8 size> <u8 type> <u32 date> <u8 zero> <name> <'i'> <u32 reference>

    `size` counts the name plus the five bytes of the suffix, and that is what allows walking from
    entry to entry.

    The `reference` has not been deciphered yet. It is not the file size — `tt_prefs.db` has 4096
    bytes and its reference is `0x2b165` — nor a direct offset. Read as two `u16` it becomes
    plausible: `tectoy.ttf` becomes `(0x0001, 0x80fc)` and `terms.txt` becomes `(0x0002, 0xb163)`,
    with the first field varying little between files of the same directory. It looks like an inode
    number with a group, but that is pattern reading, not certainty.

    **The `ref` is an index into a page table.** EFS2APPS has regions that are flat arrays of
    `u32`, one per logical page, and `table[ref + n]` is the physical page of the file's n-th 2 KB
    piece. Confirmed with `tectoy.ttf`: its page list, obtained by matching the contents of a known
    copy, shows up literally at `0x4c23104`, and the base that implies leads from its `ref`
    straight to the page with the TrueType header.

    What is missing is finding the **current generation** of the table for each `ref`. The system
    is log-structured and keeps several versions: the base deduced from `tectoy.ttf` works for it
    and not for `tt_prefs.db`, whose entry in that copy is free (`0xfffffff4`).

    Without that the contents cannot be assembled, because EFS2 **does not store a file
    contiguously**: checked against a known copy of `tectoy.ttf`, it matches exactly 16,384 bytes
    and then jumps. The following pieces show up with irregular steps — `+0x4800`, `+0x5000`,
    `+0x10000` —, so there is interleaved metadata and not a fixed step that could be deduced.
    """
    out = []
    while len(out) < count and at + 8 < len(data):
        size = data[at]
        if size < 6 or at + size + 7 > len(data):
            break
        name_end = at + 7 + (size - 5)
        name = data[at + 7 : name_end]
        if not all(32 <= b < 127 for b in name):
            break
        if data[name_end] != ord("i"):
            break
        reference = struct.unpack("<I", data[name_end + 1 : name_end + 5])[0]
        out.append((name.decode(), reference))
        at = name_end + 5
    return out


def all_names(data, minimum=4):
    """Every file name that shows up in a directory node, with how many times.

    It does not need the page map: the directory nodes are in the clear in the dump, and each
    generation of them is a copy. Sweeping everything and gathering gives **the complete list of
    names** of the file system, albeit without the tree and without the contents.

    That is how it was established what EFS2APPS does **not** have: no BREW extension at all. No
    `widgets.mod`, `forms.mod`, `framewidget.mod`, `imenu.mod` or `icontrols.mod` — only the games'
    modules (`tectoy.mod`, `reksio.mod`) and data. Three hundred and seventeen names, and none of
    them is the interface extension the Z-Wheel uses.

    The count works as a signal: a name that shows up four hundred times is a file that was
    rewritten four hundred times, which is what is expected in a log-structured system.
    """
    import collections

    seen = collections.Counter()
    at = 0
    while at < len(data) - 64:
        entries = directory_entries(data, at, 60)
        if len(entries) >= minimum:
            for name, _ in entries:
                seen[name] += 1
            at += sum(len(name) + 7 for name, _ in entries)
        else:
            at += 4
    return seen


def pages_of(data, table_base, ref, count):
    """The physical pages of a file, read from the page table.

    `table_base` is the address that makes `base + ref*4` land on page zero's entry.

    **This is not a complete reader, and the reason is measured.** Assembling `tectoy.ttf` from the
    table found at `0x4c23104`, the first thirteen pages come out **identical** to the known copy
    from the package — which confirms the model: the `ref` indexes an array of `u32` with the
    physical page of each 2 KB piece, in order. From the fourteenth on, not.

    Entry 13 of that copy points to page `0x14f5`, and the correct one is `0x17a6`. It is not
    corruption: it is an **old generation** of the table. EFS2 is log-structured and keeps several
    versions of everything, including of the tables themselves.

    And the current version does not exist anywhere as a contiguous block: looking for the font's
    real list of 94 pages, the 5-page prefix shows up once and the 16-page one shows up none. That
    is, the current map is the base table **plus a journal of changes**, and assembling it requires
    replaying that journal. That is what is missing for the reader to be ready.

    Beware of the verification trap: looking for a page's contents in the dump returns the
    **first** occurrence, and there are old copies of everything. A page list obtained that way
    looks right and mixes versions.
    """
    out = []
    for n in range(count):
        at = table_base + (ref + n) * 4
        if at + 4 > len(data):
            break
        page = struct.unpack("<I", data[at : at + 4])[0]
        if page >= len(data) // PAGE:
            break
        out.append(page)
    return out


def assemble(data, pages, size=None):
    """Joins the pages into a file."""
    out = b"".join(data[p * PAGE : (p + 1) * PAGE] for p in pages)
    return out[:size] if size else out


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return
    command, path = sys.argv[1], sys.argv[2]
    with open(path, "rb") as f:
        data = f.read()

    if command in ("partitions", "particoes"):
        for name, (off, size) in partitions(data).items():
            end = "to the end" if size is None else f"{size // 1024} KB"
            print(f"  {name:10} {off:#010x}  {end}")
    elif command in ("extract", "extrair"):
        target, out = sys.argv[3], sys.argv[4]
        off, size = partitions(data)[target]
        end = len(data) if size is None else off + size
        with open(out, "wb") as f:
            f.write(data[off:end])
        print(f"  {target}: {end - off} bytes in {out}")
    elif command in ("assemble", "montar"):
        # `assemble FILE base ref size` — joins a file from the page table.
        base, ref, size = (int(x, 0) for x in sys.argv[3:6])
        pgs = pages_of(data, base, ref, (size + PAGE - 1) // PAGE)
        out = assemble(data, pgs, size)
        dest = sys.argv[6] if len(sys.argv) > 6 else "assembled.bin"
        pathlib.Path(dest).write_bytes(out)
        print(f"  {len(pgs)} pages -> {len(out)} bytes in {dest}")
    elif command in ("names", "nomes"):
        seen = all_names(data)
        print(f"  {len(seen)} distinct names")
        for name, times in sorted(seen.items()):
            print(f"  {times:5}x  {name}")
    elif command in ("directory", "diretorio"):
        at = int(sys.argv[3], 0)
        for name, ref in directory_entries(data, at):
            print(f"  {name:34} ref={ref:#010x}")
    else:
        print(__doc__)


main()
