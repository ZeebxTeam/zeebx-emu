#!/usr/bin/env python3
"""Desmonta o firmware do console e procura o que ele faz com uma constante.

As classes que os aplicativos do Zeebo pedem — o formulário raiz, a coleção, a fonte TrueType, o
`SQLMGR` — são implementadas dentro do `1.1.2_APPS.bin`, o ELF ARM de 21 MB da partição APPS. Não
são módulos soltos no sistema de arquivos: procurados lá, não existem.

Isso é melhor do que parece. A sonda do emulador descobre um slot por execução, por hipótese e
teste; aqui a tabela de métodos **está escrita**. Ler é mais confiável que deduzir.

Uso:
    python3 ferramentas/firmware.py refs 0x01001011      # onde a constante aparece
    python3 ferramentas/firmware.py desmonta 0x112f84dc 0x112f8502 thumb

O firmware mistura ARM e Thumb, e boa parte do código de aplicação é Thumb. Quando o desmontado
sai como uma sequência de `strb`/`movs` sem sentido, é o outro modo — ou é texto, que também
acontece: os ponteiros das tabelas de log apontam para strings, e desmontá-las dá um código
plausível e falso.

O caminho do firmware sai de `ZEEBX_FIRMWARE` ou do padrão em `vendor/`.
"""

import os
import pathlib
import re
import struct
import sys

from capstone import CS_ARCH_ARM, CS_MODE_ARM, CS_MODE_THUMB, Cs

PADRAO = "vendor/zeebo/nand/1.1.2_APPS.bin"


def segmentos(data):
    """Os segmentos carregáveis, como `(offset, vaddr, tamanho)`."""
    e_phoff = struct.unpack("<I", data[28:32])[0]
    e_phentsize, e_phnum = struct.unpack("<HH", data[42:46])
    saida = []
    for i in range(e_phnum):
        o = e_phoff + i * e_phentsize
        p_type, p_offset, p_vaddr, _, p_filesz = struct.unpack("<5I", data[o : o + 20])
        if p_type == 1 and p_filesz:
            saida.append((p_offset, p_vaddr, p_filesz))
    return saida


def para_endereco(segs, offset):
    for off, vaddr, tam in segs:
        if off <= offset < off + tam:
            return vaddr + (offset - off)
    return None


def para_offset(segs, endereco):
    for off, vaddr, tam in segs:
        if vaddr <= endereco < vaddr + tam:
            return off + (endereco - vaddr)
    return None


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return
    data = pathlib.Path(os.environ.get("ZEEBX_FIRMWARE", PADRAO)).read_bytes()
    segs = segmentos(data)

    if sys.argv[1] == "refs":
        alvo = int(sys.argv[2], 0)
        pat = struct.pack("<I", alvo)
        for m in re.finditer(re.escape(pat), data):
            end = para_endereco(segs, m.start())
            if end is not None:
                print(f"  offset {m.start():#010x}  endereço {end:#010x}")
    elif sys.argv[1] == "desmonta":
        ini, fim = int(sys.argv[2], 0), int(sys.argv[3], 0)
        off = para_offset(segs, ini)
        if off is None:
            print("endereço fora dos segmentos carregáveis")
            return
        md = Cs(CS_ARCH_ARM, CS_MODE_THUMB if "thumb" in sys.argv else CS_MODE_ARM)
        for i in md.disasm(data[off : off + (fim - ini)], ini):
            nota = ""
            m = re.search(r"\[pc, #(-?0x[0-9a-f]+|-?\d+)\]", i.op_str)
            if m and i.mnemonic.startswith("ldr"):
                # Em ARM o `pc` vale a instrução mais oito; em Thumb, mais quatro e alinhado.
                if "thumb" in sys.argv:
                    alvo = ((i.address + 4) & ~3) + int(m.group(1), 0)
                else:
                    alvo = i.address + 8 + int(m.group(1), 0)
                o = para_offset(segs, alvo)
                if o is not None and o + 4 <= len(data):
                    valor = struct.unpack("<I", data[o : o + 4])[0]
                    nota = f"   ; = {valor:#010x}"
            print(f"  {i.address:#010x}  {i.mnemonic:<8} {i.op_str}{nota}")
    else:
        print(__doc__)


main()
