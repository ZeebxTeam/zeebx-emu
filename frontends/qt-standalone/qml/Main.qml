import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Window

import zeebx

// The main window: the library, as a grid or in the slider. The game, the settings, the saves and
// the log each open in their own window, as in egui; the startup and new-version notices are
// dialogs on top of the library. See `docs/implementation/21-qt-migration.md`.
// It is an `ApplicationWindow`, not a `Window`, so the background and loose texts use the same
// palette as the buttons and fields: with the window color taken from the system and the controls'
// from the style, the text came out dark on a dark background in a dark theme.
ApplicationWindow {
    id: main

    // Returns `value`, and makes the binding that calls this depend on the `versions`.
    //
    // **The versions go in as arguments, not as a loose expression.** QML is compiled ahead of
    // time (qmlcachegen), and the compiler drops a read whose value is not used:
    // `(cfg.version, cfg.device())` lost the version read, the binding stopped depending on it,
    // and switching the device from Boomerang to Z-Pad did not switch the screen. Interpreted, as
    // in qmltestrunner, it worked, which is why the tests did not catch it.
    function dependsOn(versions, value) {
        return value
    }

    // The game requested on the command line, if any: it opens directly, without going through the
    // list.
    required property string initialGame

    width: savedWidth > 0 ? savedWidth : 960
    height: savedHeight > 0 ? savedHeight : 720
    minimumWidth: 480
    minimumHeight: 360
    title: playingInMain ? "Zeebx — " + mainStage.screen.runState : "Zeebx"

    // The size the window was closed at is the size it opens at. Function calls have no change
    // signal: they evaluate once, at construction, which is when they matter.
    readonly property real savedWidth: library.windowWidth()
    readonly property real savedHeight: library.windowHeight()

    // And the reverse: drag it, the window saves. In full screen or maximized, the size is the
    // screen's, not the user's choice — so it is not saved.
    function saveSize() {
        if (visibility === Window.Windowed)
            library.saveWindowSize(width, height)
    }

    // Dragging the edge goes through dozens of sizes per second, and each one would write the
    // whole `settings.json`. Saving waits for the hand to stop.
    onWidthChanged: saveDebounce.restart()
    onHeightChanged: saveDebounce.restart()

    Timer {
        id: saveDebounce
        interval: 500
        onTriggered: main.saveSize()
    }

    // The window lost focus with the game in it: no key stays pressed.
    onActiveChanged: if (!active && playingInMain) mainStage.screen.release()

    // Closing the library ends everything, with or without a game open — as in egui. With a game
    // open, it asks first, where the game is. See `graphics.confirm_on_close`.
    property bool quitConfirmed: false
    onClosing: (close) => {
        if (playing && !quitConfirmed && stage.screen.confirmOnClose()) {
            close.accepted = false
            if (game.visible) {
                game.raise()
                game.requestActivate()
            }
            stage.requestQuit()
            return
        }
        saveDebounce.stop()
        main.saveSize()
        Qt.quit()
    }

    Component.onCompleted: {
        applyMode()
        if (warnings.hasStartupNotice())
            welcome.open()
        // After construction finishes, not during it: on the Qt 6.4 of Ubuntu 24.04, showing the
        // game window from here left it invisible — `visible` went back to false and the window
        // was never mapped. On 6.11 it worked. Opened from the library, already built, both work.
        if (initialGame !== "")
            Qt.callLater(() => main.present(library.openPath(initialGame)))
    }

    // The mode configured for the main window (`graphics.window`), as in egui. The settings call
    // this again when it changes: on the main window, it applies right away.
    function applyMode() {
        // With the game in it, the game's mode applies; the main one's comes back when it closes.
        if (playingInMain)
            return
        const mode = library.windowMode()
        if (mode === 2)
            showFullScreen()
        else if (mode === 1)
            showMaximized()
        else
            showNormal()
    }

    // The texts redo themselves when the language changes: the binding that calls this reads the
    // version.
    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    // An empty `error` means the game opened.
    //
    // **Where the game appears is only decided with no game running.** A game opened on top of
    // another (the library used with the separate window, or the Z-Wheel) stays where the previous
    // one was: with both stages active, the core would step twice per frame. The option applies to
    // the next game.
    function present(error) {
        warning.text = error
        if (error !== "")
            return
        if (game.visible)
            game.launch()
        else if (playingInMain || !settings.cfg.value("graphics.jogo_separado"))
            playInMain()
        else
            game.launch()
    }

    // The game in the main window, in place of the library. The window stays in the mode
    // configured for the game (`graphics.game_window`) while it runs, and goes back to what it was
    // when it closes.
    property bool playingInMain: false
    property int visibilityBeforeGame: Window.Windowed
    // The stage where the game is, for the menu, the toolbar and the status bar.
    readonly property var stage: playingInMain ? mainStage : game.stage
    readonly property bool playing: game.visible || playingInMain

    function playInMain() {
        if (!playingInMain) {
            visibilityBeforeGame = visibility
            playingInMain = true
            const mode = mainStage.screen.windowMode()
            if (mode === 2)
                showFullScreen()
            else if (mode === 1)
                showMaximized()
            else
                showNormal()
        }
        mainStage.prepare()
        mainStage.takeFocus()
        raise()
        requestActivate()
    }

    // Close in the menu and toolbar: the question appears where the game is, and the separate
    // window comes to the front to show it.
    function requestCloseGame() {
        if (game.visible) {
            game.raise()
            game.requestActivate()
        }
        stage.requestClose()
    }

    // Closes the game wherever it is, without asking: the one that asks is `stage.requestClose()`,
    // which calls this afterwards. On the main window, the library comes back, with the window as
    // it was.
    function closeGame() {
        if (game.visible) {
            game.shutdown()
            return
        }
        if (!playingInMain)
            return
        mainStage.close()
        playingInMain = false
        if (visibilityBeforeGame === Window.FullScreen)
            showFullScreen()
        else if (visibilityBeforeGame === Window.Maximized)
            showMaximized()
        else
            showNormal()
        main.view.forceActiveFocus()
    }

    // The game chosen now, by its list row.
    function chosen() {
        return inSlider ? slider.line(slider.cursor) : grid.currentIndex
    }

    // The row's game's own settings: the same settings window, only with the tabs that apply per
    // game. See `Core::with_game`.
    function configureGame(line) {
        const keyId = library.keyId(line)
        if (keyId !== "")
            gameSettings.openForGame(keyId, library.item(line).title)
    }

    // What is known about the row's game, in a dialog.
    function inspectGame(line) {
        if (library.lines() === 0)
            return
        details.line = line
        details.payload = library.details(line)
        details.open()
    }

    // The search or the scan rebuilt the list: the choice goes back to the first, without the
    // slider animation crossing the list.
    function reset() {
        grid.currentIndex = 0
        slider.restart()
    }

    // The library mode comes from the settings: 0 grid, 1 slider. Reread when they change.
    readonly property bool inSlider: main.dependsOn([settings.cfg.version], library.libraryMode() === 1)
    readonly property Item view: inSlider ? slider : grid
    // When the mode is changed in the settings, the arrows move to the new view.
    onViewChanged: view.forceActiveFocus()

    Library {
        id: library
    }

    // **The game is not a child of the main window.** A window declared inside another gets it as
    // `transientParent`, and the window manager keeps the child always above the parent: clicking
    // the main window during the game, to change something, did not bring it to the front. In egui
    // the two are independent as well.
    Game {
        id: game

        transientParent: null
    }
    Connections {
        target: game.stage
        function onQuitRequested() { main.quit() }
    }

    // Quit Zeebx after the question, or without it.
    function quit() {
        quitConfirmed = true
        close()
    }

    SettingsWindow {
        id: settings

        library: library
        main: main
        visible: false
    }

    // The settings of a single game. A separate window, not the one above in game mode: both can
    // be open, and each one edits its own.
    SettingsWindow {
        id: gameSettings

        library: library
        main: main
        visible: false
    }

    SavesWindow {
        id: saves

        visible: false
    }

    // The log follows the game, with the option on. It is turned on and off in the middle of the
    // game, through the settings, as in egui. It is a child of the game window, not of the main
    // one: it always stays on top of the game, not behind.
    LogWindow {
        id: logWindow

        transientParent: main.playingInMain ? main : game

        requested: main.dependsOn([settings.cfg.version, logWindow.dismissals],
                                  main.playing && logWindow.log.active())
    }

    Notices {
        id: warnings
    }

    // **With another window on top, the library listens to neither the controller nor the
    // keyboard** — not even if the main window gets focus back. When mapping the controller in the
    // settings, the pressed button would open a game by accident. The mouse still works, as in
    // egui. Every window that opens on top of the main one goes here.
    readonly property bool overlaid: main.playing || settings.visible || saves.visible
                                       || gameSettings.visible || details.visible
                                       || welcome.visible || newVersion.visible
                                       || about.visible || updateCheckResponse.visible
                                       || appMenuBar.opened

    // The controller generates no event in Qt, as it did not in egui: while the library is in
    // view, it is read here. Without focus, or with another window on top, the controller is not
    // the library's — and while typing in the search it keeps navigating, as in egui. When it
    // starts listening again, what was already pressed does not count: navigation is muted while
    // it is not listening.
    Timer {
        interval: 33
        repeat: true
        running: true
        onTriggered: {
            // The answer of the new-version check is read through `commands`: its notice comes
            // after the startup one, never together.
            if (!welcome.visible && !newVersion.visible) {
                const text = warnings.updateNoticeText()
                if (text !== "") {
                    newVersion.text = text
                    newVersion.page = warnings.updatePage()
                    newVersion.open()
                }
            }
            // The check from the Help menu answers even with no new version.
            if (!updateCheckResponse.visible) {
                const response = warnings.updateCheckResult()
                if (response !== "") {
                    updateCheckResponse.text = response
                    updateCheckResponse.open()
                }
            }
            const listening = main.active && !main.overlaid
            for (const code of library.commands(listening)) {
                if (code === 5)
                    main.present(library.openZWheel())
                else if (code === 6)
                    main.configureGame(main.chosen())
                else if (code === 7)
                    main.inspectGame(main.chosen())
                else
                    main.view.command(code)
            }
        }
    }

    // F11, or Alt+Enter, puts the library in and out of full screen, as in egui. Alt+Enter only
    // works with Alt: Enter alone is a controller button on the keyboard.
    Shortcut {
        sequences: ["F11", "Alt+Return", "Alt+Enter"]
        onActivated: main.visibility = main.visibility === Window.FullScreen
                     ? Window.Windowed : Window.FullScreen
    }

    // Ctrl+F goes to the search, from anywhere in the window. In `sequences`, not `sequence`:
    // where the system gives more than one key to "find" (Ctrl+F and F3), Qt 6.11 binds only the
    // first one and warns in the terminal.
    Shortcut {
        sequences: [StandardKey.Find]
        enabled: !main.playingInMain
        onActivated: search.forceActiveFocus()
    }

    // Show the toolbar and the status bar: the View menu turns them on and off, and it is saved.
    readonly property bool withToolbar: dependsOn([settings.cfg.version], settings.cfg.value("barras.ferramentas"))
    readonly property bool withStatusBar: dependsOn([settings.cfg.version], settings.cfg.value("barras.status"))
    // Too narrow for the labels, the toolbar keeps only the icons, and the name goes to the
    // tooltip. The width is what the Portuguese labels need, the longest of the four.
    readonly property bool compactBar: width < 1200

    // What the System menu shows of the slots, read when it opens: each read touches the disk.
    property var menuSlots: []
    property var menuOccupiedSlots: []
    property int menuSlot: 0

    function shortcut(name) {
        return dependsOn([settings.cfg.version], settings.cfg.shortcut(name))
    }

    // The full screen of the toolbar and menu: the game's, if it is open, and the library's, if
    // not.
    function toggleFullScreen() {
        if (game.visible)
            game.toggleFullScreen()
        else
            main.visibility = main.visibility === Window.FullScreen ? Window.Windowed : Window.FullScreen
    }

    // A menu item with the icon in front and the shortcut key on the right. **The key is just
    // text**: a real `shortcut` would apply to the whole window, and the P typed in the search
    // would pause the game. The game window handles the keys, as always.
    component AppMenuItem: MenuItem {
        id: appMenuItem

        property string iconName: ""
        property string shortcut: ""

        // The checkable item's indicator takes the front: the row starts after it.
        readonly property real indent: checkable && indicator ? indicator.width + spacing : 0

        contentItem: Item {
            implicitWidth: line.implicitWidth + appMenuItem.indent
            implicitHeight: line.implicitHeight

            RowLayout {
                id: line

                anchors.fill: parent
                anchors.leftMargin: appMenuItem.indent
                spacing: 8
                opacity: appMenuItem.enabled ? 1 : 0.45

                // Empty, the icon still takes its place: the menu texts stay aligned.
                Icon {
                    Layout.preferredWidth: 16
                    visible: appMenuItem.iconName !== "" || !appMenuItem.checkable
                    name: appMenuItem.iconName
                    size: 15
                    color: appMenuItem.highlighted ? appMenuItem.palette.highlightedText : appMenuItem.palette.windowText
                }
                Label {
                    Layout.fillWidth: true
                    text: appMenuItem.text
                    color: appMenuItem.highlighted ? appMenuItem.palette.highlightedText : appMenuItem.palette.windowText
                }
                Label {
                    Layout.leftMargin: 24
                    visible: appMenuItem.shortcut !== ""
                    opacity: 0.65
                    text: appMenuItem.shortcut
                    color: appMenuItem.highlighted ? appMenuItem.palette.highlightedText : appMenuItem.palette.windowText
                }
            }
        }
    }

    // A slot in the System menu: `actionId` 0 saves and 1 loads. The current slot shows the key.
    component SlotMenuItem: AppMenuItem {
        required property int slot
        property int actionId: 0

        text: main.menuSlots[slot] !== undefined ? main.menuSlots[slot] : ""
        enabled: actionId === 0 || main.menuOccupiedSlots[slot] === 1
        shortcut: slot === main.menuSlot
                ? main.shortcut(actionId === 0 ? "salvar_estado" : "carregar_estado") : ""
        onTriggered: main.stage.screen.stateFromMenu(actionId, slot)
    }

    // A toolbar button: the icon, and the name beside it while it fits. **It does not take
    // keyboard focus**: when clicked, a Qt Quick button kept it, and the arrows stopped going to
    // the grid or the slider. In egui a click did not grab the keyboard.
    component ToolbarButton: ToolButton {
        id: button

        property string iconName
        property string hint: ""

        focusPolicy: Qt.NoFocus
        contentItem: RowLayout {
            spacing: 6
            opacity: button.enabled ? 1 : 0.4

            Icon {
                Layout.preferredWidth: 20
                name: button.iconName
                size: 18
                color: button.palette.buttonText
            }
            Label {
                visible: !main.compactBar
                text: button.text
                color: button.palette.buttonText
            }
        }
        ToolTip.visible: hovered && (main.compactBar || hint !== "")
        ToolTip.delay: 500
        ToolTip.text: hint !== "" ? hint : text
    }

    component Separator: Rectangle {
        Layout.preferredWidth: 1
        Layout.preferredHeight: 22
        Layout.leftMargin: 4
        Layout.rightMargin: 4
        color: palette.mid
    }

    // With the game in the main window in full screen, the three bars go away, as in PCSX2: the
    // screen belongs to the game.
    readonly property bool gameOnly: playingInMain && visibility === Window.FullScreen

    menuBar: MenuBar {
        id: appMenuBar

        visible: !main.gameOnly

        // With a menu open, the controller does not navigate the library underneath it.
        readonly property bool opened: fileMenu.visible || systemMenu.visible
                                       || settingsMenu.visible || viewMenu.visible
                                       || toolsMenu.visible || helpMenu.visible

        Menu {
            id: fileMenu

            title: tr("menu.file")

            // The Z-Wheel and the update check belong to the library: with the game in its place,
            // they are disabled. Opening the Z-Wheel would swap the game for it without any
            // question.
            AppMenuItem {
                iconName: "house"
                text: tr("menu.file.z_wheel")
                enabled: !main.playingInMain
                         && main.dependsOn([settings.cfg.version], library.hasZWheel())
                onTriggered: main.present(library.openZWheel())
            }
            AppMenuItem {
                iconName: "refresh-cw"
                text: tr("menu.file.rescan")
                enabled: !main.playingInMain
                onTriggered: {
                    library.searchAgain()
                    main.reset()
                }
            }
            MenuSeparator {}
            AppMenuItem {
                iconName: "log-out"
                text: tr("menu.file.quit")
                onTriggered: main.close()
            }
        }

        // What is done with the game open. With no game, everything is disabled. Rewind is not
        // here: going back only makes sense while holding the key, and a menu item cannot be held.
        Menu {
            id: systemMenu

            title: tr("menu.system")
            onAboutToShow: {
                main.menuSlots = main.playing ? main.stage.screen.stateSlots() : []
                main.menuOccupiedSlots = main.playing ? main.stage.screen.occupiedSlots() : []
                main.menuSlot = main.playing ? main.stage.screen.currentSlot() : 0
                fastForwardItem.checked = main.playing && main.stage.screen.fastForwardLocked()
                undoItem.enabled = main.playing && main.stage.screen.canUndoLoad()
            }

            AppMenuItem {
                iconName: main.stage.paused ? "play" : "pause"
                text: tr(main.stage.paused ? "menu.system.resume" : "menu.system.pause")
                shortcut: "P"
                enabled: main.playing
                onTriggered: main.stage.pause()
            }
            AppMenuItem {
                iconName: "rotate-ccw"
                text: tr("menu.system.restart")
                shortcut: main.shortcut("reiniciar")
                enabled: main.playing
                onTriggered: main.stage.restart()
            }
            MenuSeparator {}
            AppMenuItem {
                id: fastForwardItem

                checkable: true
                text: tr("menu.system.fast_forward")
                shortcut: main.shortcut("avancar")
                enabled: main.playing
                onTriggered: checked = main.stage.screen.toggleFastForward()
            }
            AppMenuItem {
                iconName: "camera"
                text: tr("menu.system.screenshot")
                shortcut: main.shortcut("screenshot")
                enabled: main.playing
                onTriggered: main.stage.screenshot()
            }
            MenuSeparator {}
            Menu {
                title: tr("menu.system.save_state")
                // Wide enough for an occupied slot: the Menu measures its items the first time it
                // opens, and does not grow when an empty slot becomes occupied.
                width: Math.max(implicitWidth, 340)
                enabled: main.playing

                SlotMenuItem { slot: 0; iconName: "save" }
                SlotMenuItem { slot: 1; iconName: "save" }
                SlotMenuItem { slot: 2; iconName: "save" }
                SlotMenuItem { slot: 3; iconName: "save" }
                SlotMenuItem { slot: 4; iconName: "save" }
                SlotMenuItem { slot: 5; iconName: "save" }
                SlotMenuItem { slot: 6; iconName: "save" }
                SlotMenuItem { slot: 7; iconName: "save" }
                SlotMenuItem { slot: 8; iconName: "save" }
                SlotMenuItem { slot: 9; iconName: "save" }
            }
            Menu {
                title: tr("menu.system.load_state")
                // Wide enough for an occupied slot: the Menu measures its items the first time it
                // opens, and does not grow when an empty slot becomes occupied.
                width: Math.max(implicitWidth, 340)
                enabled: main.playing

                SlotMenuItem { slot: 0; actionId: 1; iconName: "folder-open" }
                SlotMenuItem { slot: 1; actionId: 1; iconName: "folder-open" }
                SlotMenuItem { slot: 2; actionId: 1; iconName: "folder-open" }
                SlotMenuItem { slot: 3; actionId: 1; iconName: "folder-open" }
                SlotMenuItem { slot: 4; actionId: 1; iconName: "folder-open" }
                SlotMenuItem { slot: 5; actionId: 1; iconName: "folder-open" }
                SlotMenuItem { slot: 6; actionId: 1; iconName: "folder-open" }
                SlotMenuItem { slot: 7; actionId: 1; iconName: "folder-open" }
                SlotMenuItem { slot: 8; actionId: 1; iconName: "folder-open" }
                SlotMenuItem { slot: 9; actionId: 1; iconName: "folder-open" }
            }
            AppMenuItem {
                id: undoItem

                iconName: "undo-2"
                text: tr("menu.system.undo_load")
                shortcut: main.shortcut("desfazer_carregar")
                onTriggered: main.stage.screen.stateFromMenu(2, -1)
            }
            MenuSeparator {}
            AppMenuItem {
                iconName: "square"
                text: tr("menu.system.close")
                shortcut: "Esc"
                enabled: main.playing
                onTriggered: main.requestCloseGame()
            }
        }

        // Each category opens the settings window already on it, in tab order.
        Menu {
            id: settingsMenu

            title: tr("menu.settings")

            AppMenuItem { iconName: "settings"; text: tr("settings.tab.general"); onTriggered: settings.open(0) }
            AppMenuItem { iconName: "gamepad-2"; text: tr("settings.tab.controls"); onTriggered: settings.open(1) }
            AppMenuItem { iconName: "keyboard"; text: tr("settings.tab.shortcuts"); onTriggered: settings.open(2) }
            AppMenuItem { iconName: "monitor"; text: tr("settings.tab.graphics"); onTriggered: settings.open(3) }
            AppMenuItem { iconName: "gauge"; text: tr("settings.tab.speed"); onTriggered: settings.open(4) }
            AppMenuItem { iconName: "volume-2"; text: tr("settings.tab.audio"); onTriggered: settings.open(5) }
            AppMenuItem {
                iconName: "trophy"
                text: tr("settings.tab.achievements")
                visible: Achievements.available()
                height: visible ? implicitHeight : 0
                onTriggered: settings.open(6)
            }
            AppMenuItem { iconName: "bug"; text: tr("settings.tab.debug"); onTriggered: settings.open(7) }
            MenuSeparator {}
            AppMenuItem {
                text: tr("menu.settings.game")
                shortcut: "X"
                enabled: library.empty === ""
                onTriggered: main.configureGame(main.chosen())
            }
        }

        // The checkable items are reread when the menu opens, not bound: a click on a checkable
        // item breaks its binding, and a change made in the settings would not show up here.
        Menu {
            id: viewMenu

            title: tr("menu.view")
            onAboutToShow: {
                gridItem.checked = !main.inSlider
                sliderItem.checked = main.inSlider
                toolbarItem.checked = main.withToolbar
                statusItem.checked = main.withStatusBar
            }

            AppMenuItem {
                id: gridItem

                checkable: true
                iconName: "layout-grid"
                text: tr("settings.library_view.grid")
                onTriggered: settings.cfg.define("biblioteca", 0)
            }
            AppMenuItem {
                id: sliderItem

                checkable: true
                iconName: "gallery-horizontal"
                text: tr("settings.library_view.slider")
                onTriggered: settings.cfg.define("biblioteca", 1)
            }
            MenuSeparator {}
            AppMenuItem {
                id: toolbarItem

                checkable: true
                text: tr("menu.view.toolbar")
                onTriggered: settings.cfg.define("barras.ferramentas", checked)
            }
            AppMenuItem {
                id: statusItem

                checkable: true
                text: tr("menu.view.statusbar")
                onTriggered: settings.cfg.define("barras.status", checked)
            }
            MenuSeparator {}
            AppMenuItem {
                iconName: "maximize"
                text: tr("controls.shortcut.fullscreen")
                shortcut: "F11"
                onTriggered: main.toggleFullScreen()
            }
        }

        Menu {
            id: toolsMenu

            title: tr("menu.tools")
            onAboutToShow: logItem.checked = settings.cfg.value("debug.log")

            AppMenuItem {
                iconName: "hard-drive"
                text: tr("menu.tools.saves")
                onTriggered: saves.open()
            }
            AppMenuItem {
                id: logItem

                checkable: true
                text: tr("debug.log")
                onTriggered: settings.cfg.define("debug.log", checked)
            }
            MenuSeparator {}
            // Zeeboids only allows syncing once a day, and the lock is its own: it stores the date
            // in its own database. Testing the network with that costs a day per attempt, so the
            // item moves the date back by one day.
            AppMenuItem {
                iconName: "calendar-clock"
                text: tr("menu.tools.unlock_sync")
                onTriggered: message.text = library.unlockSync()
            }
        }

        Menu {
            id: helpMenu

            title: tr("menu.help")

            AppMenuItem {
                iconName: "external-link"
                text: tr("about.repository")
                onTriggered: Qt.openUrlExternally(settings.cfg.address("repositorio"))
            }
            AppMenuItem {
                iconName: "message-circle"
                text: tr("about.discord")
                onTriggered: Qt.openUrlExternally(settings.cfg.address("discord"))
            }
            MenuSeparator {}
            AppMenuItem {
                iconName: "download"
                text: tr("menu.help.check_update")
                onTriggered: warnings.checkForUpdates()
            }
            AppMenuItem {
                iconName: "info"
                text: tr("menu.help.about")
                onTriggered: about.open()
            }
        }
    }

    // The toolbar: what is done in the library, what is done with the game open, the settings,
    // and on the right the view and the search, as in PCSX2.
    header: ToolBar {
        visible: main.withToolbar && !main.gameOnly

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 4
            anchors.rightMargin: 6
            spacing: 2

            ToolbarButton {
                id: zWheelButton

                iconName: "house"
                text: tr("nav.z_wheel")
                hint: tr(main.playingInMain ? "nav.z_wheel.playing"
                         : enabled ? "nav.z_wheel.hint" : "nav.z_wheel.missing")
                // As in the File menu: with the game in the main window, the Z-Wheel and the check
                // wait.
                enabled: !main.playingInMain
                         && main.dependsOn([settings.cfg.version], library.hasZWheel())
                onClicked: main.present(library.openZWheel())
            }
            ToolbarButton {
                iconName: "refresh-cw"
                text: tr("library.rescan")
                enabled: !main.playingInMain
                onClicked: {
                    library.searchAgain()
                    main.reset()
                }
            }
            Separator {}
            ToolbarButton {
                iconName: main.stage.paused ? "play" : "pause"
                text: tr(main.stage.paused ? "menu.system.resume" : "menu.system.pause")
                enabled: main.playing
                onClicked: main.stage.pause()
            }
            ToolbarButton {
                iconName: "rotate-ccw"
                text: tr("menu.system.restart")
                enabled: main.playing
                onClicked: main.stage.restart()
            }
            ToolbarButton {
                iconName: "square"
                text: tr("toolbar.close")
                enabled: main.playing
                onClicked: main.requestCloseGame()
            }
            Separator {}
            ToolbarButton {
                iconName: "settings"
                text: tr("nav.settings")
                onClicked: settings.open(-1)
            }
            ToolbarButton {
                iconName: "gamepad-2"
                text: tr("settings.tab.controls")
                onClicked: settings.open(1)
            }
            Separator {}
            ToolbarButton {
                iconName: "maximize"
                text: tr("controls.shortcut.fullscreen")
                onClicked: main.toggleFullScreen()
            }

            Item {
                Layout.fillWidth: true
            }

            // The library view, like PCSX2's grid/list pair. **Not checkable**: clicking a
            // checkable button flips `checked` and breaks its binding to the setting, and a change
            // made through the menu or the settings would not show up here.
            ToolButton {
                focusPolicy: Qt.NoFocus
                visible: !main.playingInMain
                checked: !main.inSlider
                contentItem: Icon { name: "layout-grid"; size: 18; color: palette.buttonText }
                onClicked: settings.cfg.define("biblioteca", 0)
                ToolTip.visible: hovered
                ToolTip.delay: 500
                ToolTip.text: tr("settings.library_view.grid")
            }
            ToolButton {
                focusPolicy: Qt.NoFocus
                visible: !main.playingInMain
                checked: main.inSlider
                contentItem: Icon { name: "gallery-horizontal"; size: 18; color: palette.buttonText }
                onClicked: settings.cfg.define("biblioteca", 1)
                ToolTip.visible: hovered
                ToolTip.delay: 500
                ToolTip.text: tr("settings.library_view.slider")
            }

            // The search: filters the list by name, ignoring accents. Esc clears; Enter plays the
            // chosen one, which is the first result as long as nobody touches the choice.
            TextField {
                id: search
                visible: !main.playingInMain

                Layout.preferredWidth: 240
                Layout.leftMargin: 6
                leftPadding: 28
                rightPadding: clearSearch.visible ? 28 : 8
                placeholderText: tr("nav.search")
                onTextChanged: {
                    library.search(text)
                    main.reset()
                }
                Keys.onEscapePressed: {
                    text = ""
                    main.view.forceActiveFocus()
                }
                Keys.onReturnPressed: if (!main.overlaid) main.present(library.open(main.chosen()))
                Keys.onEnterPressed: if (!main.overlaid) main.present(library.open(main.chosen()))
                ToolTip.visible: hovered && !activeFocus
                ToolTip.delay: 500
                ToolTip.text: tr("nav.search.hint")

                Icon {
                    anchors.left: parent.left
                    anchors.leftMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    name: "search"
                    size: 14
                    color: search.palette.placeholderText
                }
                // Like the search's Esc: the list gets the arrows back.
                ToolButton {
                    id: clearSearch

                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    width: 26
                    height: 26
                    focusPolicy: Qt.NoFocus
                    visible: search.text !== ""
                    contentItem: Icon { name: "x"; size: 14; color: search.palette.text }
                    onClicked: {
                        search.text = ""
                        main.view.forceActiveFocus()
                    }
                    ToolTip.visible: hovered
                    ToolTip.text: tr("nav.search.clear")
                }
            }
        }
    }

    // The status bar: the count and what to press on the left; with a game open, on the right,
    // its name, where it draws, and the speed of the last sample, as in PCSX2.
    //
    // **It disappears while the game runs in the main window**, and the game extends down to the
    // bottom of the window: the count and the hint belong to the library, and the speed and where
    // the game draws are in the metrics box in the corner, which is turned on under Debug. With
    // the separate window the library stays in view, and so does the bar, with the game on the
    // right.
    footer: ToolBar {
        position: ToolBar.Footer
        visible: main.withStatusBar && !main.gameOnly && !main.playingInMain

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8
            anchors.rightMargin: 8
            spacing: 8

            Label {
                text: library.count
            }
            Label {
                Layout.fillWidth: true
                elide: Text.ElideRight
                opacity: 0.6
                text: tr("library.controls_hint") + " · " + tr("library.pad_game")
            }
            Label {
                visible: main.playing && text !== ""
                text: main.stage.screen.runState + (main.stage.screen.speed !== "" ? "  ·  " + main.stage.screen.speed : "")
            }
        }
    }

    // The game, when it runs in the main window: in place of the library, without its margins.
    GameStage {
        id: mainStage

        anchors.fill: parent
        visible: main.playingInMain
        active: main.playingInMain
        onCloseRequested: main.closeGame()
        onQuitRequested: main.quit()
        onFullScreenRequested: main.toggleFullScreen()
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 8
        spacing: 6
        visible: !main.playingInMain

        // The message from the last action that has something to say, such as unlocking the sync.
        RowLayout {
            Layout.fillWidth: true
            visible: message.text !== ""

            Label {
                id: message

                Layout.fillWidth: true
                wrapMode: Text.Wrap
            }
            Button {
                focusPolicy: Qt.NoFocus
                text: tr("nav.unlock_sync.ok")
                onClicked: message.text = ""
            }
        }

        // What went wrong in the last attempt to open a game.
        Label {
            id: warning

            Layout.fillWidth: true
            visible: text !== ""
            color: Theme.error
            wrapMode: Text.Wrap
        }

        // The empty list says why: no folder, a folder with no games, or a search with no results.
        Label {
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: library.empty !== ""
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignTop
            topPadding: 48
            wrapMode: Text.Wrap
            text: library.empty
        }

        LibraryGrid {
            id: grid

            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: !main.inSlider && library.empty === ""
            // **Focus belongs to whichever is in view.** Both declared `focus: true`, and Qt gives
            // focus to only one, the first: in slider mode, the arrows moved the hidden grid and
            // the slider stood still, measured by the grid's `currentIndex` going up with `cursor`
            // at zero.
            focus: !main.inSlider
            listening: !main.overlaid
            library: library
            onOpen: (line) => main.present(library.open(line))
            onConfigure: (line) => main.configureGame(line)
            onInspect: (line) => main.inspectGame(line)
        }

        LibrarySlider {
            id: slider

            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: main.inSlider && library.empty === ""
            focus: main.inSlider
            listening: !main.overlaid
            library: library
            onOpen: (line) => main.present(library.open(line))
            onConfigure: (line) => main.configureGame(line)
            onInspect: (line) => main.inspectGame(line)
        }
    }

    // A game's details: what the library knows about it, and the way to its settings.
    Dialog {
        id: details

        property int line: -1
        property var payload: ({})

        function field(name) {
            return payload[name] !== undefined ? payload[name] : ""
        }

        anchors.centerIn: parent
        width: Math.min(560, main.width - 32)
        modal: true
        title: main.tr("details.title").replace("{game}", field("title"))

        // **No layouts in here.** Text that wraps inside nested layouts makes the height depend on
        // the width and the width on the height: the dialog never finished laying itself out and
        // came out narrow, with the text spilling over. With the widths given, there is no loop.
        Item {
            width: parent.width
            implicitHeight: Math.max(detailsCover.height, detailsInfo.implicitHeight)

            Image {
                id: detailsCover

                width: 112
                height: 145
                fillMode: Image.PreserveAspectFit
                source: details.field("cover")
                smooth: !details.field("sharpCover")
            }

            Column {
                id: detailsInfo

                x: detailsCover.width + 16
                width: parent.width - x
                spacing: 8

                Repeater {
                    // The label above the value, not beside it: the value gets the full width.
                    // The path wraps anywhere, because it has no spaces to wrap at.
                    model: [
                        { label: "details.classid", value: details.field("classId") || "—" },
                        { label: "details.module", value: details.field("module") || "—" },
                        { label: "details.file", value: details.field("path") },
                        { label: "details.size", value: details.field("size") },
                        { label: "details.custom",
                          value: main.tr(details.field("custom") ? "details.custom.yes"
                                                                              : "details.custom.no") },
                    ]

                    Column {
                        required property var modelData

                        width: detailsInfo.width

                        Label {
                            opacity: 0.6
                            font.pixelSize: 11
                            text: main.tr(modelData.label)
                        }
                        Label {
                            width: parent.width
                            wrapMode: Text.WrapAnywhere
                            text: modelData.value
                        }
                    }
                }

                // The rating is a wide strip (age, theme and content, side by side). With no width
                // given, the image kept the file's and went past the dialog's edge, over the main
                // window. Now it fits the column, and the height follows the aspect ratio.
                Image {
                    width: parent.width
                    height: sourceSize.width > 0
                            ? Math.min(64, sourceSize.height, width * sourceSize.height / sourceSize.width)
                            : 0
                    fillMode: Image.PreserveAspectFit
                    horizontalAlignment: Image.AlignLeft
                    source: details.field("rating")
                    visible: source !== ""
                }
                Label {
                    width: parent.width
                    wrapMode: Text.Wrap
                    visible: text !== ""
                    text: details.field("description")
                }
            }
        }

        footer: DialogButtonBox {
            Button {
                text: main.tr("library.menu.configure")
                onClicked: {
                    details.close()
                    main.configureGame(details.line)
                }
            }
            Button {
                text: main.tr("details.close")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
        }
    }

    // Help > About Zeebx: what used to be the settings' About tab, in a dialog like PCSX2's.
    Dialog {
        id: about

        anchors.centerIn: parent
        width: Math.min(420, main.width - 32)
        modal: true
        title: main.tr("menu.help.about")

        ColumnLayout {
            width: parent.width
            spacing: 6

            Image {
                Layout.alignment: Qt.AlignHCenter
                Layout.preferredWidth: 96
                Layout.preferredHeight: 96
                fillMode: Image.PreserveAspectFit
                source: "qrc:/zeebx/zeebx.png"
                smooth: true
                mipmap: true
            }
            Label {
                Layout.alignment: Qt.AlignHCenter
                font.pixelSize: 22
                font.bold: true
                text: main.tr("app.name")
            }
            Label {
                Layout.alignment: Qt.AlignHCenter
                text: main.tr("about.version").replace("{version}", settings.cfg.emulatorVersion())
            }
            Label {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                opacity: 0.7
                text: main.tr("about.tagline")
            }
            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: 8

                Button {
                    text: main.tr("about.repository")
                    onClicked: Qt.openUrlExternally(settings.cfg.address("repositorio"))
                }
                Button {
                    text: main.tr("about.discord")
                    onClicked: Qt.openUrlExternally(settings.cfg.address("discord"))
                }
            }
        }

        footer: DialogButtonBox {
            Button {
                text: main.tr("details.close")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
        }
    }

    // The answer of the new-version check made from the menu, when it is not a new version: up to
    // date, or why it failed. A new version opens the notice below, the usual one.
    Dialog {
        id: updateCheckResponse

        property string text: ""

        anchors.centerIn: parent
        width: Math.min(420, main.width - 32)
        modal: true
        title: main.tr("menu.help.check_update")

        Label {
            width: parent.width
            wrapMode: Text.Wrap
            text: updateCheckResponse.text
        }

        footer: DialogButtonBox {
            Button {
                text: main.tr("details.close")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
        }
    }

    // The startup notice: the emulator is still in development, and the controller needs setting
    // up before playing. The checked box saves the version, and the next version shows the notice
    // again.
    Dialog {
        id: welcome

        anchors.centerIn: parent
        width: Math.min(460, main.width - 32)
        modal: true
        title: main.tr("welcome.title")

        ColumnLayout {
            width: parent.width
            spacing: 8

            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: main.tr("welcome.development")
            }
            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: main.tr("welcome.controls")
            }
            CheckBox {
                id: dontShowAgain

                Layout.topMargin: 4
                text: main.tr("welcome.dont_show")
            }
        }

        footer: DialogButtonBox {
            Button {
                text: main.tr("welcome.controls.open")
                onClicked: {
                    welcome.close()
                    settings.open(1)
                }
            }
            Button {
                text: main.tr("welcome.dismiss")
                DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            }
        }

        // Closed by any route (one of the buttons or Esc), the notice gets out of the way.
        onClosed: warnings.dismissStartup(dontShowAgain.checked)
    }

    // The new-version notice, if the check at startup found one. In the copies that replace
    // themselves (AppImage, Windows installer, `.app`) it is also where the installation proceeds:
    // the step is read on each tick of the timer above, while the dialog is open.
    Dialog {
        id: newVersion

        property string text: ""
        // Saved on opening: `onClosed` dismisses the notice, and after that the core no longer
        // says which page it is. If the dialog closed before `onClicked`, the button opened "".
        property string page: ""
        readonly property bool alone: warnings.selfUpdates()
        // 0 none, 1 downloading, 2 installing, 3 ready, 4 failed. See `updateStep`.
        property int step: 0
        property real fraction: -1
        property string progress: ""
        readonly property bool busy: step === 1 || step === 2

        function track() {
            step = warnings.updateStep()
            fraction = warnings.updateFraction()
            progress = warnings.updateText()
        }

        anchors.centerIn: parent
        width: Math.min(460, main.width - 32)
        modal: true
        title: main.tr("update.title")
        // In the middle of the download, clicking outside or pressing Esc does not close it: the
        // installation would go on with nobody watching, and the emulator would close by itself on
        // Windows.
        closePolicy: busy ? Popup.NoAutoClose : Popup.CloseOnEscape

        Timer {
            interval: 100
            repeat: true
            running: newVersion.visible
            onTriggered: newVersion.track()
        }

        ColumnLayout {
            width: parent.width
            spacing: 12

            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: newVersion.step === 0 ? newVersion.text : newVersion.progress
            }

            ProgressBar {
                Layout.fillWidth: true
                visible: newVersion.busy
                indeterminate: newVersion.step === 2 || newVersion.fraction < 0
                value: Math.max(0, newVersion.fraction)
            }

            RowLayout {
                Layout.alignment: Qt.AlignRight
                visible: !newVersion.busy

                Button {
                    visible: newVersion.alone && newVersion.step === 0
                    text: main.tr("update.install")
                    onClicked: {
                        warnings.installUpdate()
                        newVersion.track()
                    }
                }
                Button {
                    visible: newVersion.step === 3
                    text: main.tr("update.restart")
                    onClicked: warnings.restartIntoNewVersion()
                }
                Button {
                    // Without self-replacement, or if it failed, the way is the release page.
                    visible: newVersion.step === 0 || newVersion.step === 4
                    text: main.tr(newVersion.alone && newVersion.step === 0
                                       ? "update.release_page" : "update.download")
                    onClicked: {
                        Qt.openUrlExternally(newVersion.page)
                        if (!newVersion.alone || newVersion.step === 4)
                            newVersion.close()
                    }
                }
                Button {
                    text: main.tr("update.later")
                    onClicked: newVersion.close()
                }
            }
        }

        onOpened: track()
        onClosed: warnings.dismissUpdate()
    }
}
