import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import zeebx

// The save manager: a system window, as in egui. Two lists, and the split is the console's: what
// the game wrote in its own folder, and what is in the device's file system — which belongs to
// everyone. Deleting `zeeboiddata` removes the Zeeboids characters **and** what Zeebo F.C. reads
// from them; the hint under the title says so, because the list alone would not.
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

    width: 520
    height: 420
    minimumWidth: 380
    minimumHeight: 260
    title: "Zeebx — " + tr("saves.title")

    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    // The list touches the disk: it is reread when the window opens and after each deletion, not
    // on every draw.
    function open() {
        message.text = ""
        saves.reload()
        show()
        raise()
        requestActivate()
    }

    onClosing: {
        confirmation.close()
        message.text = ""
    }

    Saves {
        id: saves
    }

    // The indices of the saves in each list, in the order they came.
    function indices(fromDevice) {
        const list = []
        const count = dependsOn([saves.version], saves.count())
        for (let i = 0; i < count; i++) {
            if (saves.belongsToDevice(i) === fromDevice)
                list.push(i)
        }
        return list
    }
    readonly property var gameSaves: indices(false)
    readonly property var fromDevice: indices(true)

    // A row of the list: what it is, how much space it takes, and the delete button.
    component Line: RowLayout {
        id: line

        required property int modelData

        width: ListView.view ? ListView.view.width : parent.width

        Label {
            Layout.fillWidth: true
            elide: Text.ElideRight
            text: window.dependsOn([saves.version], saves.title(line.modelData))
        }
        Label {
            text: window.dependsOn([saves.version, Language.version], saves.summary(line.modelData))
        }
        Button {
            text: window.tr("saves.delete")
            onClicked: confirmation.ask(line.modelData)
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 8
        spacing: 6

        // What happened in the last deletion.
        RowLayout {
            Layout.fillWidth: true
            visible: message.text !== ""

            Label {
                id: message

                Layout.fillWidth: true
                wrapMode: Text.Wrap
            }
            Button {
                text: window.tr("saves.confirm.no")
                onClicked: message.text = ""
            }
        }

        Label {
            Layout.fillWidth: true
            visible: window.gameSaves.length === 0 && window.fromDevice.length === 0
            wrapMode: Text.Wrap
            text: window.tr("saves.none")
        }

        ScrollView {
            id: scroll

            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth

            ColumnLayout {
                width: scroll.availableWidth
                spacing: 4

                Label {
                    visible: window.gameSaves.length > 0
                    font.bold: true
                    font.pixelSize: 18
                    text: window.tr("saves.games")
                }
                Repeater {
                    model: window.gameSaves
                    delegate: Line {
                        Layout.fillWidth: true
                    }
                }

                Label {
                    Layout.topMargin: window.gameSaves.length > 0 ? 12 : 0
                    visible: window.fromDevice.length > 0
                    font.bold: true
                    font.pixelSize: 18
                    text: window.tr("saves.device")
                }
                Label {
                    Layout.fillWidth: true
                    visible: window.fromDevice.length > 0
                    wrapMode: Text.Wrap
                    text: window.tr("saves.device.hint")
                }
                Repeater {
                    model: window.fromDevice
                    delegate: Line {
                        Layout.fillWidth: true
                    }
                }
            }
        }
    }

    // The confirmation is modal on purpose: deleting cannot be undone, and a wrong click on a list
    // button is far too easy.
    Dialog {
        id: confirmation

        property int slotIndex: -1

        function ask(slotIndex) {
            confirmation.slotIndex = slotIndex
            open()
        }

        anchors.centerIn: parent
        width: Math.min(420, window.width - 32)
        modal: true
        title: window.tr("saves.title")

        Label {
            width: parent.width
            wrapMode: Text.Wrap
            text: confirmation.slotIndex < 0 ? ""
                : window.dependsOn([saves.version, Language.version], saves.question(confirmation.slotIndex))
        }

        footer: DialogButtonBox {
            Button {
                text: window.tr("saves.confirm.yes")
                DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            }
            Button {
                text: window.tr("saves.confirm.no")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
        }

        onAccepted: message.text = saves.remove(slotIndex)
    }
}
