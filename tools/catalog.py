#!/usr/bin/env python3
"""No-Intro catalog of the Zeebo titles, built from a collection of `.zip` files.

What it does, per file:

1. opens the archive and finds the `.mod` and `.mif` inside it;
2. computes size, CRC32, MD5 and SHA1 of the file the DAT declares as that title's ROM
   (No-Intro hashes **one file inside `mod/<id>/`**, whose name in the DAT is `<folder><file>`);
3. checks the dump against the DAT — that is what separates "I have a copy" from "I have the
   verified dump";
4. reads the `.mif` to get the applet's ClassID and the title's icon.

What it writes:

- `<out>/Zeebo - Zeebo.lpl` — RetroArch playlist, with the No-Intro name in `label`;
- `<out>/thumbnails/Mobile - Zeebo/` — the four folders RetroArch looks in, with the exact name
  of each game, ready to receive the covers;
- `<out>/catalogo.json` — the same as data, for the UI here and for checking.

Usage:

    python3 tools/catalog.py --roms /path/to/the/zips --out /path/to/output \\
        [--dat file.dat] [--core /path/zeebx_libretro.so] [--icons]

The Portuguese option names (`--saida`, `--icones`, `--catalogo`, `--capas-ao-lado`) are still
accepted.
"""

import argparse
import hashlib
import json
import pathlib
import re
import sqlite3
import tempfile
import struct
import sys
import urllib.request
import zipfile
import zlib

import rdb

DAT_URL = (
    "https://raw.githubusercontent.com/libretro/libretro-database/master/"
    "metadat/no-intro/Mobile%20-%20Zeebo.dat"
)
DAT_NAME = "Mobile - Zeebo"

# Date of the proposal sent to No-Intro, in the format the database uses on the header's `version`
# line.
PROPOSAL_VERSION = "2026.09.21"

ROM = re.compile(
    r'rom \( name "(?P<name>[^"]+)" size (?P<size>\d+) '
    r"crc (?P<crc>[0-9A-Fa-f]{8}) md5 (?P<md5>[0-9A-Fa-f]{32}) "
    r"sha1 (?P<sha1>[0-9A-Fa-f]{40})"
)

# The keys of each title's record (`entry` below) are also the keys of `catalogo.json`, which is
# written for people and other tools to read. That is why they stay in Portuguese: `nome_no_intro`,
# `no_dat`, `verificado`, `arquivo_do_dat`, `tamanho_zip`... are the file's format, not names in
# this script.


def read_dat(text):
    """Returns `{game name: (file, size, crc, md5, sha1)}`."""
    table = {}
    for block in text.split("game (")[1:]:
        name = re.search(r'name "([^"]+)"', block)
        rom = ROM.search(block)
        if name and rom:
            table[name.group(1)] = (
                rom.group("name"),
                int(rom.group("size")),
                rom.group("crc").upper(),
                rom.group("md5").upper(),
                rom.group("sha1").upper(),
            )
    return table


def download_dat(dest):
    if not dest.is_file():
        dest.write_bytes(urllib.request.urlopen(DAT_URL, timeout=60).read())
    return dest.read_text(encoding="utf-8", errors="replace")


def file_crc(path):
    """CRC32 of a file, read in blocks."""
    crc = 0
    with path.open("rb") as source:
        while True:
            block = source.read(1 << 20)
            if not block:
                break
            crc = zlib.crc32(block, crc)
    return crc & 0xFFFFFFFF


def file_sha1(path):
    """SHA1 of a file, read in blocks."""
    sha1 = hashlib.sha1()
    with path.open("rb") as source:
        while True:
            block = source.read(1 << 20)
            if not block:
                break
            sha1.update(block)
    return sha1.hexdigest().upper()


