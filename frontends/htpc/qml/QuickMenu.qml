import QtQuick
import QtQuick.Layouts

import zeebx
import zeebx.htpc

// In-game Quick Menu overlay (translucent side drawer over the paused game).
// Supports Resume, visual Save States grid with live RGB565 thumbnails, live Z-Pad <-> Boomerang
// toggle, live Scaling cycle, Screenshot, Restart, Undo Load, Return to Library and Quit Zeebx.
Item {
    id: qm

    required property var screen
    required property var host

    // "main" | "states"
    property string subview: "main"
    property int menuIndex: 0
    property int slotIndex: 0
    property var slotLabels: []
    property var slotOccupied: []
    property string deviceLabel: ""
    property string scalingLabel: ""
    property bool canUndo: false

    function dependsOn(versions, value) {
        return value
    }

    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    function refreshMeta() {
        deviceLabel = HtpcController.port0DeviceLabel()
        scalingLabel = HtpcController.scalingLabel()
        if (screen) {
            slotLabels = screen.stateSlots()
            slotOccupied = screen.occupiedSlots()
            canUndo = screen.canUndoLoad()
        }
    }

    function reset() {
        subview = "main"
        menuIndex = 0
        slotIndex = screen ? Math.max(0, Math.min(5, screen.currentSlot())) : 0
        refreshMeta()
    }

    function mainItems() {
        const items = [
            { id: "resume", label: tr("htpc.quick.resume") },
            { id: "states", label: tr("htpc.quick.save_states") },
            { id: "device", label: tr("htpc.quick.device").replace("{device}", deviceLabel) },
            { id: "scaling", label: tr("htpc.quick.scaling").replace("{scaling}", scalingLabel) },
            { id: "screenshot", label: tr("htpc.quick.screenshot") },
            { id: "restart", label: tr("htpc.quick.restart") },
        ]
        if (canUndo)
            items.push({ id: "undo", label: tr("htpc.quick.undo_load") })
        items.push({ id: "library", label: tr("htpc.quick.return_library") })
        items.push({ id: "quit", label: tr("htpc.quick.quit_app") })
        return items
    }

    function activateMain(idx) {
        const items = mainItems()
        if (idx < 0 || idx >= items.length)
            return
        menuIndex = idx
        const id = items[idx].id
        if (id === "resume") {
            host.closeQuickMenu()
        } else if (id === "states") {
            refreshMeta()
            subview = "states"
        } else if (id === "device") {
            deviceLabel = HtpcController.togglePort0Device()
            host.showToast(tr("htpc.quick.device").replace("{device}", deviceLabel), false)
        } else if (id === "scaling") {
            scalingLabel = HtpcController.cycleScaling()
            host.showToast(tr("htpc.quick.scaling").replace("{scaling}", scalingLabel), false)
        } else if (id === "screenshot") {
            screen.screenshot(Qt.formatDateTime(new Date(), "yyyy-MM-dd_HH-mm-ss"))
            host.closeQuickMenu()
        } else if (id === "restart") {
            screen.restart()
            host.closeQuickMenu()
        } else if (id === "undo") {
            screen.stateFromMenu(2, slotIndex)
            HtpcController.bumpSlotSerial()
            host.closeQuickMenu()
        } else if (id === "library") {
            host.stopGameAndReturn()
        } else if (id === "quit") {
            screen.close()
            Qt.quit()
        }
    }

    function saveSlot(slot) {
        screen.stateFromMenu(0, slot)
        HtpcController.bumpSlotSerial()
        refreshMeta()
    }

    function loadSlot(slot) {
        screen.stateFromMenu(1, slot)
        HtpcController.bumpSlotSerial()
        host.closeQuickMenu()
    }

    function handleCommand(code) {
        if (subview === "main") {
            const count = mainItems().length
            if (code === 0) menuIndex = (menuIndex - 1 + count) % count
            else if (code === 1) menuIndex = (menuIndex + 1) % count
            else if (code === 4) activateMain(menuIndex)
            else if (code === 5) host.closeQuickMenu()
        } else {
            // 6 slots in a 3x2 grid
            if (code === 2) slotIndex = (slotIndex + 5) % 6
            else if (code === 3) slotIndex = (slotIndex + 1) % 6
            else if (code === 0 || code === 1) slotIndex = (slotIndex + 3) % 6
            else if (code === 4) saveSlot(slotIndex)
            else if (code === 6) loadSlot(slotIndex)
            else if (code === 5) subview = "main"
        }
    }

    // Dimmed backdrop over paused game (click right side to resume)
    Rectangle {
        anchors.fill: parent
        color: "#aa000000"

        MouseArea {
            anchors.fill: parent
            onClicked: host.closeQuickMenu()
        }
    }

    // Left glass drawer
    Rectangle {
        id: drawer
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        anchors.left: parent.left
        width: qm.subview === "states"
            ? Math.min(parent.width - 48, 700)
            : Math.min(parent.width - 48, 440)
        color: HtpcController.surfaceSolidColor
        border.width: 1
        border.color: HtpcController.borderColor

        Behavior on width {
            NumberAnimation { duration: 180; easing.type: Easing.OutCubic }
        }

        // Prevent clicks inside drawer from hitting backdrop
        MouseArea { anchors.fill: parent }

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 28
            spacing: 18

            // Active game header
            RowLayout {
                Layout.fillWidth: true
                spacing: 14

                Rectangle {
                    visible: HtpcController.activeGameCover !== ""
                    width: 56
                    height: 72
                    radius: 8
                    color: HtpcController.bgColor
                    clip: true
                    border.width: 1
                    border.color: HtpcController.borderColor

                    Image {
                        anchors.fill: parent
                        source: HtpcController.activeGameCover
                        fillMode: Image.PreserveAspectCrop
                        smooth: true
                    }
                }

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 4

                    Text {
                        text: qm.tr("htpc.quick.title") + "  ·  " + qm.tr("htpc.quick.paused")
                        color: HtpcController.accentColor
                        font.pixelSize: 12
                        font.bold: true
                    }

                    Text {
                        text: HtpcController.activeGameTitle
                        color: HtpcController.textColor
                        font.pixelSize: 20
                        font.bold: true
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                }
            }

            Rectangle {
                Layout.fillWidth: true
                height: 1
                color: HtpcController.borderColor
            }

            // Subview 1: Main Quick Menu actions
            ColumnLayout {
                visible: qm.subview === "main"
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 8

                Repeater {
                    model: qm.dependsOn([Language.version, qm.deviceLabel, qm.scalingLabel, qm.canUndo], qm.mainItems())

                    Rectangle {
                        readonly property bool focused: index === qm.menuIndex
                        Layout.fillWidth: true
                        Layout.preferredHeight: 48
                        radius: 10
                        color: focused ? HtpcController.accentColor : "transparent"
                        border.width: focused ? 0 : 1
                        border.color: focused ? "transparent" : HtpcController.borderColor

                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 16
                            anchors.rightMargin: 16

                            Text {
                                text: modelData.label
                                color: focused ? HtpcController.accentTextColor : HtpcController.textColor
                                font.pixelSize: 16
                                font.bold: focused
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }

                            Text {
                                visible: focused
                                text: "›"
                                color: HtpcController.accentTextColor
                                font.pixelSize: 20
                                font.bold: true
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            onClicked: qm.activateMain(index)
                        }
                    }
                }

                Item { Layout.fillHeight: true }
            }

            // Subview 2: Save States visual grid (6 slots with live RGB565 thumbnails)
            ColumnLayout {
                visible: qm.subview === "states"
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 14

                Text {
                    text: qm.tr("htpc.states.title")
                    color: HtpcController.textColor
                    font.pixelSize: 18
                    font.bold: true
                }

                GridLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    columns: 3
                    rowSpacing: 14
                    columnSpacing: 14

                    Repeater {
                        model: 6

                        Rectangle {
                            readonly property bool focused: index === qm.slotIndex
                            readonly property bool occupied: qm.slotOccupied.length > index && qm.slotOccupied[index] === 1
                            readonly property string slotText: qm.slotLabels.length > index
                                ? qm.slotLabels[index] : ("Slot " + (index + 1))

                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            radius: 12
                            color: focused ? HtpcController.accentSoftColor : HtpcController.bgAltColor
                            border.width: focused ? 3 : 1
                            border.color: focused ? HtpcController.accentColor : HtpcController.borderColor

                            ColumnLayout {
                                anchors.fill: parent
                                anchors.margins: 10
                                spacing: 8

                                Rectangle {
                                    Layout.fillWidth: true
                                    Layout.fillHeight: true
                                    radius: 8
                                    color: HtpcController.bgColor
                                    clip: true

                                    Image {
                                        anchors.fill: parent
                                        visible: occupied
                                        source: occupied
                                            ? ("image://zeebx/slot/" + HtpcController.slotSerial + "/" + index)
                                            : ""
                                        fillMode: Image.PreserveAspectCrop
                                        smooth: true
                                    }

                                    Text {
                                        anchors.centerIn: parent
                                        visible: !occupied
                                        text: qm.tr("htpc.states.empty")
                                        color: HtpcController.mutedColor
                                        font.pixelSize: 13
                                    }

                                    Rectangle {
                                        anchors.top: parent.top
                                        anchors.left: parent.left
                                        anchors.margins: 6
                                        radius: 5
                                        color: "#b8000000"
                                        width: slotNum.implicitWidth + 12
                                        height: 20

                                        Text {
                                            id: slotNum
                                            anchors.centerIn: parent
                                            text: "#" + (index + 1)
                                            color: "#ffffff"
                                            font.pixelSize: 11
                                            font.bold: true
                                        }
                                    }
                                }

                                Text {
                                    text: slotText
                                    color: HtpcController.textColor
                                    font.pixelSize: 12
                                    font.bold: focused
                                    elide: Text.ElideRight
                                    Layout.fillWidth: true
                                }
                            }

                            MouseArea {
                                anchors.fill: parent
                                onClicked: {
                                    qm.slotIndex = index
                                    if (occupied)
                                        qm.loadSlot(index)
                                    else
                                        qm.saveSlot(index)
                                }
                            }
                        }
                    }
                }
            }

            // Bottom prompts
            Rectangle {
                Layout.fillWidth: true
                height: 1
                color: HtpcController.borderColor
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 18

                Text {
                    text: qm.subview === "main"
                        ? (HtpcController.promptOk + " " + qm.tr("htpc.prompt.select")
                           + "    " + HtpcController.promptBack + " " + qm.tr("htpc.quick.resume"))
                        : (HtpcController.promptOk + " " + qm.tr("htpc.prompt.save")
                           + "    " + HtpcController.promptX + " " + qm.tr("htpc.prompt.load")
                           + "    " + HtpcController.promptBack + " " + qm.tr("htpc.prompt.back"))
                    color: HtpcController.mutedColor
                    font.pixelSize: 13
                    font.bold: true
                }
            }
        }
    }
}
