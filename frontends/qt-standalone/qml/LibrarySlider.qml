import QtQuick
import QtQuick.Controls

import zeebx

// The library the Z-Wheel way: the reel of logos spinning like a cylinder, the name, the boxes
// sliding, and the rating with the description below. The measurements and curves come from the
// slider of the egui interface (`src/ui/app/vitrine.rs`, until 0.4.2).
Item {
    id: slider

    // Returns `value`, and makes the binding that calls this depend on `versions`.
    //
    // **The versions go in as an argument, not in a bare expression.** QML is compiled ahead of
    // time (qmlcachegen), and the compiler drops a read whose value is not used:
    // `(cfg.version, cfg.device())` lost the version read, the binding stopped depending on it,
    // and switching the device from Boomerang to Z-Pad did not switch the screen. Interpreted, as
    // in qmltestrunner, it worked — which is why the tests did not catch it.
    function dependsOn(versions, value) {
        return value
    }

    required property var library
    signal open(int line)
    signal configure(int line)
    signal inspect(int line)
    // The keyboard moves through the slider. False with another window on top — see
    // `Main.overlaid`.
    property bool listening: true

    // The chosen game, unbounded: the slider wraps around, and the position in the list is the
    // remainder of the division. That way the animation from the last to the first moves one step,
    // and not the whole list.
    property int cursor: 0
    // Where the slider is drawn right now, chasing the cursor.
    property real position: 0
    // How many games the list has, and a number that changes every time it is rebuilt: `lines`
    // and `item` are calls, and a binding does not know on its own that the model changed under it.
    property int total: 0
    property int version: 0

    Component.onCompleted: total = library.lines()
    Connections {
        target: slider.library
        function onModelReset() {
            slider.total = slider.library.lines()
            slider.version += 1
        }
    }

    function line(k) {
        return total > 0 ? ((k % total) + total) % total : 0
    }
    // The search changed: the choice goes back to the first result, without the animation crossing
    // the list.
    function restart() {
        cursor = 0
        position = 0
    }
    function command(code) {
        switch (code) {
        case 2: cursor -= 1; break
        case 3: cursor += 1; break
        case 4: open(line(cursor)); break
        }
    }
    // A click on the chosen one opens it; on a neighbor, brings it to the middle.
    // The click gives the keyboard back to the slider: coming from search, the arrows stayed in
    // the field.
    function clickAt(k) {
        forceActiveFocus()
        if (k === cursor)
            open(line(cursor))
        else
            cursor = k
    }

    focus: true
    clip: true
    Keys.onLeftPressed: if (listening) cursor -= 1
    Keys.onRightPressed: if (listening) cursor += 1
    Keys.onReturnPressed: if (listening) open(line(cursor))
    Keys.onEnterPressed: if (listening) open(line(cursor))
    Keys.onSpacePressed: if (listening) open(line(cursor))
    // The same keys as the grid: X configures, C shows the details, Menu and Shift+F10 open the
    // menu.
    Keys.onPressed: (event) => {
        event.accepted = false
        if (!listening || event.isAutoRepeat || total === 0)
            return
        if (event.key === Qt.Key_X) {
            configure(line(cursor))
        } else if (event.key === Qt.Key_C) {
            inspect(line(cursor))
        } else if (event.key === Qt.Key_Menu
                   || (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier))) {
            gameMenu.line = line(cursor)
            gameMenu.popup(slider, slider.width / 2, slider.height / 2)
        } else {
            return
        }
        event.accepted = true
    }

    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    // The chosen game's menu, the same as the grid's.
    Menu {
        id: gameMenu

        property int line: -1

        MenuItem {
            text: slider.tr("library.menu.configure") + "  (X)"
            onTriggered: slider.configure(gameMenu.line)
        }
        MenuItem {
            text: slider.tr("library.menu.details") + "  (C)"
            onTriggered: slider.inspect(gameMenu.line)
        }
    }

    // The position chases the cursor with a damped spring: fast at the start, soft at the end.
    FrameAnimation {
        running: slider.visible && slider.position !== slider.cursor
        onTriggered: {
            const dt = Math.min(frameTime, 0.1)
            slider.position += (slider.cursor - slider.position) * (1 - Math.exp(-dt * 12))
            if (Math.abs(slider.cursor - slider.position) < 0.002)
                slider.position = slider.cursor
        }
    }

    // The wheel moves one game per notch, on any axis and from any device.
    //
    // **A `MouseArea`, not a `WheelHandler`.** `WheelHandler` only accepts the mouse by default
    // (`acceptedDevices` 1, checked on Qt 6.11) and only one axis: on a laptop, two-finger
    // touchpad scrolling did not move the slider. A `MouseArea` with no buttons gets the wheel from
    // everything, on both axes; the panels and boxes on top do not handle the wheel, and it gets
    // here.
    property real run: 0
    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.NoButton
        onWheel: (event) => {
            // A mouse notch is 120 in `angleDelta`; the touchpad sends the same motion in smaller
            // pieces, which add up until they are worth a step.
            slider.run += (event.angleDelta.x - event.angleDelta.y) / 2
            while (Math.abs(slider.run) >= 60) {
                slider.cursor += Math.sign(slider.run)
                slider.run -= 60 * Math.sign(slider.run)
            }
        }
    }

    readonly property real reelHeight: Math.min(Math.max(height * 0.17, 56), 104)
    readonly property real infoHeight: Math.min(Math.max(height * 0.2, 72), 130)
    readonly property real stageTop: 8 + reelHeight + 6 + 40 + 4
    readonly property real stageHeight: height - infoHeight - stageTop
    readonly property var current: slider.dependsOn([version], total > 0 ? library.item(line(cursor)) : ({}))

    // The reel: a cylinder of panels, each at an angle. The front one is the chosen game's.
    Rectangle {
        id: reel

        x: 12
        y: 8
        width: parent.width - 24
        height: slider.reelHeight
        radius: height / 2
        color: palette.dark
    }

    Repeater {
        model: 11

        Rectangle {
            id: panel

            readonly property int k: Math.round(slider.position) - 5 + index
            readonly property real angle: (k - slider.position) * 0.42
            readonly property real front: Math.cos(angle)
            readonly property var game: slider.dependsOn([slider.version], slider.total > 0 ? slider.library.item(slider.line(k)) : ({}))
            readonly property bool chosen: k === slider.cursor
                                              && Math.abs(slider.position - slider.cursor) < 0.5

            visible: slider.total > 0 && Math.abs(angle) < Math.PI / 2 * 0.95
            z: 10 - Math.abs(angle)
            width: slider.reelHeight * 1.45 * front
            height: slider.reelHeight * 0.78
            x: slider.width / 2 + slider.width * 0.42 * Math.sin(angle) - width / 2
            y: reel.y + (reel.height - height) / 2
            radius: 6
            color: Qt.rgba(236 / 255, 236 / 255, 236 / 255, 1)
            opacity: 0.25 + 0.75 * front * front
            border.width: chosen ? 2 : 0
            border.color: palette.highlight

            // The logo is square (64×64), but on the Z-Wheel stage it is stretched over a
            // rectangular panel: it was drawn already squeezed in width for that. With no scene on
            // the stage, the panel carries the name, so the reel has no holes.
            Image {
                anchors.fill: parent
                anchors.margins: 3
                visible: panel.game.logo !== undefined && panel.game.logo !== ""
                source: visible ? panel.game.logo : ""
                smooth: true
            }
            Text {
                anchors.fill: parent
                anchors.margins: 4
                visible: panel.game.logo === undefined || panel.game.logo === ""
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
                wrapMode: Text.Wrap
                elide: Text.ElideRight
                font.pixelSize: 11 + 2 * panel.front
                color: "#282828"
                text: panel.game.title !== undefined ? panel.game.title : ""
            }
            Rectangle {
                anchors.fill: parent
                radius: parent.radius
                visible: panel.chosen
                color: palette.highlight
                opacity: 0.18
            }
            MouseArea {
                anchors.fill: parent
                onClicked: slider.clickAt(panel.k)
            }
        }
    }

    // The chosen game's name, above the boxes.
    Text {
        x: 0
        y: reel.y + reel.height + 6
        width: parent.width
        height: 40
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
        font.pixelSize: 24
        font.bold: true
        color: palette.windowText
        text: slider.current.title !== undefined ? slider.current.title : ""
    }

    // The boxes: the chosen one large in the middle, the neighbors smaller and dimmed at the sides.
    Repeater {
        model: 9

        Item {
            id: container

            readonly property int k: Math.round(slider.position) - 4 + index
            readonly property real d: k - slider.position
            readonly property real zoom: 1 / (1 + 0.38 * Math.abs(d))
            readonly property real baseHeight: slider.stageHeight * 0.92
            readonly property var game: slider.dependsOn([slider.version], slider.total > 0 ? slider.library.item(slider.line(k)) : ({}))
            readonly property real aspect: cover.sourceSize.height > 0
                                            ? cover.sourceSize.width / cover.sourceSize.height : 0.77
            readonly property real step: baseHeight * 0.62

            visible: slider.total > 0 && Math.abs(d) < 3.6
            z: 20 - Math.abs(d)
            height: baseHeight * zoom
            width: Math.min(height * aspect, slider.width * 0.5)
            x: slider.width / 2 + step * Math.sign(d) * Math.pow(Math.abs(d) * 1.35, 0.8) - width / 2
            y: slider.stageTop + (slider.stageHeight - height) / 2
            opacity: Math.max(0, Math.min(1, 1 - 0.28 * Math.abs(d)))

            // The shadow gives a floor: without it the boxes look glued to the background.
            Rectangle {
                x: -2
                y: 6 * container.zoom - 2
                width: parent.width + 4
                height: parent.height + 4
                radius: 6
                color: Qt.rgba(0, 0, 0, 90 / 255)
            }
            Image {
                id: cover

                anchors.fill: parent
                source: container.game.cover !== undefined ? container.game.cover : ""
                smooth: !container.game.sharpCover
                mipmap: !container.game.sharpCover
            }
            Rectangle {
                anchors.fill: parent
                anchors.margins: -3
                radius: 6
                color: "transparent"
                visible: Math.abs(container.d) < 0.5
                border.width: 2
                border.color: palette.highlight
            }
            MouseArea {
                anchors.fill: parent
                onClicked: slider.clickAt(container.k)
            }
        }
    }

    // The info card: rating and description.
    Item {
        x: 16
        y: slider.height - slider.infoHeight + 8
        width: slider.width - 32
        height: slider.infoHeight - 12
        clip: true

        Image {
            id: rating

            height: Math.min(parent.height - 8, sourceSize.height)
            fillMode: Image.PreserveAspectFit
            source: slider.current.rating !== undefined ? slider.current.rating : ""
            visible: source !== ""
        }
        Text {
            x: rating.visible ? rating.width + 12 : 0
            width: parent.width - x
            wrapMode: Text.Wrap
            font.pixelSize: 13
            color: palette.windowText
            text: slider.current.description !== undefined ? slider.current.description : ""
        }
    }
}