def hash_member(zf, name):
    """Size, CRC32, MD5 and SHA1 of a file inside the archive, in blocks."""
    crc = 0
    md5 = hashlib.md5()
    sha1 = hashlib.sha1()
    size = 0
    with zf.open(name) as source:
        while True:
            block = source.read(1 << 20)
            if not block:
                break
            size += len(block)
            crc = zlib.crc32(block, crc)
            md5.update(block)
            sha1.update(block)
    return size, f"{crc & 0xFFFFFFFF:08X}", md5.hexdigest().upper(), sha1.hexdigest().upper()


def find_dat_file(names, dat_rom):
    """The DAT's file name is the path inside the archive **without the slashes**.

    `mod274754sound.ggz` is `mod/274754/sound.ggz`; `modnfsresourcestracksworld_3401.viv` is
    `mod/nfs/resources/tracks/world_3401.viv`. The module folder is not always numeric, so the
    comparison is made over the whole path without separators — not by prefix.
    """
    target = dat_rom.replace("\\", "/").replace("/", "").lower()
    for name in names:
        no_slash = name.replace("\\", "/").replace("/", "")
        if no_slash.lower() == target:
            return name
    for name in names:
        no_slash = name.replace("\\", "/").replace("/", "")
        if no_slash.lower().endswith(target):
            return name
    return None


def mif_icon(data):
    """The first image declared in the `.mif`, when there is one.

    The format is in `src/loader/miffile.rs`: each image section starts with the size of its own
    header as a `u16`, followed by the zero-terminated MIME type, and the file right after.
    """
    if len(data) < 0x18:
        return None
    if struct.unpack_from("<H", data, 0)[0] != 0x0011:
        return None
    table, sections = struct.unpack_from("<II", data, 0x10)
    if sections == 0 or sections > 4096:
        return None
    bounds = []
    for i in range(sections + 1):
        if table + i * 4 + 4 > len(data):
            return None
        bounds.append(struct.unpack_from("<I", data, table + i * 4)[0])
    if bounds[-1] > len(data):
        return None
    for start, end in zip(bounds, bounds[1:]):
        section = data[start:end]
        if len(section) < 4:
            continue
        header = struct.unpack_from("<H", section, 0)[0]
        if header < 4 or header > len(section):
            continue
        mime = section[2:header].split(b"\0")[0]
        if mime.startswith(b"image/"):
            return mime.decode("ascii", "replace"), section[header:]
    return None


def analyze(zip_path, table):
    no_intro_name = zip_path.stem
    expected = table.get(no_intro_name)
    with zipfile.ZipFile(zip_path) as zf:
        names = [n for n in zf.namelist() if not n.endswith("/")]
        modules = [n for n in names if n.lower().endswith(".mod")]
        manifests = [n for n in names if n.lower().endswith(".mif")]
        entry = {
            "zip": zip_path.name,
            "nome_no_intro": no_intro_name,
            "no_dat": expected is not None,
            "arquivos": len(names),
            "modulos": modules,
            "manifestos": manifests,
            "verificado": None,
            "crc_zip": None,
            "sha1_zip": None,
            "icone": None,
        }
        # The archive hash is streamed: reading two whole files into memory just to compute two
        # hashes cost, over the whole collection, two passes of 1.6 GB.
        entry["tamanho_zip"] = zip_path.stat().st_size
        entry["crc_zip"] = f"{file_crc(zip_path):08X}"
        entry["sha1_zip"] = file_sha1(zip_path)
        if expected:
            dat_file, size_exp, crc_exp, md5_exp, sha1_exp = expected
            target = find_dat_file(names, dat_file)
            if target:
                size, crc_read, md5_read, sha1_read = hash_member(zf, target)
                entry["verificado"] = (
                    size == size_exp
                    and crc_read == crc_exp
                    and md5_read == md5_exp
                    and sha1_read == sha1_exp
                )
                entry["arquivo_do_dat"] = target
                entry["hash"] = {
                    "tamanho": size,
                    "crc32": crc_read,
                    "md5": md5_read,
                    "sha1": sha1_read,
                }
            else:
                entry["verificado"] = False
                entry["erro"] = f"the DAT's file ({dat_file}) is not in the archive"
        if manifests:
            # The `.mif` that matters is the chosen module's: in an archive with two games, the
            # first manifest may be the other one's. The DAT gives the module folder
            # (`mod/<id>/...`), and its manifest is `mif/<id>.mif`.
            chosen = None
            dat_path = entry.get("arquivo_do_dat")
            if dat_path:
                parts = dat_path.replace("\\", "/").split("/")
                if len(parts) >= 3 and parts[0].lower() == "mod":
                    chosen = f"mif/{parts[1]}.mif"
            if chosen:
                chosen = next((n for n in manifests if n.replace("\\", "/").endswith(chosen)), None)
            if chosen is None:
                chosen = next(
                    (n for n in manifests if mif_clsid(zf.read(n)) is not None), manifests[0]
                )
            data = zf.read(chosen)
            entry["mif"] = chosen
            entry["clsid"] = mif_clsid(data)
            image = mif_icon(data)
            if image:
                mime, image_bytes = image
                entry["icone"] = {"mime": mime, "bytes": image_bytes, "de": manifests[0]}
        return entry


