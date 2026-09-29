#!/usr/bin/env python3
"""Instala o core Libretro no RetroArch: o `.so` **e** o `.info`, sempre juntos.

Os dois andam em par: o RetroArch lê o campo `database` do `.info` para associar o banco No-Intro
ao core, e um `.info` velho ao lado de um core novo faz o scan pela interface marcar `??` em tudo.
Foi exatamente o que aconteceu aqui uma vez, por instalar os dois em momentos diferentes.

    python3 ferramentas/instala_core.py                 # copia para o RetroArch do usuário
    python3 ferramentas/instala_core.py --destino DIR    # outra pasta de cores
    python3 ferramentas/instala_core.py --release         # usa target/release

E para o **cartão do muOS**, que tem outra arrumação (o lançador chama
`retroarch -L /opt/muos/share/core/<core>`, e o sistema precisa das associações em
`info/assign`):

    python3 ferramentas/instala_core.py --muos /media/$USER/ROOTFS
    python3 ferramentas/instala_core.py --muos /media/$USER/ROOTFS --banco Banco.sf2
    python3 ferramentas/instala_core.py --muos /media/$USER/ROOTFS \
      --so zeebx_libretro.so --info zeebx_libretro.info --banco GeneralUser-GS.sf2
"""

import argparse
import pathlib
import shutil
import sys

RAIZ = pathlib.Path(__file__).resolve().parent.parent
ORIGEM_INFO = RAIZ / "frontends" / "libretro" / "zeebx_libretro.info"
NOMES = ("libzeebx_libretro.so", "zeebx_libretro.so")


def acha_perfil():
    """A pasta de configuração do RetroArch, no lugar em que ele a procura."""
    import os

    if xdg := os.environ.get("XDG_CONFIG_HOME"):
        return pathlib.Path(xdg) / "retroarch"
    return pathlib.Path.home() / ".config" / "retroarch"


def acha_so(release):
    perfis = ["release", "debug"] if release else ["debug", "release"]
    for perfil in perfis:
        for nome in NOMES:
            caminho = RAIZ / "target" / perfil / nome
            if caminho.is_file():
                return caminho
    return None


# --- muOS -------------------------------------------------------------------------------------
#
# O cartão do muOS não usa a pasta de cores do RetroArch: o lançador chama
# `retroarch -L /opt/muos/share/core/<core>`, e quem escolhe o core é o arquivo de associação em
# `share/info/assign/<Sistema>/<core>.ini`. O `.info` do core fica em
# `share/emulator/retroarch/info/`. Medido na imagem do RG40XX-H; o procedimento está em
# `docs/libretro/HANDHELDS-ARM64.md`, e as duas armadilhas de permissão também estão lá.

SISTEMA = "Zeebo"
CHAVE = "zeebo"
BANCO_RELATIVO = pathlib.Path("emulator/retroarch/system/zeebx/aparelho/soundfonts")
FONTE_RELATIVA = pathlib.Path("emulator/retroarch/system/zeebx/aparelho/shared/fonts/tectoy.ttf")


