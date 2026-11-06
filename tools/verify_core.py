#!/usr/bin/env python3
"""Checks that the core library **loads** and exports the whole ABI.

It is not `nm` over a list of symbols: here the library is opened the way the frontend opens it,
with the `dlopen`/`LoadLibrary` behind `ctypes`, and the functions are really called. That catches
two things that reading the symbols does not:

- a library that **exists but does not load** (a dependency missing at link time);
- an ABI that answers wrong, and not just one that is absent: `retro_api_version` has to return 1,
  and `retro_get_system_info` has to fill the struct with what the core declares.

    python3 tools/verify_core.py path/to/zeebx_libretro.so
"""

import ctypes
import pathlib
import sys

# The base surface of the ABI, the same one `libretro.h` declares.
SYMBOLS = [
    "retro_api_version",
    "retro_cheat_reset",
    "retro_cheat_set",
    "retro_deinit",
    "retro_get_memory_data",
    "retro_get_memory_size",
    "retro_get_region",
    "retro_get_system_av_info",
    "retro_get_system_info",
    "retro_init",
    "retro_load_game",
    "retro_load_game_special",
    "retro_reset",
    "retro_run",
    "retro_serialize",
    "retro_serialize_size",
    "retro_set_audio_sample",
    "retro_set_audio_sample_batch",
    "retro_set_controller_port_device",
    "retro_set_environment",
    "retro_set_input_poll",
    "retro_set_input_state",
    "retro_set_video_refresh",
    "retro_unload_game",
    "retro_unserialize",
]

RETRO_API_VERSION = 1


class SystemInfo(ctypes.Structure):
    """`retro_system_info`, in `libretro.h`'s order."""

    _fields_ = [
        ("library_name", ctypes.c_char_p),
        ("library_version", ctypes.c_char_p),
        ("valid_extensions", ctypes.c_char_p),
        ("need_fullpath", ctypes.c_bool),
        ("block_extract", ctypes.c_bool),
    ]


def main(argv):
    if len(argv) != 2:
        print(__doc__)
        return 2
    path = pathlib.Path(argv[1])
    if not path.is_file():
        print(f"does not exist: {path}", file=sys.stderr)
        return 1

    try:
        library = ctypes.CDLL(str(path))
    except OSError as error:
        print(f"the library did not load: {error}", file=sys.stderr)
        return 1
    print(f"loaded: {path}")

    missing = [symbol for symbol in SYMBOLS if not hasattr(library, symbol)]
    if missing:
        print("missing symbols:", ", ".join(missing), file=sys.stderr)
        return 1
    print(f"ABI: {len(SYMBOLS)} symbols exported")

    library.retro_api_version.restype = ctypes.c_uint
    version = library.retro_api_version()
    if version != RETRO_API_VERSION:
        print(f"retro_api_version returned {version}, expected {RETRO_API_VERSION}", file=sys.stderr)
        return 1
    print(f"retro_api_version: {version}")

    info = SystemInfo()
    library.retro_get_system_info(ctypes.byref(info))
    name = (info.library_name or b"").decode()
    extensions = (info.valid_extensions or b"").decode()
    print(f"system: {name!r} | extensions: {extensions!r} | fullpath: {info.need_fullpath}")
    if name != "Zeebx":
        print("the system name is not the expected one", file=sys.stderr)
        return 1
    if extensions != "mod|zip|7z":
        print("the extensions are not the expected ones", file=sys.stderr)
        return 1
    if not info.need_fullpath or not info.block_extract:
        print("need_fullpath and block_extract have to be true", file=sys.stderr)
        return 1
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
