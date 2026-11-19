#!/usr/bin/env python3
"""Checks what is missing to build Zeebx and prints the install command.

`cargo` is not enough: `unicorn-engine` builds QEMU in C and `dynarmic` builds a C++20 JIT on the
spot. When a tool is missing, the error that shows up is just "failed to run custom build command
for dynarmic" — without saying what to install. This script says it.

    python3 tools/check_build_deps.py
"""

import glob
import os
import pathlib
import shutil
import subprocess
import sys

TOOLS = [
    # Rust does not come from a distribution package: the command is `rustup`, and that is what
    # the message says.
    ("cargo", "Rust (rustup.rs)", "rustup"),
    ("cc", "C compiler", "build-essential"),
    ("c++", "C++20 compiler", "build-essential"),
    ("make", "make", "build-essential"),
    ("cmake", "CMake (dynarmic)", "cmake"),
    ("ninja", "Ninja (dynarmic asks for this generator)", "ninja-build"),
    ("pkg-config", "pkg-config (unicorn's glib)", "pkg-config"),
    ("python3", "Python 3 (unicorn/QEMU)", "python3"),
]

# The Qt 6 for `qt-standalone` comes along: the `-dev` packages to build, and the QML modules and
# the Wayland plugin for the window to open afterwards. The command line (`frontends/cli`) does not
# need it.
PACKAGES = {
    "debian": "sudo apt install build-essential cmake ninja-build pkg-config python3 libclang-dev libglib2.0-dev"
    " qt6-base-dev qt6-base-dev-tools qt6-declarative-dev qt6-declarative-dev-tools qmake6 qt6-wayland"
    " qml6-module-qtquick qml6-module-qtquick-controls qml6-module-qtquick-layouts qml6-module-qtquick-shapes"
    " qml6-module-qtquick-templates qml6-module-qtquick-window qml6-module-qtqml-workerscript",
    "arch": "sudo pacman -S --needed base-devel cmake ninja pkgconf python clang glib2"
    " qt6-base qt6-declarative qt6-wayland",
    "fedora": "sudo dnf install gcc-c++ cmake ninja-build pkgconf-pkg-config python3 clang-devel glib2-devel"
    " qt6-qtbase-devel qt6-qtdeclarative-devel qt6-qtwayland",
}

# The oldest Qt the interface was checked on: Ubuntu 24.04's. Its Qt Quick uses `FrameAnimation`,
# which was born in 6.4.
QT_MINIMUM = (6, 4)


def distro():
    """The distribution family, to pick the install command."""
    try:
        text = pathlib.Path("/etc/os-release").read_text()
    except OSError:
        return None
    fields = dict(
        line.split("=", 1) for line in text.splitlines() if "=" in line
    )
    identification = (fields.get("ID", "") + " " + fields.get("ID_LIKE", "")).lower()
    for family in ("debian", "arch", "fedora"):
        if family in identification or (
            family == "debian" and any(n in identification for n in ("ubuntu", "mint", "pop"))
        ):
            return family
    return None


def has_libclang():
    """unicorn's `bindgen` needs the library, not the headers."""
    candidates = []
    for pattern in (
        "/usr/lib/llvm-*/lib/libclang.so*",
        "/usr/lib/*/libclang*.so*",
        "/usr/lib/libclang*.so*",
        "/Library/Developer/CommandLineTools/usr/lib/libclang.dylib",
    ):
        candidates.extend(glob.glob(pattern))
    return candidates


def has_glib():
    if shutil.which("pkg-config") is None:
        return False
    return (
        subprocess.run(
            ["pkg-config", "--exists", "glib-2.0"],
            capture_output=True,
        ).returncode
        == 0
    )


def qt_version():
    """The Qt version `cxx-qt-build` will find: `QMAKE`, or `qmake6`, or `qmake`."""
    for qmake in (os.environ.get("QMAKE"), shutil.which("qmake6"), shutil.which("qmake")):
        if not qmake:
            continue
        try:
            answer = subprocess.run(
                [qmake, "-query", "QT_VERSION"], capture_output=True, text=True
            )
        except OSError:
            continue
        version = answer.stdout.strip()
        if answer.returncode == 0 and version.startswith("6."):
            return version
    return None


def main():
    missing = []
    print("tools:")
    for command, purpose, package in TOOLS:
        found = shutil.which(command)
        mark = "ok     " if found else "MISSING"
        print(f"  [{mark}] {command:<12} {purpose}")
        if not found and package:
            missing.append(package)

    libraries = has_libclang()
    print("libraries:")
    print(
        f"  [{'ok     ' if libraries else 'MISSING'}] libclang     unicorn-engine's bindgen"
    )
    if not libraries:
        missing.append("libclang-dev")
    glib = has_glib()
    print(f"  [{'ok     ' if glib else 'MISSING'}] glib-2.0      unicorn/QEMU")
    if not glib:
        missing.append("libglib2.0-dev")

    qt = qt_version()
    qt_ok = qt is not None and tuple(int(n) for n in qt.split(".")[:2]) >= QT_MINIMUM
    print(
        f"  [{'ok     ' if qt_ok else 'MISSING'}] Qt {qt or '6':<9} zeebx-qt (6.{QT_MINIMUM[1]} or newer)"
    )
    if not qt_ok:
        missing.append("qt6")

    family = distro()
    if missing:
        if "rustup" in missing:
            print("\nRust is missing: https://rustup.rs — then `rustup default stable`.")
        missing = [package for package in missing if package != "rustup"]
        if missing:
            command = (
                PACKAGES[family]
                if family in PACKAGES
                else "install the equivalents of: " + ", ".join(sorted(set(missing)))
            )
            print("\nstill to install:\n  " + command)
        return 1
    print("\nall set: cargo build --release")
    return 0


if __name__ == "__main__":
    sys.exit(main())
