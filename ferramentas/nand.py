#!/usr/bin/env python3
"""Lê a NAND do Zeebo: tabela de partições, extração e o que se sabe do EFS2.

O dump está publicado em `tripleoxygen.net/files/devices/zeebo/dump/nand/1.1.2/`. O que interessa
dele para o emulador são os módulos de extensão do console — `widgets.mod`, `forms.mod`,
`isql.mod` — e a fonte do sistema, que fica em `fs:/shared/fonts/tectoy.ttf`. As classes que a
Z-Wheel pede e que estamos identificando slot a slot vêm desses módulos.

Uso:
    python3 ferramentas/nand.py particoes CAMINHO/1.1.2.bin
    python3 ferramentas/nand.py extrair   CAMINHO/1.1.2.bin EFS2APPS saida.bin
    python3 ferramentas/nand.py diretorio CAMINHO/part_EFS2APPS.bin 0x1340800
"""

import struct
import sys

# O tamanho do bloco de apagamento sai da própria tabela: com 128 KB, a partição APPS calculada
# bate byte a byte com o `1.1.2_APPS.bin` publicado separadamente. É a conferência que valida
# tudo o que este arquivo faz.
BLOCO = 0x20000


def particoes(data):
    """As partições, lidas da MIBIB. Devolve `nome -> (offset, tamanho)`."""
    inicio = data.find(b"0:MIBIB")
    if inicio < 0:
        return {}
    # As entradas são contíguas: nome em 16 bytes, depois bloco inicial e contagem.
    saida, at = {}, inicio - 2
    while at + 32 <= len(data):
        nome = data[at + 2 : at + 18].split(b"\0")[0]
        if not nome[:2].isdigit() and not (len(nome) > 2 and nome[1:2] == b":"):
            break
        primeiro, blocos = struct.unpack("<2I", data[at + 18 : at + 26])
        if primeiro > 0x10000:
            break
        tamanho = None if blocos == 0xFFFFFFFF else blocos * BLOCO
        saida[nome.decode()[2:]] = (primeiro * BLOCO, tamanho)
        at += 28
    return saida


def entradas_de_diretorio(data, at, quantas=200):
    """Percorre um nó de diretório do EFS2 a partir de `at`.

    O formato saiu da leitura do dump, não de documentação:

        <u8 tamanho> <u8 tipo> <u32 data> <u8 zero> <nome> <'i'> <u32 referência>

    `tamanho` conta o nome mais os cinco bytes do sufixo, e é isso que permite andar de entrada
    em entrada.

    A `referência` ainda não foi decifrada. Não é o tamanho do arquivo — `tt_prefs.db` tem 4096
    bytes e a referência dele é `0x2b165` — nem um deslocamento direto. Lida como dois `u16` ela
    fica plausível: `tectoy.ttf` vira `(0x0001, 0x80fc)` e `terms.txt` vira `(0x0002, 0xb163)`,
    com o primeiro campo variando pouco entre arquivos do mesmo diretório. Tem cara de número de
    inode com um grupo, mas isso é leitura de padrão, não certeza.

    Sem decifrá-la não dá para montar o conteúdo, porque o EFS2 **não guarda arquivo contíguo**:
    conferido contra uma cópia conhecida da `tectoy.ttf`, ele bate exatamente 16.384 bytes e
    depois pula. Os pedaços seguintes aparecem com passos irregulares — `+0x4800`, `+0x5000`,
    `+0x10000` —, então há metadado intercalado e não um passo fixo que se possa deduzir.
    """
    saida = []
    while len(saida) < quantas and at + 8 < len(data):
        tamanho = data[at]
        if tamanho < 6 or at + tamanho + 7 > len(data):
            break
        nome_fim = at + 7 + (tamanho - 5)
        nome = data[at + 7 : nome_fim]
        if not all(32 <= b < 127 for b in nome):
            break
        if data[nome_fim] != ord("i"):
            break
        referencia = struct.unpack("<I", data[nome_fim + 1 : nome_fim + 5])[0]
        saida.append((nome.decode(), referencia))
        at = nome_fim + 5
    return saida


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return
    comando, caminho = sys.argv[1], sys.argv[2]
    with open(caminho, "rb") as f:
        data = f.read()

    if comando == "particoes":
        for nome, (off, tam) in particoes(data).items():
            fim = "até o fim" if tam is None else f"{tam // 1024} KB"
            print(f"  {nome:10} {off:#010x}  {fim}")
    elif comando == "extrair":
        alvo, saida = sys.argv[3], sys.argv[4]
        off, tam = particoes(data)[alvo]
        fim = len(data) if tam is None else off + tam
        with open(saida, "wb") as f:
            f.write(data[off:fim])
        print(f"  {alvo}: {fim - off} bytes em {saida}")
    elif comando == "diretorio":
        at = int(sys.argv[3], 0)
        for nome, ref in entradas_de_diretorio(data, at):
            print(f"  {nome:34} ref={ref:#010x}")
    else:
        print(__doc__)


main()
