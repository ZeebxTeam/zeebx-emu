#!/usr/bin/env python3
"""Checks the vtable tables in `src/brew/aee_slots.rs` against the BREW SDK headers.

The order of a table is ABI: one missing or swapped method misaligns every call after it, and the
game does not crash there — it lands on the wrong method, which answers something plausible. The
tables were transcribed from the headers by hand, and this script is the transcription read back.

The headers are Qualcomm's and do not go into the repository, so this does not run in CI. Point
it at the root of an installed BREW 4.0.2 platform (the folder that holds `inc/` and `sdk/inc/`;
`BMP_BREWPLATFORM_4.0.2.20_SETUP_0.exe` extracts with 7-Zip):

    python3 tools/check_sdk_slots.py "path/to/BREW 4.0.2 SP19"

It only reports. Exit status 1 means at least one table disagrees with its header; a table with
no header (read from the firmware or found with the probe) is listed as such and is not an error.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

SLOTS = Path(__file__).resolve().parent.parent / "src" / "brew" / "aee_slots.rs"

# Table in `aee_slots.rs` -> interface in the SDK. Written by hand, not guessed from the doc
# comments: three of those comments sit above the wrong table, and a guess would compare the
# wrong pair without saying so.
INTERFACES = {
    "SHELL": "IShell",
    "MODULE": "IModule",
    "APPLET": "IApplet",
    "FILEMGR": "IFileMgr",
    "FILE": "IFile",
    "DISPLAY": "IDisplay",
    "BITMAP": "IBitmap",
    "TRANSFORM": "ITransform",
    "SIGNAL": "ISignal",
    "SIGNALCTL": "ISignalCtl",
    "SIGNALCBFACTORY": "ISignalCBFactory",
    "GRAPHICS": "IGraphics",
    "SOUND": "ISound",
    "SOUND_PLAYER": "ISoundPlayer",
    "LICENSE": "ILicense",
    "MEMASTREAM": "IMemAStream",
    "EGL_SURFACE_MANIP": "IEGLSurfaceManip",
    "GLES_IMAGEON_EXT": "IGLESImageonExt",
    "IMAGE_DECODER": "IImageDecoder",
    "FORCE_FEED": "IForceFeed",
    "IMAGE": "IImage",
    "THREAD": "IThread",
    "EGL": "IEGL11",
    "GLES11_EXT_PAK": "IGLES11ExtPak",
    "GLES10_EXT": "IGLES10Ext",
    "JOYSTICK": "IJoystick",
    "EGL_GET_POWER_LEVEL": "IEGLGetPowerLevel",
    "EGL_GET_COLOR_BUFFER": "IEGLGetColorBuffer",
    "EGL_OES_SWAP_INTERVAL": "IEGLOESSwapInterval",
    "GLES11_EXT": "IGLES11Ext",
    "GLES": "IGLES11",
    "MEDIAUTIL": "IMediaUtil",
    "MEDIA": "IMedia",
    "EGL_LEGACY": "IEGL",
    "GL_LEGACY": "IGL",
    "WEB": "IWeb",
    "SOURCE": "ISource",
    "GET_LINE": "IGetLine",
    "SOURCE_UTIL": "ISourceUtil",
    "CONTROL": "IControl",
    "FONT": "IFont",
    "HASH": "IHash",
    "HASH_CTX": "IHashCtx",
    "CIPHER_FACTORY": "ICipherFactory",
    "CIPHER": "ICipher1",
    "HEAP": "IHeap",
    "UNZIP_STREAM": "IUnzipAStream",
}

# Tables with no header to compare against: the Zeebo's own SDK (`IHID`), classes read from the
# firmware's vtables, and those answered by observation with the probe.
WITHOUT_HEADER = {
    "CANVAS", "HID", "HIDDEVICE", "SQL_MGR", "SQL_DATABASE", "WIDGET",
    "ZEEBO_MCP", "SIM_CARD_CTL", "SYSTEM_CTL", "CM", "CLASS_28E3C", "TYPEFACE", "VECTOR",
    "COLLECTION", "CONFIG",
}

# Tables that are longer than their header on purpose. Only the header's part is compared; the
# rest is ours and is listed, not judged.
#
# - GLES, EGL_LEGACY, GL_LEGACY: after the vtable come the functions `eglGetProcAddress` hands
#   out (`DrawTex*OES`, the QUALCOMM EGL extensions, the IGLES11-only ones). They sit at the end
#   so that they do not shift the real slots — see the comments in `aee_slots.rs`.
# - CONTROL: Zenonia's `0x01003109`, an `IControl` whose firmware vtable has 41 slots; the first
#   eleven are the SDK's.
EXTENDED = {"GLES", "EGL_LEGACY", "GL_LEGACY", "CONTROL"}

# Tables where the console's firmware was read and disagrees with the header. The firmware is
# what the game talks to, so it wins; the header's version is printed for whoever revisits it.
#
# - WEB: the firmware's `IWeb` vtable at `0x1087fde0` has 13 slots, with the varargs
#   `GetResponse` at 11; `AEEWeb.h` in SDK 4.0.2 has 8, with `GetResponse` at 6.
# - SOURCE_UTIL: the firmware's `ISourceUtil` vtable at `0x10a785e4` was read with 7 slots;
#   `AEESource.h` has 10. The seven agree by name.
FIRMWARE_OVER_HEADER = {"WEB", "SOURCE_UTIL"}

# What the base macros expand to. They are declared in `AEEInterface.h`/`AEEIQI.h` in shapes
# that change between SDK versions; spelling them out is shorter than parsing every variant.
BASES = {
    "DECLARE_IBASE": ["AddRef", "Release"],
    "INHERIT_IBase": ["AddRef", "Release"],
    "INHERIT_IQI": ["AddRef", "Release", "QueryInterface"],
    "INHERIT_IQueryInterface": ["AddRef", "Release", "QueryInterface"],
}


def strip_comments(text: str) -> str:
    # One pass, left to right, the way the preprocessor does it. Two passes break on the
    # `//*****` banners these headers are full of: the block pass sees a `/*` there and swallows
    # everything up to the next `*/`, and `AEEFile.h` shrank from 68 KB to 98 bytes.
    return re.sub(r"//[^\n]*|/\*.*?\*/", " ", text, flags=re.S)


def top_level_statements(body: str) -> list[str]:
    """Splits a declaration body on `;`, ignoring those inside parentheses."""
    statements, depth, start = [], 0, 0
    for i, ch in enumerate(body):
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        elif ch == ";" and depth == 0:
            statements.append(body[start:i])
            start = i + 1
    statements.append(body[start:])
    return [s.strip() for s in statements if s.strip()]


# One entry of a declaration body: either an inherited macro or a function pointer member.
INHERIT = re.compile(r"^((?:INHERIT|DECLARE)_\w+)\s*\(")
MEMBER = re.compile(r"\(\s*(?:AEE_CDECL\s+|__cdecl\s+)?\*\s*(\w+)\s*\)")


def parse_statement(statement: str) -> list[tuple[str, str]]:
    inherit = INHERIT.match(statement)
    if not inherit:
        member = MEMBER.search(statement)
        return [("method", member.group(1))] if member else []
    entry = ("inherit", inherit.group(1))
    # `DECLARE_IBASE(IGraphics)` is written with no `;` after it, so the first method of the
    # interface arrives glued to it in the same statement. What follows the closing parenthesis
    # is a declaration of its own.
    depth = 0
    for i in range(inherit.end() - 1, len(statement)):
        if statement[i] == "(":
            depth += 1
        elif statement[i] == ")":
            depth -= 1
            if depth == 0:
                return [entry] + parse_statement(statement[i + 1 :].strip())
    return [entry]


def parse_body(body: str) -> list[tuple[str, str]]:
    """`[("inherit", "IBase"), ("method", "AddRef"), ...]` in declaration order."""
    entries = []
    for statement in top_level_statements(body):
        entries.extend(parse_statement(statement))
    return entries


def read_headers(root: Path) -> tuple[dict, dict]:
    """Collects the `INHERIT_X`/`DECLARE_X` macros, keyed by their full name, and the
    `QINTERFACE`/`AEEINTERFACE` bodies, keyed by the interface."""
    macros: dict[str, tuple[list, Path]] = {}
    bodies: dict[str, tuple[list, Path]] = {}
    headers = sorted(root.glob("inc/*.h")) + sorted(root.glob("sdk/inc/*.h"))
    if not headers:
        sys.exit(f"error: no header under {root}/inc or {root}/sdk/inc")
    for header in headers:
        text = header.read_bytes().decode("latin-1").replace("\r\n", "\n")
        # Comments go before the continuation lines are joined: a `//` at the end of one line of
        # a macro would otherwise swallow every line after it.
        text = strip_comments(text).replace("\\\n", " ")
        for m in re.finditer(r"#\s*define\s+((?:INHERIT|DECLARE)_\w+)\s*\(\s*\w+\s*\)([^\n]*)", text):
            macros.setdefault(m.group(1), (parse_body(m.group(2)), header))
        # `#define INHERIT_IWebOpts INHERIT_IxOpts`: one name for another, with no parameter.
        for m in re.finditer(r"#\s*define\s+((?:INHERIT|DECLARE)_\w+)[ \t]+((?:INHERIT|DECLARE)_\w+)[ \t]*\n", text):
            macros.setdefault(m.group(1), ([("inherit", m.group(2))], header))
        for m in re.finditer(r"\b(?:QINTERFACE|AEEINTERFACE)\s*\(\s*(\w+)\s*\)\s*\{(.*?)\}", text, re.S):
            bodies.setdefault(m.group(1), (parse_body(m.group(2)), header))
    return macros, bodies


def lookup(name: str, macros: dict, bodies: dict):
    """An interface (`IFont`) or a macro (`INHERIT_IFont`, `DECLARE_ISOURCE`) to its entry."""
    if name in macros:
        return macros[name]
    if f"INHERIT_{name}" in macros:
        return macros[f"INHERIT_{name}"]
    return bodies.get(name)


def expand(name: str, macros: dict, bodies: dict, seen: frozenset = frozenset()) -> list[str] | None:
    if name in BASES:
        return list(BASES[name])
    if name in seen:
        return None
    entry = lookup(name, macros, bodies)
    if entry is None:
        return None
    entries = entry[0]
    methods: list[str] = []
    for kind, value in entries:
        if kind == "method":
            methods.append(value)
            continue
        inherited = expand(value, macros, bodies, seen | {name})
        if inherited is None:
            return None
        methods.extend(inherited)
    return methods


def source_of(name: str, macros: dict, bodies: dict) -> str:
    entry = lookup(name, macros, bodies)
    return entry[1].name if entry else "?"


def read_tables() -> dict[str, list[str]]:
    text = re.sub(r"//[^\n]*", " ", SLOTS.read_text())
    tables = {}
    for m in re.finditer(r"pub const (\w+): &\[&str\]\s*=\s*&\[(.*?)\];", text, re.S):
        tables[m.group(1)] = re.findall(r'"([^"]*)"', m.group(2))
    return tables


# The old `IEGL`/`IGL` drop the `egl`/`gl` prefix so that the same code can serve them and
# `IEGL11`/`IGLES11`; the comparison ignores the prefix and the case, nothing else.
def same_name(ours: str, sdk: str) -> bool:
    def norm(name: str) -> str:
        name = name.lower()
        for prefix in ("egl", "gl"):
            if name.startswith(prefix) and len(name) > len(prefix):
                name = name[len(prefix):]
        return name
    return ours == sdk or norm(ours) == norm(sdk)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("sdk", type=Path, help="BREW platform root (holds inc/ and sdk/inc/)")
    args = parser.parse_args()

    macros, bodies = read_headers(args.sdk)
    tables = read_tables()
    disagreements = 0

    for table, ours in tables.items():
        if table in WITHOUT_HEADER:
            print(f"skip  {table:22} no SDK header ({len(ours)} slots)")
            continue
        interface = INTERFACES.get(table)
        if interface is None:
            print(f"????  {table:22} not mapped to an interface: add it to INTERFACES or WITHOUT_HEADER")
            disagreements += 1
            continue
        sdk = expand(interface, macros, bodies)
        if sdk is None:
            print(f"????  {table:22} {interface} not found in the headers")
            disagreements += 1
            continue
        where = source_of(interface, macros, bodies)
        compared = len(sdk) if table in EXTENDED and len(ours) > len(sdk) else max(len(ours), len(sdk))
        problems = []
        for slot in range(compared):
            mine = ours[slot] if slot < len(ours) else "—"
            theirs = sdk[slot] if slot < len(sdk) else "—"
            if not same_name(mine, theirs):
                problems.append(f"        slot {slot:3}: ours {mine!r:34} SDK {theirs!r}")
        if table in FIRMWARE_OVER_HEADER and problems:
            print(f"fw    {table:22} {interface} ({where}): ours {len(ours)} from the firmware, SDK {len(sdk)}")
            print("\n".join(problems))
        elif problems:
            disagreements += 1
            print(f"DIFF  {table:22} {interface} ({where}): ours {len(ours)}, SDK {len(sdk)}")
            print("\n".join(problems))
        elif compared < len(ours):
            print(f"ok    {table:22} {interface} ({where}), {len(sdk)} slots + {len(ours) - len(sdk)} of ours")
        else:
            print(f"ok    {table:22} {interface} ({where}), {len(ours)} slots")

    # A table that exists only on another branch is not an error here: the check is of what is
    # in this tree.
    for table in sorted(set(INTERFACES) - set(tables)):
        print(f"note  {table:22} mapped here but not in this tree's aee_slots.rs")

    print(f"\n{disagreements} table(s) disagree with the SDK")
    return 1 if disagreements else 0


if __name__ == "__main__":
    sys.exit(main())