def write_playlist(out, entries, core):
    """RetroArch playlist, in the format RetroArch itself writes.

    It is not "one JSON per line" (the old format): 1.20 writes **one document** with a header and
    the list in `items`. The `crc32` carries the `|crc` suffix and the `db_name` carries the `.lpl`
    extension, the way the scanner writes them — copying the format of whoever reads it is what
    keeps the playlist from opening empty.
    """
    items = []
    for entry in entries:
        items.append(
            {
                "path": str(entry["caminho"]),
                "label": entry["nome_no_intro"],
                "core_path": core or "DETECT",
                "core_name": "Zeebx" if core else "DETECT",
                "crc32": f"{entry['crc_zip']}|crc",
                "db_name": f"{DAT_NAME}.lpl",
            }
        )
    document = {
        "version": "1.5",
        "default_core_path": core or "",
        "default_core_name": "Zeebx" if core else "",
        "label_display_mode": 0,
        "right_thumbnail_mode": 0,
        "left_thumbnail_mode": 0,
        "thumbnail_match_mode": 0,
        "sort_mode": 0,
        "items": items,
    }
    dest = out / f"{DAT_NAME}.lpl"
    dest.write_text(
        json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    return dest


def hash_module_files(zip_path, names, module_id):
    """Every file in a title's `mod/<id>/`, with size and the three hashes.

    No-Intro hashes **one** file from inside the module, and its name in the DAT is
    `<folder><file>`. For a title that is **not** in the DAT, the right file is the submitter's
    call — ours is to list them all, with what each one is. Guessing here would be worse: a dump
    proposed with the wrong file comes back rejected, and then the work is doubled.
    """
    folder = f"mod/{module_id}/"
    lines = []
    import zipfile

    with zipfile.ZipFile(zip_path) as zf:
        for name in sorted(names):
            if not name.startswith(folder) or name.endswith("/"):
                continue
            size, crc, md5, sha1 = hash_member(zf, name)
            lines.append((name, size, crc, md5, sha1))
    return lines


def mif_module_id(mif_name):
    """The module identifier, from the manifest's path: `mif/<id>.mif` -> `<id>`.

    It is the same identifier that names the module folder, `mod/<id>/`, and it is what No-Intro
    names the hashed file after (`<folder><file>`).
    """
    if not mif_name:
        return None
    name = mif_name.replace("\\", "/").split("/")[-1]
    if not name.endswith(".mif"):
        return None
    identifier = name[: -len(".mif")]
    return identifier or None


def write_missing_from_dat(out, entries):
    """Writes what is needed to propose the titles missing from No-Intro.

    The file comes out in the format the submission asks for: the title's name, and for each file
    of the module its size, CRC32, MD5 and SHA1. It is what gets copied into the proposal, and it
    avoids the round of "send the hash again, but of the other file".
    """
    missing = [entry for entry in entries if not entry["no_dat"]]
    if not missing:
        return None
    path = out / "fora-do-dat.txt"
    lines = [
        "# Titles missing from No-Intro, with what each file of the module is.",
        "# To propose them: pick, in each module, the file the DAT hashes —",
        "# by convention the one in `mod/<id>/` that is neither a signature nor game data —",
        "# and take name, size, CRC32, MD5 and SHA1 to the proposal.",
        "",
    ]
    for entry in missing:
        lines.append(f"## {entry['nome_no_intro']}")
        archive_name = pathlib.Path(entry.get("caminho") or entry["zip"]).name
        lines.append(f"archive: {archive_name}  ({entry['tamanho_zip']} bytes)")
        lines.append(f"archive crc32: {entry['crc_zip']}   sha1: {entry['sha1_zip']}")
        for name, size, crc, md5, sha1 in entry.get("arquivos_do_modulo", []):
            lines.append(f"  {name}")
            lines.append(f"    size: {size}  crc32: {crc}  md5: {md5}  sha1: {sha1}")
        if not entry.get("arquivos_do_modulo"):
            lines.append("  (could not list the module's files)")
        lines.append("")
    path.write_text("\n".join(lines), encoding="utf-8")

    # And the same content in the **DAT format**, which is what the proposal asks for. The file
    # name follows the database's convention: the module folder and the file name, with no slash —
    # No-Intro writes `mod274259font.bar` for `mod/274259/font.bar`. A `game` with several `rom`
    # entries is valid in the format and is the right thing here: **which of the files is the dump
    # is the database maintainer's call**, and the proposal takes them all, with the hashes,
    # instead of betting on one.
    dat_lines = [
        "# Proposal for inclusion in the database: Zeebo titles that are not in the",
        "# `Mobile - Zeebo.dat` of the libretro DATs (`github.com/robloach/libretro-dats`), which",
        "# is who maintains that database. The header follows the official DAT's, field by field —",
        "# `version` in quotes, in the `YYYY.MM.DD` format, and only the four fields it uses.",
        "# Checked against the database's file.",
        "clrmamepro (",
        '\tname "Mobile - Zeebo"',
        '\tdescription "Mobile - Zeebo (proposal: missing titles)"',
        f'\tversion "{PROPOSAL_VERSION}"',
        '\thomepage "https://github.com/requeijaum/zeebx-emu"',
        ")",
        "",
    ]
    for entry in missing:
        dat_lines.append("game (")
        dat_lines.append(f'\tname "{entry["nome_no_intro"]}"')
        dat_lines.append('\tregion "Brazil"')
        for name, size, crc, md5, sha1 in entry.get("arquivos_do_modulo", []):
            no_slash = name.replace("/", "").replace("\\", "")
            dat_lines.append(
                f'\trom ( name "{no_slash}" size {size} crc {crc} md5 {md5} sha1 {sha1} )'
            )
        dat_lines.append(")")
    dat_path = out / "fora-do-dat.dat"
    dat_path.write_text("\n".join(dat_lines) + "\n", encoding="utf-8")
    return path


def write_store_covers(out, entries, z_wheel, beside, to_png=False):
    """Writes the official covers, matched by the applet's ClassID.

    `beside` copies the cover next to the `.zip`, which is where the standalone frontend looks for
    it: it reads `<game>.png|jpg|bmp` next to the file (see `library::cover`).
    """
    root = out / "thumbnails" / DAT_NAME / "Named_Boxarts"
    root.mkdir(parents=True, exist_ok=True)
    covers = covers_by_class(z_wheel)
    found = 0
    without_cover = []
    beside_failed = 0
    for entry in entries:
        class_id = entry.get("clsid")
        if class_id is None:
            without_cover.append((entry["nome_no_intro"], "no ClassID in the .mif"))
            continue
        cover = covers.get(class_id)
        if cover is None:
            without_cover.append((entry["nome_no_intro"], "no store record or no image"))
            continue
        file_name, data, game_id, titles = cover
        extension = pathlib.Path(file_name).suffix.lstrip(".")
        if to_png:
            converted = convert_to_png(data)
            if converted is not None:
                data = converted
                extension = "png"
        dest = root / f"{entry['nome_no_intro']}.{extension}"
        dest.write_bytes(data)
        entry["capa"] = {"arquivo": file_name, "game_id": game_id, "bytes": len(data)}
        entry["titulos_da_loja"] = sorted(titles) if titles else []
        if beside:
            # The ROM folder may be read-only (a USB stick, a shared archive): the published cover
            # was already written above, so failing here is a warning, not an error.
            try:
                entry["caminho"].with_suffix("." + extension).write_bytes(data)
            except OSError:
                beside_failed += 1
        found += 1
    return root, found, without_cover, beside_failed


def convert_to_png(data):
    """Returns the same pixels as PNG, or `None` if they cannot be converted.

    **RetroArch's thumbnail repository accepts only PNG**, and the covers the Z-Wheel provides are
    JPEG. Converting here, at write time, avoids the surprise of finding that out on the day of
    submitting: without the conversion, all fifty-eight files would be rejected for their format.

    Without Pillow installed, it returns `None` and the caller writes the original — the conversion
    is for publishing, and RetroArch reads both formats.
    """
    try:
        import io

        from PIL import Image
    except ImportError:
        return None
    try:
        with Image.open(io.BytesIO(data)) as image:
            out = io.BytesIO()
            image.convert("RGBA" if image.mode in ("P", "LA") else "RGB").save(
                out, format="PNG", optimize=True
            )
            return out.getvalue()
    except Exception:
        return None


def write_covers(out, entries, with_icon, to_png=False):
    root = out / "thumbnails" / DAT_NAME
    folders = ["Named_Boxarts", "Named_Snaps", "Named_Titles", "Named_Logos"]
    written = 0
    for folder in folders:
        (root / folder).mkdir(parents=True, exist_ok=True)
    if with_icon:
        # The `.mif` icon is not a cover: it serves as a provisional `Named_Titles`, so the list
        # does not stay with no image at all while the real cover is not published.
        dest = root / "Named_Titles"
        for entry in entries:
            icon = entry.get("icone")
            if not icon:
                continue
            data, mime = icon["bytes"], icon["mime"]
            if to_png:
                converted = convert_to_png(data)
                if converted is not None:
                    data, mime = converted, "image/png"
            extension = {"image/png": "png", "image/jpeg": "jpg", "image/bmp": "bmp"}.get(
                mime, "bin"
            )
            (dest / f"{entry['nome_no_intro']}.{extension}").write_bytes(data)
            written += 1
    return root, written


def mif_clsid(data):
    """ClassID of the main applet, read from the `.mif` (same rule as `src/loader/miffile.rs`).

    The Z-Wheel links the cover to the game by the **applet's ClassID**, not by file or folder name
    (`GAMEINFO.class_id`), so this is the key for the match against the store catalog.
    """
    if len(data) < 0x18 or struct.unpack_from("<H", data, 0)[0] != 0x0011:
        return None
    table, sections = struct.unpack_from("<II", data, 0x10)
    if sections == 0 or sections > 4096:
        return None
    bounds = []
    for i in range(sections + 1):
        if table + i * 4 + 4 > len(data):
            return None
        bounds.append(struct.unpack_from("<I", data, table + i * 4)[0])
    for start, end in zip(bounds, bounds[1:]):
        if end - start != 20 or end > len(data):
            continue
        zeros = (
            struct.unpack_from("<I", data, start + 4)[0] == 0
            and struct.unpack_from("<I", data, start + 12)[0] == 0
        )
        class_id = struct.unpack_from("<I", data, start)[0]
        if zeros and class_id:
            return class_id
    return None


def read_z_wheel(path):
    """The store catalog that comes inside the Z-Wheel archive.

    Returns `{class_id: (game_id, cover_folder, {titles})}`. The tables are described in
    `src/ui/zwheel_metadata.rs`: `GAMEINFO` links `game_id` to the applet's `class_id` and to the
    cover's path, and `TITLETEXT` holds the name per language.
    """
    with zipfile.ZipFile(path) as zf:
        name = next((n for n in zf.namelist() if n.split("/")[-1] == "tt_game_info"), None)
        if name is None:
            raise SystemExit(f"{path}: the archive does not carry the tt_game_info database")
        data = zf.read(name)
    with tempfile.TemporaryDirectory() as folder:
        database = pathlib.Path(folder) / "tt_game_info"
        database.write_bytes(data)
        con = sqlite3.connect(str(database))
        titles = {}
        for game_id, _lang, text in con.execute(
            "select game_id, lang_id, titletext from TITLETEXT"
        ):
            titles.setdefault(game_id, set()).add(text)
        records = {}
        for class_id, game_id, cover, *_ in con.execute(
            "select class_id, game_id, boxart_path, flags, size from GAMEINFO"
        ):
            records[class_id] = (game_id, cover.rstrip("/"), titles.get(game_id, set()))
        con.close()
    return records


def covers_by_class(path):
    """The store covers, read **once** for the whole archive.

    `{class_id: (file_name, bytes, game_id, titles)}`. The previous version opened the archive and
    re-read the database for every game — 59 openings and 59 parses of the same SQLite —, and the
    cost showed up as slowness on the large collection.
    """
    records = read_z_wheel(path)
    covers = {}
    with zipfile.ZipFile(path) as zf:
        present = set(zf.namelist())
        for class_id, (game_id, folder, titles) in records.items():
            for file_name in ("boxartlg.jpg", "boxart.bmp"):
                target = f"{folder}/{file_name}".replace("./", "", 1)
                target = target if target.startswith("mod/") else f"mod/274755/{target}"
                if target in present:
                    covers[class_id] = (file_name, zf.read(target), game_id, titles)
                    break
    return covers


def write_rdb(out, entries):
    """RetroArch's RDB, with one entry per archive **and** one for the inner ROM.

    The scanner queries `crc:or(b"<file inside>", b"<archive>")`. Recording both forms makes the
    collection match by the `.zip` and also by the file No-Intro hashes — which is what survives
    repacking the archive.
    """
    records = []
    for entry in entries:
        name = entry["nome_no_intro"]
        records.append(
            rdb.record(
                name,
                name,
                entry["zip"],
                entry["tamanho_zip"],
                int(entry["crc_zip"], 16),
            )
        )
        inner_hash = entry.get("hash")
        if inner_hash:
            dat_path = entry.get("arquivo_do_dat", entry["zip"])
            # The ROM name in No-Intro's format: the inner path without the slashes.
            rom_name = dat_path.replace("\\", "/").replace("/", "").lstrip(".")
            records.append(
                rdb.record(
                    name,
                    name,
                    rom_name,
                    int(inner_hash["tamanho"]),
                    int(inner_hash["crc32"], 16),
                    bytes.fromhex(inner_hash["md5"]),
                    bytes.fromhex(inner_hash["sha1"]),
                )
            )
    return rdb.write(out, records), len(records)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--roms", required=True, type=pathlib.Path)
    ap.add_argument("--out", "--saida", required=True, type=pathlib.Path,
                    help="where the catalog, the covers and (by default) the playlist go")
    ap.add_argument("--playlists", type=pathlib.Path, default=None,
                    help="RetroArch playlists folder (default: <out>/playlists)")
    ap.add_argument("--dat", type=pathlib.Path, default=None)
    ap.add_argument("--core", default=None, help="path of zeebx_libretro.so for the playlist")
    ap.add_argument("--icons", "--icones", action="store_true",
                    help="writes the .mif icon as the title image")
    ap.add_argument(
        "--png",
        action="store_true",
        help="writes the images as PNG (RetroArch's thumbnail repository accepts only PNG)",
    )
    ap.add_argument(
        "--zwheel",
        type=pathlib.Path,
        help="Z-Wheel archive, to take the official covers by the applet's ClassID",
    )
    ap.add_argument(
        "--catalog",
        "--catalogo",
        type=pathlib.Path,
        default=None,
        help="catalog file, in JSON (default: <out>/catalogo.json)",
    )
    ap.add_argument(
        "--rdb",
        type=pathlib.Path,
        help="writes RetroArch's RDB to this path",
    )
    ap.add_argument(
        "--covers-beside",
        "--capas-ao-lado",
        action="store_true",
        help="copies the cover next to the .zip, which is where the standalone frontend looks for it",
    )
    args = ap.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    catalog = args.catalog or (args.out / "catalogo.json")
    # The DAT cache sits **next to the catalog**, not in `--out`: when the output is the
    # frontend's own configuration, the `.dat` ended up dropped at its root.
    dat_path = args.dat or (catalog.parent / f"{DAT_NAME}.dat")
    table = read_dat(download_dat(dat_path))
    print(f"DAT: {len(table)} known titles")

    zips = sorted(args.roms.glob("*.zip"))
    entries = []
    for zip_path in zips:
        entry = analyze(zip_path, table)
        entry["caminho"] = zip_path
        entries.append(entry)

    verified = [e for e in entries if e["verificado"]]
    missing = [e for e in entries if not e["no_dat"]]
    mismatched = [e for e in entries if e["no_dat"] and e["verificado"] is False]
    print(f"archives: {len(entries)}")
    print(f"verified by No-Intro: {len(verified)}")
    print(f"missing from the DAT: {len(missing)}" + (": " + ", ".join(e["nome_no_intro"] for e in missing) if missing else ""))
    # Each title missing from the DAT gets the list of the module's files with their hashes: it is
    # what the proposal to No-Intro asks for, and without it the first answer would be "send the
    # hash of the other one".
    for entry in missing:
        module_id = mif_module_id(entry.get("mif"))
        if module_id:
            import zipfile

            zip_file = entry.get("caminho") or entry["zip"]
            with zipfile.ZipFile(zip_file) as zf:
                entry["arquivos_do_modulo"] = hash_module_files(
                    zip_file, zf.namelist(), module_id
                )
    missing_from_dat = write_missing_from_dat(args.out, entries)
    if missing_from_dat:
        print(f"proposal for No-Intro: {missing_from_dat}")
    print(f"mismatched: {len(mismatched)}")
    for entry in mismatched:
        print(f"  {entry['nome_no_intro']}: {entry.get('erro', 'different hash')}")

    playlist_dest = args.playlists or (args.out / "playlists")
    playlist_dest.mkdir(parents=True, exist_ok=True)
    playlist = write_playlist(playlist_dest, entries, args.core)
    root, written = write_covers(args.out, entries, args.icons, args.png)
    print(f"playlist: {playlist}")
    print(f"covers: {root}" + (f" ({written} icons written)" if args.icons else ""))
    if args.zwheel:
        boxart_root, found, without_cover, failed = write_store_covers(
            args.out, entries, args.zwheel, args.covers_beside, args.png
        )
        print(f"official covers: {found} of {len(entries)} in {boxart_root}")
        if failed:
            print(f"  {failed} cover(s) could not be copied next to the ROM (folder not writable)")
        for name, reason in without_cover:
            print(f"  no cover: {name} ({reason})")

    if args.rdb:
        dest, count = write_rdb(args.rdb, entries)
        print(f"rdb: {dest} ({count} entries)")

    catalog.write_text(
        json.dumps(
            [
                {
                    key: (str(value) if isinstance(value, pathlib.Path) else value)
                    for key, value in entry.items()
                    if key != "icone"
                }
                for entry in entries
            ],
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    print(f"catalog: {catalog}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
