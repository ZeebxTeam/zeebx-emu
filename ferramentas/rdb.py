#!/usr/bin/env python3
"""RDB do RetroArch para os títulos de Zeebo.

O formato foi levantado do código de referência do RetroArch (`libretro-db/libretrodb.c` e
`database_info.c`) e conferido contra os 135 RDBs instalados nesta máquina:

```text
16 bytes     "RARCHDB\\0" + uint64 **big-endian** do offset dos metadados
registros    mapas msgpack, um atrás do outro
1 byte       0xC0 (nulo, sentinela de fim)
metadados    mapa msgpack { "count": N }
```

Cada registro é um mapa com os campos que o scanner lê:

| campo | tipo | para que serve |
|---|---|---|
| `name` | string | o nome que o RetroArch mostra |
| `description` | string | o mesmo nome |
| `rom_name` | string | o arquivo que aquela entrada identifica |
| `size` | uint | tamanho em bytes |
| `crc` | binário de 4 bytes, big-endian | a chave da busca |
| `md5`, `sha1` | binário de 16 e 20 bytes | conferência extra |

O scanner consulta `crc:or(b"<crc do arquivo dentro do pacote>", b"<crc do pacote>")`, então o
mesmo jogo é registrado **duas vezes**: com o CRC do `.zip` e com o CRC do arquivo que o No-Intro
hasheia dentro dele. Assim o acervo casa tanto pelo pacote quanto pelo ROM interno, e um `.zip`
recompactado ainda encontra a entrada pelo arquivo de dentro.
"""

import pathlib


def mp_str(texto):
    b = texto.encode("utf-8")
    n = len(b)
    if n < 32:
        return bytes([0xA0 | n]) + b
    if n < 256:
        return b"\xd9" + bytes([n]) + b
    if n < 65536:
        return b"\xda" + n.to_bytes(2, "big") + b
    return b"\xdb" + n.to_bytes(4, "big") + b


def mp_bin(dados):
    n = len(dados)
    if n < 256:
        return b"\xc4" + bytes([n]) + dados
    if n < 65536:
        return b"\xc5" + n.to_bytes(2, "big") + dados
    return b"\xc6" + n.to_bytes(4, "big") + dados


def mp_uint(valor):
    if valor < 0x80:
        return bytes([valor])
    if valor < 0x100:
        return b"\xcc" + bytes([valor])
    if valor < 0x10000:
        return b"\xcd" + valor.to_bytes(2, "big")
    if valor < 0x100000000:
        return b"\xce" + valor.to_bytes(4, "big")
    return b"\xcf" + valor.to_bytes(8, "big")


def mp_map(quantos):
    if quantos < 16:
        return bytes([0x80 | quantos])
    return b"\xde" + quantos.to_bytes(2, "big")


def registro(nome, descricao, rom_name, tamanho, crc, md5=None, sha1=None):
    """Um registro no formato do `libretrodb`, na mesma ordem de campos dos RDBs oficiais."""
    campos = [
        (b"name", mp_str(nome)),
        (b"description", mp_str(descricao)),
        (b"rom_name", mp_str(rom_name)),
        (b"size", mp_uint(tamanho)),
        (b"crc", mp_bin(crc.to_bytes(4, "big"))),
    ]
    if md5:
        campos.append((b"md5", mp_bin(md5)))
    if sha1:
        campos.append((b"sha1", mp_bin(sha1)))
    corpo = bytearray()
    for chave, valor in campos:
        corpo += mp_str(chave.decode("ascii")) + valor
    return mp_map(len(campos)) + bytes(corpo)


def escreve(destino, registros):
    """Grava o RDB. `registros` é uma lista de bytes já montados por [`registro`]."""
    corpo = bytearray(b"RARCHDB\x00" + b"\x00" * 8)
    for um in registros:
        corpo += um
    corpo += b"\xc0"  # sentinela: o leitor para aqui
    offset = len(corpo)
    corpo += mp_map(1) + mp_str("count") + mp_uint(len(registros))
    corpo[8:16] = offset.to_bytes(8, "big")
    destino = pathlib.Path(destino)
    destino.write_bytes(corpo)
    return destino
