#!/usr/bin/env python3
"""Recorta a fonte de ícones Lucide para os ícones que a interface Qt usa.

A interface desenha os ícones da barra de ferramentas e dos menus com uma fonte, e não com SVG: a
cor vem da paleta do tema, como a de qualquer texto, e o Qt não precisa do módulo QtSvg, que o
`.deb`, o `windeployqt` e o `macdeployqt` teriam de levar junto. A fonte inteira tem 2150 ícones e
900 KB; o recorte tem só os de `ICONES`, e vai embutido no binário.

Para acrescentar um ícone: ponha o nome dele (o de https://lucide.dev/icons) em `ICONES`, baixe o
pacote `lucide-static` da mesma versão e rode

    python3 ferramentas/icones.py caminho/para/lucide-static/package

O script reescreve `assets/icones/lucide.ttf` e a tabela de nomes do `Icone.qml`. Precisa do
`fontTools` (`pip install fonttools`). A licença (ISC, e MIT para os ícones que vieram do Feather)
está em `ferramentas/licencas/lucide-LICENSE` e entra no `THIRD-PARTY-NOTICES.txt`.
"""

import json
import pathlib
import re
import sys

from fontTools import subset

VERSAO = "1.52.0"

RAIZ = pathlib.Path(__file__).resolve().parent.parent
FONTE = RAIZ / "assets" / "icones" / "lucide.ttf"
QML = RAIZ / "frontends" / "qt-standalone" / "qml" / "Icone.qml"

# Os ícones da interface, pelo nome da Lucide.
ICONES = [
    "bug",  # depuração
    "calendar-clock",  # liberar a sincronização do Zeeboids
    "camera",  # screenshot
    "circle-help",
    "download",  # procurar atualização
    "external-link",  # o site
    "fast-forward",
    "folder-open",  # carregar estado
    "gallery-horizontal",  # a biblioteca em slider
    "gamepad-2",  # controles
    "gauge",  # velocidade
    "hard-drive",  # o gerenciador de saves
    "house",  # a Z-Wheel, a tela inicial do console
    "info",  # sobre
    "keyboard",  # atalhos
    "layout-grid",  # a biblioteca em grade
    "log-out",  # sair
    "maximize",  # tela cheia
    "message-circle",  # o Discord
    "minimize",
    "monitor",  # gráficos
    "pause",
    "play",
    "refresh-cw",  # procurar os jogos de novo
    "rotate-ccw",  # reiniciar
    "save",  # salvar estado
    "scroll-text",  # o log
    "search",
    "settings",
    "square",  # fechar o jogo
    "undo-2",  # desfazer carregar
    "volume-2",  # áudio
    "x",
]

INICIO = "    // --- a tabela de nomes, escrita por ferramentas/icones.py ---\n"
FIM = "    // --- fim da tabela ---\n"


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    pacote = pathlib.Path(sys.argv[1])
    versao = json.loads((pacote / "package.json").read_text(encoding="utf-8"))["version"]
    if versao != VERSAO:
        sys.exit(f"o pacote é a versão {versao}, e este script recorta a {VERSAO}")
    codigos = json.loads((pacote / "font" / "codepoints.json").read_text(encoding="utf-8"))
    faltam = [nome for nome in ICONES if nome not in codigos]
    if faltam:
        sys.exit(f"ícones que não existem na {VERSAO}: {', '.join(faltam)}")

    opcoes = subset.Options()
    opcoes.layout_features = []
    opcoes.name_IDs = ["*"]
    opcoes.notdef_outline = True
    fonte = subset.load_font(str(pacote / "font" / "lucide.ttf"), opcoes)
    recorte = subset.Subsetter(opcoes)
    recorte.populate(unicodes=[codigos[nome] for nome in ICONES])
    recorte.subset(fonte)
    FONTE.parent.mkdir(parents=True, exist_ok=True)
    subset.save_font(fonte, str(FONTE), opcoes)

    tabela = "".join(f'        "{nome}": 0x{codigos[nome]:x},\n' for nome in sorted(ICONES))
    bloco = INICIO + "    readonly property var codigos: ({\n" + tabela + "    })\n" + FIM
    qml = QML.read_text(encoding="utf-8")
    qml, trocas = re.subn(re.escape(INICIO) + r".*?" + re.escape(FIM), lambda _: bloco, qml, flags=re.S)
    if trocas != 1:
        sys.exit(f"{QML.name} não tem o bloco da tabela entre os marcadores")
    QML.write_text(qml, encoding="utf-8")
    print(f"{FONTE.name}: {len(ICONES)} ícones, {FONTE.stat().st_size // 1024} KB")


if __name__ == "__main__":
    main()
