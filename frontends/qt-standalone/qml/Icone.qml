import QtQuick

// Um ícone da Lucide, pelo nome (`"pause"`), desenhado como letra da fonte embutida: a cor é a do
// texto, e acompanha o tema sem imagem nenhuma para recolorir. Os nomes que existem são os da
// tabela abaixo; para acrescentar um, ver `ferramentas/icones.py`.
Text {
    id: icone

    property string nome
    // O tamanho do ícone, em pixels: a fonte é desenhada num quadrado do tamanho da letra.
    property real tamanho: 16

    font.family: carregador.name
    font.pixelSize: tamanho
    horizontalAlignment: Text.AlignHCenter
    verticalAlignment: Text.AlignVCenter
    // Um nome que não está na tabela não desenha nada, em vez de um quadrado de letra faltando.
    text: codigos[nome] !== undefined ? String.fromCharCode(codigos[nome]) : ""

    // O Qt guarda a fonte pelo endereço: cada ícone pede a mesma, e ela é lida uma vez só.
    FontLoader {
        id: carregador

        source: "qrc:/zeebx/lucide.ttf"
    }

    // --- a tabela de nomes, escrita por ferramentas/icones.py ---
    readonly property var codigos: ({
        "bug": 0xe20c,
        "calendar-clock": 0xe304,
        "camera": 0xe064,
        "circle-help": 0xe082,
        "disc-3": 0xe494,
        "download": 0xe0b2,
        "external-link": 0xe0b9,
        "fast-forward": 0xe0bd,
        "folder-open": 0xe247,
        "gallery-horizontal": 0xe4ce,
        "gamepad-2": 0xe0df,
        "gauge": 0xe1bf,
        "hard-drive": 0xe0ed,
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
