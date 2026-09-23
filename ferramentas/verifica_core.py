#!/usr/bin/env python3
"""Confere que a biblioteca do core **carrega** e exporta a ABI inteira.

Não é `nm` numa lista de símbolos: aqui a biblioteca é aberta como o frontend a abre, com o
`dlopen`/`LoadLibrary` por trás do `ctypes`, e as funções são chamadas de verdade. Isso apanha
duas coisas que a leitura de símbolos não apanha:

- biblioteca que **existe mas não carrega** (dependência que faltou no link);
- ABI que responde errado, e não só ausente: `retro_api_version` tem de devolver 1, e
  `retro_get_system_info` tem de preencher a struct com o que o core declara.

    python3 ferramentas/verifica_core.py caminho/para/zeebx_libretro.so
"""

import ctypes
import pathlib
import sys

# A superfície base da ABI, a mesma que o `libretro.h` declara.
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

RETRO_API_VERSION = 1


class SistemaInfo(ctypes.Structure):
    """`retro_system_info`, na ordem do `libretro.h`."""

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
    print(f"carregou: {caminho}")

    faltando = [simbolo for simbolo in SIMBOLOS if not hasattr(biblioteca, simbolo)]
    if faltando:
        print("símbolos que faltam:", ", ".join(faltando), file=sys.stderr)
        return 1
    print(f"ABI: {len(SIMBOLOS)} símbolos exportados")

    biblioteca.retro_api_version.restype = ctypes.c_uint
    versao = biblioteca.retro_api_version()
    if versao != RETRO_API_VERSION:
        print(f"retro_api_version devolveu {versao}, esperado {RETRO_API_VERSION}", file=sys.stderr)
        return 1
    print(f"retro_api_version: {versao}")

    info = SistemaInfo()
    biblioteca.retro_get_system_info(ctypes.byref(info))
    nome = (info.library_name or b"").decode()
    extensoes = (info.valid_extensions or b"").decode()
    print(f"sistema: {nome!r} | extensões: {extensoes!r} | fullpath: {info.need_fullpath}")
    if nome != "Zeebx":
        print("o nome do sistema não é o esperado", file=sys.stderr)
        return 1
    if extensoes != "mod|zip|7z":
        print("as extensões não são as esperadas", file=sys.stderr)
        return 1
    if not info.need_fullpath or not info.block_extract:
        print("need_fullpath e block_extract têm de ser verdadeiros", file=sys.stderr)
        return 1
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
