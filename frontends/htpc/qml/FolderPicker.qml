import QtQuick
import QtQuick.Layouts

import zeebx
import zeebx.htpc

// 100% gamepad-navigable directory picker modal (replaces OS mouse dialogs like `rfd`
// which break inside Steam Big Picture / Gamescope).
Rectangle {
    id: picker

    required property var browser
    required property var host

    property int cursor: 0

    color: "#cc000000"

    function dependsOn(versions, value) {
        return value
    }

    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    function reset() {
        cursor = 0
    }

    function handleCommand(code) {
        const count = browser.entries.length
        if (code === 0 && count > 0) {
            cursor = (cursor - 1 + count) % count
            listView.positionViewAtIndex(cursor, ListView.Contain)
        } else if (code === 1 && count > 0) {
            cursor = (cursor + 1) % count
            listView.positionViewAtIndex(cursor, ListView.Contain)
        } else if (code === 4 && count > 0) {
            browser.enter(cursor)
            cursor = 0
        } else if (code === 6 || code === 10) {
            // Action X or Start confirms the current folder
            host.closeFolderPicker(browser.currentPath)
        } else if (code === 8) {
            // L1 goes up one level
            browser.goUp()
            cursor = 0
        } else if (code === 5) {
            host.closeFolderPicker("")
        }
    }

    MouseArea { anchors.fill: parent }

    Rectangle {
        anchors.centerIn: parent
        width: Math.min(parent.width - 80, 760)
        height: Math.min(parent.height - 80, 560)
        radius: 18
        color: HtpcController.surfaceSolidColor
        border.width: 2
        border.color: HtpcController.accentColor

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 26
            spacing: 14

            RowLayout {
                Layout.fillWidth: true
                spacing: 12

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 4

                    Text {
                        text: picker.tr("htpc.folder.title")
                        color: HtpcController.textColor
                        font.pixelSize: 22
                        font.bold: true
                    }

                    Rectangle {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 34
                        radius: 8
                        color: HtpcController.bgColor

                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 12
                            anchors.rightMargin: 12

                            Text {
                                text: browser.currentPath
                                color: HtpcController.mutedColor
                                font.pixelSize: 13
                                font.family: "monospace"
                                elide: Text.ElideLeft
                                Layout.fillWidth: true
                            }

                            Text {
                                visible: browser.romCount > 0
                                text: browser.romCount + " ROM(s)"
                                color: HtpcController.accentColor
                                font.pixelSize: 12
                                font.bold: true
                            }
                        }
                    }
                }

                Rectangle {
                    radius: 10
                    color: HtpcController.accentColor
                    Layout.preferredHeight: 44
                    Layout.preferredWidth: useBtnLbl.implicitWidth + 28

                    Text {
                        id: useBtnLbl
                        anchors.centerIn: parent
                        text: HtpcController.promptX + "  ·  " + picker.tr("htpc.prompt.choose_folder")
                        color: HtpcController.accentTextColor
                        font.pixelSize: 13
                        font.bold: true
                    }
                    MouseArea {
                        anchors.fill: parent
                        onClicked: host.closeFolderPicker(browser.currentPath)
                    }
                }
            }

            ListView {
                id: listView
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                spacing: 6
                model: browser.entries

                delegate: Rectangle {
                    readonly property bool focused: index === picker.cursor
                    width: listView.width
                    height: 46
                    radius: 10
                    color: focused ? HtpcController.accentSoftColor : HtpcController.bgAltColor
                    border.width: focused ? 2 : 1
                    border.color: focused ? HtpcController.accentColor : HtpcController.borderColor

                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.left: parent.left
                        anchors.leftMargin: 16
                        anchors.right: parent.right
                        anchors.rightMargin: 16
                        elide: Text.ElideRight
                        text: modelData
                        color: HtpcController.textColor
                        font.pixelSize: 15
                        font.bold: focused
                    }

                    MouseArea {
                        anchors.fill: parent
                        onClicked: {
                            browser.enter(index)
                            picker.cursor = 0
                        }
                    }
                }
            }

            Text {
                visible: browser.entries.length === 0
                text: picker.tr("htpc.folder.empty")
                color: HtpcController.mutedColor
                font.pixelSize: 14
            }

            Text {
                text: HtpcController.promptOk + " " + picker.tr("htpc.prompt.open_folder")
                    + "    " + HtpcController.promptX + " " + picker.tr("htpc.prompt.choose_folder")
                    + "    " + HtpcController.promptL1 + " " + picker.tr("htpc.prompt.parent_folder")
                    + "    " + HtpcController.promptBack + " " + picker.tr("htpc.prompt.back")
                color: HtpcController.mutedColor
                font.pixelSize: 13
                font.bold: true
            }
        }
    }
}
