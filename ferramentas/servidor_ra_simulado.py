#!/usr/bin/env python3
"""Um servidor do RetroAchievements de mentira, para ver as conquistas na janela do Zeebx.

Nenhum jogo do Zeebo tem conquistas no servidor do RA ainda: com ele, todo jogo abre como
desconhecido, e o desbloqueio, a lista, os placares e o hardcore só aparecem nos testes. Este
servidor responde como o do RA para **um** jogo, com um conjunto de conquistas sobre um byte que
muda sozinho durante a partida, e o Zeebx fala com ele pela variável `ZEEBX_RA_HOST`:

    python3 ferramentas/servidor_ra_simulado.py --jogo "…/Peggle (Brazil) (Es,Pt)/mod/278962/peggle.mod"
    ZEEBX_RA_HOST=http://127.0.0.1:8090 ./target/release/zeebx-qt

Qualquer usuário e senha entram. O conjunto tem:

- 101 e 102: "o byte mudou desde o quadro anterior", 10 e 600 vezes;
- 103: a mesma condição com progresso medido (*Measured*), até 100;
- 104: um endereço que o Zeebx não lê, para a conquista cair em "sem suporte";
- um placar que começa na primeira mudança e é enviado na trecentésima.

O byte padrão é o `0x10000088`, que muda a cada 100 ms no Peggle (medido em 2026-10-17, já com a
região interna do emulador fora do heap; antes dela era o `0x100000a0`. Ver a
tabela em `docs/implementacao/25-conquistas.md`). Para outro jogo, passe `--endereco` com um byte da
mesma tabela, ou um achado com o teste `uma_conquista_dispara_num_jogo_de_verdade`.

`--sem-rede-para awardachievement` responde 503 aos desbloqueios: é como testar à mão a fila de
desbloqueios pendentes (`<config>/conquistas/pendentes.json`). Os desbloqueios ficam guardados na
memória do servidor enquanto ele roda, e voltam na sessão seguinte, como no RA.

Só a biblioteca padrão do Python. Escuta em 127.0.0.1, e não na rede.
"""

import argparse
import hashlib
import json
import struct
import time
import urllib.parse
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# O console do Zeebo no rcheevos (`RC_CONSOLE_ZEEBO`).
CONSOLE_ZEEBO = 70
ID_DO_JOGO = 9999
TOKEN = "token-simulado"


def conjunto(endereco: int, base: str) -> dict:
    """O conjunto de conquistas no formato do `r=achievementsets` do servidor do RA."""
    e = f"0xH{endereco:x}"
    mudou = f"{e}!=d{e}"

    def conquista(id_, titulo, descricao, pontos, condicao):
        return {
            "ID": id_,
            "Title": titulo,
            "Description": descricao,
            "Flags": 3,
            "Points": pontos,
            "MemAddr": condicao,
            "Author": "zeebx",
            "BadgeName": f"{id_:05d}",
            "Created": 0,
            "Modified": 0,
        }

    return {
        "Success": True,
        "GameId": ID_DO_JOGO,
        "Title": "Jogo de teste do Zeebx",
        "ConsoleId": CONSOLE_ZEEBO,
        "ImageIconUrl": f"{base}/Images/000001.png",
        "RichPresenceGameId": ID_DO_JOGO,
        "RichPresencePatch": "Display:\nTestando as conquistas do Zeebx",
        "Sets": [
            {
                "AchievementSetId": 1,
                "GameId": ID_DO_JOGO,
                "Title": None,
                "Type": "core",
                "ImageIconUrl": f"{base}/Images/000001.png",
                "Achievements": [
                    conquista(101, "Primeiros passos", "O byte de teste mudou 10 vezes", 5, f"{mudou}.10."),
                    conquista(102, "Persistente", "O byte de teste mudou 600 vezes", 10, f"{mudou}.600."),
                    conquista(103, "Medida", "O byte de teste mudou 100 vezes, com progresso", 5, f"M:{mudou}.100."),
                    conquista(104, "Fora do mapa", "Lê um endereço que o Zeebx não tem", 1, "0xHd0000000=1"),
                ],
                "Leaderboards": [
                    {
                        "ID": 201,
                        "Title": "Contagem",
                        "Description": "Começa na primeira mudança e envia na trecentésima",
                        "Mem": f"STA:{mudou}::CAN:0=1::SUB:{mudou}.300.::VAL:{e}",
                        "Format": "SCORE",
                    }
                ],
            }
        ],
    }


