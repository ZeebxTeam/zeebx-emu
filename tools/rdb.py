#!/usr/bin/env python3
"""RetroArch RDB for the Zeebo titles.

The format was worked out from RetroArch's reference code (`libretro-db/libretrodb.c` and
`database_info.c`) and checked against the 135 RDBs installed on this machine:

```text
16 bytes     "RARCHDB\\0" + **big-endian** uint64 of the metadata offset
records      msgpack maps, one after the other
1 byte       0xC0 (nil, end sentinel)
metadata     msgpack map { "count": N }
```

Each record is a map with the fields the scanner reads:

| field | type | what it is for |
|---|---|---|
| `name` | string | the name RetroArch shows |
| `description` | string | the same name |
| `rom_name` | string | the file that entry identifies |
| `size` | uint | size in bytes |
| `crc` | 4-byte binary, big-endian | the lookup key |
| `md5`, `sha1` | 16- and 20-byte binary | extra checking |

The scanner queries `crc:or(b"<crc of the file inside the archive>", b"<crc of the archive>")`, so
the same game is recorded **twice**: with the CRC of the `.zip` and with the CRC of the file that
No-Intro hashes inside it. That way the collection matches both by the archive and by the inner
ROM, and a repacked `.zip` still finds its entry through the file inside.
"""

import pathlib


def mp_str(text):
    b = text.encode("utf-8")
    n = len(b)
    if n < 32:
        return bytes([0xA0 | n]) + b
    if n < 256:
        return b"\xd9" + bytes([n]) + b
    if n < 65536:
        return b"\xda" + n.to_bytes(2, "big") + b
    return b"\xdb" + n.to_bytes(4, "big") + b


def mp_bin(data):
    n = len(data)
    if n < 256:
        return b"\xc4" + bytes([n]) + data
    if n < 65536:
        return b"\xc5" + n.to_bytes(2, "big") + data
    return b"\xc6" + n.to_bytes(4, "big") + data


def mp_uint(value):
    if value < 0x80:
        return bytes([value])
    if value < 0x100:
        return b"\xcc" + bytes([value])
    if value < 0x10000:
        return b"\xcd" + value.to_bytes(2, "big")
    if value < 0x100000000:
        return b"\xce" + value.to_bytes(4, "big")
    return b"\xcf" + value.to_bytes(8, "big")


def mp_map(count):
    if count < 16:
        return bytes([0x80 | count])
    return b"\xde" + count.to_bytes(2, "big")


def record(name, description, rom_name, size, crc, md5=None, sha1=None):
    """One record in the `libretrodb` format, in the same field order as the official RDBs."""
    fields = [
        (b"name", mp_str(name)),
        (b"description", mp_str(description)),
        (b"rom_name", mp_str(rom_name)),
        (b"size", mp_uint(size)),
        (b"crc", mp_bin(crc.to_bytes(4, "big"))),
    ]
    if md5:
        fields.append((b"md5", mp_bin(md5)))
    if sha1:
        fields.append((b"sha1", mp_bin(sha1)))
    body = bytearray()
    for key, value in fields:
        body += mp_str(key.decode("ascii")) + value
    return mp_map(len(fields)) + bytes(body)


def write(dest, records):
    """Writes the RDB. `records` is a list of bytes already built by [`record`]."""
    body = bytearray(b"RARCHDB\x00" + b"\x00" * 8)
    for one in records:
        body += one
    body += b"\xc0"  # sentinel: the reader stops here
    offset = len(body)
    body += mp_map(1) + mp_str("count") + mp_uint(len(records))
    body[8:16] = offset.to_bytes(8, "big")
    dest = pathlib.Path(dest)
    dest.write_bytes(body)
    return dest
