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
    python3 ferramentas/nand.py nomes     CAMINHO/part_EFS2APPS.bin
"""

import pathlib
import struct
import sys

# O tamanho do bloco de apagamento sai da própria tabela: com 128 KB, a partição APPS calculada
# bate byte a byte com o `1.1.2_APPS.bin` publicado separadamente. É a conferência que valida
# tudo o que este arquivo faz.
BLOCO = 0x20000

# O dump vem em dois arquivos. O `1.1.2.bin` é só a área de dados; o `1.1.2_spare.bin` é a mesma
# coisa **intercalada** com a área fora de banda, em setores de 512 mais 16. Conferido: os 512
# primeiros bytes de cada setor do intercalado batem com o arquivo de dados.
#
# A OOB, porém, só tem ECC: os seis últimos bytes de cada setor são `0xff`. Não há ali número de
# arquivo nem de página lógica, então o mapa do EFS2 é interno, e não da NAND.
SETOR, FORA_DE_BANDA = 512, 16

# A página lógica do EFS2. Os arquivos são partidos nela, e o mapa é uma tabela de `u32` com a
# página física de cada pedaço.
PAGINA = 0x800


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

    **A `ref` é índice numa tabela de páginas.** O EFS2APPS tem regiões que são vetores planos de
    `u32`, um por página lógica, e `tabela[ref + n]` é a página física do n-ésimo pedaço de 2 KB
    do arquivo. Confirmado com a `tectoy.ttf`: a lista de páginas dela, obtida casando o conteúdo
    de uma cópia conhecida, aparece literalmente em `0x4c23104`, e a base que isso implica leva
    do `ref` dela direto para a página com o cabeçalho TrueType.

    O que falta é achar a **geração corrente** da tabela para cada `ref`. O sistema é
    log-estruturado e guarda várias versões: a base deduzida da `tectoy.ttf` serve para ela e não
    para o `tt_prefs.db`, cuja entrada naquela cópia está livre (`0xfffffff4`).

    Sem isso não dá para montar o conteúdo, porque o EFS2 **não guarda arquivo contíguo**:
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


def todos_os_nomes(data, minimo=4):
    """Todo nome de arquivo que aparece num nó de diretório, com quantas vezes.

    Não precisa do mapa de páginas: os nós de diretório estão em claro no dump, e cada geração
    deles é uma cópia. Varrer tudo e juntar dá **a lista completa de nomes** do sistema de
    arquivos, ainda que sem a árvore e sem o conteúdo.

    Foi assim que se estabeleceu o que o EFS2APPS **não** tem: nenhuma extensão do BREW. Nada de
    `widgets.mod`, `forms.mod`, `framewidget.mod`, `imenu.mod` ou `icontrols.mod` — só os módulos
    dos jogos (`tectoy.mod`, `reksio.mod`) e dados. Trezentos e dezessete nomes, e nenhum deles é
    a extensão de interface que a Z-Wheel usa.

    A contagem serve de sinal: um nome que aparece quatrocentas vezes é um arquivo que foi
    reescrito quatrocentas vezes, o que é o esperado num sistema log-estruturado.
    """
    import collections

    vistos = collections.Counter()
    at = 0
    while at < len(data) - 64:
        entradas = entradas_de_diretorio(data, at, 60)
        if len(entradas) >= minimo:
            for nome, _ in entradas:
                vistos[nome] += 1
            at += sum(len(nome) + 7 for nome, _ in entradas)
        else:
            at += 4
    return vistos


def paginas_de(dados, base_tabela, ref, quantas):
    """As páginas físicas de um arquivo, lidas da tabela de páginas.

    `base_tabela` é o endereço que faz `base + ref*4` cair na entrada da página zero.

    **Isto não é um leitor completo, e o motivo está medido.** Montando a `tectoy.ttf` pela
    tabela encontrada em `0x4c23104`, as treze primeiras páginas saem **idênticas** à cópia
    conhecida do pacote — o que confirma o modelo: a `ref` indexa um vetor de `u32` com a página
    física de cada pedaço de 2 KB, em ordem. Da décima quarta em diante, não.

    A entrada 13 daquela cópia aponta para a página `0x14f5`, e a correta é `0x17a6`. Não é
    corrupção: é uma **geração antiga** da tabela. O EFS2 é log-estruturado e guarda várias
    versões de tudo, inclusive das próprias tabelas.

    E a versão corrente não existe em lugar nenhum como bloco contíguo: procurando a lista real
    de 94 páginas da fonte, o prefixo de 5 aparece uma vez e o de 16 não aparece nenhuma. Ou
    seja, o mapa corrente é a tabela base **mais um diário de alterações**, e montá-lo pede
    reproduzir esse diário. É o que falta para o leitor ficar pronto.

    Cuidado com a armadilha de verificação: procurar o conteúdo de uma página no dump devolve a
    **primeira** ocorrência, e há cópias antigas de tudo. Uma lista de páginas obtida assim
    parece certa e mistura versões.
    """
    saida = []
    for n in range(quantas):
        at = base_tabela + (ref + n) * 4
        if at + 4 > len(dados):
            break
        pagina = struct.unpack("<I", dados[at : at + 4])[0]
        if pagina >= len(dados) // PAGINA:
            break
        saida.append(pagina)
    return saida


def monta(dados, paginas, tamanho=None):
    """Junta as páginas num arquivo."""
    saida = b"".join(dados[p * PAGINA : (p + 1) * PAGINA] for p in paginas)
    return saida[:tamanho] if tamanho else saida


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
    elif comando == "montar":
        # `montar ARQUIVO base ref tamanho` — junta um arquivo a partir da tabela de páginas.
        base, ref, tam = (int(x, 0) for x in sys.argv[3:6])
        pgs = paginas_de(data, base, ref, (tam + PAGINA - 1) // PAGINA)
        saida = monta(data, pgs, tam)
        destino = sys.argv[6] if len(sys.argv) > 6 else "montado.bin"
        pathlib.Path(destino).write_bytes(saida)
        print(f"  {len(pgs)} páginas -> {len(saida)} bytes em {destino}")
    elif comando == "nomes":
        vistos = todos_os_nomes(data)
        print(f"  {len(vistos)} nomes distintos")
        for nome, vezes in sorted(vistos.items()):
            print(f"  {vezes:5}x  {nome}")
    elif comando == "diretorio":
        at = int(sys.argv[3], 0)
        for nome, ref in entradas_de_diretorio(data, at):
            print(f"  {nome:34} ref={ref:#010x}")
    else:
        print(__doc__)


main()
