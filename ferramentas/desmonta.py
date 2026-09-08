#!/usr/bin/env python3
"""Desmonta um trecho do módulo de um jogo, resolvendo os literais e as strings.

O emulador já mostra **o que** foi executado (`--code`) e **o que** foi chamado (`--trace`).
O que faltava era ler o código do jogo: por que ele desviou, o que ele testou, qual constante
estava no pool. Sem isso, "o app morre em 0x78b70" e "0x78b70 é o ramo de erro que só imprime a
mensagem" são indistinguíveis — e foram, por um bom tempo.

Uso:  python3 ferramentas/desmonta.py 0x83420 0x834a0 [padrão-do-jogo]

A base de mapeamento (`BASE`) é do `tectoy.mod` da Z-Wheel. Para outro módulo, descubra-a assim:
pegue um ponteiro de string que apareceu num rastro do emulador — o `IFILEMGR_OpenFile` recebe o
caminho em `r1` —, procure o conteúdo dentro do arquivo e subtraia o deslocamento do endereço.

Precisa do capstone: `pacman -S python-capstone`.
"""
import sys, pathlib, struct, re
from capstone import Cs, CS_ARCH_ARM, CS_MODE_ARM

BASE = 0x10000  # byte zero do arquivo: o carregador mapeia a imagem em MODULE_BASE - MODULE_PREFIX e zera o prefixo

def carrega(padrao='Z-Wheel'):
    root = pathlib.Path.home()/'.config/zeebx/cache'
    d = [p for p in root.iterdir() if padrao in p.name][0]
    return max((m for m in d.rglob('*.mod')), key=lambda m: m.stat().st_size).read_bytes()

def texto(data, addr, n=70):
    off = addr - BASE
    if not (0 <= off < len(data)):
        return None
    fim = data.find(b'\0', off, off + n)
    if fim < 0:
        return None
    bruto = data[off:fim]
    if len(bruto) >= 4 and all(0x20 <= b < 0x7f or b in (9, 10) for b in bruto):
        return bruto.decode('latin1')
    return None

def main():
    ini = int(sys.argv[1], 16)
    fim = int(sys.argv[2], 16) if len(sys.argv) > 2 else ini + 0x100
    data = carrega(sys.argv[3] if len(sys.argv) > 3 else 'Z-Wheel')
    md = Cs(CS_ARCH_ARM, CS_MODE_ARM)
    trecho = data[ini-BASE:fim-BASE]
    for i in md.disasm(trecho, ini):
        nota = ''
        # literal pool: ldr rX, [pc, #n]
        m = re.search(r'\[pc, #(-?\d+)\]', i.op_str)
        if m:
            alvo = i.address + 8 + int(m.group(1))
            off = alvo - BASE
            if 0 <= off < len(data) - 4:
                valor = struct.unpack('<I', data[off:off+4])[0]
                nota = f'   ; = {valor:#010x}'
                s = texto(data, valor)
                if s:
                    nota += f' -> {s!r}'
        if not nota:
            s = texto(data, i.address)
            if s and len(s) > 6:
                nota = f'   ; texto: {s!r}'
        print(f'  {i.address:#08x}  {i.mnemonic:<8} {i.op_str}{nota}')

main()
