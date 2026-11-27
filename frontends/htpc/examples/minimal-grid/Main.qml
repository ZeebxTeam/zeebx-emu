import QtQuick
import QtQuick.Layouts

import zeebx
import zeebx.htpc

// Example community HTPC theme: copy this folder to `<config>/htpc-themes/minimal-grid/`
// and select it in Settings -> Appearance & Themes -> Interface theme.
Item {
    id: gridTheme

    property var library: null
    property var htpc: HtpcController
    property var host: null

    property int cursor: 0
    property int total: 0
    property int cols: 4

    function dependsOn(versions, value) { return value }
    function tr(keyId) { return dependsOn([Language.version], Language.text(keyId)) }

    function refreshModel() {
        if (!library)
            return
        total = library.lines()
        if (cursor >= total)
            cursor = Math.max(0, total - 1)
    }

    onLibraryChanged: refreshModel()

    Connections {
        target: gridTheme.library
        ignoreUnknownSignals: true
        function onModelReset() { gridTheme.refreshModel() }
    }

    function handleCommand(code) {
        if (total <= 0)
            return
        if (code === 2) cursor = (cursor - 1 + total) % total
        else if (code === 3) cursor = (cursor + 1) % total
        else if (code === 0) cursor = Math.max(0, cursor - cols)
        else if (code === 1) cursor = Math.min(total - 1, cursor + cols)
        else if (code === 4 && host) host.launchGame(cursor)
        else if (code === 7 && host) host.openDetails(cursor)
        gridView.positionViewAtIndex(cursor, GridView.Contain)
    }

    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop { position: 0.0; color: htpc.bgColor }
            GradientStop { position: 1.0; color: htpc.bgAltColor }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 28
        spacing: 16

        RowLayout {
            Layout.fillWidth: true
            Text {
                text: "ZEEBX HTPC  ·  " + (library ? library.count : "")
                color: htpc.textColor
                font.pixelSize: 22
                font.bold: true
            }
            Item { Layout.fillWidth: true }
            Text {
                text: htpc.controllerStatus
                color: htpc.mutedColor
                font.pixelSize: 14
            }
        }

        GridView {
            id: gridView
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: gridTheme.library
            cellWidth: Math.floor(width / gridTheme.cols)
            cellHeight: Math.round(cellWidth * 1.15)

            delegate: Item {
                readonly property bool focused: index === gridTheme.cursor
                width: gridView.cellWidth
                height: gridView.cellHeight

                Rectangle {
                    anchors.fill: parent
                    anchors.margins: 10
                    radius: 12
                    color: htpc.surfaceSolidColor
                    border.width: focused ? 4 : 1
                    border.color: focused ? htpc.accentColor : htpc.borderColor
                    clip: true

                    Image {
                        anchors.fill: parent
                        source: cover
                        fillMode: Image.PreserveAspectCrop
                        smooth: !sharpCover
                    }

                    Rectangle {
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.bottom: parent.bottom
                        height: 36
                        color: focused ? htpc.accentColor : "#cc000000"

                        Text {
                            anchors.centerIn: parent
                            width: parent.width - 16
                            elide: Text.ElideRight
                            horizontalAlignment: Text.AlignHCenter
                            text: title
                            color: focused ? htpc.accentTextColor : "#ffffff"
                            font.pixelSize: 13
                            font.bold: true
                        }
                    }

                    MouseArea {
                        anchors.fill: parent
                        onClicked: {
                            if (gridTheme.cursor === index && gridTheme.host)
                                gridTheme.host.launchGame(index)
                            else
                                gridTheme.cursor = index
                        }
                    }
                }
            }
        }

        Text {
            text: htpc.promptOk + " " + gridTheme.tr("htpc.prompt.play")
                + "    " + htpc.promptY + " " + gridTheme.tr("htpc.prompt.details")
                + "    " + htpc.promptStart + " " + gridTheme.tr("htpc.prompt.settings")
            color: htpc.mutedColor
            font.pixelSize: 14
            font.bold: true
        }
    }
}
