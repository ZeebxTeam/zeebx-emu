import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Shapes

import zeebx

// The game on screen: the frame, the notices on top of it, the keyboard and the per-frame step. It
// lives in a component, and not in a window, because the game can be in two places: in the main
// window, in place of the library, which is the default, or in a window of its own
// (`graphics.separate_game_window`). Each one has its own stage, and only the one where the game is
// runs: there is a single core. See `docs/implementation/28-game-in-the-main-window.md`.
//
// What belongs to the window, like closing or going full screen, leaves here as a signal: the host
// decides.
Item {
    id: stage

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
    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    // Whether the game is on this stage: only the active one steps every frame.
    //
    // **There is no status line under the frame.** The separate window had one, 24 px tall, with
    // the title and where the game draws, and no option removed it. The title is in the window's
    // own, and where the game draws is in the metrics box, which is turned on in Debug.
    property bool active: false
    // Whether the game is stopped by the pause. The main window's menu and toolbar read it.
    property bool paused: false
    property alias screen: screen

    // The game should close now: confirmed in the question, with no question to ask, or the game
    // exited on its own with no Z-Wheel to return to.
    signal closeRequested()
    // Quit Zeebx, confirmed in the question. See `requestQuit`.
    signal quitRequested()
    // F11, or Alt+Enter.
    signal fullScreenRequested()

    // A game started on this stage: its title, and the pause symbol of the previous game goes
    // away. The new session starts running.
    function prepare() {
        screen.refresh()
        symbol.opacity = 0
        paused = false
    }
    function takeFocus() {
        screen.forceActiveFocus()
    }
    // Closes the game for good: the session leaves the core.
    function close() {
        screen.close()
    }

    // Esc, Close in the menu and toolbar, and closing the window all go through here: with the
    // option on, the game pauses and asks first. See `graphics.confirm_on_close`.
    function requestClose() {
        if (screen.confirmOnClose())
            confirmation.ask(false)
        else
            closeRequested()
    }
    // The same, for quitting Zeebx with the game open.
    function requestQuit() {
        if (screen.confirmOnClose())
            confirmation.ask(true)
        else
            quitRequested()
    }

    // What the keyboard and the System menu ask for, through the same path: the symbol in the
    // middle of the screen shows up either way.
    function pause() {
        paused = screen.pause()
        symbol.show(paused ? "pausa" : "play", paused)
    }
    function restart() {
        if (screen.restart()) {
            paused = false
            symbol.show("reinicio", false)
        }
    }
    function screenshot() {
        screen.screenshot(Qt.formatDateTime(new Date(), "yyyy-MM-dd HH-mm-ss"))
    }

    // The background behind the frame, when it does not fill everything.
    Rectangle {
        anchors.fill: parent
        color: "black"
    }

    GameScreen {
        id: screen

        anchors.fill: parent
        // The debug panel is a strip, not an overlay: reserving space shrinks the game frame
        // instead of covering it, as in egui.
        anchors.bottomMargin: panel.visible ? panel.height : 0
        focus: true

        // Esc closes and P pauses, as in the egui window. Neither collides with the Zeebo
        // controller, which uses the arrows, Z, X, C, V, Q, W, F, G, H, Backspace and Enter. F11 and
        // Alt+Enter toggle full screen. Screenshot (F9 by default) and reset (R by default),
        // rebindable, are checked before the game: a button mapped to the same key does not get it.
        // See `docs/implementation/22-screenshots.md`. The save state ones come before all of them:
        // they are combinations, which only match with the same modifiers, and the Shift+F9 of one
        // of them must not turn into the screenshot's F9. See
        // `docs/implementation/25-desktop-save-states.md`.
        Keys.onPressed: (event) => {
            if (screen.isStateShortcut(event.key, event.modifiers)) {
                // Holding does not save or load thirty times. The notice arrives via `stateSerial`.
                if (!event.isAutoRepeat)
                    screen.stateShortcut(event.key, event.modifiers)
            } else if (screen.isScreenshotShortcut(event.key)) {
                // Holding the key takes one screenshot, not thirty.
                if (!event.isAutoRepeat)
                    stage.screenshot()
            } else if (screen.isResetShortcut(event.key)) {
                // Holding does not reset thirty times: the game would restart on every repeat.
                if (!event.isAutoRepeat)
                    stage.restart()
            } else if (event.key === Qt.Key_Escape) {
                // Held down, the repeated Esc would close the question it just opened.
                if (!event.isAutoRepeat)
                    stage.requestClose()
            } else if (event.key === Qt.Key_P && !event.isAutoRepeat) {
                stage.pause()
            } else if (event.key === Qt.Key_F11
                       || (event.key === Qt.Key_Return && (event.modifiers & Qt.AltModifier))) {
                stage.fullScreenRequested()
            } else if (!event.isAutoRepeat) {
                key(event.key, true)
            }
            event.accepted = true
        }
        Keys.onReleased: (event) => {
            if (!event.isAutoRepeat && !screen.isScreenshotShortcut(event.key)
                    && !screen.isResetShortcut(event.key)) {
                key(event.key, false)
            }
            event.accepted = true
        }
        onActiveFocusChanged: if (!activeFocus) release()
        // A game that exits on its own with no Z-Wheel to return to closes, as in egui.
        onClosed: stage.closeRequested()
        onAchievement: (title, text, icon) => achievementToast.show(title, text, icon)
    }

    AchievementToast {
        id: achievementToast

        anchors.left: screen.left
        anchors.bottom: screen.bottom
        anchors.margins: 16
    }

    // The symbol in the middle of the screen, as in a video player: ⏸ while the game is paused; ▶
    // for a moment when it resumes; ↻ when R reset it. Drawn, not glyphs: not every font has ⏸
    // and ↻, and the window does not know which font the system will pick.
    Item {
        id: symbol

        // "pausa", "play" or "reinicio".
        property string shape: ""

        // The Qt 6.6 curve renderer, with smooth edges without multisampling. **Assigned only
        // where it exists**: written as a `Shape` property, the Qt 6.4 of Ubuntu 24.04 rejected
        // the whole file, and with it the main window, which did not open. There it keeps the
        // default renderer.
        function smoothCurves(shape) {
            if (shape.preferredRendererType !== undefined)
                shape.preferredRendererType = Shape.CurveRenderer
        }

        // Shows `which`. What does not `persist` goes away on its own, fading out.
        function show(which, persist) {
            output.stop()
            shape = which
            opacity = 1
            if (!persist)
                output.restart()
        }

        anchors.centerIn: screen
        width: 112
        height: 112
        opacity: 0
        visible: opacity > 0

        SequentialAnimation {
            id: output

            PauseAnimation { duration: 350 }
            NumberAnimation {
                target: symbol
                property: "opacity"
                to: 0
                duration: 450
                easing.type: Easing.InQuad
            }
        }

        Rectangle {
            anchors.fill: parent
            radius: width / 2
            color: "#a0000000"
            border.color: "#60ffffff"
            border.width: 2
        }

        Row {
            anchors.centerIn: parent
            spacing: 14
            visible: symbol.shape === "pausa"

            Repeater {
                model: 2

                Rectangle {
                    width: 14
                    height: 44
                    radius: 3
                    color: "white"
                }
            }
        }

        // The play triangle, a little right of center: its center of mass is not the box's, and
        // centered by the box it looks pulled to the left.
        Shape {
            anchors.centerIn: parent
            anchors.horizontalCenterOffset: 5
            width: 44
            height: 48
            visible: symbol.shape === "play"
            Component.onCompleted: symbol.smoothCurves(this)

            ShapePath {
                fillColor: "white"
                strokeColor: "transparent"
                startX: 0
                startY: 0
                PathLine { x: 44; y: 24 }
                PathLine { x: 0; y: 48 }
                PathLine { x: 0; y: 0 }
            }
        }

        // The circular reset arrow: a 300-degree arc, clockwise, with the arrowhead at its end.
        Shape {
            anchors.centerIn: parent
            width: 56
            height: 56
            visible: symbol.shape === "reinicio"
            Component.onCompleted: symbol.smoothCurves(this)

            ShapePath {
                fillColor: "transparent"
                strokeColor: "white"
                strokeWidth: 7
                capStyle: ShapePath.RoundCap
                PathAngleArc {
                    centerX: 28
                    centerY: 28
                    radiusX: 21
                    radiusY: 21
                    startAngle: -50
                    sweepAngle: 280
                }
            }
            // The arrowhead, at the end of the arc (230 degrees), pointing where it continues.
            ShapePath {
                fillColor: "white"
                strokeColor: "transparent"
                startX: 28 + 21 * Math.cos(230 * Math.PI / 180) + 12 * Math.cos(320 * Math.PI / 180)
                startY: 28 + 21 * Math.sin(230 * Math.PI / 180) + 12 * Math.sin(320 * Math.PI / 180)
                PathLine {
                    x: 28 + 21 * Math.cos(230 * Math.PI / 180) + 9 * Math.cos(230 * Math.PI / 180)
                    y: 28 + 21 * Math.sin(230 * Math.PI / 180) + 9 * Math.sin(230 * Math.PI / 180)
                }
                PathLine {
                    x: 28 + 21 * Math.cos(230 * Math.PI / 180) - 9 * Math.cos(230 * Math.PI / 180)
                    y: 28 + 21 * Math.sin(230 * Math.PI / 180) - 9 * Math.sin(230 * Math.PI / 180)
                }
                PathLine {
                    x: 28 + 21 * Math.cos(230 * Math.PI / 180) + 12 * Math.cos(320 * Math.PI / 180)
                    y: 28 + 21 * Math.sin(230 * Math.PI / 180) + 12 * Math.sin(320 * Math.PI / 180)
                }
            }
        }
    }

    // The word under ⏸, for those who do not recognize the symbol.
    Text {
        anchors.top: symbol.bottom
        anchors.topMargin: 8
        anchors.horizontalCenter: symbol.horizontalCenter
        visible: symbol.shape === "pausa" && symbol.visible
        color: "white"
        style: Text.Outline
        font.pixelSize: 20
        text: stage.tr("play.paused")
    }

    // The Boomerang calibration notice, in the bottom corner: the model tilts like the controller,
    // and the text says whether it is still. It is a floating area, which takes no space from the
    // frame. See `src/ui/calibration.rs`.
    Rectangle {
        anchors.right: screen.right
        anchors.bottom: screen.bottom
        anchors.margins: 16
        width: content.implicitWidth + 24
        height: content.implicitHeight + 16
        visible: screen.noticeTitle !== ""
        radius: 6
        color: "#e6202020"
        border.color: "#505050"

        Row {
            id: content

            anchors.centerIn: parent
            spacing: 12

            Item {
                width: 110
                height: 77

                Image {
                    anchors.centerIn: parent
                    width: 110
                    fillMode: Image.PreserveAspectFit
                    source: "qrc:/zeebx/boomerang.png"
                    rotation: screen.noticeRotation
                    smooth: true
                }
            }

            Column {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 4

                Text {
                    color: "white"
                    font.bold: true
                    text: screen.noticeTitle
                }

                Text {
                    visible: text !== ""
                    color: screen.noticeStill ? "#d0d0d0" : "#ffb040"
                    text: screen.noticeStatus
                }
            }
        }
    }

    // Fast-forward and turbo when on, in the top-left corner, while they apply: a toggle left on
    // by mistake looks like a game bug. See `docs/implementation/24-speed.md`.
    Rectangle {
        anchors.left: screen.left
        anchors.top: screen.top
        anchors.margins: 16
        width: indicatorsLabel.implicitWidth + 24
        height: indicatorsLabel.implicitHeight + 16
        visible: screen.indicators !== ""
        radius: 6
        color: "#e6202020"
        border.color: "#505050"

        Text {
            id: indicatorsLabel

            anchors.centerIn: parent
            color: "white"
            font.bold: true
            text: screen.indicators
        }
    }

    // The metrics, in the top-right corner, with the Debug option: frames per second highlighted,
    // the speed and where the game draws. On top of the frame, like the other notices, and not a
    // strip: the bottom panel shrinks the game, and this is meant to stay on while playing.
    Rectangle {
        id: metrics

        anchors.right: screen.right
        anchors.top: screen.top
        anchors.margins: 16
        width: Math.min(metricsLines.implicitWidth + 24, screen.width / 2)
        height: metricsLines.implicitHeight + 16
        visible: screen.metrics.length > 0
        radius: 6
        color: "#c0202020"
        border.color: "#505050"

        // A layout, not a `Column`: its natural width is that of the widest text, not the one it
        // was given, so the box can depend on it without closing a loop.
        ColumnLayout {
            id: metricsLines

            anchors.centerIn: parent
            width: parent.width - 24
            spacing: 2

            Repeater {
                model: screen.metrics

                Text {
                    required property int index
                    required property string modelData

                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignRight
                    elide: Text.ElideRight
                    color: index === 0 ? "white" : "#d0d0d0"
                    font.bold: index === 0
                    font.pixelSize: index === 0 ? 18 : 12
                    text: modelData
                }
            }
        }
    }

    // The screenshot notice, in the top corner, for two seconds. Qt draws it on top of the frame,
    // and that is why it does not end up in the PNG, which comes from the core. Clicking opens the
    // game's folder. Below the metrics, when they are showing.
    Rectangle {
        id: screenshotNotice

        anchors.right: screen.right
        anchors.top: metrics.visible ? metrics.bottom : screen.top
        anchors.topMargin: metrics.visible ? 8 : 16
        anchors.rightMargin: 16
        width: screenshotLabel.implicitWidth + 24
        height: screenshotLabel.implicitHeight + 16
        visible: false
        radius: 6
        color: "#e6202020"
        border.color: "#505050"

        Text {
            id: screenshotLabel

            anchors.centerIn: parent
            color: screen.screenshotFailed ? "#ffb040" : "white"
            text: screen.screenshotText
        }

        MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: Qt.openUrlExternally(screen.screenshotDir)
        }

        Timer {
            id: hideScreenshotNotice

            interval: 2000
            onTriggered: screenshotNotice.visible = false
        }

        Connections {
            target: screen
            function onScreenshotSerialChanged() {
                screenshotNotice.visible = true
                hideScreenshotNotice.restart()
            }
        }
    }

    // The save state notice, in the same corner, for the same time: the chosen slot, the state
    // saved or loaded, or why it failed. Below the screenshot one, so both fit together.
    Rectangle {
        id: saveStateNotice

        anchors.right: screen.right
        anchors.top: screenshotNotice.visible ? screenshotNotice.bottom
                     : metrics.visible ? metrics.bottom : screen.top
        anchors.topMargin: screenshotNotice.visible || metrics.visible ? 8 : 16
        anchors.rightMargin: 16
        width: saveStateLabel.implicitWidth + 24
        height: saveStateLabel.implicitHeight + 16
        visible: false
        radius: 6
        color: "#e6202020"
        border.color: "#505050"

        Text {
            id: saveStateLabel

            anchors.centerIn: parent
            color: screen.stateFailed ? "#ffb040" : "white"
            text: screen.stateText
        }

        Timer {
            id: hideSaveStateNotice

            interval: 2000
            onTriggered: saveStateNotice.visible = false
        }

        Connections {
            target: screen
            function onStateSerialChanged() {
                saveStateNotice.visible = true
                hideSaveStateNotice.restart()
            }
        }
    }

    // The debug panel: the numbers and, if it fits, the timeline. See `ui/debug_panel.rs`.
    Rectangle {
        id: panel

        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: 48
        visible: screen.debugPanel !== ""
        color: "#1c1c1c"

        Text {
            anchors.left: parent.left
            anchors.right: chart.visible ? chart.left : parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.margins: 8
            elide: Text.ElideRight
            color: "#d0d0d0"
            font.family: "monospace"
            text: screen.debugPanel
        }

        // Speed (blue, up to 200%) and frames per second (green, up to 60) over the last minute,
        // with the 100% line in the middle: above it the game keeps the console's pace, below it,
        // falls behind. The width is fixed for the same reason as in egui: one that changed with
        // the window would make the same drop look different on every resize.
        Canvas {
            id: chart

            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.rightMargin: 8
            width: 220
            height: 40
            visible: screen.timeline.length > 0 && panel.width >= width + 200

            Connections {
                target: screen
                function onTimelineChanged() { chart.requestPaint() }
            }

            onPaint: {
                const g = getContext("2d")
                const points = screen.timeline
                const samples = points.length / 2
                g.reset()
                g.fillStyle = "rgba(0, 0, 0, 0.47)"
                g.fillRect(0, 0, width, height)
                g.strokeStyle = "rgba(255, 255, 255, 0.16)"
                g.beginPath()
                g.moveTo(0, height / 2)
                g.lineTo(width, height / 2)
                g.stroke()
                const step = width / Math.max(samples, 2)
                for (const [generation, color, cap] of [[0, "rgb(120, 200, 255)", 200], [1, "rgb(160, 255, 160)", 60]]) {
                    g.strokeStyle = color
                    g.beginPath()
                    for (let i = 0; i < samples; i++) {
                        const y = height - height * Math.min(points[2 * i + generation] / cap, 1)
                        if (i === 0)
                            g.moveTo(0, y)
                        else
                            g.lineTo(i * step, y)
                    }
                    g.stroke()
                }
            }
        }
    }

    // A game that stopped keeps its last frame showing, but the reason has to appear somewhere: a
    // strip at the bottom, as in egui, with what to do next.
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.bottomMargin: panel.visible ? panel.height : 0
        height: strip.implicitHeight + 12
        visible: screen.stopped !== ""
        color: "#c8000000"

        Column {
            id: strip

            anchors.verticalCenter: parent.verticalCenter
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.margins: 8
            spacing: 2

            Text {
                width: parent.width
                wrapMode: Text.Wrap
                color: "#ffb040"
                text: screen.stopped
            }

            Text {
                color: "#9a9a9a"
                text: stage.tr("play.stop.hint")
            }
        }
    }

    // The question before closing the game, or quitting Zeebx with it open. It lives on the stage,
    // and not in the main window, to appear where the game is: in the separate window, a dialog of
    // the main one would open behind it. The game stays paused while it is open, and cancelling
    // leaves it as it was.
    Dialog {
        id: confirmation

        // Whether the question is about quitting Zeebx, and not just closing the game.
        property bool quitting: false
        // Whether it was the question that paused the game: a game that was already paused stays
        // that way.
        property bool pausedGame: false
        property bool confirmed: false

        function ask(quit) {
            quitting = quit
            confirmed = false
            pausedGame = !stage.paused
            if (pausedGame)
                stage.pause()
            open()
        }

        anchors.centerIn: parent
        width: Math.min(420, stage.width - 32)
        modal: true
        title: stage.tr(quitting ? "play.confirm_quit.title" : "play.confirm_close.title")

        Label {
            width: parent.width
            wrapMode: Text.Wrap
            text: stage.tr(confirmation.quitting ? "play.confirm_quit.text" : "play.confirm_close.text")
        }

        footer: DialogButtonBox {
            Button {
                text: stage.tr("play.confirm.cancel")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
            Button {
                text: stage.tr(confirmation.quitting ? "play.confirm_quit.ok" : "play.confirm_close.ok")
                DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            }
        }

        onAccepted: confirmed = true
        // Through `closed`, not `rejected`: Esc and clicking outside close without going through it.
        onClosed: {
            if (confirmed) {
                if (quitting)
                    stage.quitRequested()
                else
                    stage.closeRequested()
                return
            }
            if (pausedGame && stage.paused)
                stage.pause()
            stage.takeFocus()
        }
    }

    // One step per window frame, at the vsync pace — like egui's `request_repaint`. A zero-interval
    // `Timer` would keep the CPU at 100% for nothing. Only on the game's stage: the other one does
    // not run.
    FrameAnimation {
        running: stage.active
        onTriggered: screen.step()
    }
}
