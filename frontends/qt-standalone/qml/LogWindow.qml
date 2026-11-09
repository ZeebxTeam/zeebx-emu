import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import zeebx

// The run's log window. Separate from the game's on purpose, as in egui: whoever is reading the
// log wants both at the same time, and a panel inside the game window would steal space from the
// frame. It follows the game: it only exists while there is a run to record.
ApplicationWindow {
    id: window

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

    // Whether the window should be showing: the game open, the option on, and not closed in this
    // run. The main window sets it, since it knows about the game and the settings.
    property bool requested: false
    // How many times the window was closed, so the `requested` binding rereads `Log`.
    property int dismissals: 0
    // What the last export resulted in.
    property string message: ""
    readonly property alias log: log

    width: 640
    height: 420
    minimumWidth: 360
    minimumHeight: 200
    visible: false
    // **The log does not take the keyboard.** KWin gave it focus when it appeared, and on Wayland
    // focus only goes back to another window in response to a user action: asking for it back for
    // the game did not help, and the keyboard stayed on the log. The mouse still works — scroll,
    // export, close.
    flags: Qt.Window | Qt.WindowDoesNotAcceptFocus
    title: "Zeebx — " + tr("debug.log.title")

    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    onRequestedChanged: {
        if (requested) {
            message = ""
            pinned = true
            reloadLines()
            if (transientParent === null || transientParent.active)
                present()
        } else {
            hide()
        }
    }

    // **Only after the game window exists in the compositor.** The log is a child of the game
    // window, and that is what keeps it on top of it. Shown at the same instant as it, the log
    // reached KWin before the game window existed, with no parent: KWin saw it as a standalone
    // window (`transient=false`, measured by a KWin script that lists the stack) and left it behind
    // the game. The game becoming active is the sign that its window is already there.
    Connections {
        target: window.transientParent
        function onActiveChanged() {
            if (window.requested && !window.visible && window.transientParent.active)
                window.present()
        }
    }

    // On top of the game, and with the keyboard given back to it: the log is for reading, and the
    // game keeps being played.
    function present() {
        showNormal()
        if (transientParent)
            Qt.callLater(() => window.transientParent.requestActivate())
    }

    // Closing the window dismisses the log for **this** run, not the preference: saving that
    // would turn the option off on its own, and the window would open once and never again.
    onClosing: {
        log.dismiss()
        dismissals += 1
    }

    Log {
        id: log
    }

    // The lines change while the game runs, and the log does not notify: it is reread from time
    // to time. Reread only if it changed, so as not to lose the scroll of someone reading the
    // middle.
    property var lines: []
    // Stuck to the end: a log that does not follow what just happened is useless. Scrolling up to
    // read releases the list, and scrolling down to the end sticks it again. Only the user changes
    // this: swapping the lines moves the scroll, and is marked as `adjusting` so it does not count.
    property bool pinned: true
    property bool adjusting: false
    function reloadLines() {
        const fresh = log.lines()
        if (fresh.length === lines.length
                && (fresh.length === 0 || fresh[fresh.length - 1] === lines[lines.length - 1]))
            return
        const height = list.contentY
        adjusting = true
        lines = fresh
        if (pinned)
            list.positionViewAtEnd()
        else
            list.contentY = height
        adjusting = false
        // On the first read the window has not even appeared yet, and the list has no height: the
        // end only exists after layout.
        if (pinned)
            Qt.callLater(window.scrollToEnd)
    }
    function scrollToEnd() {
        adjusting = true
        list.positionViewAtEnd()
        adjusting = false
    }
    Timer {
        interval: 250
        repeat: true
        running: window.visible
        onTriggered: window.reloadLines()
    }

    header: ToolBar {
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8
            anchors.rightMargin: 8

            Button {
                text: window.tr("debug.log.export")
                onClicked: {
                    const message = log.save()
                    if (message !== "")
                        window.message = message
                }
            }
            // What the export resulted in, or, before it, the fixed path in sight: whoever wants
            // the file does not need to export anything.
            Label {
                Layout.fillWidth: true
                elide: Text.ElideMiddle
                opacity: 0.6
                text: window.message !== "" ? window.message : window.requested ? log.path() : ""
            }
        }
    }

    Label {
        anchors.centerIn: parent
        visible: window.lines.length === 0
        opacity: 0.6
        text: window.tr("debug.log.empty")
    }

    ListView {
        id: list

        anchors.fill: parent
        anchors.margins: 8
        clip: true
        model: window.lines
        boundsBehavior: Flickable.StopAtBounds
        onContentYChanged: if (!window.adjusting) window.pinned = atYEnd
        ScrollBar.vertical: ScrollBar {}

        delegate: Text {
            required property string modelData

            width: ListView.view.width
            wrapMode: Text.WrapAnywhere
            color: palette.text
            font.family: "monospace"
            text: modelData
        }
    }
}
