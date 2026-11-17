#!/usr/bin/env python3
"""Varredura da Z-Wheel: o que o binário pede, o que o emu serve, e o que falta, tela por tela.

Duas metades:

1. **Estática**, sobre o `tectoy.mod`: os ClassIDs que aparecem como literais no código, cruzados
   com os que o Zeebx (e o `zeebx-core`) conhecem; os arquivos-fonte `.c` citados nas strings, que
   são o mapa das telas; e as mensagens de erro do próprio jogo.
2. **Dinâmica**: roda `zeebx session` uma vez por cenário (cada opção do menu e da tela
   Configurar), cada uma num perfil descartável (`XDG_CONFIG_HOME` próprio), e junta as seções do
   relatório — classes ausentes, APIs ausentes, APIs por hipótese, acessos inválidos, log do jogo —
   e as marcas da serial (`<fault`, `did not return`...).

Por que perfil descartável: o perfil da Z-Wheel persiste entre execuções (idioma, cadastro,
`tt_prefs.db`) e o jogo escreve dentro do pacote extraído. Sem isolar, um cenário muda o resultado
do seguinte, e a varredura mexeria no perfil de quem a roda.

    python3 tools/zwheel_survey.py --zip roms/Z-Wheel.zip [--out DIR] [--only nome,...] [--jobs 4]
"""
import argparse
import concurrent.futures as cf
import collections
import pathlib
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
BIN = ROOT / "target/release/zeebx"
USER_CONFIG = pathlib.Path.home() / ".config/zeebx"

# O menu sobe por volta de 15 s. A roda de baixo, com o fim de vida desligado, lê
# `Jogar · zeebo · Comprar · Navegar · Desligar · Configurar` (os do meio dependem do cfg), com o
# foco em `zeebo`: duas à esquerda dão em Configurar. Em Configurar o foco abre em Z-Credits.
MENU = 16000
CFG = f"{MENU}:left,{MENU + 2000}:left,{MENU + 4000}:b1"  # abre Configurar (~20 s)
EOL_OFF = ["--no-end-of-life"]

# (nome, segundos, teclas, flags extras)
SCENARIOS = [
    ("boot-eol", 30, "", []),
    ("boot", 30, "", EOL_OFF),
    ("ajuda-eol", 40, f"{MENU}:right,{MENU + 2000}:b1,{MENU + 6000}:down,{MENU + 8000}:down", []),
    ("jogar", 40, f"{MENU}:left,{MENU + 2000}:b1,{MENU + 6000}:down,{MENU + 8000}:right", EOL_OFF),
    ("zeebo", 40, f"{MENU}:b1,{MENU + 6000}:down", EOL_OFF),
    ("menu-dir1", 40, f"{MENU}:right,{MENU + 2000}:b1", EOL_OFF),
    ("menu-dir2", 40, f"{MENU}:right,{MENU + 1500}:right,{MENU + 3000}:b1", EOL_OFF),
    ("menu-dir3", 40, f"{MENU}:right,{MENU + 1500}:right,{MENU + 3000}:right,{MENU + 4500}:b1", EOL_OFF),
    ("cfg", 30, CFG, EOL_OFF),
    # Z-Credits: o item central é Comprar; esquerda = Pré-pago, direita = Verificar.
    ("cfg-comprar", 45, f"{CFG},24000:b1", EOL_OFF),
    ("cfg-prepago", 45, f"{CFG},24000:left,25500:b1", EOL_OFF),
    ("cfg-verificar", 45, f"{CFG},24000:right,25500:b1", EOL_OFF),
    # Idioma: English e confirmar. O app se reinicia no lugar (doc 31, §1); o perfil é descartável.
    ("cfg-idioma", 50, f"{CFG},23000:down,24500:right,26000:b1,28000:b1,40000:right", EOL_OFF),
    # Mais Opções: Ajuda · Cadastro · Apagar.
    ("cfg-ajuda", 45, f"{CFG},23000:down,24000:down,25000:left,26500:b1,30000:down", EOL_OFF),
    ("cfg-cadastro", 60, f"{CFG},23000:down,24000:down,25000:b1,"
     "29000:right,30000:b1,31000:up,32000:b1,34000:b2,36000:b1,40000:down,42000:b1", EOL_OFF),
    ("cfg-apagar", 45, f"{CFG},23000:down,24000:down,25000:right,26500:b1,30000:down", EOL_OFF),
]

