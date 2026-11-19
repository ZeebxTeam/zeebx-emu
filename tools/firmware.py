#!/usr/bin/env python3
"""Disassembles the console's firmware and looks for what it does with a constant.

The classes the Zeebo applications ask for — the root form, the collection, the TrueType font,
`SQLMGR` — are implemented inside `1.1.2_APPS.bin`, the 21 MB ARM ELF of the APPS partition. They
are not loose modules in the file system: looked for there, they do not exist.

That is better than it looks. The emulator's probe discovers one slot per run, by hypothesis and
test; here the method table **is written down**. Reading is more reliable than deducing.

Usage:
    python3 tools/firmware.py class 0x01001011       # the class's constructor and vtable
    python3 tools/firmware.py class 0x0100102e 80    # the same, with another slot ceiling
    python3 tools/firmware.py refs 0x01001011        # where the constant shows up
    python3 tools/firmware.py disassemble 0x112f84dc 0x112f8502 thumb

The Portuguese command names (`classe`, `desmonta`) are still accepted.

The firmware mixes ARM and Thumb, and a good part of the application code is Thumb. When the
disassembly comes out as a meaningless sequence of `strb`/`movs`, it is the other mode — or it is
text, which also happens: the pointers in the log tables point to strings, and disassembling them
gives plausible and false code.

The firmware path comes from `ZEEBX_FIRMWARE` or from the default in `vendor/`.
"""

import os
import pathlib
import re
import struct
import sys

from capstone import CS_ARCH_ARM, CS_MODE_ARM, CS_MODE_THUMB, Cs

DEFAULT = "vendor/zeebo/nand/1.1.2_APPS.bin"


def segments(data):
    """The loadable segments, as `(offset, vaddr, size)`."""
    e_phoff = struct.unpack("<I", data[28:32])[0]
    e_phentsize, e_phnum = struct.unpack("<HH", data[42:46])
    out = []
    for i in range(e_phnum):
        o = e_phoff + i * e_phentsize
        p_type, p_offset, p_vaddr, _, p_filesz = struct.unpack("<5I", data[o : o + 20])
        if p_type == 1 and p_filesz:
            out.append((p_offset, p_vaddr, p_filesz))
    return out


def to_address(segs, offset):
    for off, vaddr, size in segs:
        if off <= offset < off + size:
            return vaddr + (offset - off)
    return None


def to_offset(segs, address):
    for off, vaddr, size in segs:
        if vaddr <= address < vaddr + size:
            return off + (address - vaddr)
    return None


def tables(data, segs):
    """All the firmware's class tables, as `{CLSID: [(address, flags, a, b)]}`.

    The entry is sixteen bytes long and has the shape `<u32 CLSID> <u32 flags> <u32 a> <u32 b>`,
    with the constructor in `b` — and sometimes a second pointer in `a`, as in `AEECLSID_FILEMGR`.

    **Looking for an isolated entry does not work**, and it is what masked the real size of the
    record. The flags are not a handful of values: `0x1`, `0x4`, `0x5`, `0x8`, `0x20004`,
    `0x2000004`, `0xffff0000`… show up. Filtering by a list of them gave 105 entries; what
    identifies the table is the **run**, four or more consecutive entries whose first field falls
    in the ClassID range. That way 84 tables and 704 entries come out, with 425 distinct classes.

    The biggest one is at `0x10c3a018`, with 62 entries.
    """
    out = {}
    o = 0
    while o + 16 <= len(data):
        if not is_clsid(struct.unpack_from("<I", data, o)[0]):
            o += 4
            continue
        start = o
        while o + 16 <= len(data) and is_clsid(struct.unpack_from("<I", data, o)[0]):
            o += 16
        if (o - start) // 16 < 4:
            continue
        for at in range(start, o, 16):
            c, flags, a, b = struct.unpack("<4I", data[at : at + 16])
            out.setdefault(c, []).append((to_address(segs, at), flags, a, b))
    return out


def is_clsid(x):
    """The range of the BREW ClassIDs on this console."""
    return 0x01000000 <= x < 0x01200000


