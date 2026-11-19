#!/usr/bin/env python3
"""Checks the interface themes: the `.json` format and the contrast of colors that go together.

A theme is a Qt palette on top of the Fusion style (see `assets/themes/`). The wrong format does not
stop the emulator from opening — the theme falls back to Fusion's colors —, but bad contrast slips
by without anyone seeing it: grey text on a grey button is readable on the monitor of whoever made
the theme and vanishes on the player's. That is why the math is WCAG 2.1's, and not the eye.

    python3 tools/check_themes.py                   # the factory ones
    python3 tools/check_themes.py my-theme.json     # any theme
    python3 tools/check_themes.py --json            # the result, for another tool

The pairs are the ones the interface really draws. The chosen card in the grid, for example, is
`highlight` at 16% over `button`, with the title in `buttonText` (`LibraryGrid.qml`); the slider's
reel is `dark`, and its chosen box a fixed light panel with the border in `highlight`
(`LibrarySlider.qml`).

The theme file's keys (`formato`, `nome`, `escuro`, `cores`, `desabilitado`, `aviso`, `erro`) stay
in Portuguese: they are the format the emulator reads, including from themes users wrote.
"""

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
FACTORY = ROOT / "assets" / "themes"

# The `QPalette` roles a theme defines, with the name QML uses (`palette.windowText`). The same list
# as `ROLES` in `src/ui/themes.rs`, which is what reads the theme in the emulator: the two go
# together.
ROLES = [
    "window", "windowText", "base", "alternateBase", "text", "placeholderText", "button",
    "buttonText", "brightText", "light", "midlight", "mid", "dark", "shadow", "highlight",
    "highlightedText", "link", "linkVisited", "toolTipBase", "toolTipText",
]
DISABLED = ["windowText", "text", "buttonText"]
LANGUAGES = ["pt-BR", "en", "es", "es-MX"]

# The panel of the slider's boxes: fixed in the QML, the same in every theme.
SLIDER_PANEL = "#ececec"

# Text asks for 4.5:1; a border or an outline that needs to be seen, 3:1 (WCAG 1.4.3 and 1.4.11).
# The placeholder text of an empty field gets 3:1: it is a hint, and not content.
TEXT = 4.5
OUTLINE = 3.0


def color(text):
    if not isinstance(text, str) or not re.fullmatch(r"#[0-9a-fA-F]{6}", text):
        raise ValueError(f"invalid color: {text!r} (use #rrggbb)")
    return tuple(int(text[i:i + 2], 16) / 255 for i in (1, 3, 5))


def luminance(rgb):
    def channel(c):
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = (channel(c) for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(a, b):
    la, lb = sorted((luminance(a), luminance(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


def blend(background, foreground, alpha):
    return tuple(f * alpha + b * (1 - alpha) for b, f in zip(background, foreground))


def to_hex(rgb):
    return "#" + "".join(f"{round(c * 255):02x}" for c in rgb)


def pairs(theme):
    """The pairs the interface draws: (what it is, foreground, background, minimum)."""
    c = {role: color(value) for role, value in theme["cores"].items()}
    d = {role: color(value) for role, value in theme["desabilitado"].items()}
    chosen = blend(c["button"], c["highlight"], 0.16)
    return [
        ("Window text", c["windowText"], c["window"], TEXT),
        ("Field text", c["text"], c["base"], TEXT),
        ("Button text", c["buttonText"], c["button"], TEXT),
        ("Title of the card under the pointer", c["buttonText"], c["midlight"], TEXT),
        ("Title of the chosen card", c["buttonText"], chosen, TEXT),
        ("Selected text", c["highlightedText"], c["highlight"], TEXT),
        ("Hint (tooltip)", c["toolTipText"], c["toolTipBase"], TEXT),
        ("Link", c["link"], c["base"], TEXT),
        ("Warning", color(theme["aviso"]), c["window"], TEXT),
        ("Error on opening", color(theme["erro"]), c["window"], TEXT),
        ("Field placeholder text", c["placeholderText"], c["base"], OUTLINE),
        ("Border of the chosen card", c["highlight"], c["window"], OUTLINE),
        ("Border of the chosen box in the slider", c["highlight"], color(SLIDER_PANEL), OUTLINE),
        ("Disabled, against the button (informative)", d["buttonText"], c["button"], 0),
    ]


def read(path):
    theme = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    errors = []
    if theme.get("formato") != 1:
        errors.append(f"format {theme.get('formato')!r}: this checker reads 1")
    name = theme.get("nome", {})
    errors += [f"no name in {language}" for language in LANGUAGES if not name.get(language)]
    if not isinstance(theme.get("escuro"), bool):
        errors.append("`escuro` must be true or false")
    for group, roles in (("cores", ROLES), ("desabilitado", DISABLED)):
        values = theme.get(group, {})
        errors += [f"{group}.{role} missing" for role in roles if role not in values]
        errors += [f"{group}.{role} is not a Qt role" for role in values if role not in roles]
        for role, value in values.items():
            try:
                color(value)
            except ValueError as error:
                errors.append(f"{group}.{role}: {error}")
    for field in ("aviso", "erro"):
        try:
            color(theme.get(field))
        except ValueError as error:
            errors.append(f"{field}: {error}")
    return theme, errors


def check(path):
    theme, errors = read(path)
    result = {"file": pathlib.Path(path).name, "theme": theme, "errors": errors, "pairs": []}
    if errors:
        return result
    for label, foreground, background, minimum in pairs(theme):
        ratio = contrast(foreground, background)
        result["pairs"].append({
            "label": label, "foreground": to_hex(foreground), "background": to_hex(background),
            "ratio": round(ratio, 2), "minimum": minimum, "passes": ratio >= minimum,
        })
    return result


def main():
    arguments = [a for a in sys.argv[1:] if a != "--json"]
    paths = arguments or sorted(FACTORY.glob("*.json"))
    results = [check(path) for path in paths]
    if "--json" in sys.argv:
        print(json.dumps(results, ensure_ascii=False, indent=2))
    else:
        for r in results:
            print(f"== {r['file']}")
            for error in r["errors"]:
                print(f"   ERROR {error}")
            for p in r["pairs"]:
                mark = "ok  " if p["passes"] else "FAIL"
                minimum = f"≥ {p['minimum']}" if p["minimum"] else "     "
                print(f"   {mark} {p['ratio']:5.2f}:1 {minimum}  {p['label']}")
    failed = any(r["errors"] or not all(p["passes"] for p in r["pairs"]) for r in results)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
