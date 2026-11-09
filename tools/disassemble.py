#!/usr/bin/env python3
"""Disassembles a stretch of a game's module, resolving the literals and the strings.

The emulator already shows **what** was executed (`--code`) and **what** was called (`--trace`).
What was missing was reading the game's code: why it branched, what it tested, which constant was
in the pool. Without that, "the app dies at 0x78b70" and "0x78b70 is the error branch that only
prints the message" are indistinguishable — and they were, for a good while.

Usage:  python3 tools/disassemble.py 0x83420 0x834a0 [game-pattern]

The mapping base (`BASE`) is the Z-Wheel's `tectoy.mod`. For another module, find it like this:
take a string pointer that showed up in an emulator trace — `IFILEMGR_OpenFile` receives the path
in `r1` —, look for the contents inside the file and subtract the offset from the address.

Needs capstone: `pacman -S python-capstone`.
"""
import sys, pathlib, struct, re
from capstone import Cs, CS_ARCH_ARM, CS_MODE_ARM

BASE = 0x10000  # byte zero of the file: the loader maps the image at MODULE_BASE - MODULE_PREFIX and zeroes the prefix

def load(pattern='Z-Wheel'):
    root = pathlib.Path.home()/'.config/zeebx/cache'
    d = [p for p in root.iterdir() if pattern in p.name][0]
    return max((m for m in d.rglob('*.mod')), key=lambda m: m.stat().st_size).read_bytes()

def text(data, addr, n=70):
    off = addr - BASE
    if not (0 <= off < len(data)):
        return None
    end = data.find(b'\0', off, off + n)
    if end < 0:
        return None
    raw = data[off:end]
    if len(raw) >= 4 and all(0x20 <= b < 0x7f or b in (9, 10) for b in raw):
        return raw.decode('latin1')
    return None

def main():
    start = int(sys.argv[1], 16)
    end = int(sys.argv[2], 16) if len(sys.argv) > 2 else start + 0x100
    data = load(sys.argv[3] if len(sys.argv) > 3 else 'Z-Wheel')
    md = Cs(CS_ARCH_ARM, CS_MODE_ARM)
    chunk = data[start-BASE:end-BASE]
    for i in md.disasm(chunk, start):
        note = ''
        # literal pool: ldr rX, [pc, #n]
        m = re.search(r'\[pc, #(-?\d+)\]', i.op_str)
        if m:
            target = i.address + 8 + int(m.group(1))
            off = target - BASE
            if 0 <= off < len(data) - 4:
                value = struct.unpack('<I', data[off:off+4])[0]
                note = f'   ; = {value:#010x}'
                s = text(data, value)
                if s:
                    note += f' -> {s!r}'
        if not note:
            s = text(data, i.address)
            if s and len(s) > 6:
                note = f'   ; text: {s!r}'
        print(f'  {i.address:#08x}  {i.mnemonic:<8} {i.op_str}{note}')

main()
