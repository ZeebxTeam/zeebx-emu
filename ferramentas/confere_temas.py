#!/usr/bin/env python3
"""Confere os temas da interface: o formato do `.json` e o contraste das cores que andam juntas.

Um tema é uma paleta do Qt por cima do estilo Fusion (ver `assets/temas/`). O formato errado não
impede o emulador de abrir — o tema cai nas cores do Fusion —, mas o contraste ruim passa sem
ninguém ver: um texto cinza sobre um botão cinza é legível no monitor de quem fez o tema e some no
de quem joga. Por isso a conta é a do WCAG 2.1, e não o olho.

    python3 ferramentas/confere_temas.py                   # os de fábrica
    python3 ferramentas/confere_temas.py meu-tema.json     # um tema qualquer
    python3 ferramentas/confere_temas.py --json            # o resultado, para outra ferramenta

Os pares são os que a interface desenha de verdade. O cartão escolhido da grade, por exemplo, é o
`highlight` a 16% por cima do `button`, com o título em `buttonText` (`GradeDaBiblioteca.qml`); o
rolo do slider é o `dark`, e a caixa escolhida dele, um painel claro fixo com a borda em
`highlight` (`SliderDaBiblioteca.qml`).
"""

import json
import pathlib
import re
import sys

RAIZ = pathlib.Path(__file__).resolve().parent.parent
FABRICA = RAIZ / "assets" / "temas"

# Os papéis do `QPalette` que um tema define, com o nome que o QML usa (`palette.windowText`). A
# mesma lista da `PAPEIS` de `src/ui/temas.rs`, que é quem lê o tema no emulador: as duas andam juntas.
PAPEIS = [
    "window", "windowText", "base", "alternateBase", "text", "placeholderText", "button",
    "buttonText", "brightText", "light", "midlight", "mid", "dark", "shadow", "highlight",
    "highlightedText", "link", "linkVisited", "toolTipBase", "toolTipText",
]
DESABILITADOS = ["windowText", "text", "buttonText"]
IDIOMAS = ["pt-BR", "en", "es", "es-MX"]

# O painel das caixas do slider: fixo no QML, igual em todo tema.
PAINEL_DO_SLIDER = "#ececec"

# Texto pede 4,5:1; uma borda ou um contorno que precisa ser visto, 3:1 (WCAG 1.4.3 e 1.4.11). O
# texto de exemplo de um campo vazio fica com 3:1: é dica, e não conteúdo.
TEXTO = 4.5
CONTORNO = 3.0


def cor(texto):
    if not isinstance(texto, str) or not re.fullmatch(r"#[0-9a-fA-F]{6}", texto):
        raise ValueError(f"cor inválida: {texto!r} (use #rrggbb)")
    return tuple(int(texto[i:i + 2], 16) / 255 for i in (1, 3, 5))


def luminancia(rgb):
    def canal(c):
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = (canal(c) for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contraste(a, b):
    la, lb = sorted((luminancia(a), luminancia(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


def mistura(fundo, frente, alfa):
    return tuple(f * alfa + b * (1 - alfa) for b, f in zip(fundo, frente))


def hexa(rgb):
    return "#" + "".join(f"{round(c * 255):02x}" for c in rgb)


def pares(tema):
    """Os pares que a interface desenha: (o que é, frente, fundo, mínimo)."""
    c = {papel: cor(valor) for papel, valor in tema["cores"].items()}
    d = {papel: cor(valor) for papel, valor in tema["desabilitado"].items()}
    escolhido = mistura(c["button"], c["highlight"], 0.16)
    return [
        ("Texto da janela", c["windowText"], c["window"], TEXTO),
        ("Texto de campo", c["text"], c["base"], TEXTO),
        ("Texto de botão", c["buttonText"], c["button"], TEXTO),
        ("Título do cartão sob o ponteiro", c["buttonText"], c["midlight"], TEXTO),
        ("Título do cartão escolhido", c["buttonText"], escolhido, TEXTO),
        ("Texto selecionado", c["highlightedText"], c["highlight"], TEXTO),
        ("Dica (tooltip)", c["toolTipText"], c["toolTipBase"], TEXTO),
        ("Link", c["link"], c["base"], TEXTO),
        ("Aviso", cor(tema["aviso"]), c["window"], TEXTO),
        ("Erro ao abrir", cor(tema["erro"]), c["window"], TEXTO),
        ("Texto de exemplo do campo", c["placeholderText"], c["base"], CONTORNO),
        ("Borda do cartão escolhido", c["highlight"], c["window"], CONTORNO),
        ("Borda da caixa escolhida no slider", c["highlight"], cor(PAINEL_DO_SLIDER), CONTORNO),
        ("Desabilitado, contra o botão (informativo)", d["buttonText"], c["button"], 0),
    ]


def le(caminho):
    tema = json.loads(pathlib.Path(caminho).read_text(encoding="utf-8"))
    erros = []
    if tema.get("formato") != 1:
        erros.append(f"formato {tema.get('formato')!r}: este conferidor lê o 1")
    nome = tema.get("nome", {})
    erros += [f"sem nome em {idioma}" for idioma in IDIOMAS if not nome.get(idioma)]
    if not isinstance(tema.get("escuro"), bool):
        erros.append("`escuro` tem de ser true ou false")
    for grupo, papeis in (("cores", PAPEIS), ("desabilitado", DESABILITADOS)):
        valores = tema.get(grupo, {})
        erros += [f"{grupo}.{papel} faltando" for papel in papeis if papel not in valores]
        erros += [f"{grupo}.{papel} não é papel do Qt" for papel in valores if papel not in papeis]
        for papel, valor in valores.items():
            try:
                cor(valor)
            except ValueError as erro:
                erros.append(f"{grupo}.{papel}: {erro}")
    for campo in ("aviso", "erro"):
        try:
            cor(tema.get(campo))
        except ValueError as erro:
            erros.append(f"{campo}: {erro}")
    return tema, erros


def confere(caminho):
    tema, erros = le(caminho)
    resultado = {"arquivo": pathlib.Path(caminho).name, "tema": tema, "erros": erros, "pares": []}
    if erros:
        return resultado
    for rotulo, frente, fundo, minimo in pares(tema):
        razao = contraste(frente, fundo)
        resultado["pares"].append({
            "rotulo": rotulo, "frente": hexa(frente), "fundo": hexa(fundo),
            "razao": round(razao, 2), "minimo": minimo, "passa": razao >= minimo,
        })
    return resultado


def main():
    argumentos = [a for a in sys.argv[1:] if a != "--json"]
    caminhos = argumentos or sorted(FABRICA.glob("*.json"))
    resultados = [confere(caminho) for caminho in caminhos]
    if "--json" in sys.argv:
        print(json.dumps(resultados, ensure_ascii=False, indent=2))
    else:
        for r in resultados:
            print(f"== {r['arquivo']}")
            for erro in r["erros"]:
                print(f"   ERRO  {erro}")
            for p in r["pares"]:
                marca = "ok   " if p["passa"] else "FALHA"
                minimo = f"≥ {p['minimo']}" if p["minimo"] else "     "
                print(f"   {marca} {p['razao']:5.2f}:1 {minimo}  {p['rotulo']}")
    falhou = any(r["erros"] or not all(p["passa"] for p in r["pares"]) for r in resultados)
    sys.exit(1 if falhou else 0)


if __name__ == "__main__":
    main()
