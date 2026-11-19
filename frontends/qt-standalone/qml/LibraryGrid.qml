import QtQuick
import QtQuick.Controls

import zeebx

// The library as a grid: portrait cards, in the proportion of the boxes the Z-Wheel provides, as
// many as fit in the width. It is the grid the egui interface had (`src/ui/app/vitrine.rs`, until
// 0.4.2).
GridView {
    id: grid

    required property var library
    // A card was chosen to play: by click, by Enter or by the controller.
    signal open(int line)
    // The right-click menu: the game's own settings, or its details.
    signal configure(int line)
    signal inspect(int line)
    // The keyboard moves through the grid. False with another window on top — see `Main.overlaid`.
    property bool listening: true

    // The card is 136 wide; the image fits in a 112×145 frame — the `COVER_FRAME` of
    // `library.rs`, which decides when to upscale without interpolating —, and the title takes two
    // lines.
    readonly property int cardWidth: 136
    readonly property int cardHeight: 145 + 36 + 24

    clip: true
    cellWidth: cardWidth + 8
    cellHeight: cardHeight + 8
    model: library
    focus: true
    // The grid does not wrap around: going down from the last row stays on the last one.
    keyNavigationWraps: false
    keyNavigationEnabled: listening
    highlightFollowsCurrentItem: true
    ScrollBar.vertical: ScrollBar {}

    Keys.onReturnPressed: if (listening) open(currentIndex)
    Keys.onEnterPressed: if (listening) open(currentIndex)
    Keys.onSpacePressed: if (listening) open(currentIndex)
    // X and C are settings and details, like b2 and b3 on the controller — that is where they sit
    // on the factory keyboard. The Menu key and Shift+F10 open the chosen card's menu, like the
    // right click.
    Keys.onPressed: (event) => {
        event.accepted = false
        if (!listening || event.isAutoRepeat || count === 0)
            return
        if (event.key === Qt.Key_X) {
            configure(currentIndex)
        } else if (event.key === Qt.Key_C) {
            inspect(currentIndex)
        } else if (event.key === Qt.Key_Menu
                   || (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier))) {
            gameMenu.line = currentIndex
            if (currentItem)
                gameMenu.popup(currentItem, currentItem.width / 2, currentItem.height / 2)
            else
                gameMenu.popup()
        } else {
            return
        }
        event.accepted = true
    }

    // Returns `value`, and makes the binding depend on `versions`: see the slider's `dependsOn`.
    function dependsOn(versions, value) {
        return value
    }
    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    // A single menu for the whole grid, opened over the clicked card.
    Menu {
        id: gameMenu

        property int line: -1

        MenuItem {
            text: grid.tr("library.menu.configure") + "  (X)"
            onTriggered: grid.configure(gameMenu.line)
        }
        MenuItem {
            text: grid.tr("library.menu.details") + "  (C)"
            onTriggered: grid.inspect(gameMenu.line)
        }
    }

    // The title and the description go into the tooltip as rich text: a "&" or a "<" in the name
    // must not turn into markup.
    function escapeHtml(text) {
        return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    }

    // What the controller asks for, through `Library.commands`: the directions move, and open
    // plays.
    function command(code) {
        switch (code) {
        case 0: moveCurrentIndexUp(); break
        case 1: moveCurrentIndexDown(); break
        case 2: moveCurrentIndexLeft(); break
        case 3: moveCurrentIndexRight(); break
        case 4: open(currentIndex); break
        }
    }

    delegate: Item {
        id: card

        required property int index
        required property string title
        required property string description
        required property string classId
        required property string cover
        required property bool sharpCover

        readonly property bool chosen: GridView.isCurrentItem

        width: grid.cellWidth
        height: grid.cellHeight

        Rectangle {
            id: backdrop

            anchors.centerIn: parent
            width: grid.cardWidth
            height: grid.cardHeight
            radius: 8
            color: touch.containsMouse ? palette.midlight : palette.button
            border.color: card.chosen ? palette.highlight : palette.mid
            border.width: card.chosen ? 2 : 1

            // The image fits in the frame without stretching: a 65×42 icon distorted into a
            // square looks worse than one with margins on the sides. And a small icon upscales
            // without interpolation, which is the square block the console showed.
            Image {
                id: image

                x: (parent.width - 112) / 2
                y: 12
                width: 112
                height: 145
                fillMode: Image.PreserveAspectFit
                source: card.cover
                smooth: !card.sharpCover
                mipmap: !card.sharpCover
            }

            Text {
                anchors.top: image.bottom
                anchors.topMargin: 8
                anchors.horizontalCenter: parent.horizontalCenter
                width: parent.width - 16
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                maximumLineCount: 2
                elide: Text.ElideRight
                font.pixelSize: 12
                color: palette.buttonText
                text: card.title
            }

            // The chosen one gets the translucent fill, like the Z-Wheel selection.
            Rectangle {
                anchors.fill: parent
                radius: parent.radius
                visible: card.chosen
                color: palette.highlight
                opacity: 0.16
            }

            MouseArea {
                id: touch

                anchors.fill: parent
                hoverEnabled: true
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                // The click gives the keyboard back to the grid: coming from search, the arrows
                // stayed in the field. The right one chooses the card and opens its menu, without
                // playing.
                onClicked: (mouse) => {
                    grid.forceActiveFocus()
                    grid.currentIndex = card.index
                    if (mouse.button === Qt.RightButton) {
                        gameMenu.line = card.index
                        gameMenu.popup()
                    } else {
                        grid.open(card.index)
                    }
                }
            }

            // The card cuts a long title, so the full name waits for the pointer, with the
            // Z-Wheel description when it knows the game. The width has a cap, like egui's 320
            // points: without it the text broke into one word per line.
            ToolTip {
                visible: touch.containsMouse
                delay: 500
                width: Math.min(implicitWidth, 320)
                contentItem: Label {
                    // The tooltip's color, not the window text's: in a theme with a dark tooltip
                    // and a light window, the text disappeared.
                    color: palette.toolTipText
                    wrapMode: Text.Wrap
                    text: "<b>" + grid.escapeHtml(card.title) + "</b>"
                          + (card.description !== "" ? "<br><br>" + grid.escapeHtml(card.description) : "")
                          + (card.classId !== "" ? "<br><br><small>" + card.classId + "</small>" : "")
                }
            }
        }
    }
}
