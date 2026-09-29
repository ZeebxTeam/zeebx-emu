#!/usr/bin/env python3
"""Instala Zeebx em ArkOS/AeolusUX/dArkOS/dArkOSen.

Exemplo:
  python3 ferramentas/instala_arkos.py --rootfs /media/$USER/root --roms /media/$USER/EASYROMS \\
    --core zeebx_libretro.so --info zeebx_libretro.info \\
    --soundfont GeneralUser-GS.sf2 --rom 'Double Dragon (Brazil) (Es,Pt).zip'
"""
from __future__ import annotations
import argparse
import datetime as dt
import hashlib
import pathlib
import shutil
import sys
import xml.etree.ElementTree as ET

ENTRY = """\t<system>
\t\t<name>zeebo</name>
\t\t<fullname>Zeebo</fullname>
\t\t<path>/roms/zeebo/</path>
\t\t<extension>.mod .MOD .zip .ZIP .7z .7Z</extension>
\t\t<command>sudo perfmax %GOVERNOR% %ROM%; nice -n -19 /usr/local/bin/retroarch -L /home/ark/.config/retroarch/cores/zeebx_libretro.so %ROM%; sudo perfnorm</command>
\t\t<platform>zeebo</platform>
\t\t<theme>zeebo</theme>
\t\t<manufacturer>Zeebo Inc.</manufacturer>
\t\t<release>2009</release>
\t\t<hardware>console</hardware>
\t</system>\n"""

def sha(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()

def backup(path, rootfs):
    if not path.is_file():
        return None
    stamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    try:
        relativo = path.resolve().relative_to(rootfs.resolve())
    except ValueError:
        # EASYROMS é outra partição: preserve pelo menos pasta final + nome, sem tentar escrever
        # fora da árvore de backup nem colidir com um arquivo homônimo do ROOTFS.
        relativo = pathlib.Path("external") / path.parent.name / path.name
    out = rootfs / "zeebx-backup" / stamp / relativo
    out.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(path, out)
    return out

def update_es(path):
    text = path.read_text(encoding="utf-8")
    text = text.replace("2>&1 > /dev/tty1", "2&gt;&amp;1 > /dev/tty1")
    if "<name>zeebo</name>" not in text:
        close = text.rfind("</systemList>")
        if close < 0:
            raise ValueError(f"{path}: não achei </systemList>")
        text = text[:close] + ENTRY + text[close:]
    path.write_text(text, encoding="utf-8")
    ET.parse(path)

def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--rootfs", type=pathlib.Path, required=True)
    ap.add_argument("--roms", type=pathlib.Path, required=True)
    ap.add_argument("--core", type=pathlib.Path, required=True)
    ap.add_argument("--info", type=pathlib.Path, required=True)
    ap.add_argument("--soundfont", type=pathlib.Path)
    ap.add_argument("--font", type=pathlib.Path, help="tectoy.ttf (opcional; a Z-Wheel também pode instalá-la)")
    ap.add_argument("--rom", type=pathlib.Path, action="append", default=[])
    args = ap.parse_args()
    rootfs, romroot = args.rootfs, args.roms
    for p in (rootfs, romroot, args.core, args.info):
        if not p.exists():
            print(f"não existe: {p}", file=sys.stderr)
            return 2
    if args.core.stat().st_size < 100_000 or args.core.read_bytes()[:4] != b"\x7fELF":
        print("--core não parece ELF válido", file=sys.stderr)
        return 2
    core_dst = rootfs / "home/ark/.config/retroarch/cores/zeebx_libretro.so"
    info_dst = rootfs / "home/ark/.config/retroarch/cores/zeebx_libretro.info"
    es_paths = [rootfs / "etc/emulationstation/es_systems.cfg"]
    user_es = rootfs / "home/ark/.emulationstation/es_systems.cfg"
    if user_es.is_file():
        es_paths.append(user_es)
    template_es = rootfs / "usr/local/bin/es_systems.cfg"
    if template_es.is_file():
        es_paths.append(template_es)
    sf_dst = romroot / "bios/zeebx/aparelho/soundfonts/GeneralUser-GS.sf2"
    font_dst = romroot / "bios/zeebx/aparelho/shared/fonts/tectoy.ttf"
    for p in [core_dst, info_dst, *es_paths, sf_dst, font_dst]:
        old = backup(p, rootfs)
        if old:
            print(f"backup: {old}")
    core_dst.parent.mkdir(parents=True, exist_ok=True)
    info_dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(args.core, core_dst)
    shutil.copy2(args.info, info_dst)
    core_dst.chmod(0o755)
    info_dst.chmod(0o644)
    ark_home = rootfs / "home/ark"
    if ark_home.exists() and hasattr(shutil, "chown"):
        try:
            shutil.chown(core_dst, user=ark_home.stat().st_uid, group=ark_home.stat().st_gid)
            shutil.chown(info_dst, user=ark_home.stat().st_uid, group=ark_home.stat().st_gid)
        except (PermissionError, LookupError):
            pass
    print(f"core: {core_dst} sha256={sha(core_dst)[:16]}...")
    print(f"info: {info_dst}")
    for es in es_paths:
        update_es(es)
        print(f"EmulationStation: {es}")
    rom_dst = romroot / "zeebo"
    rom_dst.mkdir(parents=True, exist_ok=True)
    for rom in args.rom:
        if not rom.is_file():
            print(f"ROM não existe: {rom}", file=sys.stderr)
            return 2
        shutil.copy2(rom, rom_dst / rom.name)
        print(f"ROM: {rom_dst / rom.name}")
    if args.soundfont:
        if not args.soundfont.is_file():
            print(f"SoundFont não existe: {args.soundfont}", file=sys.stderr)
            return 2
        sf_dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(args.soundfont, sf_dst)
        print(f"SoundFont: {sf_dst}")
    elif sf_dst.exists():
        print(f"SoundFont já presente: {sf_dst}")
    if args.font:
        if not args.font.is_file():
            print(f"Fonte não existe: {args.font}", file=sys.stderr)
            return 2
        font_dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(args.font, font_dst)
        print(f"Fonte: {font_dst}")
    elif font_dst.exists():
        print(f"Fonte já presente: {font_dst}")
    print("Tudo instalado. Sincronize e desmonte as partições antes de remover o cartão.")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