def instala_no_muos(raiz, origem_so, origem_info, banco, fonte, roms_raiz, roms):
    """Instala o core, o `.info`, as associações e, se pedido, o banco de amostras."""
    share = raiz / "opt/muos/share"
    if not (share / "core").is_dir():
        print(
            f"não parece um cartão do muOS montado: falta {share / 'core'}\n"
            "  monte a partição ROOTFS (a do sistema, não a de ROMs) e aponte para ela",
            file=sys.stderr,
        )
        return 1

    # Backup com data no nome: o core anterior é a única forma de voltar atrás sem recompilar.
    import datetime

    guarda = raiz / f"opt/muos/share/core/backup-{datetime.datetime.now():%Y%m%d-%H%M%S}"
    alvo_so = share / "core/zeebx_libretro.so"
    if alvo_so.is_file():
        guarda.mkdir(parents=True, exist_ok=True)
        shutil.copy2(alvo_so, guarda / alvo_so.name)
        print(f"backup: {guarda / alvo_so.name}")

    shutil.copy(origem_so, alvo_so)
    alvo_so.chmod(0o755)
    info = share / "emulator/retroarch/info/zeebx_libretro.info"
    info.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy(origem_info, info)
    print(f"core:  {alvo_so} ({alvo_so.stat().st_size} bytes)")
    print(f"info:  {info}")

    # As associações. `global.ini` diz o nome e o core padrão; o `[friendly]` é a chave que liga a
    # pasta de ROMs ao sistema, e é o que o `assign.sh` lê para gerar o `assign.json`.
    assign = share / f"info/assign/{SISTEMA}"
    assign.mkdir(parents=True, exist_ok=True)
    (assign / "global.ini").write_text(
        f"[global]\nname={SISTEMA}\ndefault=zeebx\ncatalogue=Mobile - {SISTEMA}\n"
        "lookup=0\ngovernor=performance\n\n[friendly]\n"
        f"{CHAVE}\n",
        encoding="utf-8",
    )
    (assign / "zeebx.ini").write_text(
        "[zeebx]\nname=Zeebx\ncore=zeebx_libretro.so\n\n[launch]\nprep=\n"
        "exec=/opt/muos/script/launch/lr-general.sh\ndone=\n",
        encoding="utf-8",
    )
    print(f"assign: {assign}")

    # O `assign.json` é gerado pelo `assign.sh`, que **não** roda a cada boot. Acrescentar a chave
    # aqui é fazer o que ele faria, e é o que faz o sistema aparecer sem uma tarefa manual.
    assoc = share / "info/assign/assign.json"
    if assoc.is_file():
        import json

        dados = json.loads(assoc.read_text(encoding="utf-8"))
        if dados.get(CHAVE) != SISTEMA:
            dados[CHAVE] = SISTEMA
            assoc.write_text(
                json.dumps(dados, indent=2, ensure_ascii=False, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            print(f"assign.json: + \"{CHAVE}\": \"{SISTEMA}\" ({len(dados)} entradas)")
    else:
        print(f"aviso: {assoc} não existe; rode a tarefa *Refresh Automatic Core Assign* no muOS")

    # O nome exibido da pasta. Fica na loja (`MUOS/info/name`), que é onde o resto dos nomes mora.
    candidatos_nome = [raiz / "ROMS/MUOS/info/name/folder.json", raiz / "MUOS/info/name/folder.json"]
    if roms_raiz is not None:
        candidatos_nome.insert(0, roms_raiz / "MUOS/info/name/folder.json")
    for nome in candidatos_nome:
        if not nome.is_file():
            continue
        import json

        dados = json.loads(nome.read_text(encoding="utf-8"))
        if dados.get(CHAVE) != SISTEMA:
            dados[CHAVE] = SISTEMA
            nome.write_text(
                json.dumps(dados, indent=2, ensure_ascii=False, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            print(f"{nome.name}: + \"{CHAVE}\": \"{SISTEMA}\"")
        break

    if roms:
        if roms_raiz is None:
            print("--rom exige --roms com a raiz da partição de ROMs", file=sys.stderr)
            return 1
        alvo_roms = roms_raiz / "ROMS/Zeebo"
        alvo_roms.mkdir(parents=True, exist_ok=True)
        for rom in roms:
            if not rom.is_file():
                print(f"a ROM {rom} não existe", file=sys.stderr)
                return 1
            shutil.copy2(rom, alvo_roms / rom.name)
            print(f"ROM:   {alvo_roms / rom.name}")

    if banco is not None:
        if not banco.is_file():
            print(f"o banco {banco} não existe", file=sys.stderr)
            return 1
        alvo_banco = share / BANCO_RELATIVO
        alvo_banco.mkdir(parents=True, exist_ok=True)
        shutil.copy(banco, alvo_banco / banco.name)
        print(f"banco: {alvo_banco / banco.name} ({banco.stat().st_size} bytes)")
        print("       o próprio core diz este caminho no log quando não acha o banco")

    if fonte is not None:
        if not fonte.is_file():
            print(f"a fonte {fonte} não existe", file=sys.stderr)
            return 1
        alvo_fonte = share / FONTE_RELATIVA
        alvo_fonte.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(fonte, alvo_fonte)
        print(f"fonte: {alvo_fonte} ({fonte.stat().st_size} bytes)")

    # Conferência final: copiar sem conferir é copiar sem saber.
    import hashlib

    def digest(caminho):
        return hashlib.sha256(caminho.read_bytes()).hexdigest()

    if digest(origem_so) != digest(alvo_so):
        print("o sha256 do core copiado não confere", file=sys.stderr)
        return 1
    print(f"sha256 confere: {digest(alvo_so)[:16]}...")
    print("\ndesmonte o cartão antes de tirá-lo (`udisksctl unmount -b /dev/sdX`)")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--destino", type=pathlib.Path, default=None)
    ap.add_argument("--release", action="store_true", help="prefere target/release")
    ap.add_argument("--muos", type=pathlib.Path, default=None, help="raiz do cartão do muOS montado")
    ap.add_argument("--banco", type=pathlib.Path, default=None, help=".sf2 do MIDI, com --muos")
    ap.add_argument("--so", type=pathlib.Path, help=".so do pacote; dispensa build local")
    ap.add_argument("--info", type=pathlib.Path, help=".info do pacote; dispensa o arquivo do repo")
    ap.add_argument("--font", type=pathlib.Path, help="tectoy.ttf, com --muos")
    ap.add_argument("--roms", type=pathlib.Path, help="raiz montada da partição de ROMs do muOS")
    ap.add_argument("--rom", type=pathlib.Path, action="append", default=[], help="ROM para ROMS/Zeebo; pode repetir")
    args = ap.parse_args()

    origem_so = args.so or acha_so(args.release)
    origem_info = args.info or ORIGEM_INFO
    if origem_so is None or not pathlib.Path(origem_so).is_file():
        print("não achou o core compilado; use --so zeebx_libretro.so ou faça um build local", file=sys.stderr)
        return 1
    if not pathlib.Path(origem_info).is_file():
        print(f"não achou {origem_info}", file=sys.stderr)
        return 1

    if args.muos is not None:
        return instala_no_muos(args.muos, pathlib.Path(origem_so), pathlib.Path(origem_info), args.banco, args.font, args.roms, args.rom)

    destino = args.destino or (acha_perfil() / "cores")
    destino.mkdir(parents=True, exist_ok=True)
    alvo_so = destino / "zeebx_libretro.so"
    alvo_info = destino / "zeebx_libretro.info"

    # Avisa quando o `.info` que está lá é diferente do que vai entrar: é o caso que quebrou o
    # scan uma vez, e um aviso no console custa menos que descobrir pelo sintoma.
    if alvo_info.is_file() and alvo_info.read_bytes() != pathlib.Path(origem_info).read_bytes():
        print(f"aviso: o .info em {alvo_info} era diferente e será substituído")

    shutil.copy(origem_so, alvo_so)
    shutil.copy(origem_info, alvo_info)
    print(f"core:  {alvo_so}  ({origem_so.stat().st_size} bytes, {origem_so.parent.name})")
    print(f"info:  {alvo_info}")
    print("\nabra o RetroArch e escolha o core Zeebx; o banco No-Intro sai do campo `database` daqui")
    return 0


if __name__ == "__main__":
    sys.exit(main())
