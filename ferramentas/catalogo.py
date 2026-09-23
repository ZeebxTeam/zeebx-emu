#!/usr/bin/env python3
"""Catálogo No-Intro dos títulos de Zeebo a partir de um acervo de `.zip`.

O que ele faz, por arquivo:

1. abre o pacote e localiza os `.mod` e `.mif` de dentro;
2. calcula tamanho, CRC32, MD5 e SHA1 do arquivo que o DAT declara como a ROM daquele título
   (o No-Intro hasheia **um arquivo dentro de `mod/<id>/`**, cujo nome no DAT é `<pasta><arquivo>`);
3. confere o dump contra o DAT — é o que separa "tenho uma cópia" de "tenho o dump verificado";
4. lê o `.mif` para tirar o ClassID do applet e o ícone do título.

O que ele escreve:

- `<saída>/Zeebo - Zeebo.lpl` — playlist do RetroArch, com o nome No-Intro em `label`;
- `<saída>/thumbnails/Mobile - Zeebo/` — as quatro pastas que o RetroArch procura, com o nome exato
  de cada jogo, prontas para receber as capas;
- `<saída>/catalogo.json` — o mesmo em dados, para a UI daqui e para conferência.

Uso:

    python3 ferramentas/catalogo.py --roms /caminho/dos/zips --saida /caminho/de/saida \\
        [--dat arquivo.dat] [--core /caminho/zeebx_libretro.so] [--icones]
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
DAT_NOME = "Mobile - Zeebo"

# Data da proposta enviada ao No-Intro, no formato que o banco usa na linha `version` do cabeçalho.
VERSAO_DA_PROPOSTA = "2026.09.21"

ROM = re.compile(
    r'rom \( name "(?P<nome>[^"]+)" size (?P<tamanho>\d+) '
    r"crc (?P<crc>[0-9A-Fa-f]{8}) md5 (?P<md5>[0-9A-Fa-f]{32}) "
    r"sha1 (?P<sha1>[0-9A-Fa-f]{40})"
)


def le_dat(texto):
    """Devolve `{nome do jogo: (arquivo, tamanho, crc, md5, sha1)}`."""
    tabela = {}
    for bloco in texto.split("game (")[1:]:
        nome = re.search(r'name "([^"]+)"', bloco)
        rom = ROM.search(bloco)
        if nome and rom:
            tabela[nome.group(1)] = (
                rom.group("nome"),
                int(rom.group("tamanho")),
                rom.group("crc").upper(),
                rom.group("md5").upper(),
                rom.group("sha1").upper(),
            )
    return tabela


def baixa_dat(destino):
    if not destino.is_file():
        destino.write_bytes(urllib.request.urlopen(DAT_URL, timeout=60).read())
    return destino.read_text(encoding="utf-8", errors="replace")


def crc_do_arquivo(caminho):
    """CRC32 de um arquivo, lido em blocos."""
    crc = 0
    with caminho.open("rb") as fonte:
        while True:
            bloco = fonte.read(1 << 20)
            if not bloco:
                break
            crc = zlib.crc32(bloco, crc)
    return crc & 0xFFFFFFFF


def sha1_do_arquivo(caminho):
    """SHA1 de um arquivo, lido em blocos."""
    sha1 = hashlib.sha1()
    with caminho.open("rb") as fonte:
        while True:
            bloco = fonte.read(1 << 20)
            if not bloco:
                break
            sha1.update(bloco)
    return sha1.hexdigest().upper()


def hasheia(zf, nome):
    """Tamanho, CRC32, MD5 e SHA1 de um arquivo dentro do pacote, em blocos."""
    crc = 0
    md5 = hashlib.md5()
    sha1 = hashlib.sha1()
    tamanho = 0
    with zf.open(nome) as fonte:
        while True:
            bloco = fonte.read(1 << 20)
            if not bloco:
                break
            tamanho += len(bloco)
            crc = zlib.crc32(bloco, crc)
            md5.update(bloco)
            sha1.update(bloco)
    return tamanho, f"{crc & 0xFFFFFFFF:08X}", md5.hexdigest().upper(), sha1.hexdigest().upper()


def acha_arquivo_do_dat(nomes, rom_do_dat):
    """O arquivo do DAT tem o caminho interno do pacote **sem as barras**.

    `mod274754sound.ggz` é `mod/274754/sound.ggz`; `modnfsresourcestracksworld_3401.viv` é
    `mod/nfs/resources/tracks/world_3401.viv`. A pasta do módulo nem sempre é numérica, então a
    comparação é feita sobre o caminho inteiro sem separador — não por prefixo.
    """
    alvo = rom_do_dat.replace("\\", "/").replace("/", "").lower()
    for nome in nomes:
        sem_barra = nome.replace("\\", "/").replace("/", "")
        if sem_barra.lower() == alvo:
            return nome
    for nome in nomes:
        sem_barra = nome.replace("\\", "/").replace("/", "")
        if sem_barra.lower().endswith(alvo):
            return nome
    return None


def icone_do_mif(dados):
    """A primeira imagem declarada no `.mif`, quando existe.

    O formato está em `src/loader/miffile.rs`: cada seção de imagem começa com o tamanho do
    próprio cabeçalho em `u16`, seguido do MIME terminado em zero, e o arquivo logo depois.
    """
    if len(dados) < 0x18:
        return None
    if struct.unpack_from("<H", dados, 0)[0] != 0x0011:
        return None
    tabela, secoes = struct.unpack_from("<II", dados, 0x10)
    if secoes == 0 or secoes > 4096:
        return None
    limites = []
    for i in range(secoes + 1):
        if tabela + i * 4 + 4 > len(dados):
            return None
        limites.append(struct.unpack_from("<I", dados, tabela + i * 4)[0])
    if limites[-1] > len(dados):
        return None
    for inicio, fim in zip(limites, limites[1:]):
        secao = dados[inicio:fim]
        if len(secao) < 4:
            continue
        cabecalho = struct.unpack_from("<H", secao, 0)[0]
        if cabecalho < 4 or cabecalho > len(secao):
            continue
        mime = secao[2:cabecalho].split(b"\0")[0]
        if mime.startswith(b"image/"):
            return mime.decode("ascii", "replace"), secao[cabecalho:]
    return None


def analisa(zip_path, tabela):
    nome_no_intro = zip_path.stem
    esperado = tabela.get(nome_no_intro)
    with zipfile.ZipFile(zip_path) as zf:
        nomes = [n for n in zf.namelist() if not n.endswith("/")]
        modulos = [n for n in nomes if n.lower().endswith(".mod")]
        manif = [n for n in nomes if n.lower().endswith(".mif")]
        ficha = {
            "zip": zip_path.name,
            "nome_no_intro": nome_no_intro,
            "no_dat": esperado is not None,
            "arquivos": len(nomes),
            "modulos": modulos,
            "manifestos": manif,
            "verificado": None,
            "crc_zip": None,
            "sha1_zip": None,
            "icone": None,
        }
        # O hash do pacote é streamado: ler dois arquivos inteiros na memória só para somar dois
        # hashes custava, no acervo inteiro, duas passadas de 1,6 GB.
        ficha["tamanho_zip"] = zip_path.stat().st_size
        ficha["crc_zip"] = f"{crc_do_arquivo(zip_path):08X}"
        ficha["sha1_zip"] = sha1_do_arquivo(zip_path)
        if esperado:
            arquivo, tam_esp, crc_esp, md5_esp, sha1_esp = esperado
            alvo = acha_arquivo_do_dat(nomes, arquivo)
            if alvo:
                tam, crc_lido, md5_lido, sha1_lido = hasheia(zf, alvo)
                ficha["verificado"] = (
                    tam == tam_esp
                    and crc_lido == crc_esp
                    and md5_lido == md5_esp
                    and sha1_lido == sha1_esp
                )
                ficha["arquivo_do_dat"] = alvo
                ficha["hash"] = {
                    "tamanho": tam,
                    "crc32": crc_lido,
                    "md5": md5_lido,
                    "sha1": sha1_lido,
                }
            else:
                ficha["verificado"] = False
                ficha["erro"] = f"o arquivo do DAT ({arquivo}) não está no pacote"
        if manif:
            # O `.mif` que interessa é o do módulo escolhido: num pacote com dois jogos, o primeiro
            # manifesto pode ser o do outro. O DAT diz a pasta do módulo (`mod/<id>/...`), e o
            # manifesto dela é `mif/<id>.mif`.
            escolhido = None
            arquivo_dat = ficha.get("arquivo_do_dat")
            if arquivo_dat:
                partes = arquivo_dat.replace("\\", "/").split("/")
                if len(partes) >= 3 and partes[0].lower() == "mod":
                    escolhido = f"mif/{partes[1]}.mif"
            if escolhido:
                escolhido = next((n for n in manif if n.replace("\\", "/").endswith(escolhido)), None)
            if escolhido is None:
                escolhido = next(
                    (n for n in manif if clsid_do_mif(zf.read(n)) is not None), manif[0]
                )
            dados = zf.read(escolhido)
            ficha["mif"] = escolhido
            ficha["clsid"] = clsid_do_mif(dados)
            imagem = icone_do_mif(dados)
            if imagem:
                mime, bytes_imagem = imagem
                ficha["icone"] = {"mime": mime, "bytes": bytes_imagem, "de": manif[0]}
        return ficha


def escreve_playlist(saida, fichas, core):
    """Playlist do RetroArch, no formato que o próprio RetroArch grava.

    Não é "um JSON por linha" (o formato antigo): a 1.20 grava **um documento** com cabeçalho e a
    lista em `items`. O `crc32` leva o sufixo `|crc` e o `db_name` leva a extensão `.lpl`, como o
    scanner escreve — copiar o formato de quem lê é o que evita a playlist abrir vazia.
    """
    itens = []
    for ficha in fichas:
        itens.append(
            {
                "path": str(ficha["caminho"]),
                "label": ficha["nome_no_intro"],
                "core_path": core or "DETECT",
                "core_name": "Zeebx" if core else "DETECT",
                "crc32": f"{ficha['crc_zip']}|crc",
                "db_name": f"{DAT_NOME}.lpl",
            }
        )
    documento = {
        "version": "1.5",
        "default_core_path": core or "",
        "default_core_name": "Zeebx" if core else "",
        "label_display_mode": 0,
        "right_thumbnail_mode": 0,
        "left_thumbnail_mode": 0,
        "thumbnail_match_mode": 0,
        "sort_mode": 0,
        "items": itens,
    }
    destino = saida / f"{DAT_NOME}.lpl"
    destino.write_text(
        json.dumps(documento, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    return destino


def hasheia_arquivos_do_modulo(zip_path, nomes, id_do_modulo):
    """Todos os arquivos de `mod/<id>/` de um título, com tamanho e os três hashes.

    O No-Intro hasheia **um** arquivo de dentro do módulo, e o nome dele no DAT é
    `<pasta><arquivo>`. Para um título que **não está** no DAT, o arquivo certo é decisão de quem
    envia — a nossa é listar todos, com o que cada um é. Adivinhar aqui seria pior: um dump
    proposto com o arquivo errado volta recusado, e aí o trabalho é o dobro.
    """
    pasta = f"mod/{id_do_modulo}/"
    linhas = []
    import zipfile

    with zipfile.ZipFile(zip_path) as zf:
        for nome in sorted(nomes):
            if not nome.startswith(pasta) or nome.endswith("/"):
                continue
            tam, crc, md5, sha1 = hasheia(zf, nome)
            linhas.append((nome, tam, crc, md5, sha1))
    return linhas


def id_do_mif(nome_do_mif):
    """O identificador do módulo, a partir do caminho do manifesto: `mif/<id>.mif` -> `<id>`.

    É o mesmo identificador que nomeia a pasta do módulo, `mod/<id>/`, e é por ele que o No-Intro
    nomeia o arquivo hasheado (`<pasta><arquivo>`).
    """
    if not nome_do_mif:
        return None
    nome = nome_do_mif.replace("\\", "/").split("/")[-1]
    if not nome.endswith(".mif"):
        return None
    identificador = nome[: -len(".mif")]
    return identificador or None


def escreve_fora_do_dat(saida, fichas):
    """Grava o que falta para propor os títulos fora do No-Intro.

    O arquivo sai no formato que a submissão pede: nome do título, e para cada arquivo do módulo o
    tamanho, o CRC32, o MD5 e o SHA1. É o que se copia para a proposta, e evita a rodada de
    "manda o hash de novo, mas do outro arquivo".
    """
    fora = [ficha for ficha in fichas if not ficha["no_dat"]]
    if not fora:
        return None
    caminho = saida / "fora-do-dat.txt"
    linhas = [
        "# Títulos fora do No-Intro, com o que cada arquivo do módulo é.",
        "# Para propor: escolher, em cada módulo, o arquivo que o DAT hasheia —",
        "# por convenção o de `mod/<id>/` que não é assinatura nem dado do jogo —",
        "# e levar nome, tamanho, CRC32, MD5 e SHA1 para a proposta.",
        "",
    ]
    for ficha in fora:
        linhas.append(f"## {ficha['nome_no_intro']}")
        nome_do_pacote = pathlib.Path(ficha.get("caminho") or ficha["zip"]).name
        linhas.append(f"pacote: {nome_do_pacote}  ({ficha['tamanho_zip']} bytes)")
        linhas.append(f"crc32 do pacote: {ficha['crc_zip']}   sha1: {ficha['sha1_zip']}")
        for nome, tam, crc, md5, sha1 in ficha.get("arquivos_do_modulo", []):
            linhas.append(f"  {nome}")
            linhas.append(f"    tamanho: {tam}  crc32: {crc}  md5: {md5}  sha1: {sha1}")
        if not ficha.get("arquivos_do_modulo"):
            linhas.append("  (não deu para listar os arquivos do módulo)")
        linhas.append("")
    caminho.write_text("\n".join(linhas), encoding="utf-8")

    # E o mesmo conteúdo no **formato do DAT**, que é o que a proposta pede. O nome do arquivo
    # segue a convenção do banco: a pasta do módulo e o nome do arquivo, sem barra — o No-Intro
    # grava `mod274259font.bar` para `mod/274259/font.bar`. Um `game` com vários `rom` é válido no
    # formato e é o certo aqui: **qual dos arquivos é o dump é decisão de quem mantém o banco**, e
    # a proposta leva todos, com os hashes, em vez de apostar num.
    linhas_dat = [
        "# Proposta de inclusão no banco: títulos de Zeebo que não estão no `Mobile - Zeebo.dat`",
        "# dos DATs do libretro (`github.com/robloach/libretro-dats`), que é quem mantém esse banco.",
        "# O cabeçalho segue o do DAT oficial, campo por campo — `version` entre aspas, no formato",
        "# `AAAA.MM.DD`, e só os quatro campos que ele usa. Conferido contra o arquivo do banco.",
        "clrmamepro (",
        '\tname "Mobile - Zeebo"',
        '\tdescription "Mobile - Zeebo (proposta: titulos ausentes)"',
        f'\tversion "{VERSAO_DA_PROPOSTA}"',
        '\thomepage "https://github.com/requeijaum/zeebx-emu"',
        ")",
        "",
    ]
    for ficha in fora:
        linhas_dat.append("game (")
        linhas_dat.append(f'\tname "{ficha["nome_no_intro"]}"')
        linhas_dat.append('\tregion "Brazil"')
        for nome, tam, crc, md5, sha1 in ficha.get("arquivos_do_modulo", []):
            sem_barra = nome.replace("/", "").replace("\\", "")
            linhas_dat.append(
                f'\trom ( name "{sem_barra}" size {tam} crc {crc} md5 {md5} sha1 {sha1} )'
            )
        linhas_dat.append(")")
    caminho_dat = saida / "fora-do-dat.dat"
    caminho_dat.write_text("\n".join(linhas_dat) + "\n", encoding="utf-8")
    return caminho


def escreve_capas_da_loja(saida, fichas, z_wheel, ao_lado, para_png=False):
    """Grava as capas oficiais, casadas pelo ClassID do applet.

    `ao_lado` copia a capa para o lado do `.zip`, que é onde o frontend standalone a procura: ele
    lê `<jogo>.png|jpg|bmp` ao lado do arquivo (ver `library::cover`).
    """
    raiz = saida / "thumbnails" / DAT_NOME / "Named_Boxarts"
    raiz.mkdir(parents=True, exist_ok=True)
    capas = capas_por_classe(z_wheel)
    achadas = 0
    sem_capa = []
    ao_lado_falhou = 0
    for ficha in fichas:
        class_id = ficha.get("clsid")
        if class_id is None:
            sem_capa.append((ficha["nome_no_intro"], "sem ClassID no .mif"))
            continue
        capa = capas.get(class_id)
        if capa is None:
            sem_capa.append((ficha["nome_no_intro"], "sem ficha na loja ou sem imagem"))
            continue
        arquivo, dados, game_id, titulos = capa
        extensao = pathlib.Path(arquivo).suffix.lstrip(".")
        if para_png:
            convertido = converte_png(dados)
            if convertido is not None:
                dados = convertido
                extensao = "png"
        destino = raiz / f"{ficha['nome_no_intro']}.{extensao}"
        destino.write_bytes(dados)
        ficha["capa"] = {"arquivo": arquivo, "game_id": game_id, "bytes": len(dados)}
        ficha["titulos_da_loja"] = sorted(titulos) if titulos else []
        if ao_lado:
            # A pasta da ROM pode ser somente-leitura (um pendrive, um pacote compartilhado): a
            # capa publicada já foi escrita acima, então falhar aqui é aviso, não erro.
            try:
                ficha["caminho"].with_suffix("." + extensao).write_bytes(dados)
            except OSError:
                ao_lado_falhou += 1
        achadas += 1
    return raiz, achadas, sem_capa, ao_lado_falhou


def converte_png(dados):
    """Devolve os mesmos pixels em PNG, ou `None` se não der para converter.

    **O repositório de thumbnails do RetroArch aceita só PNG**, e as capas que a Z-Wheel entrega
    são JPEG. Converter aqui, na hora de gravar, evita a surpresa de descobrir isso no dia de
    enviar: sem a conversão, os cinquenta e oito arquivos seriam recusados por formato.

    Sem o Pillow instalado, devolve `None` e quem chama grava o original — a conversão é para a
    publicação, e o RetroArch lê os dois formatos.
    """
    try:
        import io

        from PIL import Image
    except ImportError:
        return None
    try:
        with Image.open(io.BytesIO(dados)) as imagem:
            saida = io.BytesIO()
            imagem.convert("RGBA" if imagem.mode in ("P", "LA") else "RGB").save(
                saida, format="PNG", optimize=True
            )
            return saida.getvalue()
    except Exception:
        return None


def escreve_capas(saida, fichas, com_icone, para_png=False):
    raiz = saida / "thumbnails" / DAT_NOME
    pastas = ["Named_Boxarts", "Named_Snaps", "Named_Titles", "Named_Logos"]
    escritos = 0
    for pasta in pastas:
        (raiz / pasta).mkdir(parents=True, exist_ok=True)
    if com_icone:
        # O ícone do `.mif` não é capa: serve de `Named_Titles` provisório, para a lista não ficar
        # sem imagem nenhuma enquanto a capa de verdade não é publicada.
        destino = raiz / "Named_Titles"
        for ficha in fichas:
            icone = ficha.get("icone")
            if not icone:
                continue
            dados, mime = icone["bytes"], icone["mime"]
            if para_png:
                convertido = converte_png(dados)
                if convertido is not None:
                    dados, mime = convertido, "image/png"
            extensao = {"image/png": "png", "image/jpeg": "jpg", "image/bmp": "bmp"}.get(
                mime, "bin"
            )
            (destino / f"{ficha['nome_no_intro']}.{extensao}").write_bytes(dados)
            escritos += 1
    return raiz, escritos


def clsid_do_mif(dados):
    """ClassID do applet principal, lido do `.mif` (mesma regra de `src/loader/miffile.rs`).

    A Z-Wheel liga a capa ao jogo pelo **ClassID do applet**, não por nome de arquivo nem de
    pasta (`GAMEINFO.class_id`), então esta é a chave do cruzamento com o catálogo da loja.
    """
    if len(dados) < 0x18 or struct.unpack_from("<H", dados, 0)[0] != 0x0011:
        return None
    tabela, secoes = struct.unpack_from("<II", dados, 0x10)
    if secoes == 0 or secoes > 4096:
        return None
    limites = []
    for i in range(secoes + 1):
        if tabela + i * 4 + 4 > len(dados):
            return None
        limites.append(struct.unpack_from("<I", dados, tabela + i * 4)[0])
    for inicio, fim in zip(limites, limites[1:]):
        if fim - inicio != 20 or fim > len(dados):
            continue
        zeros = (
            struct.unpack_from("<I", dados, inicio + 4)[0] == 0
            and struct.unpack_from("<I", dados, inicio + 12)[0] == 0
        )
        classe = struct.unpack_from("<I", dados, inicio)[0]
        if zeros and classe:
            return classe
    return None


def le_z_wheel(caminho):
    """O catálogo da loja que vem dentro do pacote da Z-Wheel.

    Devolve `{class_id: (game_id, pasta_da_capa, {títulos})}`. As tabelas estão descritas em
    `src/ui/acervo.rs`: `GAMEINFO` liga `game_id` ao `class_id` do applet e ao caminho da capa, e
    `TITLETEXT` guarda o nome por idioma.
    """
    with zipfile.ZipFile(caminho) as zf:
        nome = next((n for n in zf.namelist() if n.split("/")[-1] == "tt_game_info"), None)
        if nome is None:
            raise SystemExit(f"{caminho}: o pacote não traz o banco tt_game_info")
        dados = zf.read(nome)
    with tempfile.TemporaryDirectory() as pasta:
        banco = pathlib.Path(pasta) / "tt_game_info"
        banco.write_bytes(dados)
        con = sqlite3.connect(str(banco))
        titulos = {}
        for game_id, _lang, texto in con.execute(
            "select game_id, lang_id, titletext from TITLETEXT"
        ):
            titulos.setdefault(game_id, set()).add(texto)
        fichas = {}
        for class_id, game_id, capa, *_ in con.execute(
            "select class_id, game_id, boxart_path, flags, size from GAMEINFO"
        ):
            fichas[class_id] = (game_id, capa.rstrip("/"), titulos.get(game_id, set()))
        con.close()
    return fichas


def capas_por_classe(caminho):
    """As capas da loja, lidas **uma vez** para o pacote inteiro.

    `{class_id: (nome_do_arquivo, bytes, game_id, títulos)}`. A versão anterior abria o pacote e
    relia o banco a cada jogo — 59 aberturas e 59 parses do mesmo SQLite —, e o custo aparecia como
    lentidão no acervo grande.
    """
    fichas = le_z_wheel(caminho)
    capas = {}
    with zipfile.ZipFile(caminho) as zf:
        presentes = set(zf.namelist())
        for class_id, (game_id, pasta, titulos) in fichas.items():
            for arquivo in ("boxartlg.jpg", "boxart.bmp"):
                alvo = f"{pasta}/{arquivo}".replace("./", "", 1)
                alvo = alvo if alvo.startswith("mod/") else f"mod/274755/{alvo}"
                if alvo in presentes:
                    capas[class_id] = (arquivo, zf.read(alvo), game_id, titulos)
                    break
    return capas


def escreve_rdb(saida, fichas):
    """O RDB do RetroArch, com uma entrada por pacote **e** uma pelo ROM interno.

    O scanner consulta `crc:or(b"<arquivo de dentro>", b"<pacote>")`. Registrar as duas formas faz
    o acervo casar pelo `.zip` e também pelo arquivo que o No-Intro hasheia — que é o que sobrevive
    a recompactar o pacote.
    """
    registros = []
    for ficha in fichas:
        nome = ficha["nome_no_intro"]
        registros.append(
            rdb.registro(
                nome,
                nome,
                ficha["zip"],
                ficha["tamanho_zip"],
                int(ficha["crc_zip"], 16),
            )
        )
        hash_interno = ficha.get("hash")
        if hash_interno:
            arquivo = ficha.get("arquivo_do_dat", ficha["zip"])
            # O nome do ROM no formato do No-Intro: o caminho de dentro sem as barras.
            rom_name = arquivo.replace("\\", "/").replace("/", "").lstrip(".")
            registros.append(
                rdb.registro(
                    nome,
                    nome,
                    rom_name,
                    int(hash_interno["tamanho"]),
                    int(hash_interno["crc32"], 16),
                    bytes.fromhex(hash_interno["md5"]),
                    bytes.fromhex(hash_interno["sha1"]),
                )
            )
    return rdb.escreve(saida, registros), len(registros)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--roms", required=True, type=pathlib.Path)
    ap.add_argument("--saida", required=True, type=pathlib.Path,
                    help="onde ficam catálogo, capas e (por padrão) a playlist")
    ap.add_argument("--playlists", type=pathlib.Path, default=None,
                    help="pasta de playlists do RetroArch (padrão: <saída>/playlists)")
    ap.add_argument("--dat", type=pathlib.Path, default=None)
    ap.add_argument("--core", default=None, help="caminho do zeebx_libretro.so para a playlist")
    ap.add_argument("--icones", action="store_true", help="grava o ícone do .mif como título")
    ap.add_argument(
        "--png",
        action="store_true",
        help="grava as imagens em PNG (o repositório de thumbnails do RetroArch aceita só PNG)",
    )
    ap.add_argument(
        "--zwheel",
        type=pathlib.Path,
        help="pacote da Z-Wheel, para tirar as capas oficiais pelo ClassID do applet",
    )
    ap.add_argument(
        "--catalogo",
        type=pathlib.Path,
        default=None,
        help="arquivo do catálogo em JSON (padrão: <saída>/catalogo.json)",
    )
    ap.add_argument(
        "--rdb",
        type=pathlib.Path,
        help="grava o RDB do RetroArch neste caminho",
    )
    ap.add_argument(
        "--capas-ao-lado",
        action="store_true",
        help="copia a capa para o lado do .zip, que é onde o frontend standalone a procura",
    )
    args = ap.parse_args()

    args.saida.mkdir(parents=True, exist_ok=True)
    catalogo = args.catalogo or (args.saida / "catalogo.json")
    # O cache do DAT fica **ao lado do catálogo**, não em `--saida`: quando a saída é a própria
    # configuração do frontend, o `.dat` acabava largado na raiz dela.
    caminho_dat = args.dat or (catalogo.parent / f"{DAT_NOME}.dat")
    tabela = le_dat(baixa_dat(caminho_dat))
    print(f"DAT: {len(tabela)} títulos conhecidos")

    zips = sorted(args.roms.glob("*.zip"))
    fichas = []
    for zip_path in zips:
        ficha = analisa(zip_path, tabela)
        ficha["caminho"] = zip_path
        fichas.append(ficha)

    verificados = [f for f in fichas if f["verificado"]]
    fora = [f for f in fichas if not f["no_dat"]]
    divergentes = [f for f in fichas if f["no_dat"] and f["verificado"] is False]
    print(f"pacotes: {len(fichas)}")
    print(f"verificados pelo No-Intro: {len(verificados)}")
    print(f"fora do DAT: {len(fora)}" + (": " + ", ".join(f["nome_no_intro"] for f in fora) if fora else ""))
    # Cada título fora do DAT ganha a lista dos arquivos do módulo com os hashes: é o que a
    # proposta ao No-Intro pede, e sem ela a primeira resposta seria "manda o hash do outro".
    for ficha in fora:
        id_do_modulo = id_do_mif(ficha.get("mif"))
        if id_do_modulo:
            import zipfile

            caminho_do_zip = ficha.get("caminho") or ficha["zip"]
            with zipfile.ZipFile(caminho_do_zip) as zf:
                ficha["arquivos_do_modulo"] = hasheia_arquivos_do_modulo(
                    caminho_do_zip, zf.namelist(), id_do_modulo
                )
    fora_do_dat = escreve_fora_do_dat(args.saida, fichas)
    if fora_do_dat:
        print(f"proposta para o No-Intro: {fora_do_dat}")
    print(f"divergentes: {len(divergentes)}")
    for ficha in divergentes:
        print(f"  {ficha['nome_no_intro']}: {ficha.get('erro', 'hash diferente')}")

    destino_playlist = args.playlists or (args.saida / "playlists")
    destino_playlist.mkdir(parents=True, exist_ok=True)
    playlist = escreve_playlist(destino_playlist, fichas, args.core)
    raiz, escritos = escreve_capas(args.saida, fichas, args.icones, args.png)
    print(f"playlist: {playlist}")
    print(f"capas: {raiz}" + (f" ({escritos} ícones gravados)" if args.icones else ""))
    if args.zwheel:
        raiz_box, achadas, sem_capa, falhou = escreve_capas_da_loja(
            args.saida, fichas, args.zwheel, args.capas_ao_lado, args.png
        )
        print(f"capas oficiais: {achadas} de {len(fichas)} em {raiz_box}")
        if falhou:
            print(f"  {falhou} capa(s) não puderam ser copiadas para o lado da ROM (pasta sem escrita)")
        for nome, motivo in sem_capa:
            print(f"  sem capa: {nome} ({motivo})")

    if args.rdb:
        destino, quantos = escreve_rdb(args.rdb, fichas)
        print(f"rdb: {destino} ({quantos} entradas)")

    catalogo.write_text(
        json.dumps(
            [
                {
                    chave: (str(valor) if isinstance(valor, pathlib.Path) else valor)
                    for chave, valor in ficha.items()
                    if chave != "icone"
                }
                for ficha in fichas
            ],
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    print(f"catálogo: {catalogo}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
