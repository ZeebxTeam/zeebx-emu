import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import zeebx

// The settings window: a system window, not a panel inside the main one as in egui. The options
// are those of `settings.json`, read and written by key (see `src/qt/settings.rs`). Each write
// saves the file and takes effect right away.
ApplicationWindow {
    id: window

    // Returns `value`, and makes the binding that calls this depend on the `versions`.
    //
    // **The versions go in as arguments, not as a loose expression.** QML is compiled ahead of
    // time (qmlcachegen), and the compiler drops a read whose value is not used:
    // `(cfg.version, cfg.deviceKind())` lost the version read, the binding stopped depending on
    // it, and switching the device from Boomerang to Z-Pad did not switch the screen.
    // Interpreted, as in qmltestrunner, it worked, which is why the tests did not catch it.
    function dependsOn(versions, value) {
        return value
    }

    // The main window's library, to rebuild the list when the folder, the Z-Wheel or the language
    // change.
    required property var library
    // The main window, to apply its mode right away.
    required property var main

    // The settings object, so the main window rereads what depends on them.
    property alias cfg: cfg

    // **Game mode.** With `game` filled in, the window edits only that game's settings: the
    // controls, graphics, speed and audio tabs stay, and what belongs to the window goes away.
    // See `Core::with_game`.
    property string game: ""
    property string gameName: ""
    readonly property bool forGame: game !== ""
    readonly property bool custom: forGame && v("jogo.personalizado")

    width: 900
    height: 660
    minimumWidth: 600
    minimumHeight: 400
    title: "Zeebx — " + (forGame ? tr("game_settings.title").replace("{game}", gameName)
                                : tr("settings.title"))

    // The texts and values redo themselves: the language's `version` and the settings' are read
    // here, and a binding that calls these functions depends on them.
    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }
    function v(keyId) {
        return dependsOn([cfg.version], cfg.value(keyId))
    }
    function define(keyId, value) {
        cfg.define(keyId, value)
    }

    // Shows the window on the requested tab: 1 is the controls one, which the startup notice asks
    // to set up before playing; -1 stays on the current one.
    function open(tab) {
        if (tab >= 0)
            tabs.currentIndex = tab
        show()
        raise()
        requestActivate()
    }

    // A capture started: the key has to reach `keyboard`, the tab's FocusScope.
    //
    // **On the click, not only when the tab appears.** Focus used to be requested when showing
    // the tab, and the capture stayed on "Waiting" forever when focus was not there: the window
    // opened by the welcome notice, which on Wayland can be born without the keyboard, or a list
    // on this tab that kept focus after being used. The assign buttons do not take focus on
    // purpose (with it, the captured space would click the button again), so this is what gives
    // it back.
    function listenForKeys(keyboard) {
        window.requestActivate()
        keyboard.forceActiveFocus()
    }

    // Opens in game mode for `keyId`, on the controls tab.
    function openForGame(keyId, name) {
        game = keyId
        gameName = name
        cfg.editGame(keyId)
        open(1)
    }

    // The new-version check and Discord answer from another thread: their state is reread every
    // so often while the window is open.
    property int tick: 0
    Timer {
        interval: 250
        repeat: true
        running: window.visible
        onTriggered: window.tick += 1
    }

    SettingsModel {
        id: cfg

        onLanguageChanged: Language.reload()
        onThemeChanged: Theme.reload()
        onLibraryChanged: (scan) => {
            if (scan)
                window.library.searchAgain()
            else
                window.library.rebuild()
        }
        onMainWindowChanged: window.main.applyMode()
    }

    // **Each option's explanation lives in the footer**, as in PCSX2: that of the option under the
    // pointer, or with keyboard focus. Below each option, it stretched the pages until they no
    // longer fit the window. What changes with the state (the port turned off, the capture
    // waiting, no controller connected) stays next to the option: it is a warning, not an
    // explanation.
    //
    // A group is marked by a `SectionTitle`, with the `description` of what comes below it up to
    // the next title, and by an `Option`, with its `hint`. The page finds the nearest marker above
    // the pointer (`describe`). An option with no hint shows its group title's.
    property string description: ""

    function describe(page, y) {
        // By index, not with `for...of`: on Qt 6.4 an item's list of children is not iterable,
        // and the loop threw `TypeError`.
        // An `Explanation` applies from the child before it, which is what it explains.
        const above = []
        const children = page.children
        for (let i = 0; i < children.length; i++) {
            const child = children[i]
            if (!child.visible || child.descriptionMark === undefined)
                continue
            let start = child.y
            if (child.describesPrevious) {
                for (let j = i - 1; j >= 0; j--) {
                    if (children[j].visible) {
                        start = children[j].y
                        break
                    }
                }
            }
            if (start <= y)
                above.push({ y: start, mark: child })
        }
        above.sort((a, b) => b.y - a.y)
        for (const { mark } of above) {
            if (mark.descriptionMark !== "")
                return showDescription(mark.descriptionMark)
            if (mark.isTitle)
                break
        }
        showDescription(page.pageDescription)
    }
    function showDescription(text) {
        description = text
    }

    // Keyboard focus describes too: whoever moves through the options with Tab reads what each
    // one does.
    onActiveFocusItemChanged: {
        for (let item = activeFocusItem; item; item = item.parent) {
            if (item.isSettingsPage === true) {
                const point = activeFocusItem.mapToItem(item, 0, 0)
                describe(item, point.y)
                return
            }
        }
    }

    // A page's column: whatever the pointer hovers over in it becomes the description.
    component SettingsPage: ColumnLayout {
        id: page

        readonly property bool isSettingsPage: true
        // What the footer says above the first title: the whole page, or nothing.
        property string pageDescription: ""

        HoverHandler {
            onPointChanged: window.describe(page, point.position.y)
        }
    }

    // An on/off switch. Its explanation goes to the footer.
    component Option: ColumnLayout {
        id: option

        required property string keyId
        required property string label
        property string hint: ""
        readonly property string descriptionMark: hint !== "" ? window.tr(hint) : ""
        readonly property bool isTitle: false

        Layout.fillWidth: true
        spacing: 0

        CheckBox {
            text: window.tr(option.label)
            checked: window.v(option.keyId)
            onToggled: window.define(option.keyId, checked)
        }
    }

    // A list choice: the label on the left and the box.
    component Choice: RowLayout {
        id: choice

        required property string keyId
        required property string label

        Layout.fillWidth: true

        Label {
            text: window.tr(choice.label)
        }
        ComboBox {
            Layout.preferredWidth: 280
            model: window.dependsOn([Language.version], cfg.options(choice.keyId))
            currentIndex: window.v(choice.keyId)
            onActivated: (index) => window.define(choice.keyId, index)
        }
    }

    component Hint: Label {
        Layout.fillWidth: true
        wrapMode: Text.Wrap
        opacity: 0.6
    }

    // The explanation of what is right above it, for the footer: a choice box, a row of buttons.
    // It takes no space. The negative margin gives back the spacing the layout would add for it
    // being one more child.
    component Explanation: Item {
        property string text: ""
        readonly property string descriptionMark: text
        readonly property bool isTitle: false
        readonly property bool describesPrevious: true

        Layout.preferredHeight: 0
        Layout.topMargin: -(parent && parent.spacing ? parent.spacing : 0)
    }

    // The title of a group of options, with the group's explanation for the footer.
    component SectionTitle: Label {
        property string description: ""
        readonly property string descriptionMark: description
        readonly property bool isTitle: true

        Layout.topMargin: 12
        font.bold: true
    }

    header: ColumnLayout {
        spacing: 0

        // In game mode, the option that turns on its settings. When off, the tabs show the global
        // ones, without allowing changes: that is what the game is using.
        Pane {
            Layout.fillWidth: true
            visible: window.forGame

            ColumnLayout {
                width: parent.width
                spacing: 0

                CheckBox {
                    text: window.tr("game_settings.custom")
                    checked: window.custom
                    onToggled: window.define("jogo.personalizado", checked)
                }
                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: 32
                    wrapMode: Text.Wrap
                    opacity: 0.6
                    text: window.tr("game_settings.custom.hint")
                }
            }
        }
    }

    // The footer with the explanation of the option under the pointer. The height is fixed: one
    // that followed the text would make the window jump with each option hovered.
    footer: Pane {
        padding: 10

        Label {
            width: parent.width
            height: 64
            wrapMode: Text.Wrap
            elide: Text.ElideRight
            maximumLineCount: 4
            opacity: window.description !== "" ? 0.85 : 0.5
            text: window.description !== "" ? window.description : window.tr("settings.description.empty")
        }
    }

    RowLayout {
        anchors.fill: parent
        spacing: 0

        // The categories, like PCSX2's column. **Keeps `id: tabs` and `currentIndex`**: whoever
        // opens the window on a category (`open`) and the pages that read the controller while in
        // view ask for them. In game mode, the ones that do not apply per game disappear.
        ListView {
            id: tabs

            Layout.preferredWidth: 190
            Layout.fillHeight: true
            Layout.topMargin: 6
            interactive: false
            currentIndex: 0
            model: [
                { keyId: "settings.tab.general", iconName: "settings", general: true },
                { keyId: "settings.tab.controls", iconName: "gamepad-2", general: false },
                { keyId: "settings.tab.shortcuts", iconName: "keyboard", general: true },
                { keyId: "settings.tab.graphics", iconName: "monitor", general: false },
                { keyId: "settings.tab.speed", iconName: "gauge", general: false },
                { keyId: "settings.tab.audio", iconName: "volume-2", general: false },
                { keyId: "settings.tab.debug", iconName: "bug", general: true }
            ]

            delegate: ItemDelegate {
                id: category

                required property var modelData
                required property int index

                width: ListView.view.width
                visible: !modelData.general || !window.forGame
                height: visible ? implicitHeight : 0
                // Like the buttons in the other windows: the click does not take keyboard focus,
                // which the key capture on the controls and shortcuts pages needs.
                focusPolicy: Qt.NoFocus
                highlighted: ListView.isCurrentItem
                onClicked: tabs.currentIndex = index

                contentItem: RowLayout {
                    spacing: 10

                    Icon {
                        Layout.preferredWidth: 20
                        name: category.modelData.iconName
                        size: 18
                        color: category.highlighted ? category.palette.highlightedText : category.palette.windowText
                    }
                    Label {
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                        text: window.tr(category.modelData.keyId)
                        color: category.highlighted ? category.palette.highlightedText : category.palette.windowText
                    }
                }
            }
        }

        Rectangle {
            Layout.preferredWidth: 1
            Layout.fillHeight: true
            color: palette.mid
        }

        // Every page scrolls: the graphics one no longer fits the window's default height, and an
        // option that disappears below the edge is an option that does not exist.
        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: tabs.currentIndex
            enabled: !window.forGame || window.custom
            // The page changed: the previous one's description no longer applies.
            onCurrentIndexChanged: window.showDescription("")

            // General
            ScrollView {
                contentWidth: availableWidth

                SettingsPage {
                    width: parent.width - 32
                    x: 16
                    spacing: 6

                    SectionTitle {
                        text: window.tr("settings.roms_folder")
                        description: window.tr("settings.roms_folder.hint")
                    }
                    RowLayout {
                        Label {
                            Layout.fillWidth: true
                            elide: Text.ElideMiddle
                            font.family: "monospace"
                            text: window.v("roms_dir")
                        }
                        Button {
                            text: window.tr("settings.browse")
                            onClicked: cfg.chooseRomsDir()
                        }
                    }
                    Label {
                        visible: window.dependsOn([cfg.version], cfg.romsDirMissing())
                        color: Theme.warning
                        text: window.tr("common.folder_missing")
                    }

                    // The default lives in `~/.config`, which is hidden: that is why the path shows
                    // even with no choice made, and there is the open button. See
                    // `docs/implementation/22-screenshots.md`.
                    SectionTitle {
                        text: window.tr("settings.screenshots_folder")
                        description: window.tr("settings.screenshots_folder.hint")
                    }
                    RowLayout {
                        Label {
                            Layout.fillWidth: true
                            elide: Text.ElideMiddle
                            font.family: "monospace"
                            text: window.dependsOn([cfg.version], cfg.screenshotsDir())
                        }
                        Button {
                            text: window.tr("settings.browse")
                            onClicked: cfg.chooseScreenshotsDir()
                        }
                        Button {
                            visible: !window.dependsOn([cfg.version], cfg.screenshotsInDefaultDir())
                            text: window.tr("settings.screenshots_folder.default")
                            onClicked: cfg.useDefaultScreenshotsDir()
                        }
                        Button {
                            text: window.tr("settings.screenshots_folder.open")
                            onClicked: Qt.openUrlExternally(cfg.screenshotsDirUrl())
                        }
                    }

                    SectionTitle {
                        text: window.tr("settings.language")
                        description: window.tr("settings.language.hint")
                    }
                    ComboBox {
                        Layout.preferredWidth: 280
                        model: cfg.options("language")
                        currentIndex: window.v("language")
                        onActivated: (index) => window.define("language", index)
                    }

                    // The palette changes right away, in every window: see
                    // `docs/implementation/26-themes.md`.
                    SectionTitle {
                        text: window.tr("settings.theme")
                        description: window.tr("settings.theme.hint")
                    }
                    ComboBox {
                        Layout.preferredWidth: 280
                        model: window.dependsOn([Language.version], cfg.options("tema"))
                        currentIndex: window.v("tema")
                        onActivated: (index) => window.define("tema", index)
                    }

                    SectionTitle {
                        text: window.tr("settings.library_view")
                        description: window.tr("settings.library_view.hint")
                    }
                    ComboBox {
                        Layout.preferredWidth: 280
                        model: window.dependsOn([Language.version], cfg.options("biblioteca"))
                        currentIndex: window.v("biblioteca")
                        onActivated: (index) => window.define("biblioteca", index)
                    }

                    SectionTitle {
                        text: window.tr("settings.z_wheel")
                        description: window.tr("settings.z_wheel.hint")
                    }
                    RowLayout {
                        Label {
                            Layout.fillWidth: true
                            elide: Text.ElideMiddle
                            font.family: "monospace"
                            text: window.v("z_wheel_path")
                        }
                        Button {
                            text: window.tr("settings.browse")
                            onClicked: cfg.chooseZWheel(false)
                        }
                        Button {
                            text: window.tr("settings.z_wheel.folder")
                            onClicked: cfg.chooseZWheel(true)
                        }
                        Button {
                            text: window.tr("settings.z_wheel.detect")
                            onClicked: cfg.detectZWheel()
                        }
                    }
                    Label {
                        Layout.fillWidth: true
                        wrapMode: Text.Wrap
                        opacity: window.dependsOn([cfg.version], cfg.zWheelFound()) ? 0.6 : 1
                        color: window.dependsOn([cfg.version], cfg.zWheelFound()) ? palette.windowText : Theme.warning
                        text: window.dependsOn([cfg.version, Language.version], cfg.zWheelStatus())
                    }
                    Option {
                        keyId: "z_wheel.fim_de_vida"
                        label: "settings.z_wheel_eol"
                        hint: "settings.z_wheel_eol.hint"
                    }
                    Option {
                        keyId: "z_wheel.transicoes_sempre"
                        label: "settings.z_wheel_transitions"
                        hint: "settings.z_wheel_transitions.hint"
                    }

                    SectionTitle { text: window.tr("settings.discord") }
                    Option {
                        keyId: "discord.ativo"
                        label: "settings.discord.on"
                    }
                    Label {
                        Layout.leftMargin: 32
                        enabled: window.v("discord.ativo")
                        text: window.dependsOn([window.tick], cfg.discordConnected())
                              ? window.tr("settings.discord.connected")
                              : window.tr("settings.discord.waiting")
                    }

                    SectionTitle { text: window.tr("settings.updates") }
                    Hint {
                        text: window.tr("settings.updates.version").replace("{version}", cfg.emulatorVersion())
                    }
                    Option {
                        keyId: "atualizacoes.ao_abrir"
                        label: "settings.updates.on_start"
                    }
                    Option {
                        keyId: "atualizacoes.pre_lancamentos"
                        label: "settings.updates.prereleases"
                    }
                    RowLayout {
                        Button {
                            text: window.tr("settings.updates.check")
                            enabled: !window.dependsOn([window.tick], cfg.checkingForUpdate())
                            onClicked: cfg.checkForUpdates()
                        }
                        Label {
                            text: window.dependsOn([window.tick, Language.version], cfg.updateStatus())
                        }
                        Button {
                            readonly property string page: window.dependsOn([window.tick], cfg.updatePage())
                            readonly property bool alone: cfg.selfUpdates()
                            visible: page !== ""
                            // A self-replacing copy goes back to the main window's notice, which
                            // is where the installation's progress shows.
                            text: window.tr(alone ? "update.install" : "update.download")
                            onClicked: alone ? cfg.showUpdateNotice() : Qt.openUrlExternally(page)
                        }
                    }

                    Hint {
                        Layout.topMargin: 16
                        Layout.bottomMargin: 16
                        text: window.dependsOn([Language.version], cfg.settingsLocation())
                    }
                }
            }

            // Controls
            ScrollView {
                id: controlsTab

                contentWidth: availableWidth

                // The controller generates no event in Qt, as it did not in egui: while the tab is
                // in view, it is read here: the drawing lights up, capture by controller button
                // advances and calibration collects readings. `tick` redoes what is live; the
                // settings' `version`, what was saved.
                property int tick: 0
                readonly property bool inView: window.visible && tabs.currentIndex === 1
                Timer {
                    interval: 33
                    repeat: true
                    running: controlsTab.inView
                    onTriggered: {
                        cfg.readController()
                        controlsTab.tick += 1
                    }
                }
                onInViewChanged: if (inView) keyboard.forceActiveFocus()

                // This window's keyboard lights up the drawing and, with a capture open, becomes
                // the button's source. The tab's buttons do not take focus: with it, the captured
                // space would click "assign" again and abandon the capture.
                FocusScope {
                    id: keyboard

                    width: controlsTab.availableWidth
                    implicitHeight: controls.implicitHeight + 32
                    focus: true
                    Keys.onPressed: (event) => {
                        if (!event.isAutoRepeat)
                            cfg.key(event.key, event.modifiers, true)
                        event.accepted = true
                    }
                    Keys.onReleased: (event) => {
                        if (!event.isAutoRepeat)
                            cfg.key(event.key, event.modifiers, false)
                        event.accepted = true
                    }

                    SettingsPage {
                        id: controls

                        width: parent.width - 32
                        x: 16
                        y: 12
                        spacing: 6

                        readonly property bool isOn: window.dependsOn([cfg.version], cfg.portEnabled())
                        readonly property int deviceKind: window.dependsOn([cfg.version], cfg.deviceKind())

                        // The port: the console's two USB ports are real ports, each with its own
                        // mapping and its own device.
                        RowLayout {
                            Label { text: window.tr("controls.port") }
                            Repeater {
                                model: 2

                                Button {
                                    required property int index
                                    focusPolicy: Qt.NoFocus
                                    checkable: true
                                    checked: cfg.editedPort === index
                                    text: window.tr("controls.port.n").replace("{n}", index + 1)
                                    onClicked: cfg.editPort(index)
                                }
                            }
                        }
                        RowLayout {
                            CheckBox {
                                focusPolicy: Qt.NoFocus
                                text: window.tr("controls.port.on")
                                checked: controls.isOn
                                onToggled: cfg.setPortEnabled(checked)
                            }
                            Label {
                                enabled: controls.isOn
                                text: window.tr("controls.kind")
                            }
                            // The Z-Pad and the Dragon are a single button: the buttons, the axes
                            // and the drawing are the same. The model goes in the box beside it,
                            // because the console sees each one's USB identifier, and the Boomerang
                            // Sports games change their handling by it. The indices remain those of
                            // `cfg.deviceKind()`: 0 Z-Pad, 1 Dragon, 2 Boomerang, 3 keyboard.
                            Repeater {
                                model: [["controls.kind.pad", 1], ["controls.kind.boomerang", 2],
                                        ["controls.kind.keyboard", 3]]

                                Button {
                                    required property var modelData
                                    readonly property bool isController: modelData[1] === 1
                                    focusPolicy: Qt.NoFocus
                                    enabled: controls.isOn
                                    checkable: true
                                    // Like radio buttons: clicking the already chosen Controller
                                    // does not uncheck it. Without this the click toggled
                                    // `checked`, and since the device did not change, the binding
                                    // did not check it again.
                                    autoExclusive: true
                                    checked: isController ? controls.deviceKind <= 1
                                                        : controls.deviceKind === modelData[1]
                                    text: window.tr(modelData[0])
                                    // Already on a controller, the click does not change the
                                    // chosen model. Coming from another device, it is the Dragon,
                                    // which is the emulator's default.
                                    onClicked: cfg.setDevice(isController && controls.deviceKind <= 1
                                                                  ? controls.deviceKind : modelData[1])
                                }
                            }
                            Label {
                                visible: controls.deviceKind <= 1
                                enabled: controls.isOn
                                Layout.leftMargin: 8
                                text: window.tr("controls.kind.model")
                            }
                            ComboBox {
                                visible: controls.deviceKind <= 1
                                enabled: controls.isOn
                                focusPolicy: Qt.NoFocus
                                model: [window.tr("controls.kind.zpad"), window.tr("controls.kind.dragon")]
                                currentIndex: controls.deviceKind
                                onActivated: (index) => cfg.setDevice(index)
                            }
                        }
                        Hint {
                            text: window.tr(controls.deviceKind === 3 ? "controls.kind.hint"
                                            : controls.deviceKind === 2 ? "controls.kind.boomerang.hint"
                                            : "controls.kind.pad.hint")
                        }
                        Explanation { text: window.tr("controls.hint") }

                        // A free port has nothing to configure, and showing the controller drawing
                        // on it would invite mapping a device the console will not enumerate.
                        Hint {
                            visible: !controls.isOn
                            text: window.tr("controls.port.off_hint")
                        }

                        // The Boomerang: the image tilts with the sensor, and below it are what is
                        // pressed, the acceleration and the calibration.
                        ColumnLayout {
                            Layout.fillWidth: true
                            visible: controls.isOn && controls.deviceKind === 2
                            spacing: 6

                            Option {
                                keyId: "movimento.aviso_de_calibracao"
                                label: "calibration.toast.setting"
                            }
                            Item {
                                Layout.alignment: Qt.AlignHCenter
                                Layout.preferredWidth: 360
                                Layout.preferredHeight: 324

                                Image {
                                    anchors.centerIn: parent
                                    width: 360
                                    fillMode: Image.PreserveAspectFit
                                    source: "qrc:/zeebx/boomerang.png"
                                    rotation: window.dependsOn([controlsTab.tick], cfg.rotation())
                                    smooth: true
                                }
                            }
                            Label {
                                Layout.alignment: Qt.AlignHCenter
                                text: window.dependsOn([controlsTab.tick], cfg.sensor())
                            }
                            // The most common case of a controller with a sensor that does not move
                            // the Boomerang: the sensor's device node without permission. One click
                            // writes the rule that fixes it, asking for the password.
                            ColumnLayout {
                                Layout.fillWidth: true
                                visible: window.dependsOn([controlsTab.tick], cfg.sensorLacksPermission())
                                readonly property int runState: window.dependsOn([controlsTab.tick], cfg.grantStatus())

                                Button {
                                    focusPolicy: Qt.NoFocus
                                    visible: parent.runState !== 1
                                    text: window.tr("controls.boomerang.unlock")
                                    onClicked: cfg.unlockSensors()
                                }
                                Hint {
                                    text: window.tr(parent.runState === 1 ? "controls.boomerang.unlock_waiting"
                                                    : "controls.boomerang.unlock_hint")
                                }
                                // The rule is in, and reading resumes on the next attempt, within a
                                // second.
                                Hint {
                                    visible: parent.runState === 2
                                    text: window.tr("controls.boomerang.unlock_done")
                                }
                                Label {
                                    Layout.fillWidth: true
                                    visible: parent.runState === 3
                                    wrapMode: Text.Wrap
                                    color: Theme.warning
                                    text: window.dependsOn([controlsTab.tick], cfg.grantFailure())
                                }
                                TextField {
                                    Layout.fillWidth: true
                                    visible: parent.runState === 3
                                    readOnly: true
                                    font.family: "monospace"
                                    text: cfg.udevRule()
                                }
                            }
                            Label {
                                Layout.alignment: Qt.AlignHCenter
                                font.family: "monospace"
                                text: window.dependsOn([controlsTab.tick], cfg.motionLine())
                            }
                            RowLayout {
                                Layout.alignment: Qt.AlignHCenter

                                Button {
                                    focusPolicy: Qt.NoFocus
                                    enabled: window.dependsOn([controlsTab.tick], cfg.hasReading() && !cfg.calibrating())
                                    text: window.tr("controls.boomerang.calibrate")
                                    onClicked: cfg.calibrate()
                                }
                                Button {
                                    focusPolicy: Qt.NoFocus
                                    text: window.tr("controls.boomerang.calibrate_reset")
                                    onClicked: cfg.resetCalibration()
                                }
                            }
                            Label {
                                Layout.alignment: Qt.AlignHCenter
                                visible: window.dependsOn([controlsTab.tick], cfg.calibrationRejected() && !cfg.calibrating())
                                color: Theme.warning
                                text: window.tr("controls.boomerang.calibrate_refused")
                            }
                            Hint {
                                horizontalAlignment: Text.AlignHCenter
                                text: window.tr(window.dependsOn([controlsTab.tick], cfg.calibrating())
                                                ? "controls.boomerang.calibrating"
                                                : "controls.boomerang.calibrate_hint")
                            }
                        }

                        // The controller drawing: the art, and over it each button's silhouette,
                        // lit when pressed, bluish under the cursor, pulsing while waiting for
                        // capture. The click is tested against the silhouette, not against a box.
                        Item {
                            id: drawing

                            readonly property real ratio: cfg.artAspectRatio()
                            // At most 500 wide and 210 high, as in egui: the window also needs room
                            // for the list of buttons.
                            readonly property real boxWidth: Math.min(Math.min(parent.width, 500), 210 * ratio)
                            property string underPointer: ""

                            Layout.fillWidth: true
                            Layout.preferredHeight: ratio > 0 ? boxWidth / ratio : 0
                            visible: controls.isOn && controls.deviceKind !== 2 && ratio > 0

                            Item {
                                id: art

                                anchors.centerIn: parent
                                width: drawing.boxWidth
                                height: drawing.ratio > 0 ? width / drawing.ratio : 0

                                Image {
                                    anchors.fill: parent
                                    source: "image://zeebx/controle/base"
                                    smooth: true
                                }

                                Repeater {
                                    model: cfg.parts()

                                    Image {
                                        id: silhouette

                                        required property int index
                                        required property string modelData
                                        readonly property rect limits: cfg.partBounds(index)
                                        readonly property bool waiting: window.dependsOn([controlsTab.tick], cfg.capturing() === modelData)
                                        readonly property bool lit: window.dependsOn([controlsTab.tick], cfg.pressed(modelData))

                                        x: limits.x * art.width
                                        y: limits.y * art.height
                                        width: limits.width * art.width
                                        height: limits.height * art.height
                                        smooth: true
                                        visible: waiting || lit || drawing.underPointer === modelData
                                        // egui's colors: lit green, bluish under the cursor, and
                                        // the capture orange, pulsing on the interface's clock.
                                        source: "image://zeebx/controle/parte/" + index + "/"
                                                + (waiting ? "ffa028ff" : lit ? "2fd68ab4" : "6c9cff50")
                                        opacity: waiting ? (100 + 110 * (0.5 + 0.5 * Math.sin(controlsTab.tick * 0.2))) / 255 : 1
                                    }
                                }

                                // Each stick's dot, inside the drawn circle: shows that an analog
                                // stick is really reaching the emulator, and with how much travel.
                                Repeater {
                                    model: [["lthumb", 0, 1], ["rthumb", 2, 3]]

                                    Rectangle {
                                        required property var modelData
                                        readonly property int part: cfg.parts().indexOf(modelData[0])
                                        readonly property rect limits: part >= 0 ? cfg.partBounds(part) : Qt.rect(0, 0, 0, 0)
                                        readonly property real dotRadius: Math.min(limits.width * art.width, limits.height * art.height) / 2

                                        visible: part >= 0
                                        width: Math.max(dotRadius * 0.44, 4)
                                        height: width
                                        radius: width / 2
                                        color: "#d02f6bd6"
                                        x: (limits.x + limits.width / 2) * art.width
                                           + window.dependsOn([controlsTab.tick], cfg.axis(modelData[1])) * dotRadius * 0.6 - width / 2
                                        y: (limits.y + limits.height / 2) * art.height
                                           + window.dependsOn([controlsTab.tick], cfg.axis(modelData[2])) * dotRadius * 0.6 - height / 2
                                    }
                                }

                                MouseArea {
                                    anchors.fill: parent
                                    hoverEnabled: true
                                    cursorShape: drawing.underPointer !== "" ? Qt.PointingHandCursor : Qt.ArrowCursor
                                    onPositionChanged: (mouse) => drawing.underPointer = cfg.buttonAt(mouse.x / width, mouse.y / height)
                                    onExited: drawing.underPointer = ""
                                    // Clicking the piece is the same as clicking "assign" on its
                                    // row.
                                    onClicked: if (drawing.underPointer !== "") {
                                        cfg.capture(drawing.underPointer)
                                        window.listenForKeys(keyboard)
                                    }
                                }
                            }
                        }
                        Hint {
                            visible: drawing.visible
                            horizontalAlignment: Text.AlignHCenter
                            text: window.tr("controls.art_hint")
                        }

                        // The rest only exists with the port turned on.
                        ColumnLayout {
                            Layout.fillWidth: true
                            visible: controls.isOn
                            spacing: 6

                            // Controller choice. The keyboard never goes away: whoever connects a
                            // controller can still use the keys.
                            RowLayout {
                                id: controllerChoice

                                property int searches: 0

                                Label { text: window.tr("controls.device") }
                                ComboBox {
                                    id: controllerList

                                    Layout.preferredWidth: 320
                                    focusPolicy: Qt.NoFocus
                                    model: window.dependsOn([cfg.version, controllerChoice.searches], cfg.controls())
                                    currentIndex: window.dependsOn([cfg.version, controllerChoice.searches], cfg.currentController())
                                    onActivated: (index) => cfg.chooseController(index)
                                }
                                Button {
                                    focusPolicy: Qt.NoFocus
                                    text: window.tr("controls.rescan")
                                    onClicked: {
                                        cfg.readController()
                                        controllerChoice.searches += 1
                                    }
                                }
                            }
                            Hint {
                                visible: controllerList.count <= 1
                                text: window.tr("controls.no_devices")
                            }
                            Button {
                                focusPolicy: Qt.NoFocus
                                text: window.tr("controls.reset")
                                onClicked: cfg.resetMapping()
                            }

                            // The Zeebo buttons: where each one comes from, assign and clear.
                            GridLayout {
                                Layout.fillWidth: true
                                Layout.topMargin: 8
                                columns: 4

                                Repeater {
                                    model: cfg.buttons()

                                    Label {
                                        required property int index
                                        required property string modelData
                                        Layout.row: index
                                        Layout.column: 0
                                        text: window.tr("button." + modelData)
                                    }
                                }
                                Repeater {
                                    model: cfg.buttons()

                                    Label {
                                        required property int index
                                        required property string modelData
                                        Layout.row: index
                                        Layout.column: 1
                                        Layout.fillWidth: true
                                        elide: Text.ElideRight
                                        text: window.dependsOn([cfg.version, cfg.editedPort, Language.version], cfg.buttonSources(modelData))
                                    }
                                }
                                Repeater {
                                    model: cfg.buttons()

                                    Button {
                                        required property int index
                                        required property string modelData
                                        readonly property bool waiting: window.dependsOn([controlsTab.tick], cfg.capturing() === modelData)
                                        Layout.row: index
                                        Layout.column: 2
                                        focusPolicy: Qt.NoFocus
                                        checkable: true
                                        checked: waiting
                                        text: window.tr(waiting ? "controls.assigning" : "controls.assign")
                                        onClicked: {
                                            cfg.capture(modelData)
                                            window.listenForKeys(keyboard)
                                        }
                                    }
                                }
                                Repeater {
                                    model: cfg.buttons()

                                    Button {
                                        required property int index
                                        required property string modelData
                                        Layout.row: index
                                        Layout.column: 3
                                        focusPolicy: Qt.NoFocus
                                        text: window.tr("controls.clear")
                                        onClicked: cfg.clearButton(modelData)
                                    }
                                }
                            }

                            // The port's turbo: the mode and the default button belong to the
                            // player; the presses per second, to everyone. The turbo key is the
                            // last row of the grid above. See `docs/implementation/24-speed.md`.
                            SectionTitle {
                                text: window.tr("turbo")
                                description: window.tr("turbo.hint")
                            }
                            Repeater {
                                model: [["turbo.mode", "turbo.mode"], ["turbo.button", "turbo.button"], ["turbo.rate", "turbo.rate"]]

                                RowLayout {
                                    required property var modelData

                                    Label {
                                        Layout.preferredWidth: 180
                                        text: window.tr(modelData[1])
                                    }
                                    ComboBox {
                                        Layout.preferredWidth: 280
                                        focusPolicy: Qt.NoFocus
                                        model: window.dependsOn([Language.version], cfg.options(modelData[0]))
                                        currentIndex: window.dependsOn([cfg.version, cfg.editedPort], cfg.value(modelData[0]))
                                        onActivated: (index) => window.define(modelData[0], index)
                                    }
                                }
                            }

                            Label {
                                readonly property string reason: window.dependsOn([cfg.version, controlsTab.tick], cfg.rejection(false))
                                Layout.fillWidth: true
                                visible: reason !== ""
                                wrapMode: Text.Wrap
                                color: Theme.warning
                                text: reason
                            }
                            Hint {
                                readonly property string hint: window.dependsOn([controlsTab.tick], cfg.captureHint())
                                visible: hint !== ""
                                text: hint
                            }

                            // The axes: an axis is not a button, it has travel, and so the source
                            // is a single one and gets a direction. The live value tells "not
                            // mapped" from "mapped to the wrong axis": a stick that does not get
                            // through shows up as a stubborn zero.
                            SectionTitle {
                                text: window.tr("controls.axes")
                                description: window.tr("controls.axes.hint")
                            }
                            Label {
                                font.family: "monospace"
                                function line() {
                                    const names = cfg.axes()
                                    let line = []
                                    for (let i = 0; i < names.length; i++)
                                        line.push(names[i] + ": " + (cfg.axis(i) >= 0 ? "+" : "") + cfg.axis(i).toFixed(2))
                                    return line.join("   ")
                                }
                                text: window.dependsOn([controlsTab.tick], line())
                            }
                            Repeater {
                                model: cfg.axes()

                                RowLayout {
                                    required property string modelData
                                    readonly property int origin: window.dependsOn([cfg.version, cfg.editedPort], cfg.axisSource(modelData))

                                    Label {
                                        Layout.preferredWidth: 180
                                        text: window.tr("axis." + modelData)
                                    }
                                    ComboBox {
                                        Layout.preferredWidth: 200
                                        focusPolicy: Qt.NoFocus
                                        model: window.dependsOn([Language.version], cfg.axisSources())
                                        currentIndex: parent.origin
                                        onActivated: (index) => cfg.setAxisSource(parent.modelData, index)
                                    }
                                    CheckBox {
                                        focusPolicy: Qt.NoFocus
                                        visible: parent.origin > 0
                                        text: window.tr("controls.invert")
                                        checked: window.dependsOn([cfg.version, cfg.editedPort], cfg.axisInverted(parent.modelData))
                                        onToggled: cfg.invertAxis(parent.modelData, checked)
                                    }
                                }
                            }

                            Hint {
                                Layout.topMargin: 12
                                Layout.bottomMargin: 16
                                text: window.tr("controls.players_note")
                            }
                        }
                    }
                }
            }

            // Shortcuts: every key of the game window in one place, like Dolphin's "Hotkey
            // Settings": the rebindable ones, and the fixed ones read-only. The key comes from this
            // window's keyboard, and the controller button from a timer of its own: the controller
            // generates no event in Qt.
            ScrollView {
                id: shortcutsTab

                readonly property bool inView: window.visible && tabs.currentIndex === 2
                property int tick: 0

                contentWidth: availableWidth

                Timer {
                    interval: 33
                    repeat: true
                    running: shortcutsTab.inView
                    onTriggered: {
                        cfg.readControllerShortcut()
                        shortcutsTab.tick += 1
                    }
                }
                onInViewChanged: if (inView) shortcutsKeyboard.forceActiveFocus()

                // The same keyboard as the controls tab: with a capture open, the key becomes the
                // shortcut.
                FocusScope {
                    id: shortcutsKeyboard

                    width: shortcutsTab.availableWidth
                    implicitHeight: shortcuts.implicitHeight + 32
                    focus: true
                    Keys.onPressed: (event) => {
                        if (!event.isAutoRepeat)
                            cfg.key(event.key, event.modifiers, true)
                        event.accepted = true
                    }
                    Keys.onReleased: (event) => {
                        if (!event.isAutoRepeat)
                            cfg.key(event.key, event.modifiers, false)
                        event.accepted = true
                    }

                    SettingsPage {
                        id: shortcuts

                        width: parent.width - 32
                        x: 16
                        y: 12
                        spacing: 6
                        pageDescription: window.tr("controls.shortcuts.hint") + " "
                                           + window.tr("controls.shortcuts.combo.hint")


                        Repeater {
                            model: [
                                ["screenshot", "controls.shortcut.screenshot", "controls.shortcut.waiting", "controls.shortcut.reset"],
                                ["avancar", "controls.shortcut.fast_forward", "controls.shortcut.waiting_any", "controls.shortcut.reset_ff"],
                                ["voltar", "controls.shortcut.rewind", "controls.shortcut.waiting_any", "controls.shortcut.reset_rewind"],
                                ["reiniciar", "controls.shortcut.restart", "controls.shortcut.waiting", "controls.shortcut.reset_restart"],
                                // The save state ones: the factory default changes with the
                                // system, and the button says so.
                                ["salvar_estado", "controls.shortcut.save_state", "controls.shortcut.waiting_combo", ""],
                                ["carregar_estado", "controls.shortcut.load_state", "controls.shortcut.waiting_combo", ""],
                                ["proximo_slot", "controls.shortcut.next_slot", "controls.shortcut.waiting_combo", ""],
                                ["slot_anterior", "controls.shortcut.previous_slot", "controls.shortcut.waiting_combo", ""],
                                ["desfazer_carregar", "controls.shortcut.undo_load", "controls.shortcut.waiting_combo", ""]
                            ]

                            RowLayout {
                                required property var modelData
                                readonly property bool waiting: window.dependsOn([shortcutsTab.tick], cfg.capturingShortcut(modelData[0]))

                                Label {
                                    Layout.preferredWidth: 180
                                    text: window.tr(modelData[1])
                                }
                                Label {
                                    Layout.preferredWidth: 160
                                    elide: Text.ElideRight
                                    font.family: "monospace"
                                    text: window.dependsOn([cfg.version], cfg.shortcut(modelData[0]))
                                }
                                Button {
                                    focusPolicy: Qt.NoFocus
                                    checkable: true
                                    checked: parent.waiting
                                    text: window.tr(parent.waiting ? modelData[2] : "controls.shortcut.change")
                                    onClicked: {
                                        cfg.captureShortcut(modelData[0])
                                        window.listenForKeys(shortcutsKeyboard)
                                    }
                                }
                                Button {
                                    focusPolicy: Qt.NoFocus
                                    text: modelData[3] !== ""
                                          ? window.tr(modelData[3])
                                          : window.tr("controls.shortcut.reset_key").replace("{key}", cfg.defaultShortcut(modelData[0]))
                                    onClicked: cfg.resetShortcut(modelData[0])
                                }
                            }
                        }
                        Label {
                            readonly property string reason: window.dependsOn([cfg.version, shortcutsTab.tick], cfg.rejection(true))
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            color: Theme.warning
                            text: reason
                        }
                        Hint {
                            readonly property string hint: window.dependsOn([shortcutsTab.tick], cfg.captureHint())
                            visible: hint !== ""
                            text: hint
                        }

                        SectionTitle { text: window.tr("controls.shortcut.fixed") }
                        Repeater {
                            model: [
                                ["controls.shortcut.quit", "Esc"],
                                ["controls.shortcut.pause", "P"],
                                ["controls.shortcut.fullscreen", "F11 · Alt+Enter"]
                            ]

                            RowLayout {
                                required property var modelData

                                Label {
                                    Layout.preferredWidth: 180
                                    text: window.tr(modelData[0])
                                }
                                Label {
                                    font.family: "monospace"
                                    opacity: 0.6
                                    text: modelData[1]
                                }
                            }
                        }
                    }
                }
            }

            // Graphics
            ScrollView {
                contentWidth: availableWidth

                SettingsPage {
                    width: parent.width - 32
                    x: 16
                    spacing: 6
                    pageDescription: window.forGame ? "" : window.tr("graphics.window.hint")

                    // The windows belong to the emulator, not to the game: in game mode, they go
                    // away.
                    Choice {
                        Layout.topMargin: 12
                        visible: !window.forGame
                        keyId: "graphics.janela"
                        label: "graphics.window.main"
                    }
                    Choice {
                        visible: !window.forGame
                        keyId: "graphics.janela_do_jogo"
                        label: "graphics.window.game"
                    }
                    // Like PCSX2's "Render to Separate Window": when off, the game is drawn in the
                    // main window, in place of the library.
                    Option {
                        visible: !window.forGame
                        keyId: "graphics.jogo_separado"
                        label: "graphics.separate_window"
                        hint: "graphics.separate_window.hint"
                    }
                    // Like PCSX2's "Confirm Shutdown". Global, like the window: it does not change
                    // per game.
                    Option {
                        visible: !window.forGame
                        keyId: "graphics.confirmar_ao_fechar"
                        label: "graphics.confirm_close"
                        hint: "graphics.confirm_close.hint"
                    }

                    SectionTitle { text: window.tr("graphics.scaling") }
                    Repeater {
                        model: window.dependsOn([Language.version], cfg.options("graphics.scaling"))

                        RadioButton {
                            required property int index
                            required property string modelData

                            text: modelData
                            checked: window.v("graphics.scaling") === index
                            onClicked: window.define("graphics.scaling", index)
                        }
                    }
                    Hint {
                        // "Integer" is the first option.
                        visible: window.v("graphics.scaling") === 0
                        text: window.tr("graphics.scaling.integer.hint")
                    }

                    Option {
                        Layout.topMargin: 12
                        keyId: "graphics.smooth"
                        label: "graphics.smooth"
                    }
                    Option {
                        keyId: "graphics.keep_aspect"
                        label: "graphics.keep_aspect"
                    }
                    // egui's "put the frame on screen through GL" does not appear here: the Qt
                    // window always puts the frame through the scene graph, with the GPU texture
                    // when there is one.
                    Option {
                        Layout.topMargin: 12
                        keyId: "graphics.neblina"
                        label: "graphics.fog"
                        hint: "graphics.fog.hint"
                    }
                    Option {
                        Layout.topMargin: 12
                        keyId: "graphics.gpu_rasterizer"
                        label: "graphics.gpu_rasterizer"
                        hint: "graphics.gpu_rasterizer.hint"
                    }

                    // What depends on the GPU rasterizer.
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.topMargin: 12
                        enabled: window.v("graphics.gpu_rasterizer")
                        spacing: 6

                        Choice {
                            keyId: "graphics.resolucao_interna"
                            label: "graphics.internal_resolution"
                        }
                        Hint { text: window.tr("graphics.internal_resolution.hint") }
                        Choice {
                            keyId: "graphics.proporcao"
                            label: "graphics.aspect"
                        }
                        Label {
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            color: Theme.warning
                            text: window.tr("graphics.aspect.hint")
                        }
                        Choice {
                            keyId: "graphics.antialias"
                            label: "graphics.antialias"
                        }
                        Hint { text: window.tr("graphics.antialias.hint") }
                        Choice {
                            keyId: "graphics.anisotropico"
                            label: "graphics.anisotropic"
                        }
                        Hint {
                            Layout.bottomMargin: 16
                            text: window.tr("graphics.anisotropic.hint")
                        }
                    }
                }
            }

            // Speed: the game's pace against the wall clock. See
            // `docs/implementation/24-speed.md`.
            ScrollView {
                contentWidth: availableWidth

                SettingsPage {
                    width: parent.width - 32
                    x: 16
                    spacing: 6

                    Choice {
                        Layout.topMargin: 12
                        keyId: "speed.fps_limit"
                        label: "speed.fps_limit"
                    }
                    Explanation { text: window.tr("speed.fps_limit.hint") }
                    Choice {
                        Layout.topMargin: 12
                        keyId: "speed.frameskip"
                        label: "speed.frameskip"
                    }
                    Explanation { text: window.tr("speed.frameskip.hint") }

                    SectionTitle {
                        text: window.tr("speed.ff")
                        description: window.tr("speed.ff.hint")
                    }
                    Choice {
                        keyId: "speed.ff.ratio"
                        label: "speed.ff.ratio"
                    }
                    Choice {
                        keyId: "speed.ff.mode"
                        label: "speed.ff.mode"
                    }
                    Option {
                        keyId: "speed.ff.mute"
                        label: "speed.ff.mute"
                        hint: "speed.ff.mute.hint"
                    }

                    SectionTitle { text: window.tr("speed.rewind") }
                    Option {
                        keyId: "speed.rewind.on"
                        label: "speed.rewind.on"
                        hint: "speed.rewind.on.hint"
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.bottomMargin: 16
                        enabled: window.v("speed.rewind.on")
                        spacing: 6

                        Choice {
                            keyId: "speed.rewind.interval"
                            label: "speed.rewind.interval"
                        }
                        Choice {
                            keyId: "speed.rewind.memory"
                            label: "speed.rewind.memory"
                        }
                        Hint { text: window.tr("speed.rewind.memory.hint") }
                    }
                }
            }

            // Audio
            ScrollView {
                contentWidth: availableWidth

                SettingsPage {
                    width: parent.width - 32
                    x: 16
                    spacing: 6
                    pageDescription: window.tr("audio.hint")

                    Option {
                        Layout.topMargin: 12
                        keyId: "audio.enabled"
                        label: "audio.enabled"
                    }
                    RowLayout {
                        enabled: window.v("audio.enabled")

                        Slider {
                            Layout.preferredWidth: 280
                            from: 0
                            to: 100
                            stepSize: 1
                            value: window.v("audio.volume")
                            onMoved: window.define("audio.volume", Math.round(value))
                        }
                        Label {
                            text: window.v("audio.volume") + " — " + window.tr("audio.volume")
                        }
                    }

                    // The bank is opened when the game opens: the hint says the change applies to
                    // the next one.
                    SectionTitle {
                        text: window.tr("audio.soundfont")
                        description: window.tr("audio.soundfont.hint")
                    }
                    RowLayout {
                        Label {
                            Layout.fillWidth: true
                            elide: Text.ElideMiddle
                            font.family: "monospace"
                            text: window.dependsOn([cfg.version], cfg.soundfont())
                        }
                        Button {
                            text: window.tr("settings.browse")
                            onClicked: cfg.chooseSoundfont()
                        }
                        Button {
                            visible: !window.dependsOn([cfg.version], cfg.autoSoundfont())
                            text: window.tr("audio.soundfont.clear")
                            onClicked: cfg.useAutoSoundfont()
                        }
                    }
                    Option {
                        Layout.topMargin: 12
                        keyId: "audio.midi_effects"
                        label: "audio.midi_effects"
                        hint: "audio.midi_effects.hint"
                    }
                }
            }

            // Debug
            ScrollView {
                contentWidth: availableWidth

                SettingsPage {
                    width: parent.width - 32
                    x: 16
                    spacing: 6
                    pageDescription: window.tr("debug.hint")

                    Option {
                        Layout.topMargin: 8
                        keyId: "debug.overlay"
                        label: "debug.overlay"
                    }
                    // What follows only exists inside the panel; without it, checking would have no
                    // effect at all.
                    ColumnLayout {
                        Layout.leftMargin: 24
                        enabled: window.v("debug.overlay")
                        spacing: 0

                        Option { keyId: "debug.speed"; label: "debug.speed" }
                        Option { keyId: "debug.clock"; label: "debug.clock" }
                        Option { keyId: "debug.memory"; label: "debug.memory" }
                        Option { keyId: "debug.timeline"; label: "debug.timeline" }
                    }
                    Option {
                        Layout.topMargin: 8
                        keyId: "debug.metricas"
                        label: "debug.metricas"
                        hint: "debug.metricas.hint"
                    }
                    Option {
                        Layout.topMargin: 8
                        keyId: "debug.log"
                        label: "debug.log"
                        hint: "debug.log.hint"
                    }
                    Option {
                        Layout.topMargin: 8
                        keyId: "debug.gravar_sessao"
                        label: "debug.gravar_sessao"
                        hint: "debug.gravar_sessao.hint"
                    }
                    Button {
                        Layout.leftMargin: 24
                        text: window.tr("debug.gravar_sessao.open")
                        onClicked: Qt.openUrlExternally(cfg.sessionsDirUrl())
                    }
                }
            }
        }
    }
}
