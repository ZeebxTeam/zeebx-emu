import QtQuick
import QtQuick.Controls

import zeebx

// The library as a list: one game per row, the small box on the left and, beside it, the name, the
// ClassID and the description. It is the view that shows the most about each game at once, for
// whoever recognizes a title by its details rather than by its cover.
ListView {
    id: list

    required property var library
    // A game was chosen to play: by click, by Enter or by the controller.
    signal open(int line)
    // The right-click menu: the game's own settings, or its details.
    signal configure(int line)
    signal inspect(int line)
    // The keyboard moves through the list. False with another window on top — see `Main.overlaid`.
    property bool listening: true

    // The box keeps the 112×145 proportion of the grid card, so the same cover reads the same way
    // in both views; the row is tall enough for the name and two lines of detail.
    readonly property int coverWidth: 46
    readonly property int coverHeight: 60
    readonly property int rowHeight: 80

    clip: true
    model: library
    focus: true
    spacing: 2
    // The list does not wrap around, like the grid: going down from the last row stays on it.
    keyNavigationWraps: false
    keyNavigationEnabled: listening
    highlightFollowsCurrentItem: true
    ScrollBar.vertical: ScrollBar {}

    Keys.onReturnPressed: if (listening) open(currentIndex)
    Keys.onEnterPressed: if (listening) open(currentIndex)
    Keys.onSpacePressed: if (listening) open(currentIndex)
    // The same keys as the grid and the slider: X and C are settings and details, like b2 and b3 on
    // the controller — that is where they sit on the factory keyboard. The Menu key and Shift+F10
    // open the chosen row's menu, like the right click.
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

    // A single menu for the whole list, opened over the chosen row.
    Menu {
        id: gameMenu

        property int line: -1

        MenuItem {
            text: list.tr("library.menu.configure") + "  (X)"
            onTriggered: list.configure(gameMenu.line)
        }
        MenuItem {
            text: list.tr("library.menu.details") + "  (C)"
            onTriggered: list.inspect(gameMenu.line)
        }
    }

    // The title and the description go into the tooltip as rich text: a "&" or a "<" in the name
    // must not turn into markup.
    function escapeHtml(text) {
        return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    }

    // What the controller asks for, through `Library.commands`: the directions move, and open
    // plays. Left and right do nothing here: there is no second axis in a list.
    function command(code) {
        switch (code) {
        case 0: step(-1); break
        case 1: step(1); break
        case 4: open(currentIndex); break
        }
    }

    // One row up or down, without leaving the list. `incrementCurrentIndex` would wrap around, and
    // the grid and the slider's keys do not — see `keyNavigationWraps`.
    function step(delta) {
        if (count === 0)
            return
        const next = currentIndex + delta
        if (next < 0 || next >= count)
            return
        currentIndex = next
        positionViewAtIndex(next, ListView.Contain)
    }

    delegate: Item {
        id: row

        required property int index
        required property string title
        required property string description
        required property string classId
        required property string cover
        required property bool sharpCover
        required property string rating

        readonly property bool chosen: ListView.isCurrentItem

        width: ListView.view ? ListView.view.width : 0
        height: list.rowHeight

        Rectangle {
            id: backdrop

            anchors.fill: parent
            anchors.leftMargin: 4
            // Room for the scroll bar, which is drawn over the rows.
            anchors.rightMargin: 14
            radius: 6
            color: touch.containsMouse ? palette.midlight : palette.button
            border.color: row.chosen ? palette.highlight : palette.mid
            border.width: row.chosen ? 2 : 1

            Rectangle {
                id: frame

                anchors.left: parent.left
                anchors.leftMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                width: list.coverWidth
                height: list.coverHeight
                radius: 3
                color: palette.dark

                Image {
                    anchors.fill: parent
                    anchors.margins: 1
                    fillMode: Image.PreserveAspectFit
                    source: row.cover
                    smooth: !row.sharpCover
                    mipmap: !row.sharpCover
                }
            }

            // The rating strip is wide and short: it takes the right edge, and the text stops at it.
            Image {
                id: ratingImage

                anchors.right: parent.right
                anchors.rightMargin: 10
                anchors.top: parent.top
                anchors.topMargin: 10
                height: 25
                fillMode: Image.PreserveAspectFit
                source: row.rating
                smooth: true
            }

            // Three lines, as tall as the row allows: the name, the ClassID and the description.
            Text {
                anchors.left: frame.right
                anchors.leftMargin: 12
                anchors.right: ratingImage.visible ? ratingImage.left : parent.right
                anchors.rightMargin: 10
                anchors.top: parent.top
                anchors.topMargin: 7
                elide: Text.ElideRight
                font.pixelSize: 14
                font.bold: true
                color: palette.buttonText
                text: row.title
            }
            Text {
                anchors.left: parent.left
                anchors.leftMargin: 8 + list.coverWidth + 12
                anchors.right: ratingImage.visible ? ratingImage.left : parent.right
                anchors.rightMargin: 10
                anchors.top: parent.top
                anchors.topMargin: 29
                elide: Text.ElideRight
                font.pixelSize: 11
                font.family: "monospace"
                color: palette.buttonText
                opacity: 0.55
                text: row.classId
            }
            Text {
                anchors.left: parent.left
                anchors.leftMargin: 8 + list.coverWidth + 12
                anchors.right: ratingImage.visible ? ratingImage.left : parent.right
                anchors.rightMargin: 10
                anchors.top: parent.top
                anchors.topMargin: 46
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 6
                wrapMode: Text.Wrap
                elide: Text.ElideRight
                maximumLineCount: 2
                font.pixelSize: 12
                color: palette.buttonText
                opacity: 0.85
                text: row.description
            }

            // The chosen one gets the translucent fill, like the grid card.
            Rectangle {
                anchors.fill: parent
                radius: parent.radius
                visible: row.chosen
                color: palette.highlight
                opacity: 0.16
            }

            MouseArea {
                id: touch

                anchors.fill: parent
                hoverEnabled: true
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                // The click gives the keyboard back to the list: coming from search, the arrows
                // stayed in the field. The right one chooses the row and opens its menu, without
                // playing.
                onClicked: (mouse) => {
                    list.forceActiveFocus()
                    list.currentIndex = row.index
                    if (mouse.button === Qt.RightButton) {
                        gameMenu.line = row.index
                        gameMenu.popup()
                    } else {
                        list.open(row.index)
                    }
                }
            }

            // The row elides a long description, so the full text waits for the pointer, with the
            // Z-Wheel description when it knows the game. The width has a cap, like the grid's.
            ToolTip {
                visible: touch.containsMouse
                delay: 500
                width: Math.min(implicitWidth, 320)
                contentItem: Label {
                    // The tooltip's color, not the window text's: in a theme with a dark tooltip
                    // and a light window, the text disappeared.
                    color: palette.toolTipText
                    wrapMode: Text.Wrap
                    text: "<b>" + list.escapeHtml(row.title) + "</b>"
                          + (row.description !== "" ? "<br><br>" + list.escapeHtml(row.description) : "")
                          + (row.classId !== "" ? "<br><br><small>" + row.classId + "</small>" : "")
                }
            }
        }
    }
}