def registration(data, segs, clsid):
    """The firmware class table's entry, if there is one.

    Returns `(constructor, flags)`. The constructor is the entry's `b` field, or `a` when `b` does
    not look like a function — `AEECLSID_FILEMGR` has both filled in.

    **This reading was once shifted by one word**, taking the previous entry's constructor as if
    it were this one's. The shift broke nothing visibly — it returned a real constructor, in a real
    vtable, of the wrong class. It took disassembling the constructor to notice: what the table
    gave for `0x01006c05` started by comparing the received CLSID with `0x01006c01`, which is
    precisely the entry above.

    The rule that came out of it stays: **disassemble the constructor before copying the vtable**.
    If it tests a CLSID, it has to be the one you asked for.

    **And "not in the table" does not mean "does not exist".** Even with the 84 tables read,
    `0x01028e51`, `0x0100104f`, `0x01035156` and `AEECLSID_SQLMGR` are still absent from the
    whole `1.1.2_APPS.bin` — and the console uses them. They come from something that is not in
    this material.
    """
    for address, flags, a, b in tables(data, segs).get(clsid, []):
        for candidate in (b, a):
            if candidate & 1 and starts_function(data, segs, candidate & ~1):
                return candidate & ~1, flags
        _ = address
    return None, None


def starts_function(data, segs, address):
    """Whether a function really starts there: inside a segment and opening with `push`.

    An address in a loadable segment already rules out almost every false positive, but not all of
    them — a data word can fall in the range by chance. Every one of these constructors we
    disassembled opens by pushing registers, and requiring that closes the rest.
    """
    off = to_offset(segs, address)
    if off is None:
        return False
    md = Cs(CS_ARCH_ARM, CS_MODE_THUMB)
    first = next(md.disasm(data[off : off + 4], address), None)
    return first is not None and first.mnemonic == "push"


# `ldr pc, [pc, #-4]` in ARM: the long jump the linker puts between distant segments. The
# destination address is the word right after the instruction.
VENEER = 0xE51FF004


def through_veneer(data, segs, address):
    """The real destination of an address, going through a long jump if there is one.

    Without this, following a call leads to the trampoline and the disassembly comes out as
    garbage — that is what hid the vtable of the network class Opera Mini asks for.
    """
    off = to_offset(segs, address & ~1)
    if off is None or off + 8 > len(data):
        return address & ~1
    instruction, destination = struct.unpack("<II", data[off : off + 8])
    if instruction == VENEER and to_offset(segs, destination & ~1) is not None:
        return destination & ~1
    return address & ~1


def stores_vtable(data, segs, function, window=0x60):
    """The literal `function` stores in `[obj]`, and the calls it makes along the way.

    The pattern is always the same: it loads a literal and stores it in `[obj]`. We look for the
    first `ldr rX, [pc, #N]` followed by `str rX, [r?]` and return what the literal points to.
    """
    off = to_offset(segs, function)
    if off is None:
        return None, []
    md = Cs(CS_ARCH_ARM, CS_MODE_THUMB)
    target, calls = None, []
    for i in md.disasm(data[off : off + window], function):
        m = re.search(r"\[pc, #(0x[0-9a-f]+|\d+)\]", i.op_str)
        if m and i.mnemonic.startswith("ldr"):
            pos = to_offset(segs, ((i.address + 4) & ~3) + int(m.group(1), 0))
            if pos is not None:
                value = struct.unpack("<I", data[pos : pos + 4])[0]
                if to_offset(segs, value) is not None:
                    target = value
        elif target and i.mnemonic == "str" and re.search(r"\[r\d+\]$", i.op_str):
            return target, calls
        elif i.mnemonic in ("bl", "blx") and i.op_str.startswith("#"):
            calls.append(int(i.op_str[1:], 0))
    return None, calls