SECTION = re.compile(r"^— (.+) —$")
SERIAL_MARKS = re.compile(r"<(fault[^>]*|[^>]*did not return[^>]*)>")


# ---------- estática ----------

def find_mod(zip_path, work):
    with zipfile.ZipFile(zip_path) as z:
        name = max((i for i in z.infolist() if i.filename.lower().endswith(".mod")),
                   key=lambda i: i.file_size).filename
        z.extract(name, work)
    return (work / name).read_bytes()


def known_classes():
    """Todo literal `0x01xxxxxx` no código do emu e do zeebx-core: o que alguém já nomeou."""
    srcs = list((ROOT / "src").rglob("*.rs"))
    core = ROOT.parent / "zeebx-core/src"
    if core.exists():
        srcs += core.rglob("*.rs")
    else:
        reg = pathlib.Path.home() / ".cargo/registry/src"
        srcs += [p for p in reg.glob("*/zeebx-core-*/src/**/*.rs")]
    found = set()
    for p in srcs:
        for m in re.finditer(r"0x0*1_?([0-9a-fA-F]{2})_?([0-9a-fA-F]{4})\b", p.read_text(errors="ignore")):
            found.add(0x0100_0000 | int(m.group(1) + m.group(2), 16))
    return found


def static_survey(mod):
    words = collections.Counter(
        w for (w,) in struct.iter_unpack("<I", mod[: len(mod) // 4 * 4])
        if 0x0100_0000 <= w < 0x0110_0000 and w & 0xFFFF not in (0, 0xFFFF))
    strings = [s.decode("latin1") for s in re.findall(rb"[\x20-\x7e]{5,}", mod)]
    sources = sorted({m.group(0) for s in strings for m in re.finditer(r"[\w\\./-]+\.c\b", s)})
    errors = sorted({s.strip() for s in strings if re.search(r"(?i)\b(error|unable|fail|cannot)\b", s)})
    clsids = sorted({s for s in strings if "AEECLSID" in s or "AEEIID" in s})
    return words, sources, errors, clsids


# ---------- dinâmica ----------

def sandbox(base):
    cfg = base / "zeebx"
    cfg.mkdir(parents=True)
    for f in ("settings.json", "catalog.json", "config.ini"):
        if (USER_CONFIG / f).exists():
            shutil.copy2(USER_CONFIG / f, cfg / f)
    if (USER_CONFIG / "aparelho").exists():
        shutil.copytree(USER_CONFIG / "aparelho", cfg / "aparelho", symlinks=True)
    return base


def parse_report(text):
    sections, current = collections.defaultdict(list), None
    for line in text.splitlines():
        m = SECTION.match(line.strip())
        if m:
            current = m.group(1)
        elif current and line.startswith("  "):
            sections[current].append(line.strip())
    return sections


def run(zip_path, out, scenario):
    name, secs, keys, extra = scenario
    with tempfile.TemporaryDirectory(prefix=f"zw-{name}-") as tmp:
        home = sandbox(pathlib.Path(tmp))
        serial = out / f"{name}.serial"
        cmd = [str(BIN), "session", str(zip_path), f"--seconds={secs}", f"--serial={serial}",
               f"--dump={out / name}.bmp", *extra]
        if keys:
            cmd.append(f"--keys={keys}")
        env = {**__import__("os").environ, "XDG_CONFIG_HOME": str(home)}
        p = subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=secs * 6 + 120)
    report = p.stdout + p.stderr
    (out / f"{name}.txt").write_text(report)
    serial_text = serial.read_text(errors="ignore") if serial.exists() else ""
    marks = collections.Counter(re.sub(r"0x[0-9a-f]+", "#", m) for m in SERIAL_MARKS.findall(serial_text))
    return name, p.returncode, parse_report(report), marks


# ---------- relatório ----------

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--zip", required=True, type=pathlib.Path)
    ap.add_argument("--out", type=pathlib.Path, default=pathlib.Path("out/zwheel-survey"))
    ap.add_argument("--only", default="")
    ap.add_argument("--jobs", type=int, default=4)
    args = ap.parse_args()
    if not BIN.exists():
        sys.exit("falta target/release/zeebx: cargo build --release -p zeebx-cli")
    args.out.mkdir(parents=True, exist_ok=True)
    zip_path = args.zip.resolve()

    with tempfile.TemporaryDirectory() as tmp:
        mod = find_mod(zip_path, pathlib.Path(tmp))
    words, sources, errors, clsid_names = static_survey(mod)
    known = known_classes()

    chosen = [s for s in SCENARIOS if not args.only or s[0] in args.only.split(",")]
    with cf.ThreadPoolExecutor(args.jobs) as pool:
        results = sorted(pool.map(lambda s: run(zip_path, args.out, s), chosen))

    seen_missing = collections.defaultdict(set)   # (seção, linha) -> cenários
    for name, _, sections, _ in results:
        for sec, lines in sections.items():
            if sec in ("presses delivered to the game", "what the game did with the sound"):
                continue
            for line in lines:
                seen_missing[(sec, line)].add(name)
    runtime_classes = {int(m, 16) for (sec, line) in seen_missing
                       if sec == "classes the game requested that we do not have"
                       for m in re.findall(r"0x[0-9a-f]{8}", line)}

    w = []
    w.append(f"# Varredura da Z-Wheel\n\n`{zip_path.name}`, {len(mod)} bytes de `.mod`.\n")
    w.append("## Cenários\n\n| cenário | saída | marcas na serial |\n|---|---|---|")
    for name, code, _, marks in results:
        m = "; ".join(f"{k} ×{v}" for k, v in marks.most_common(4)) or "—"
        w.append(f"| {name} | {code} | {m} |")

    by_section = collections.defaultdict(list)
    for (sec, line), names in seen_missing.items():
        by_section[sec].append((line, names))
    order = ["classes the game requested that we do not have", "missing APIs",
             "invalid accesses the game carried on past", "refused pointers",
             "APIs served on assumption", "files not found", "GL served without doing anything",
             "addresses the game requested through IWeb", "game log"]
    for sec in order + sorted(set(by_section) - set(order)):
        if sec not in by_section:
            continue
        rows = sorted(by_section[sec], key=lambda r: (-len(r[1]), r[0]))
        if sec == "game log":
            rows = [r for r in rows if re.search(r"(?i)error|unable|fail|cannot|not supported", r[0])]
        if not rows:
            continue
        w.append(f"\n## {sec}\n")
        for line, names in rows[:80]:
            w.append(f"- `{line}` — {', '.join(sorted(names))}")

    unknown = sorted((c, n) for c, n in words.items() if c not in known)
    w.append("\n## ClassIDs no binário que o emu nunca nomeia\n")
    w.append("Literais em `0x0100xxxx–0x010fxxxx` do `.mod` sem nenhuma menção no código do emu "
             "nem do core. Muitos são ruído (IIDs, constantes); os que também aparecem como "
             "classe ausente em tempo de execução vêm marcados com **★**.\n")
    for c, n in unknown:
        star = " **★**" if c in runtime_classes else ""
        w.append(f"- `{c:#010x}` ×{n}{star}")
    w.append("\n## Nomes AEECLSID/AEEIID nas strings\n")
    w += [f"- `{s}`" for s in clsid_names]
    w.append("\n## Arquivos-fonte citados (mapa das telas)\n")
    w += [f"- `{s}`" for s in sources]
    w.append("\n## Mensagens de erro do jogo (estáticas)\n")
    w += [f"- `{s}`" for s in errors]

    report = args.out / "survey.md"
    report.write_text("\n".join(w) + "\n")
    print(report)


if __name__ == "__main__":
    main()
