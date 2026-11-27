import QtQuick
import QtQuick.Layouts

import zeebx
import zeebx.htpc

// Built-in default HTPC theme: Z-Wheel HD.
//
// Also serves as the reference implementation of the Zeebx HTPC community theme contract:
// receives `library`, `htpc` (HtpcController) and `host` (HtpcMain), reads all colors from
// `htpc.*` so per-theme palettes and accent overrides apply automatically, and responds to
// unified gamepad/keyboard commands through `handleCommand(code)`.
Item {
    id: theme

    property var library: null
    property var htpc: HtpcController
    property var host: null

    // 0 = All games, 1 = Z-Wheel shortcut view
    property int activeTab: 0
    property int cursor: 0
    property real position: 0
    property int total: 0
    property int version: 0
    property string clockText: Qt.formatTime(new Date(), "HH:mm")

    function dependsOn(versions, value) {
        return value
    }

    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    function line(k) {
        return total > 0 ? ((k % total) + total) % total : 0
    }

    function refreshModel() {
        if (!library)
            return
        total = library.lines()
        if (total === 0) {
            cursor = 0
            position = 0
        }
        version += 1
    }

    onLibraryChanged: refreshModel()

    Connections {
        target: theme.library
        ignoreUnknownSignals: true
        function onModelReset() {
            theme.refreshModel()
        }
    }

    Timer {
        interval: 15000
        repeat: true
        running: theme.visible
        onTriggered: theme.clockText = Qt.formatTime(new Date(), "HH:mm")
    }

    // Smooth damped spring chasing `cursor` (60/120fps on GPU).
    FrameAnimation {
        running: theme.visible && theme.position !== theme.cursor
        onTriggered: {
            const dt = Math.min(frameTime, 0.1)
            theme.position += (theme.cursor - theme.position) * (1 - Math.exp(-dt * 12))
            if (Math.abs(theme.cursor - theme.position) < 0.002)
                theme.position = theme.cursor
        }
    }

    function handleCommand(code) {
        switch (code) {
        case 2: // Left
            if (activeTab === 0 && total > 0)
                cursor -= 1
            break
        case 3: // Right
            if (activeTab === 0 && total > 0)
                cursor += 1
            break
        case 4: // Confirm / Play
            if (activeTab === 1) {
                if (host) host.launchZWheel()
            } else if (total > 0 && host) {
                host.launchGame(line(cursor))
            }
            break
        case 7: // Action Y / Details
            if (activeTab === 0 && total > 0 && host)
                host.openDetails(line(cursor))
            break
        case 8: // L1 / TabPrev
        case 9: // R1 / TabNext
            if (library && library.hasZWheel())
                activeTab = activeTab === 0 ? 1 : 0
            break
        case 11: // Select -> quick launch Z-Wheel if installed
            if (library && library.hasZWheel() && host)
                host.launchZWheel()
            break
        }
    }

    readonly property var currentGame: dependsOn(
        [version, cursor],
        (library && total > 0) ? library.item(line(cursor)) : ({})
    )

    readonly property real reelHeight: Math.min(Math.max(height * 0.13, 62), 104)
    readonly property real infoHeight: Math.min(Math.max(height * 0.15, 76), 124)

    // -------------------------------------------------------------------------
    // Dynamic ambient backdrop synced to per-theme colors
    // -------------------------------------------------------------------------
    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop { position: 0.0; color: htpc.bgColor }
            GradientStop { position: 1.0; color: htpc.bgAltColor }
        }
    }

    // Soft ambient accent aura behind the center stage
    Rectangle {
        width: Math.min(parent.width * 0.72, 920)
        height: width * 0.56
        radius: height / 2
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.verticalCenter: parent.verticalCenter
        color: htpc.accentSoftColor
        opacity: 0.55
    }

    // Concentric Z-Wheel decorative rings (pure QtQuick, 100% Qt 6.4 safe)
    Rectangle {
        width: parent.width * 0.86
        height: parent.height * 0.44
        radius: height / 2
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.verticalCenter: parent.verticalCenter
        anchors.verticalCenterOffset: 24
        color: "transparent"
        border.width: 2
        border.color: htpc.accentColor
        opacity: 0.18
    }

    Rectangle {
        width: parent.width * 0.68
        height: parent.height * 0.32
        radius: height / 2
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.verticalCenter: parent.verticalCenter
        anchors.verticalCenterOffset: 24
        color: "transparent"
        border.width: 1
        border.color: htpc.textColor
        opacity: 0.10
    }

    // -------------------------------------------------------------------------
    // Main vertical layout
    // -------------------------------------------------------------------------
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 24
        spacing: 12

        // Top Bar: Brand + Tabs + Controller Status + Clock
        RowLayout {
            Layout.fillWidth: true
            spacing: 16

            // Brand badge
            RowLayout {
                spacing: 10

                Rectangle {
                    width: 34
                    height: 34
                    radius: 9
                    color: htpc.accentColor

                    Text {
                        anchors.centerIn: parent
                        text: "Z"
                        color: htpc.accentTextColor
                        font.pixelSize: 19
                        font.bold: true
                    }
                }

                Text {
                    text: "ZEEBX"
                    color: htpc.textColor
                    font.pixelSize: 19
                    font.bold: true
                    font.letterSpacing: 1
                }

                Rectangle {
                    radius: 6
                    color: htpc.accentSoftColor
                    Layout.preferredWidth: badgeLbl.implicitWidth + 14
                    Layout.preferredHeight: 22

                    Text {
                        id: badgeLbl
                        anchors.centerIn: parent
                        text: theme.tr("htpc.badge")
                        color: htpc.accentColor
                        font.pixelSize: 11
                        font.bold: true
                    }
                }
            }

            // Tab selector pill (L1 / R1)
            Rectangle {
                visible: theme.library && theme.library.hasZWheel()
                radius: 18
                color: htpc.surfaceColor
                border.width: 1
                border.color: htpc.borderColor
                Layout.preferredHeight: 36
                Layout.preferredWidth: tabsRow.implicitWidth + 20

                RowLayout {
                    id: tabsRow
                    anchors.centerIn: parent
                    spacing: 8

                    Text {
                        text: htpc.promptL1
                        color: htpc.mutedColor
                        font.pixelSize: 11
                        font.bold: true
                    }

                    Rectangle {
                        radius: 14
                        color: theme.activeTab === 0 ? htpc.accentColor : "transparent"
                        Layout.preferredHeight: 26
                        Layout.preferredWidth: allTabLbl.implicitWidth + 22

                        Text {
                            id: allTabLbl
                            anchors.centerIn: parent
                            text: theme.tr("htpc.tab.all")
                            color: theme.activeTab === 0 ? htpc.accentTextColor : htpc.mutedColor
                            font.pixelSize: 13
                            font.bold: true
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: theme.activeTab = 0
                        }
                    }

                    Rectangle {
                        radius: 14
                        color: theme.activeTab === 1 ? htpc.accentColor : "transparent"
                        Layout.preferredHeight: 26
                        Layout.preferredWidth: zwTabLbl.implicitWidth + 22

                        Text {
                            id: zwTabLbl
                            anchors.centerIn: parent
                            text: theme.tr("htpc.tab.zwheel")
                            color: theme.activeTab === 1 ? htpc.accentTextColor : htpc.mutedColor
                            font.pixelSize: 13
                            font.bold: true
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: theme.activeTab = 1
                        }
                    }

                    Text {
                        text: htpc.promptR1
                        color: htpc.mutedColor
                        font.pixelSize: 11
                        font.bold: true
                    }
                }
            }

            Item { Layout.fillWidth: true }

            // Connected controller pill
            Rectangle {
                radius: 16
                color: htpc.surfaceColor
                border.width: 1
                border.color: htpc.borderColor
                Layout.preferredHeight: 34
                Layout.preferredWidth: padRow.implicitWidth + 22

                RowLayout {
                    id: padRow
                    anchors.centerIn: parent
                    spacing: 8

                    Rectangle {
                        width: 8
                        height: 8
                        radius: 4
                        color: "#34d399"
                    }

                    Text {
                        text: htpc.controllerStatus
                        color: htpc.textColor
                        font.pixelSize: 13
                        font.bold: true
                    }
                }
            }

            // Settings quick button
            Rectangle {
                radius: 16
                color: htpc.surfaceColor
                border.width: 1
                border.color: htpc.borderColor
                Layout.preferredHeight: 34
                Layout.preferredWidth: cfgBtnLbl.implicitWidth + 24

                Text {
                    id: cfgBtnLbl
                    anchors.centerIn: parent
                    text: theme.tr("htpc.prompt.settings")
                    color: htpc.textColor
                    font.pixelSize: 13
                    font.bold: true
                }
                MouseArea {
                    anchors.fill: parent
                    onClicked: if (theme.host) theme.host.openSettings()
                }
            }

            Text {
                text: theme.clockText
                color: htpc.textColor
                font.pixelSize: 17
                font.bold: true
                font.family: "monospace"
            }
        }

        // ---------------------------------------------------------------------
        // Tab 0: Z-Wheel HD Cylindrical Logo Reel + 3D Perspective Box Stage
        // ---------------------------------------------------------------------
        Item {
            visible: theme.activeTab === 0
            Layout.fillWidth: true
            Layout.fillHeight: true

            // Cylindrical logo reel capsule
            Rectangle {
                id: reel
                anchors.top: parent.top
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.leftMargin: 12
                anchors.rightMargin: 12
                height: theme.reelHeight
                radius: height / 2
                color: htpc.reelBgColor
                border.width: 1
                border.color: htpc.borderColor
                clip: true

                Repeater {
                    model: 11

                    Rectangle {
                        id: panel
                        readonly property int k: Math.round(theme.position) - 5 + index
                        readonly property real angle: (k - theme.position) * 0.42
                        readonly property real front: Math.cos(angle)
                        readonly property var game: theme.dependsOn(
                            [theme.version],
                            (theme.library && theme.total > 0) ? theme.library.item(theme.line(k)) : ({})
                        )
                        readonly property bool chosen: k === theme.cursor
                            && Math.abs(theme.position - theme.cursor) < 0.5

                        visible: theme.total > 0 && Math.abs(angle) < Math.PI / 2 * 0.94
                        z: 10 - Math.abs(angle)
                        width: Math.max(24, theme.reelHeight * 1.65 * front)
                        height: theme.reelHeight * (chosen ? 0.80 : 0.70)
                        x: reel.width / 2 + reel.width * 0.44 * Math.sin(angle) - width / 2
                        y: (reel.height - height) / 2
                        radius: 10
                        color: "#ececec"
                        opacity: 0.22 + 0.78 * front * front
                        border.width: chosen ? 3 : 0
                        border.color: htpc.accentColor

                        Image {
                            anchors.fill: parent
                            anchors.margins: 4
                            visible: panel.game.logo !== undefined && panel.game.logo !== ""
                            source: visible ? panel.game.logo : ""
                            fillMode: Image.PreserveAspectFit
                            smooth: true
                        }
                        Text {
                            anchors.fill: parent
                            anchors.margins: 6
                            visible: panel.game.logo === undefined || panel.game.logo === ""
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                            wrapMode: Text.Wrap
                            elide: Text.ElideRight
                            font.pixelSize: Math.max(11, Math.round(12 + 3 * panel.front))
                            font.bold: true
                            color: "#202428"
                            text: panel.game.title !== undefined ? panel.game.title : ""
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: {
                                if (panel.k === theme.cursor && theme.host)
                                    theme.host.launchGame(theme.line(theme.cursor))
                                else
                                    theme.cursor = panel.k
                            }
                        }
                    }
                }
            }

            // Selected game title
            Text {
                id: gameTitle
                anchors.top: reel.bottom
                anchors.topMargin: 12
                anchors.left: parent.left
                anchors.right: parent.right
                height: 44
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
                font.pixelSize: Math.min(34, Math.max(22, Math.round(theme.height * 0.042)))
                font.bold: true
                color: htpc.textColor
                text: theme.total > 0
                    ? (theme.currentGame.title !== undefined ? theme.currentGame.title : "")
                    : (theme.library ? theme.library.empty : "")
            }

            // 3D perspective box art stage
            Item {
                id: boxStage
                anchors.top: gameTitle.bottom
                anchors.topMargin: 6
                anchors.bottom: infoCard.top
                anchors.bottomMargin: 12
                anchors.left: parent.left
                anchors.right: parent.right
                clip: true

                Repeater {
                    model: 9

                    Item {
                        id: box
                        readonly property int k: Math.round(theme.position) - 4 + index
                        readonly property real d: k - theme.position
                        readonly property real zoom: 1.0 / (1.0 + 0.36 * Math.abs(d))
                        readonly property real baseHeight: Math.max(120, boxStage.height * 0.90)
                        readonly property var game: theme.dependsOn(
                            [theme.version],
                            (theme.library && theme.total > 0) ? theme.library.item(theme.line(k)) : ({})
                        )
                        readonly property real aspect: coverImg.sourceSize.height > 0
                            ? coverImg.sourceSize.width / coverImg.sourceSize.height : 0.77
                        readonly property real step: baseHeight * 0.68
                        readonly property bool chosen: Math.abs(d) < 0.5

                        visible: theme.total > 0 && Math.abs(d) < 3.6
                        z: 20 - Math.abs(d)
                        height: baseHeight * zoom
                        width: Math.min(height * aspect, boxStage.width * 0.42)
                        x: boxStage.width / 2 + step * (d < 0 ? -1 : 1) * Math.pow(Math.abs(d) * 1.32, 0.82) - width / 2
                        y: (boxStage.height - height) / 2
                        opacity: Math.max(0, Math.min(1, 1.0 - 0.26 * Math.abs(d)))

                        transform: Rotation {
                            origin.x: box.width / 2
                            origin.y: box.height / 2
                            axis { x: 0; y: 1; z: 0 }
                            angle: Math.max(-28, Math.min(28, -box.d * 14))
                        }

                        // Soft drop shadow
                        Rectangle {
                            x: -4
                            y: 10 * box.zoom
                            width: parent.width + 8
                            height: parent.height + 6
                            radius: 12
                            color: "#66000000"
                        }

                        // Accent glow halo behind the chosen box
                        Rectangle {
                            visible: box.chosen
                            anchors.fill: parent
                            anchors.margins: -6
                            radius: 14
                            color: htpc.accentGlowColor
                            opacity: 0.45
                        }

                        Rectangle {
                            anchors.fill: parent
                            radius: 10
                            color: htpc.surfaceSolidColor
                            clip: true

                            Image {
                                id: coverImg
                                anchors.fill: parent
                                source: box.game.cover !== undefined ? box.game.cover : ""
                                fillMode: Image.PreserveAspectCrop
                                smooth: !box.game.sharpCover
                                mipmap: !box.game.sharpCover
                            }
                        }

                        // Crisp focus border on chosen cover
                        Rectangle {
                            anchors.fill: parent
                            anchors.margins: -3
                            radius: 12
                            color: "transparent"
                            border.width: box.chosen ? 4 : 1
                            border.color: box.chosen ? htpc.accentColor : htpc.borderColor
                        }

                        MouseArea {
                            anchors.fill: parent
                            onClicked: {
                                if (box.k === theme.cursor && theme.host)
                                    theme.host.launchGame(theme.line(theme.cursor))
                                else
                                    theme.cursor = box.k
                            }
                        }
                    }
                }
            }

            // Bottom Info Card: Rating + Description + Index counter
            Rectangle {
                id: infoCard
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                height: theme.infoHeight
                radius: 14
                color: htpc.surfaceColor
                border.width: 1
                border.color: htpc.borderColor

                RowLayout {
                    anchors.fill: parent
                    anchors.margins: 14
                    spacing: 16

                    Image {
                        visible: theme.currentGame.rating !== undefined && theme.currentGame.rating !== ""
                        source: visible ? theme.currentGame.rating : ""
                        Layout.preferredHeight: Math.min(infoCard.height - 28, 64)
                        Layout.preferredWidth: Layout.preferredHeight
                        fillMode: Image.PreserveAspectFit
                        smooth: true
                    }

                    Text {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        verticalAlignment: Text.AlignVCenter
                        wrapMode: Text.Wrap
                        elide: Text.ElideRight
                        maximumLineCount: 3
                        font.pixelSize: 15
                        color: htpc.textColor
                        text: theme.currentGame.description !== undefined && theme.currentGame.description !== ""
                            ? theme.currentGame.description
                            : (theme.library ? theme.library.count : "")
                    }

                    Text {
                        visible: theme.total > 0
                        text: (theme.line(theme.cursor) + 1) + " / " + theme.total
                        color: htpc.mutedColor
                        font.pixelSize: 15
                        font.bold: true
                        font.family: "monospace"
                    }
                }
            }
        }

        // ---------------------------------------------------------------------
        // Tab 1: Z-Wheel System Card (when installed)
        // ---------------------------------------------------------------------
        Item {
            visible: theme.activeTab === 1
            Layout.fillWidth: true
            Layout.fillHeight: true

            Rectangle {
                anchors.centerIn: parent
                width: Math.min(parent.width - 80, 560)
                height: 280
                radius: 20
                color: htpc.surfaceColor
                border.width: 3
                border.color: htpc.accentColor

                ColumnLayout {
                    anchors.centerIn: parent
                    spacing: 16

                    Image {
                        source: "qrc:/zeebx/zeebx.png"
                        Layout.preferredWidth: 96
                        Layout.preferredHeight: 96
                        Layout.alignment: Qt.AlignHCenter
                        fillMode: Image.PreserveAspectFit
                        smooth: true
                    }

                    Text {
                        text: "Z-Wheel"
                        color: htpc.textColor
                        font.pixelSize: 30
                        font.bold: true
                        Layout.alignment: Qt.AlignHCenter
                    }

                    Rectangle {
                        radius: 12
                        color: htpc.accentColor
                        Layout.preferredHeight: 44
                        Layout.preferredWidth: launchZwLbl.implicitWidth + 44
                        Layout.alignment: Qt.AlignHCenter

                        Text {
                            id: launchZwLbl
                            anchors.centerIn: parent
                            text: htpc.promptOk + "  ·  " + theme.tr("htpc.prompt.play")
                            color: htpc.accentTextColor
                            font.pixelSize: 15
                            font.bold: true
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: if (theme.host) theme.host.launchZWheel()
                        }
                    }
                }
            }
        }

        // ---------------------------------------------------------------------
        // Bottom Controller Prompt Bar
        // ---------------------------------------------------------------------
        RowLayout {
            Layout.fillWidth: true
            spacing: 22

            Repeater {
                model: [
                    { glyph: htpc.promptOk, label: theme.tr("htpc.prompt.play"), primary: true, show: true },
                    { glyph: htpc.promptY, label: theme.tr("htpc.prompt.details"), primary: false, show: theme.activeTab === 0 && theme.total > 0 },
                    { glyph: htpc.promptStart, label: theme.tr("htpc.prompt.settings"), primary: false, show: true },
                    { glyph: htpc.promptSelect, label: theme.tr("htpc.prompt.zwheel"), primary: false, show: !!(theme.library && theme.library.hasZWheel()) },
                ]

                RowLayout {
                    visible: modelData.show
                    spacing: 8

                    Rectangle {
                        radius: 6
                        color: modelData.primary ? htpc.accentColor : htpc.surfaceSolidColor
                        border.width: modelData.primary ? 0 : 1
                        border.color: htpc.borderColor
                        Layout.preferredHeight: 26
                        Layout.preferredWidth: Math.max(28, glyphTxt.implicitWidth + 14)

                        Text {
                            id: glyphTxt
                            anchors.centerIn: parent
                            text: modelData.glyph
                            color: modelData.primary ? htpc.accentTextColor : htpc.textColor
                            font.pixelSize: 12
                            font.bold: true
                        }
                    }

                    Text {
                        text: modelData.label
                        color: htpc.textColor
                        font.pixelSize: 14
                        font.bold: modelData.primary
                    }
                }
            }

            Item { Layout.fillWidth: true }

            Text {
                text: theme.tr("htpc.prompt.quick_menu") + ": " + htpc.promptQuick
                color: htpc.mutedColor
                font.pixelSize: 13
            }
        }
    }
}
