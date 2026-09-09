#!/usr/bin/env python3
"""Desmonta o firmware do console e procura o que ele faz com uma constante.

As classes que os aplicativos do Zeebo pedem — o formulário raiz, a coleção, a fonte TrueType, o
`SQLMGR` — são implementadas dentro do `1.1.2_APPS.bin`, o ELF ARM de 21 MB da partição APPS. Não
são módulos soltos no sistema de arquivos: procurados lá, não existem.

Isso é melhor do que parece. A sonda do emulador descobre um slot por execução, por hipótese e
teste; aqui a tabela de métodos **está escrita**. Ler é mais confiável que deduzir.

Uso:
    python3 ferramentas/firmware.py classe 0x01001011    # construtor e vtable da classe
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


def tabelas(data, segs):
    """Todas as tabelas de classes do firmware, como `{CLSID: [(endereço, flags, a, b)]}`.

    A entrada tem dezesseis bytes e a forma `<u32 CLSID> <u32 sinalizadores> <u32 a> <u32 b>`,
    com o construtor em `b` — e às vezes um segundo ponteiro em `a`, como no `AEECLSID_FILEMGR`.

    **Procurar entrada isolada não funciona**, e foi o que mascarou o tamanho real do registro.
    Os sinalizadores não são um punhado de valores: aparecem `0x1`, `0x4`, `0x5`, `0x8`,
    `0x20004`, `0x2000004`, `0xffff0000`… Filtrando por uma lista deles saíam 105 entradas; o
    que identifica a tabela é a **corrida**, quatro ou mais entradas seguidas cujo primeiro campo
    cai na faixa dos ClassIDs. Assim saem 84 tabelas e 704 entradas, com 425 classes distintas.

    A maior fica em `0x10c3a018`, com 62 entradas.
    """
    saida = {}
    o = 0
    while o + 16 <= len(data):
        if not e_clsid(struct.unpack_from("<I", data, o)[0]):
            o += 4
            continue
        ini = o
        while o + 16 <= len(data) and e_clsid(struct.unpack_from("<I", data, o)[0]):
            o += 16
        if (o - ini) // 16 < 4:
            continue
        for at in range(ini, o, 16):
            c, flags, a, b = struct.unpack("<4I", data[at : at + 16])
            saida.setdefault(c, []).append((para_endereco(segs, at), flags, a, b))
    return saida


def e_clsid(x):
    """A faixa dos ClassIDs do BREW neste console."""
    return 0x01000000 <= x < 0x01200000


def registro(data, segs, clsid):
    """A entrada da tabela de classes do firmware, se houver.

    Devolve `(construtor, sinalizadores)`. O construtor é o campo `b` da entrada, ou o `a` quando
    o `b` não parece função — o `AEECLSID_FILEMGR` tem os dois preenchidos.

    **Esta leitura já esteve deslocada de uma palavra**, tomando o construtor da entrada anterior
    como se fosse desta. O deslocamento não quebrava nada visivelmente — devolvia um construtor
    de verdade, numa vtable de verdade, da classe errada. Foi preciso desmontar o construtor para
    perceber: o que a tabela dava para a `0x01006c05` começava comparando o CLSID recebido com
    `0x01006c01`, que é justamente a entrada de cima.

    Fica a regra que saiu daí: **desmontar o construtor antes de copiar a vtable**. Se ele testa
    um CLSID, tem de ser o que você pediu.

    **E "não está na tabela" não quer dizer "não existe".** Mesmo com as 84 tabelas lidas, a
    `0x01028e51`, a `0x0100104f`, a `0x01035156` e o `AEECLSID_SQLMGR` continuam ausentes do
    `1.1.2_APPS.bin` inteiro — e o console as usa. Elas vêm de algo que não está neste material.
    """
    for endereco, flags, a, b in tabelas(data, segs).get(clsid, []):
        for candidato in (b, a):
            if candidato & 1 and comeca_funcao(data, segs, candidato & ~1):
                return candidato & ~1, flags
        _ = endereco
    return None, None


def comeca_funcao(data, segs, endereco):
    """Se ali começa mesmo uma função: dentro de um segmento e abrindo com `push`.

    Endereço em segmento carregável já elimina quase todo falso positivo, mas não todos — uma
    palavra de dados pode cair na faixa por acaso. Todo construtor destes que desmontamos abre
    empilhando registradores, e exigir isso fecha o resto.
    """
    off = para_offset(segs, endereco)
    if off is None:
        return False
    md = Cs(CS_ARCH_ARM, CS_MODE_THUMB)
    primeira = next(md.disasm(data[off : off + 4], endereco), None)
    return primeira is not None and primeira.mnemonic == "push"


def vtable_de(data, segs, construtor, quantos=16):
    """A vtable que um construtor grava no objeto recém-alocado.

    O padrão é sempre o mesmo: aloca, carrega um literal e o grava em `[obj]`. Procuramos o
    primeiro `ldr rX, [pc, #N]` seguido de `str rX, [r?]` e devolvemos o que o literal aponta.

    A vtable termina onde os ponteiros deixam de ser Thumb — no ARM do console todo método é
    Thumb, então o primeiro valor com o bit 0 zerado já não é método.
    """
    off = para_offset(segs, construtor)
    if off is None:
        return None, []
    md = Cs(CS_ARCH_ARM, CS_MODE_THUMB)
    alvo = None
    for i in md.disasm(data[off : off + 0x60], construtor):
        m = re.search(r"\[pc, #(0x[0-9a-f]+|\d+)\]", i.op_str)
        if m and i.mnemonic.startswith("ldr"):
            pos = para_offset(segs, ((i.address + 4) & ~3) + int(m.group(1), 0))
            if pos is not None:
                valor = struct.unpack("<I", data[pos : pos + 4])[0]
                if para_offset(segs, valor) is not None:
                    alvo = valor
        elif alvo and i.mnemonic == "str" and re.search(r"\[r\d+\]$", i.op_str):
            break
    if alvo is None:
        return None, []
    base = para_offset(segs, alvo)
    slots = []
    for i in range(quantos):
        v = struct.unpack("<I", data[base + i * 4 : base + i * 4 + 4])[0]
        if not v & 1:
            break
        slots.append(v & ~1)
    return alvo, slots


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
    elif sys.argv[1] == "classe":
        clsid = int(sys.argv[2], 0)
        construtor, flags = registro(data, segs, clsid)
        if construtor is None:
            print(f"  {clsid:#010x} não está na tabela de classes deste firmware")
            return
        print(f"  construtor  {construtor:#010x}  (sinalizadores {flags:#x})")
        alvo, slots = vtable_de(data, segs, construtor)
        if alvo is None:
            print("  não achei a vtable no começo do construtor")
            return
        print(f"  vtable      {alvo:#010x}  ({len(slots)} métodos)")
        for i, v in enumerate(slots):
            print(f"    slot[{i:2}] = {v:#010x}")
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
