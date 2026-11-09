import QtQuick

// A Lucide icon, by name (`"pause"`), drawn as a glyph of the embedded font: the color is the
// text's, and it follows the theme with no image to recolor. The names that exist are the ones in
// the table below; to add one, see `tools/icons.py`.
Text {
    id: iconName

    property string name
    // The icon size, in pixels: the font is drawn in a square the size of the glyph.
    property real size: 16

    font.family: fontLoader.name
    font.pixelSize: size
    horizontalAlignment: Text.AlignHCenter
    verticalAlignment: Text.AlignVCenter
    // A name that is not in the table draws nothing, instead of a missing-glyph box.
    text: codes[name] !== undefined ? String.fromCharCode(codes[name]) : ""

    // Qt caches the font by its URL: every icon asks for the same one, and it is read only once.
    FontLoader {
        id: fontLoader

        source: "qrc:/zeebx/lucide.ttf"
    }

    // --- a tabela de nomes, escrita por tools/icons.py ---
    readonly property var codes: ({
        "bug": 0xe20c,
        "calendar-clock": 0xe304,
        "camera": 0xe064,
        "circle-help": 0xe082,
        "download": 0xe0b2,
        "external-link": 0xe0b9,
        "fast-forward": 0xe0bd,
        "folder-open": 0xe247,
        "gallery-horizontal": 0xe4ce,
        "gamepad-2": 0xe0df,
        "gauge": 0xe1bf,
        "hard-drive": 0xe0ed,
        "house": 0xe0f5,
        "info": 0xe0f9,
        "keyboard": 0xe284,
        "layout-grid": 0xe0ff,
        "log-out": 0xe10e,
        "maximize": 0xe112,
        "message-circle": 0xe116,
        "minimize": 0xe11a,
        "monitor": 0xe11d,
        "pause": 0xe12e,
        "play": 0xe13c,
        "refresh-cw": 0xe145,
        "rotate-ccw": 0xe148,
        "save": 0xe14d,
        "scroll-text": 0xe45f,
        "search": 0xe151,
        "settings": 0xe154,
        "square": 0xe167,
        "undo-2": 0xe2a1,
        "volume-2": 0xe1ab,
        "x": 0xe1b2,
    })
    // --- fim da tabela ---
}