def vtable_of(data, segs, constructor, count=64):
    """The vtable a constructor stores in the freshly allocated object.

    **Not every constructor stores the vtable itself.** The ones in the Opera Mini group allocate
    the object and hand the job to an initialization function, reached by a long jump — and the
    whole constructor disassembles without a single `str` into `[obj]`. That is why, when the start
    of the constructor does not deliver the literal, we follow the calls it makes, once.

    It is not a depth-first search on purpose: one level covers the classes we know, and each
    extra level raises the chance of finding the vtable *of something else* — the constructor also
    calls `malloc` and the `CreateInstance` of the object it depends on.

    The vtable ends where the pointers stop being Thumb — on the console's ARM every method is
    Thumb, so the first value with bit 0 cleared is no longer a method.
    """
    target, calls = stores_vtable(data, segs, constructor)
    if target is None:
        for call in calls:
            init = through_veneer(data, segs, call)
            if not starts_function(data, segs, init):
                continue
            target, _ = stores_vtable(data, segs, init)
            if target is not None:
                break
    if target is None:
        return None, []
    base = to_offset(segs, target)
    slots = []
    for i in range(count):
        v = struct.unpack("<I", data[base + i * 4 : base + i * 4 + 4])[0]
        if not v & 1:
            break
        slots.append(v & ~1)
    return target, slots


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return
    data = pathlib.Path(os.environ.get("ZEEBX_FIRMWARE", DEFAULT)).read_bytes()
    segs = segments(data)

    if sys.argv[1] == "refs":
        target = int(sys.argv[2], 0)
        pat = struct.pack("<I", target)
        for m in re.finditer(re.escape(pat), data):
            addr = to_address(segs, m.start())
            if addr is not None:
                print(f"  offset {m.start():#010x}  address {addr:#010x}")
    elif sys.argv[1] in ("class", "classe"):
        clsid = int(sys.argv[2], 0)
        count = int(sys.argv[3], 0) if len(sys.argv) > 3 else 64
        constructor, flags = registration(data, segs, clsid)
        if constructor is None:
            print(f"  {clsid:#010x} is not in this firmware's class table")
            return
        print(f"  constructor {constructor:#010x}  (flags {flags:#x})")
        target, slots = vtable_of(data, segs, constructor, count)
        if target is None:
            print("  did not find the vtable in the constructor nor in what it calls")
            return
        print(f"  vtable      {target:#010x}  ({len(slots)} methods)")
        for i, v in enumerate(slots):
            print(f"    slot[{i:2}] = {v:#010x}")
        # The ceiling exists so as not to go reading data as if it were a method, and for a long
        # time it was invisible: a 59-slot vtable was reported as "16 methods", and the number went
        # into the documentation as if it were the size of the interface. When the count hits the
        # ceiling, it is not an answer — it is the ceiling.
        if len(slots) == count:
            print(f"  warning: the count stopped at the ceiling of {count}; there may be more")
    elif sys.argv[1] in ("disassemble", "desmonta"):
        start, end = int(sys.argv[2], 0), int(sys.argv[3], 0)
        off = to_offset(segs, start)
        if off is None:
            print("address outside the loadable segments")
            return
        md = Cs(CS_ARCH_ARM, CS_MODE_THUMB if "thumb" in sys.argv else CS_MODE_ARM)
        for i in md.disasm(data[off : off + (end - start)], start):
            note = ""
            m = re.search(r"\[pc, #(-?0x[0-9a-f]+|-?\d+)\]", i.op_str)
            if m and i.mnemonic.startswith("ldr"):
                # In ARM `pc` is the instruction plus eight; in Thumb, plus four and aligned.
                if "thumb" in sys.argv:
                    target = ((i.address + 4) & ~3) + int(m.group(1), 0)
                else:
                    target = i.address + 8 + int(m.group(1), 0)
                o = to_offset(segs, target)
                if o is not None and o + 4 <= len(data):
                    value = struct.unpack("<I", data[o : o + 4])[0]
                    note = f"   ; = {value:#010x}"
            print(f"  {i.address:#010x}  {i.mnemonic:<8} {i.op_str}{note}")
    else:
        print(__doc__)


main()
