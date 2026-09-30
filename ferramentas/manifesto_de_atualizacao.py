#!/usr/bin/env python3
"""Monta o `latest-qt.json` e o `latest-egui.json` que o atualizador do emulador lê na release.

Cada frontend desktop pergunta pelo seu (`src/ui/atualizacao.rs`, função `instala`), no endereço
da release escolhida: `.../releases/download/<tag>/latest-qt.json`. O manifesto diz, por sistema,
qual pacote baixar e a assinatura minisign dele, que o CI gerou com `cargo packager signer sign`
ao lado de cada pacote (`<pacote>.sig`). Um pacote sem `.sig` fica de fora, e aquele sistema cai
no aviso com a página, como antes.

Só entram os formatos que se trocam sozinhos: o AppImage, o instalador NSIS e o `.app` em
`.tar.gz`. O `.deb` e o `.dmg` não — o primeiro é do dpkg, o segundo é só a embalagem do `.app`.

    python3 ferramentas/manifesto_de_atualizacao.py <pasta-dos-pacotes> <tag>

Os `.sig` são apagados da pasta depois de lidos: a assinatura já está no manifesto, e a release
não precisa de mais doze arquivos.
"""

import datetime
import json
import pathlib
import sys

REPOSITORIO = "ZeebxTeam/zeebx-emu"

# O nome do pacote na release (o passo "Nomear" do CI) → a chave que o `cargo-packager-updater`
# procura (`<sistema>-<arquitetura>`) e o formato. O `macos-arm64` do CI é o `aarch64` dele.
PACOTES = [
    ("linux-x86_64.AppImage", "linux-x86_64", "appimage"),
    ("windows-x86_64-setup.exe", "windows-x86_64", "nsis"),
    ("macos-arm64.app.tar.gz", "macos-aarch64", "app"),
    ("macos-x86_64.app.tar.gz", "macos-x86_64", "app"),
]


def manifesto(pasta: pathlib.Path, frontend: str, tag: str) -> dict | None:
    plataformas = {}
    for sufixo, chave, formato in PACOTES:
        nome = f"zeebx-standalone-{frontend}-{sufixo}"
        assinatura = pasta / f"{nome}.sig"
        if not (pasta / nome).is_file() or not assinatura.is_file():
            print(f"{frontend}: sem {nome} assinado, {chave} fica só com o aviso")
            continue
        plataformas[chave] = {
            "url": f"https://github.com/{REPOSITORIO}/releases/download/{tag}/{nome}",
            "signature": assinatura.read_text(encoding="utf-8").strip(),
            "format": formato,
        }
    if not plataformas:
        return None
    return {
        "version": tag.removeprefix("v").removeprefix("V"),
        "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": plataformas,
    }


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    pasta, tag = pathlib.Path(sys.argv[1]), sys.argv[2]
    for frontend in ("qt", "egui"):
        dados = manifesto(pasta, frontend, tag)
        if dados is None:
            print(f"{frontend}: nenhum pacote assinado, sem latest-{frontend}.json")
            continue
        saida = pasta / f"latest-{frontend}.json"
        saida.write_text(json.dumps(dados, indent=2) + "\n", encoding="utf-8")
        print(f"{saida.name}: {', '.join(sorted(dados['platforms']))}")
    for assinatura in pasta.glob("*.sig"):
        assinatura.unlink()


if __name__ == "__main__":
    main()
