import QtQuick
import QtQuick.Window

import zeebx

// The separate game window, the only one until 0.4.2, for those who turned on
// `graphics.separate_game_window`. The game itself lives in `GameStage`, the same one the main
// window uses. It is only shown after the library opens a game, and closing it closes the game.
Window {
    id: window

    width: savedWidth > 0 ? savedWidth : 640
    height: savedHeight > 0 ? savedHeight : 512
    minimumWidth: 320
    minimumHeight: 240
    visible: false
    color: "black"
    title: "Zeebx — " + windowStage.screen.runState

    // As in the main window: the size it closes at is the size it reopens at.
    readonly property real savedWidth: windowStage.screen.windowWidth()
    readonly property real savedHeight: windowStage.screen.windowHeight()

    // The stage, and what the main window's menu reads from it.
    property alias stage: windowStage
    property alias screen: windowStage.screen

    function saveSize() {
        if (visibility === Window.Windowed)
            windowStage.screen.saveWindowSize(width, height)
    }

    onWidthChanged: saveDebounce.restart()
    onHeightChanged: saveDebounce.restart()

    Timer {
        id: saveDebounce
        interval: 500
        onTriggered: window.saveSize()
    }

    // The window reappears for every game opened: that is the time to read its title.
    onVisibleChanged: if (visible) windowStage.prepare()

    // Shows the window in the mode configured for it (`graphics.game_window`).
    function launch() {
        const mode = windowStage.screen.windowMode()
        if (mode === 2)
            showFullScreen()
        else if (mode === 1)
            showMaximized()
        else
            showNormal()
        closeAllowed = false
        raise()
        requestActivate()
    }

    // Closing through the window asks first, like Esc. After the question, or after a game that
    // exited on its own, `close()` closes for good.
    property bool closeAllowed: false
    function shutdown() {
        closeAllowed = true
        close()
    }

    function toggleFullScreen() {
        visibility = visibility === Window.FullScreen ? Window.Windowed : Window.FullScreen
    }

    onClosing: (close) => {
        if (!closeAllowed && windowStage.screen.confirmOnClose()) {
            close.accepted = false
            windowStage.requestClose()
            return
        }
        saveDebounce.stop()
        window.saveSize()
        windowStage.close()
    }
    onActiveChanged: if (!active) windowStage.screen.release()

    GameStage {
        id: windowStage

        anchors.fill: parent
        active: window.visible
        onCloseRequested: window.shutdown()
        onFullScreenRequested: window.toggleFullScreen()
    }
}
