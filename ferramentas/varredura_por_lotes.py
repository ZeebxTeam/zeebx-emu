#!/usr/bin/env python3
"""Roda a varredura das ROMs em lotes, separando regressão de jogo sabidamente fora.

Existe porque a varredura é o teste que cobra o comportamento de todos os jogos de uma vez, e ele
tem duas armadilhas que já custaram uma tarde cada uma:

1. **O cache de extração tem de ser limpo ANTES de cada lote.** O jogo grava os próprios arquivos
   (`udata/dooopt.sav`, `udata/options`, `default.opt`) **dentro do pacote extraído**, e o cache
   sobrevive entre execuções. Com o cache sujo, o `open` que falhava passa a funcionar, o relatório
   **perde** uma pendência, e a linha de base acusa regressão onde não houve nenhuma. Medido: cinco
   jogos "mudaram" por causa de save deixado para trás.
2. **Nem toda falha do teste é regressão.** `a_rom_indicada_avanca` falha por dois motivos —
   categoria que não passa (jogo sabidamente fora da lista) e diferença de linha de base —, e só a
   segunda interessa. Sem separar as duas, seis jogos conhecidos escondem a resposta que se quer.

E o cache é limpo em lotes porque a extração de todas as ROMs de uma vez não cabe no disco.

    python3 ferramentas/varredura_por_lotes.py --roms <pasta> [--lote 8] [--ms 6000]
"""
import argparse
import os
import pathlib
import re
import shutil
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
CACHE = pathlib.Path.home() / ".config/zeebx/cache"


def limpa_o_cache():
    shutil.rmtree(CACHE, ignore_errors=True)
    CACHE.mkdir(parents=True, exist_ok=True)


def roda_o_lote(roms, ms, saida):
    """Roda um lote e devolve o texto da saída do teste."""
    pasta = pathlib.Path("/tmp/zeebx-lote")
    shutil.rmtree(pasta, ignore_errors=True)
    pasta.mkdir(parents=True)
    for rom in roms:
        (pasta / rom.name).symlink_to(rom)
    ambiente = {
        **os.environ,
        "ZEEBX_ROM": str(pasta),
        "ZEEBX_ROM_MS": str(ms),
        "ZEEBX_ROM_SAIDA": str(saida),
        "ZEEBX_ROM_BASE": str(REPO / "docs/varredura"),
        "CARGO_PROFILE_DEV_DEBUG": "0",
        "CARGO_PROFILE_TEST_DEBUG": "0",
        "CARGO_BUILD_JOBS": "2",
    }
    feito = subprocess.run(
        ["cargo", "test", "--lib", "--locked", "--no-default-features",
         "varredura::tests::a_rom_indicada_avanca", "--", "--nocapture"],
        cwd=REPO, env=ambiente, capture_output=True, text=True,
    )
    return feito.returncode, feito.stdout + feito.stderr


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--roms", required=True, help="pasta com os .zip das ROMs")
    parser.add_argument("--lote", type=int, default=8, help="ROMs por lote (padrão 8)")
    parser.add_argument("--ms", type=int, default=6000, help="milissegundos virtuais por ROM")
    parser.add_argument("--saida", default="/tmp/zeebx-varredura", help="onde gravar os relatórios")
    args = parser.parse_args()

    roms = sorted(pathlib.Path(args.roms).glob("*.zip"))
    if not roms:
        print(f"nenhuma ROM em {args.roms}", file=sys.stderr)
        return 1
    saida = pathlib.Path(args.saida)
    shutil.rmtree(saida, ignore_errors=True)
    saida.mkdir(parents=True, exist_ok=True)
    print(f"{len(roms)} ROMs, lotes de {args.lote}", flush=True)

    diferencas, fora = [], []
    for i in range(0, len(roms), args.lote):
        limpa_o_cache()
        lote = roms[i:i + args.lote]
        _, texto = roda_o_lote(lote, args.ms, saida)
        # Cada falha começa com "FALHA " e vai até a linha seguinte que não é continuação.
        for bloco in re.findall(r"FALHA (.+?)(?=\n(?:FALHA|ok |\n|$))", texto, re.S):
            alvo = diferencas if "mudou desde" in bloco else fora
            alvo.append(bloco.strip().splitlines()[0])
        prontos = len(list(saida.glob("*.txt")))
        print(f"lote {i // args.lote + 1}: {len(lote)} ROMs, {prontos} relatório(s)", flush=True)
    limpa_o_cache()

    print()
    print(f"relatórios: {len(list(saida.glob('*.txt')))} de {len(roms)}")
    print(f"diferenças de linha de base: {len(diferencas)}  <- só isto é regressão")
    for d in diferencas:
        print("   ", d[:160])
    print(f"jogos fora da lista (esperado): {len(fora)}")
    for f in fora:
        print("   ", f[:160])
    return 1 if diferencas else 0


if __name__ == "__main__":
    sys.exit(main())
