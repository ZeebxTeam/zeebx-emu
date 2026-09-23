#!/usr/bin/env python3
"""Confere o que falta para compilar o Zeebx e diz o comando de instalação.

O `cargo` não basta: o `unicorn-engine` compila o QEMU em C e o `dynarmic` compila um JIT em C++20
na hora. Quando falta ferramenta, o erro que aparece é só "failed to run custom build command for
dynarmic" — sem dizer o que instalar. Este script diz.

    python3 ferramentas/prepara_build.py
"""

import glob
import pathlib
import shutil
import subprocess
import sys

FERRAMENTAS = [
    # O Rust não vem de pacote de distribuição: o comando é o `rustup`, e é o que a mensagem diz.
    ("cargo", "Rust (rustup.rs)", "rustup"),
    ("cc", "compilador C", "build-essential"),
    ("c++", "compilador C++20", "build-essential"),
    ("make", "make", "build-essential"),
    ("cmake", "CMake (dynarmic)", "cmake"),
    ("ninja", "Ninja (dynarmic pede este gerador)", "ninja-build"),
    ("pkg-config", "pkg-config (glib do unicorn)", "pkg-config"),
    ("python3", "Python 3 (unicorn/QEMU)", "python3"),
]

PACOTES = {
    "debian": "sudo apt install build-essential cmake ninja-build pkg-config python3 libclang-dev libglib2.0-dev",
    "arch": "sudo pacman -S --needed base-devel cmake ninja pkgconf python clang glib2",
    "fedora": "sudo dnf install gcc-c++ cmake ninja-build pkgconf-pkg-config python3 clang-devel glib2-devel",
}


def distro():
    """A família da distribuição, para escolher o comando de instalação."""
    try:
        texto = pathlib.Path("/etc/os-release").read_text()
    except OSError:
        return None
    campos = dict(
        linha.split("=", 1) for linha in texto.splitlines() if "=" in linha
    )
    identificacao = (campos.get("ID", "") + " " + campos.get("ID_LIKE", "")).lower()
    for familia in ("debian", "arch", "fedora"):
        if familia in identificacao or (
            familia == "debian" and any(n in identificacao for n in ("ubuntu", "mint", "pop"))
        ):
            return familia
    return None


def tem_libclang():
    """O `bindgen` do unicorn precisa da biblioteca, não dos cabeçalhos."""
    candidatos = []
    for padrao in (
        "/usr/lib/llvm-*/lib/libclang.so*",
        "/usr/lib/*/libclang*.so*",
        "/usr/lib/libclang*.so*",
        "/Library/Developer/CommandLineTools/usr/lib/libclang.dylib",
    ):
        candidatos.extend(glob.glob(padrao))
    return candidatos


def tem_glib():
    if shutil.which("pkg-config") is None:
        return False
    return (
        subprocess.run(
            ["pkg-config", "--exists", "glib-2.0"],
            capture_output=True,
        ).returncode
        == 0
    )


def main():
    faltando = []
    print("ferramentas:")
    for comando, para_que, pacote in FERRAMENTAS:
        achado = shutil.which(comando)
        marca = "ok " if achado else "FALTA"
        print(f"  [{marca}] {comando:<12} {para_que}")
        if not achado and pacote:
            faltando.append(pacote)

    bibliotecas = tem_libclang()
    print("bibliotecas:")
    print(
        f"  [{'ok ' if bibliotecas else 'FALTA'}] libclang     bindgen do unicorn-engine"
    )
    if not bibliotecas:
        faltando.append("libclang-dev")
    glib = tem_glib()
    print(f"  [{'ok ' if glib else 'FALTA'}] glib-2.0      unicorn/QEMU")
    if not glib:
        faltando.append("libglib2.0-dev")

    familia = distro()
    if faltando:
        if "rustup" in faltando:
            print("\nFalta o Rust: https://rustup.rs — depois `rustup default stable`.")
        faltando = [pacote for pacote in faltando if pacote != "rustup"]
        if faltando:
            comando = (
                PACOTES[familia]
                if familia in PACOTES
                else "instale os equivalentes de: " + ", ".join(sorted(set(faltando)))
            )
            print("\nfalta instalar:\n  " + comando)
        return 1
    print("\ntudo pronto: cargo build --release")
    return 0


if __name__ == "__main__":
    sys.exit(main())
