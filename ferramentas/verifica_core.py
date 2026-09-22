#!/usr/bin/env python3
"""Confere que a biblioteca do core carrega e exporta a ABI Libretro."""

import ctypes
import pathlib
import sys

SIMBOLOS = [
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


class SistemaInfo(ctypes.Structure):
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
    caminho = pathlib.Path(argv[1])
    if not caminho.is_file():
        print(f"não existe: {caminho}", file=sys.stderr)
        return 1
    try:
        biblioteca = ctypes.CDLL(str(caminho))
    except OSError as erro:
        print(f"a biblioteca não carregou: {erro}", file=sys.stderr)
        return 1
    faltando = [s for s in SIMBOLOS if not hasattr(biblioteca, s)]
    if faltando:
        print("símbolos que faltam:", ", ".join(faltando), file=sys.stderr)
        return 1
    biblioteca.retro_api_version.restype = ctypes.c_uint
    if biblioteca.retro_api_version() != 1:
        print("retro_api_version não devolveu 1", file=sys.stderr)
        return 1
    info = SistemaInfo()
    biblioteca.retro_get_system_info(ctypes.byref(info))
    nome = (info.library_name or b"").decode()
    extensoes = (info.valid_extensions or b"").decode()
    if nome != "Zeebx":
        print(f"nome inesperado: {nome!r}", file=sys.stderr)
        return 1
    if extensoes != "mod|zip":
        print(f"extensões inesperadas: {extensoes!r}", file=sys.stderr)
        return 1
    if not info.need_fullpath or not info.block_extract:
        print("need_fullpath e block_extract têm de ser verdadeiros", file=sys.stderr)
        return 1
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
