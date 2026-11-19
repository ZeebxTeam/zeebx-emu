#!/usr/bin/env python3
"""Builds the `latest-qt.json` that the emulator's updater reads from the release.

The Qt interface asks for it (`src/ui/updater.rs`, function `install`), at the address of the
chosen release: `.../releases/download/<tag>/latest-qt.json`. The manifest says, per system,
which package to download and its minisign signature, which CI generated with
`cargo packager signer sign` next to each package (`<package>.sig`). A package without a `.sig` is
left out, and that system falls back to the notice with the page, as before.

Only the formats that replace themselves go in: the AppImage, the NSIS installer and the `.app` in
`.tar.gz`. The `.deb` and the `.dmg` do not — the first belongs to dpkg, the second is just the
`.app`'s packaging.

    python3 tools/update_manifest.py <packages-folder> <tag> [version]

The version is the tag's, without the `v`. The nightly is the exception, and the reason for the
third argument: its tag is the word `nightly`, the same on every run, and what tells one build
from the next is its name (`0.7.0-nightly.202611200300`). The files are still under the tag.

The `.sig` files are deleted from the folder after being read: the signature is already in the
manifest, and the release does not need twelve more files.
"""

import datetime
import json
import pathlib
import sys

REPOSITORY = "ZeebxTeam/zeebx-emu"

# The package's name in the release (CI's "Nomear" step) → the key `cargo-packager-updater` looks
# for (`<system>-<architecture>`) and the format. CI's `macos-arm64` is its `aarch64`.
PACKAGES = [
    ("linux-x86_64.AppImage", "linux-x86_64", "appimage"),
    ("windows-x86_64-setup.exe", "windows-x86_64", "nsis"),
    ("macos-arm64.app.tar.gz", "macos-aarch64", "app"),
    ("macos-x86_64.app.tar.gz", "macos-x86_64", "app"),
]


def manifest(folder: pathlib.Path, frontend: str, tag: str, version: str) -> dict | None:
    platforms = {}
    for suffix, key, fmt in PACKAGES:
        name = f"zeebx-standalone-{frontend}-{suffix}"
        signature = folder / f"{name}.sig"
        if not (folder / name).is_file() or not signature.is_file():
            print(f"{frontend}: no signed {name}, {key} gets only the notice")
            continue
        platforms[key] = {
            "url": f"https://github.com/{REPOSITORY}/releases/download/{tag}/{name}",
            "signature": signature.read_text(encoding="utf-8").strip(),
            "format": fmt,
        }
    if not platforms:
        return None
    return {
        "version": version,
        "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }


def main() -> None:
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    folder, tag = pathlib.Path(sys.argv[1]), sys.argv[2]
    version = sys.argv[3] if len(sys.argv) == 4 else tag.removeprefix("v").removeprefix("V")
    for frontend in ("qt",):
        data = manifest(folder, frontend, tag, version)
        if data is None:
            print(f"{frontend}: no signed package, no latest-{frontend}.json")
            continue
        out = folder / f"latest-{frontend}.json"
        out.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
        print(f"{out.name}: {', '.join(sorted(data['platforms']))}")
    for signature in folder.glob("*.sig"):
        signature.unlink()


if __name__ == "__main__":
    main()
