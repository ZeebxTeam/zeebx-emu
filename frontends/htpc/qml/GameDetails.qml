import QtQuick
import QtQuick.Layouts

import zeebx
import zeebx.htpc

// 10-foot game details modal (opened with Action Y on a game in the library).
Rectangle {
    id: modal

    property var details: ({})
    property int line: -1
    required property var host

    color: "#cc000000"

    function dependsOn(versions, value) {
        return value
    }

    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    function reset() {}

    function handleCommand(code) {
        if (code === 4 && line >= 0) {
            host.closeDetails()
            host.launchGame(line)
        } else if (code === 5 || code === 7) {
            host.closeDetails()
        }
    }

    MouseArea {
        anchors.fill: parent
        onClicked: host.closeDetails()
    }

    Rectangle {
        anchors.centerIn: parent
        width: Math.min(parent.width - 80, 820)
        height: Math.min(parent.height - 80, 480)
        radius: 18
        color: HtpcController.surfaceSolidColor
        border.width: 2
        border.color: HtpcController.accentColor

        MouseArea { anchors.fill: parent }

        RowLayout {
            anchors.fill: parent
            anchors.margins: 28
            spacing: 24

            Rectangle {
                Layout.preferredWidth: 230
                Layout.fillHeight: true
                radius: 12
                color: HtpcController.bgColor
                clip: true
                border.width: 1
                border.color: HtpcController.borderColor

                Image {
                    anchors.fill: parent
                    source: details.cover !== undefined ? details.cover : ""
                    fillMode: Image.PreserveAspectCrop
                    smooth: !details.sharpCover
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 12

                Text {
                    text: details.title !== undefined ? details.title : ""
                    color: HtpcController.textColor
                    font.pixelSize: 24
                    font.bold: true
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }

                Text {
                    text: details.description !== undefined ? details.description : ""
                    color: HtpcController.mutedColor
                    font.pixelSize: 15
                    wrapMode: Text.Wrap
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                }

                Rectangle {
                    Layout.fillWidth: true
                    height: 1
                    color: HtpcController.borderColor
                }

                ColumnLayout {
                    spacing: 4
                    Text {
                        visible: details.classId !== undefined && details.classId !== ""
                        text: "ClassID: " + details.classId + "   ·   " + (details.size || "")
                        color: HtpcController.mutedColor
                        font.pixelSize: 13
                        font.family: "monospace"
                    }
                    Text {
                        visible: details.path !== undefined && details.path !== ""
                        text: details.path || ""
                        color: HtpcController.mutedColor
                        font.pixelSize: 12
                        font.family: "monospace"
                        elide: Text.ElideMiddle
                        Layout.fillWidth: true
                    }
                }

                RowLayout {
                    spacing: 14
                    Layout.topMargin: 6

                    Rectangle {
                        radius: 10
                        color: HtpcController.accentColor
                        Layout.preferredHeight: 44
                        Layout.preferredWidth: playLbl.implicitWidth + 36

                        Text {
                            id: playLbl
                            anchors.centerIn: parent
                            text: HtpcController.promptOk + "  ·  " + modal.tr("htpc.prompt.play")
                            color: HtpcController.accentTextColor
                            font.pixelSize: 15
                            font.bold: true
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: {
                                host.closeDetails()
                                host.launchGame(modal.line)
                            }
                        }
                    }

                    Rectangle {
                        radius: 10
                        color: "transparent"
                        border.width: 1
                        border.color: HtpcController.borderColor
                        Layout.preferredHeight: 44
                        Layout.preferredWidth: closeLbl.implicitWidth + 32

                        Text {
                            id: closeLbl
                            anchors.centerIn: parent
                            text: HtpcController.promptBack + "  ·  " + modal.tr("htpc.prompt.back")
                            color: HtpcController.textColor
                            font.pixelSize: 14
                            font.bold: true
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: host.closeDetails()
                        }
                    }
                }
            }
        }
    }
}
