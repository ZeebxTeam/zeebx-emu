import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Window

import zeebx
import zeebx.htpc

// Fixed HTPC host shell: single fullscreen window (Gamescope/Steam Big Picture safe),
// zero-copy GameScreen, pluggable theme Loader with automatic fallback to Z-Wheel HD,
// Quick Menu overlay, gamepad-only Settings, Folder Picker and Game Details.
ApplicationWindow {
    id: root

    required property string initialGame

    width: 1280
    height: 720
    visible: true
    visibility: Window.FullScreen
    color: "#000000"
    title: playing ? ("Zeebx HTPC — " + HtpcController.activeGameTitle) : "Zeebx HTPC"

    // True when launched as `zeebx-htpc <rom>` from Steam / Steam ROM Manager / ES-DE:
    // exiting the game closes the process cleanly so focus returns to the launcher.
    property bool launcherMode: false
    property bool playing: false
    property bool quickMenuOpen: false
    property bool settingsOpen: false
    property bool detailsOpen: false
    property bool folderPickerOpen: false
    property bool onboardingOpen: false
    property var selectedDetails: ({})
    property int selectedLine: -1

    // Toast notification state
    property string toastText: ""
    property bool toastError: false

    function dependsOn(versions, value) {
        return value
    }

    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    function showToast(text, isError) {
        if (!text || text === "")
            return
        toastText = text
        toastError = !!isError
        toastTimer.restart()
    }

    function safeMargin() {
        return Math.round(Math.min(root.width, root.height) * HtpcController.overscanPercent / 100.0)
    }

    function presentOpenError(error) {
        if (error !== "") {
            showToast(error, true)
            return false
        }
        HtpcController.refreshActiveGame()
        HtpcController.setPaused(false)
        HtpcController.muteNavigation()
        playing = true
        quickMenuOpen = false
        settingsOpen = false
        detailsOpen = false
        folderPickerOpen = false
        onboardingOpen = false
        screen.refresh()
        return true
    }

    function launchGame(line) {
        const err = library.open(line)
        presentOpenError(err)
    }

    function launchZWheel() {
        const err = library.openZWheel()
        presentOpenError(err)
    }

    function openQuickMenu() {
        if (!playing)
            return
        HtpcController.refreshActiveGame()
        HtpcController.setPaused(true)
        HtpcController.muteNavigation()
        screen.release()
        quickMenu.reset()
        quickMenuOpen = true
    }

    function closeQuickMenu() {
        quickMenuOpen = false
        HtpcController.muteNavigation()
        HtpcController.setPaused(false)
    }

    function stopGameAndReturn() {
        screen.close()
        quickMenuOpen = false
        playing = false
        HtpcController.muteNavigation()
        if (launcherMode) {
            Qt.quit()
        }
    }

    function openSettings() {
        settingsOpen = true
        detailsOpen = false
        onboardingOpen = false
        HtpcController.muteNavigation()
        settingsOverlay.reset()
    }

    function closeSettings() {
        settingsOpen = false
        HtpcController.muteNavigation()
        if (!HtpcController.hasRomsDir() && library.lines() === 0)
            onboardingOpen = true
    }

    function reloadActiveTheme() {
        themeLoader.active = false
        HtpcController.reloadThemes()
        themeLoader.active = true
    }

    function openDetails(line) {
        if (line < 0 || line >= library.lines())
            return
        selectedLine = line
        selectedDetails = library.details(line)
        detailsOpen = true
        HtpcController.muteNavigation()
        detailsOverlay.reset()
    }

    function closeDetails() {
        detailsOpen = false
        HtpcController.muteNavigation()
    }

    function openFolderPicker() {
        folderBrowser.open(HtpcController.romsDir())
        folderPicker.reset()
        folderPickerOpen = true
        HtpcController.muteNavigation()
    }

    function closeFolderPicker(chosenPath) {
        folderPickerOpen = false
        HtpcController.muteNavigation()
        if (chosenPath && chosenPath !== "") {
            HtpcController.setRomsDir(chosenPath)
            library.searchAgain()
            onboardingOpen = false
            showToast(chosenPath, false)
        }
    }

    function dispatchCommand(code) {
        if (folderPickerOpen) {
            folderPicker.handleCommand(code)
            return
        }
        if (playing) {
            if (code === 12) {
                if (quickMenuOpen)
                    closeQuickMenu()
                else
                    openQuickMenu()
                return
            }
            if (quickMenuOpen) {
                quickMenu.handleCommand(code)
            }
            return
        }
        if (settingsOpen) {
            settingsOverlay.handleCommand(code)
            return
        }
        if (detailsOpen) {
            detailsOverlay.handleCommand(code)
            return
        }
        if (onboardingOpen) {
            if (code === 4)
                openFolderPicker()
            else if (code === 10)
                openSettings()
            return
        }
        // Global shortcuts from the library showcase
        if (code === 10) {
            openSettings()
            return
        }
        if (themeLoader.item && typeof themeLoader.item.handleCommand === "function") {
            themeLoader.item.handleCommand(code)
        }
    }

    Component.onCompleted: {
        if (initialGame !== "") {
            launcherMode = true
            Qt.callLater(() => {
                const ok = presentOpenError(library.openPath(initialGame))
                if (!ok)
                    launcherMode = false
            })
        } else if (!HtpcController.hasRomsDir() && library.lines() === 0) {
            onboardingOpen = true
        }
    }

    onActiveChanged: {
        if (!active && playing)
            screen.release()
    }

    onClosing: (close) => {
        if (playing)
            screen.close()
        Qt.quit()
    }

    Library {
        id: library
    }

    SettingsModel {
        id: coreSettings
    }

    FolderBrowser {
        id: folderBrowser
    }

    // -------------------------------------------------------------------------
    // Gamepad polling loop (60 Hz) — drives HTPC navigation & Quick Menu combo
    // -------------------------------------------------------------------------
    Timer {
        interval: 16
        repeat: true
        running: root.visible
        onTriggered: {
            const cmds = HtpcController.pollCommands(root.playing, root.quickMenuOpen)
            for (let i = 0; i < cmds.length; i++) {
                root.dispatchCommand(cmds[i])
            }
        }
    }

    Timer {
        id: toastTimer
        interval: 2600
        onTriggered: root.toastText = ""
    }

    // -------------------------------------------------------------------------
    // Keyboard handler (for HTPCs with keyboards / dev testing / Steam bindings)
    // -------------------------------------------------------------------------
    Item {
        id: keyCatcher
        anchors.fill: parent
        focus: true

        Keys.onPressed: (event) => {
            // In-game keys go directly to the emulator, except Escape/F1/M for Quick Menu
            // and F11 for fullscreen toggle.
            if (root.playing && !root.quickMenuOpen) {
                if (event.key === Qt.Key_F11 && !event.isAutoRepeat) {
                    root.visibility = root.visibility === Window.FullScreen ? Window.Windowed : Window.FullScreen
                    event.accepted = true
                    return
                }
                if ((event.key === Qt.Key_Escape || event.key === Qt.Key_F1) && !event.isAutoRepeat) {
                    root.openQuickMenu()
                    event.accepted = true
                    return
                }
                if (screen.isScreenshotShortcut(event.key) && !event.isAutoRepeat) {
                    screen.screenshot(Qt.formatDateTime(new Date(), "yyyy-MM-dd_HH-mm-ss"))
                    event.accepted = true
                    return
                }
                if (screen.isStateShortcut(event.key, event.modifiers) && !event.isAutoRepeat) {
                    screen.stateShortcut(event.key, event.modifiers)
                    event.accepted = true
                    return
                }
                if (!event.isAutoRepeat)
                    screen.key(event.key, true)
                event.accepted = true
                return
            }

            if (event.key === Qt.Key_F11 && !event.isAutoRepeat) {
                root.visibility = root.visibility === Window.FullScreen ? Window.Windowed : Window.FullScreen
                event.accepted = true
                return
            }

            // Map keyboard keys to the same HTPC command codes as gamepads.
            let code = -1
            switch (event.key) {
            case Qt.Key_Up: code = 0; break
            case Qt.Key_Down: code = 1; break
            case Qt.Key_Left: code = 2; break
            case Qt.Key_Right: code = 3; break
            case Qt.Key_Return:
            case Qt.Key_Enter:
            case Qt.Key_Space:
                if (!event.isAutoRepeat) code = 4
                break
            case Qt.Key_Escape:
            case Qt.Key_Backspace:
                if (!event.isAutoRepeat) code = 5
                break
            case Qt.Key_X:
                if (!event.isAutoRepeat) code = 6
                break
            case Qt.Key_Y:
            case Qt.Key_C:
                if (!event.isAutoRepeat) code = 7
                break
            case Qt.Key_Q:
            case Qt.Key_PageUp:
                if (!event.isAutoRepeat) code = 8
                break
            case Qt.Key_E:
            case Qt.Key_PageDown:
                if (!event.isAutoRepeat) code = 9
                break
            case Qt.Key_S:
            case Qt.Key_Menu:
                if (!event.isAutoRepeat) code = 10
                break
            case Qt.Key_Tab:
                if (!event.isAutoRepeat) code = 11
                break
            case Qt.Key_M:
            case Qt.Key_F1:
                if (!event.isAutoRepeat) code = 12
                break
            }
            if (code >= 0) {
                root.dispatchCommand(code)
                event.accepted = true
            }
        }

        Keys.onReleased: (event) => {
            if (root.playing && !root.quickMenuOpen && !event.isAutoRepeat) {
                screen.key(event.key, false)
                event.accepted = true
            }
        }
    }

    // -------------------------------------------------------------------------
    // Layer 1: Pluggable HTPC Theme Showcase (Z-Wheel HD by default, or community)
    // -------------------------------------------------------------------------
    Item {
        id: safeFrame
        anchors.fill: parent
        anchors.margins: root.safeMargin()
        visible: !root.playing

        Loader {
            id: themeLoader
            anchors.fill: parent
            source: HtpcController.themeUrl

            onLoaded: {
                if (!item)
                    return
                if (item.hasOwnProperty("library"))
                    item.library = library
                if (item.hasOwnProperty("htpc"))
                    item.htpc = HtpcController
                if (item.hasOwnProperty("host"))
                    item.host = root
            }

            onStatusChanged: {
                if (status === Loader.Error) {
                    const failed = HtpcController.themeName
                    console.warn("HTPC theme failed to load:", source)
                    if (HtpcController.themeId !== "zwheel-hd") {
                        HtpcController.fallbackToDefault(failed)
                    }
                }
            }
        }

        Connections {
            target: HtpcController
            function onFallbackNoticeChanged() {
                if (HtpcController.fallbackNotice !== "")
                    root.showToast(HtpcController.fallbackNotice, true)
            }
        }
    }

    // -------------------------------------------------------------------------
    // Layer 2: Fullscreen Zero-Copy Game Screen
    // -------------------------------------------------------------------------
    Item {
        id: gameLayer
        anchors.fill: parent
        visible: root.playing

        GameScreen {
            id: screen
            anchors.fill: parent

            onClosed: {
                root.quickMenuOpen = false
                root.playing = false
                HtpcController.muteNavigation()
                if (root.launcherMode)
                    Qt.quit()
            }

            onScreenshotSerialChanged: {
                if (screenshotText !== "")
                    root.showToast(screenshotText, screenshotFailed)
            }

            onStateSerialChanged: {
                if (stateText !== "")
                    root.showToast(stateText, stateFailed)
            }
        }

        FrameAnimation {
            running: root.playing && !root.quickMenuOpen
            onTriggered: screen.step()
        }

        // Optional corner metrics (shares `debug.metrics` setting with zeebx-qt)
        Rectangle {
            visible: screen.metrics.length > 0 && !root.quickMenuOpen
            anchors.top: parent.top
            anchors.right: parent.right
            anchors.margins: root.safeMargin() + 14
            radius: 8
            color: "#b8000000"
            width: metricsCol.implicitWidth + 20
            height: metricsCol.implicitHeight + 14

            Column {
                id: metricsCol
                anchors.centerIn: parent
                spacing: 2
                Repeater {
                    model: screen.metrics
                    Text {
                        text: modelData
                        color: "#e6f5ec"
                        font.pixelSize: 13
                        font.family: "monospace"
                    }
                }
            }
        }

        // Fast-forward / rewind / turbo indicator
        Rectangle {
            visible: screen.indicators !== "" && !root.quickMenuOpen
            anchors.top: parent.top
            anchors.left: parent.left
            anchors.margins: root.safeMargin() + 14
            radius: 8
            color: "#b8000000"
            width: indText.implicitWidth + 20
            height: indText.implicitHeight + 12

            Text {
                id: indText
                anchors.centerIn: parent
                text: screen.indicators
                color: HtpcController.accentColor
                font.pixelSize: 14
                font.bold: true
            }
        }

        // Stopped / fatal error banner
        Rectangle {
            visible: screen.stopped !== ""
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottom: parent.bottom
            anchors.bottomMargin: root.safeMargin() + 28
            radius: 10
            color: "#e01a1212"
            border.width: 2
            border.color: HtpcController.errorColor
            width: Math.min(parent.width - 64, stopText.implicitWidth + 36)
            height: stopText.implicitHeight + 24

            Text {
                id: stopText
                anchors.centerIn: parent
                width: parent.width - 28
                wrapMode: Text.Wrap
                horizontalAlignment: Text.AlignHCenter
                text: screen.stopped
                color: "#ffffff"
                font.pixelSize: 15
            }
        }
    }

    // -------------------------------------------------------------------------
    // Layer 3: Onboarding (first launch with no ROMs folder configured)
    // -------------------------------------------------------------------------
    Rectangle {
        visible: root.onboardingOpen && !root.playing && !root.settingsOpen && !root.folderPickerOpen
        anchors.fill: parent
        color: "#cc000000"
        z: 30

        Rectangle {
            anchors.centerIn: parent
            width: Math.min(parent.width - 80, 620)
            height: onboardCol.implicitHeight + 64
            radius: 18
            color: HtpcController.surfaceSolidColor
            border.width: 2
            border.color: HtpcController.accentColor

            ColumnLayout {
                id: onboardCol
                anchors.fill: parent
                anchors.margins: 32
                spacing: 18

                Text {
                    text: root.tr("htpc.onboarding.title")
                    color: HtpcController.textColor
                    font.pixelSize: 28
                    font.bold: true
                    Layout.fillWidth: true
                }

                Text {
                    text: root.tr("htpc.onboarding.body")
                    color: HtpcController.mutedColor
                    font.pixelSize: 16
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }

                RowLayout {
                    spacing: 14
                    Layout.topMargin: 8

                    Rectangle {
                        radius: 12
                        color: HtpcController.accentColor
                        Layout.preferredHeight: 48
                        Layout.preferredWidth: chooseLbl.implicitWidth + 44

                        Text {
                            id: chooseLbl
                            anchors.centerIn: parent
                            text: HtpcController.promptOk + "  ·  " + root.tr("htpc.onboarding.choose")
                            color: HtpcController.accentTextColor
                            font.pixelSize: 15
                            font.bold: true
                        }

                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.openFolderPicker()
                        }
                    }

                    Rectangle {
                        radius: 12
                        color: "transparent"
                        border.width: 1
                        border.color: HtpcController.borderColor
                        Layout.preferredHeight: 48
                        Layout.preferredWidth: openSetLbl.implicitWidth + 36

                        Text {
                            id: openSetLbl
                            anchors.centerIn: parent
                            text: HtpcController.promptStart + "  ·  " + root.tr("htpc.prompt.settings")
                            color: HtpcController.textColor
                            font.pixelSize: 14
                            font.bold: true
                        }

                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.openSettings()
                        }
                    }
                }
            }
        }
    }

    // -------------------------------------------------------------------------
    // Layer 4: Quick Menu, Settings, Game Details, Folder Picker
    // -------------------------------------------------------------------------
    QuickMenu {
        id: quickMenu
        anchors.fill: parent
        visible: root.playing && root.quickMenuOpen
        z: 40
        screen: screen
        host: root
    }

    HtpcSettings {
        id: settingsOverlay
        anchors.fill: parent
        anchors.margins: root.safeMargin()
        visible: root.settingsOpen && !root.playing
        z: 50
        coreSettings: coreSettings
        library: library
        host: root
    }

    GameDetails {
        id: detailsOverlay
        anchors.fill: parent
        visible: root.detailsOpen && !root.playing && !root.settingsOpen
        z: 55
        details: root.selectedDetails
        line: root.selectedLine
        host: root
    }

    FolderPicker {
        id: folderPicker
        anchors.fill: parent
        visible: root.folderPickerOpen
        z: 60
        browser: folderBrowser
        host: root
    }

    // -------------------------------------------------------------------------
    // Layer 5: Console-style Toast Banner
    // -------------------------------------------------------------------------
    Rectangle {
        visible: root.toastText !== ""
        z: 80
        anchors.top: parent.top
        anchors.topMargin: root.safeMargin() + 20
        anchors.horizontalCenter: parent.horizontalCenter
        radius: 22
        color: HtpcController.surfaceSolidColor
        border.width: 2
        border.color: root.toastError ? HtpcController.errorColor : HtpcController.accentColor
        width: Math.min(parent.width - 80, toastLabel.implicitWidth + 44)
        height: 44

        Text {
            id: toastLabel
            anchors.centerIn: parent
            width: parent.width - 28
            horizontalAlignment: Text.AlignHCenter
            elide: Text.ElideRight
            text: root.toastText
            color: HtpcController.textColor
            font.pixelSize: 15
            font.bold: true
        }
    }
}