def png(cor: tuple[int, int, int], lado: int = 64) -> bytes:
    """Um PNG de uma cor só, para os ícones."""
    linha = b"\x00" + bytes(cor) * lado
    dados = zlib.compress(linha * lado)

    def bloco(tipo: bytes, conteudo: bytes) -> bytes:
        crc = zlib.crc32(tipo + conteudo) & 0xFFFFFFFF
        return struct.pack(">I", len(conteudo)) + tipo + conteudo + struct.pack(">I", crc)

    cabecalho = struct.pack(">IIBBBBB", lado, lado, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + bloco(b"IHDR", cabecalho) + bloco(b"IDAT", dados) + bloco(b"IEND", b"")


class Estado:
    def __init__(self, hash_do_jogo: str, endereco: int, base: str, sem_rede: set[str]):
        self.hash = hash_do_jogo.lower()
        self.conjunto = conjunto(endereco, base)
        self.sem_rede = sem_rede
        # id -> momento do desbloqueio, por modo
        self.casual: dict[int, int] = {}
        self.hardcore: dict[int, int] = {}


def responde(estado: Estado, parametros: dict[str, str]) -> tuple[int, dict]:
    r = parametros.get("r", "")
    if r in estado.sem_rede:
        return 503, {"Success": False, "Error": "sem rede (simulado)"}
    usuario = parametros.get("u", "Usuario")
    if r == "login2":
        if not parametros.get("p") and parametros.get("t") != TOKEN:
            return 401, {"Success": False, "Error": "Token inválido (simulado)", "Code": "invalid_credentials"}
        return 200, {"Success": True, "User": usuario, "Token": TOKEN, "Score": len(estado.hardcore) * 5,
                     "SoftcoreScore": len(estado.casual) * 5, "Messages": 0}
    if r == "achievementsets":
        if parametros.get("m", "").lower() != estado.hash:
            return 404, {"Success": False, "Error": "Unknown game", "Code": "not_found", "Status": 404}
        return 200, estado.conjunto
    if r == "startsession":
        def lista(desbloqueios):
            return [{"ID": id_, "When": quando} for id_, quando in desbloqueios.items()]
        return 200, {"Success": True, "Unlocks": lista(estado.casual),
                     "HardcoreUnlocks": lista(estado.hardcore), "ServerNow": int(time.time())}
    if r == "awardachievement":
        id_ = int(parametros.get("a", "0"))
        agora = int(time.time())
        estado.casual.setdefault(id_, agora)
        if parametros.get("h") == "1":
            estado.hardcore.setdefault(id_, agora)
        total = len(estado.conjunto["Sets"][0]["Achievements"])
        return 200, {"Success": True, "Score": len(estado.hardcore) * 5, "SoftcoreScore": len(estado.casual) * 5,
                     "AchievementID": id_, "AchievementsRemaining": total - len(estado.casual)}
    if r == "submitlbentry":
        valor = int(parametros.get("s", "0"))
        return 200, {"Success": True, "Response": {
            "Score": valor, "BestScore": valor,
            "TopEntries": [{"User": usuario, "Score": valor, "Rank": 1}],
            "RankInfo": {"Rank": 1, "NumEntries": "1"}}}
    if r == "ping":
        return 200, {"Success": True}
    return 400, {"Success": False, "Error": f"o servidor simulado não conhece r={r}"}


def manipulador(estado: Estado):
    class Manipulador(BaseHTTPRequestHandler):
        def _envia(self, status: int, tipo: str, corpo: bytes):
            self.send_response(status)
            self.send_header("Content-Type", tipo)
            self.send_header("Content-Length", str(len(corpo)))
            self.end_headers()
            self.wfile.write(corpo)

        def do_GET(self):
            caminho = urllib.parse.urlparse(self.path).path
            if caminho.startswith(("/Badge/", "/Images/")) and caminho.endswith(".png"):
                cor = (110, 110, 110) if caminho.endswith("_lock.png") else (200, 160, 64)
                return self._envia(200, "image/png", png(cor))
            self._api(urllib.parse.urlparse(self.path).query)

        def do_POST(self):
            tamanho = int(self.headers.get("Content-Length", "0"))
            self._api(self.rfile.read(tamanho).decode("utf-8", "replace"))

        def _api(self, consulta: str):
            parametros = dict(urllib.parse.parse_qsl(consulta))
            status, resposta = responde(estado, parametros)
            # A senha e o token não vão ao terminal.
            visiveis = {k: v for k, v in parametros.items() if k not in ("p", "t")}
            print(f"{status} {self.headers.get('User-Agent', '?')} {visiveis}", flush=True)
            self._envia(status, "application/json", json.dumps(resposta).encode())

        def log_message(self, *_):
            pass

    return Manipulador


def main():
    argumentos = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    jogo = argumentos.add_mutually_exclusive_group(required=True)
    jogo.add_argument("--jogo", help="o .mod do jogo; o hash é o MD5 dele, como no Zeebx")
    jogo.add_argument("--hash", help="o hash já calculado")
    argumentos.add_argument("--endereco", default="0x10000088", help="o byte que muda (padrão: o do Peggle)")
    argumentos.add_argument("--porta", type=int, default=8090)
    argumentos.add_argument("--sem-rede-para", action="append", default=[], metavar="R",
                            help="responde 503 a este r= (ex.: awardachievement); pode repetir")
    opcoes = argumentos.parse_args()

    try:
        hash_do_jogo = opcoes.hash or hashlib.md5(open(opcoes.jogo, "rb").read()).hexdigest()
    except OSError as erro:
        argumentos.error(f"não deu para ler o jogo: {erro}")
    base = f"http://127.0.0.1:{opcoes.porta}"
    estado = Estado(hash_do_jogo, int(opcoes.endereco, 0), base, set(opcoes.sem_rede_para))
    servidor = ThreadingHTTPServer(("127.0.0.1", opcoes.porta), manipulador(estado))
    print(f"servidor simulado do RA em {base}, jogo {hash_do_jogo}, byte {opcoes.endereco}")
    print(f"    ZEEBX_RA_HOST={base} ./target/release/zeebx-qt", flush=True)
    try:
        servidor.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
