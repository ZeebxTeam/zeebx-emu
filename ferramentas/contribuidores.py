#!/usr/bin/env python3
"""Reescreve a lista "Quem faz o Zeebx" do README com os contribuidores do GitHub.

É a mesma lista do site (`docs/site.js`, `buscarContribuidores`): a aba "Contributors" do
repositório, sem os robôs. O site a busca a cada visita; o README não roda nada, então a lista
fica escrita nele, entre os marcadores `<!-- contribuidores -->` e `<!-- /contribuidores -->`, e é
refeita por este script quando entra gente nova.

    python3 ferramentas/contribuidores.py
"""

import json
import pathlib
import urllib.request

REPOSITORIO = "ZeebxTeam/zeebx-emu"
README = pathlib.Path(__file__).resolve().parent.parent / "README.md"
INICIO, FIM = "<!-- contribuidores -->", "<!-- /contribuidores -->"
POR_LINHA = 6


def contribuidores() -> list[dict]:
    pedido = urllib.request.Request(
        f"https://api.github.com/repos/{REPOSITORIO}/contributors?per_page=100",
        headers={"Accept": "application/vnd.github+json", "User-Agent": "zeebx-readme"},
    )
    with urllib.request.urlopen(pedido, timeout=30) as resposta:
        return [c for c in json.load(resposta) if c["type"] == "User"]


# O formato é o do all-contributors, e não por gosto: a tabela do GitHub dimensiona cada coluna
# pelo que há nela, e sem a largura fixa um nome comprido deixa as colunas desiguais. Na prévia do
# editor fica bonito de qualquer jeito; na página do repositório, não.
def tabela(pessoas: list[dict]) -> str:
    largura = f"{100 / POR_LINHA:.2f}%"
    celulas = [
        f'<td align="center" valign="top" width="{largura}"><a href="{p["html_url"]}">'
        f'<img src="{p["avatar_url"]}&s=100" width="100px;" alt="{p["login"]}"/><br />'
        f'<sub><b>{p["login"]}</b></sub></a></td>'
        for p in pessoas
    ]
    linhas = [celulas[i : i + POR_LINHA] for i in range(0, len(celulas), POR_LINHA)]
    corpo = "\n".join("  <tr>\n    " + "\n    ".join(linha) + "\n  </tr>" for linha in linhas)
    return f"<table>\n{corpo}\n</table>"


def main() -> None:
    texto = README.read_text(encoding="utf-8")
    antes, resto = texto.split(INICIO, 1)
    _, depois = resto.split(FIM, 1)
    pessoas = contribuidores()
    README.write_text(f"{antes}{INICIO}\n{tabela(pessoas)}\n{FIM}{depois}", encoding="utf-8")
    print(f"README: {len(pessoas)} contribuidores")


if __name__ == "__main__":
    main()
