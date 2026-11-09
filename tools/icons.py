#!/usr/bin/env python3
"""Cuts the Lucide icon font down to the icons the Qt interface uses.

The interface draws the toolbar and menu icons with a font, and not with SVG: the color comes from
the theme's palette, like any text's, and Qt does not need the QtSvg module, which the `.deb`,
`windeployqt` and `macdeployqt` would have to carry along. The whole font has 2150 icons and
900 KB; the cut has only the ones in `ICONS`, and is embedded in the binary.

To add an icon: put its name (the one from https://lucide.dev/icons) in `ICONS`, download the
`lucide-static` package of the same version and run

    python3 tools/icons.py path/to/lucide-static/package

The script rewrites `assets/icons/lucide.ttf` and the name table in `Icon.qml`. It needs
`fontTools` (`pip install fonttools`). The license (ISC, and MIT for the icons that came from
Feather) is in `tools/licenses/lucide-LICENSE` and goes into `THIRD-PARTY-NOTICES.txt`.
"""

import json
import pathlib
import re
import sys

from fontTools import subset

VERSION = "1.52.0"

ROOT = pathlib.Path(__file__).resolve().parent.parent
FONT = ROOT / "assets" / "icons" / "lucide.ttf"
QML = ROOT / "frontends" / "qt-standalone" / "qml" / "Icon.qml"

# The interface's icons, by their Lucide name.
ICONS = [
    "bug",  # debugging
    "calendar-clock",  # unlocking the Zeeboids sync
    "camera",  # screenshot
    "circle-help",
    "download",  # check for updates
    "external-link",  # the site
    "fast-forward",
    "folder-open",  # load state
    "gallery-horizontal",  # the library as a slider
    "gamepad-2",  # controls
    "gauge",  # speed
    "hard-drive",  # the save manager
    "house",  # the Z-Wheel, the console's home screen
    "info",  # about
    "keyboard",  # shortcuts
    "layout-grid",  # the library as a grid
    "log-out",  # quit
    "maximize",  # full screen
    "message-circle",  # Discord
    "minimize",
    "monitor",  # graphics
    "pause",
    "play",
    "refresh-cw",  # look for the games again
    "rotate-ccw",  # restart
    "save",  # save state
    "scroll-text",  # the log
    "search",
    "settings",
    "square",  # close the game
    "trophy",  # achievements
    "undo-2",  # undo load
    "volume-2",  # audio
    "x",
]

START = "    // --- the name table, written by tools/icons.py ---\n"
END = "    // --- end of the table ---\n"
# The markers as they were written before the translation: still recognized, and replaced by the
# English ones on the next run.
OLD_START = "    // --- a tabela de nomes, escrita por tools/icons.py ---\n"
OLD_END = "    // --- fim da tabela ---\n"


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    package = pathlib.Path(sys.argv[1])
    version = json.loads((package / "package.json").read_text(encoding="utf-8"))["version"]
    if version != VERSION:
        sys.exit(f"the package is version {version}, and this script cuts {VERSION}")
    codes = json.loads((package / "font" / "codepoints.json").read_text(encoding="utf-8"))
    missing = [name for name in ICONS if name not in codes]
    if missing:
        sys.exit(f"icons that do not exist in {VERSION}: {', '.join(missing)}")

    options = subset.Options()
    options.layout_features = []
    options.name_IDs = ["*"]
    options.notdef_outline = True
    font = subset.load_font(str(package / "font" / "lucide.ttf"), options)
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=[codes[name] for name in ICONS])
    subsetter.subset(font)
    FONT.parent.mkdir(parents=True, exist_ok=True)
    subset.save_font(font, str(FONT), options)

    table = "".join(f'        "{name}": 0x{codes[name]:x},\n' for name in sorted(ICONS))
    block = START + "    readonly property var codes: ({\n" + table + "    })\n" + END
    qml = QML.read_text(encoding="utf-8")
    start = "(?:" + re.escape(START) + "|" + re.escape(OLD_START) + ")"
    end = "(?:" + re.escape(END) + "|" + re.escape(OLD_END) + ")"
    qml, replaced = re.subn(start + r".*?" + end, lambda _: block, qml, flags=re.S)
    if replaced != 1:
        sys.exit(f"{QML.name} does not have the table block between the markers")
    QML.write_text(qml, encoding="utf-8")
    print(f"{FONT.name}: {len(ICONS)} icons, {FONT.stat().st_size // 1024} KB")


if __name__ == "__main__":
    main()
