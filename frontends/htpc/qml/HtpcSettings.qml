import QtQuick
import QtQuick.Layouts

import zeebx
import zeebx.htpc

// 100% gamepad-driven 2-column HTPC settings screen.
// Shares settings.json with zeebx-qt and adds HTPC theme selection, per-theme palette & accent
// colors, button prompts, Quick Menu combo, TV overscan and in-app ROMs folder picker.
Rectangle {
    id: settingsView

    required property var coreSettings
    required property var library
    required property var host

    property int categoryIndex: 0
    property int rowIndex: 0
    property int revision: 0

    color: HtpcController.bgColor

    function dependsOn(versions, value) {
        return value
    }

    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    function reset() {
        categoryIndex = 0
        rowIndex = 0
        revision += 1
    }

    function categories() {
        return [
            { id: "appearance", label: tr("htpc.settings.cat.appearance") },
            { id: "library", label: tr("htpc.settings.cat.library") },
            { id: "video", label: tr("htpc.settings.cat.video") },
            { id: "audio", label: tr("htpc.settings.cat.audio") },
            { id: "controls", label: tr("htpc.settings.cat.controls") },
            { id: "system", label: tr("htpc.settings.cat.system") },
        ]
    }

    function safeLabel(list, idx) {
        if (!list || list.length === 0)
            return "—"
        const i = Math.max(0, Math.min(list.length - 1, idx))
        return list[i]
    }

    function rowsForCategory(catIdx) {
        if (catIdx === 0) {
            // Appearance & HTPC Themes (with per-theme color palette and accent!)
            const tNames = HtpcController.themeNames()
            const pLabels = HtpcController.paletteLabels()
            const aLabels = HtpcController.accentLabels()
            const prLabels = HtpcController.promptLabels()
            return [
                {
                    id: "htpc.theme",
                    label: tr("htpc.settings.theme"),
                    value: safeLabel(tNames, HtpcController.currentThemeIndex()),
                    kind: "cycle",
                    count: tNames.length,
                    index: HtpcController.currentThemeIndex(),
                },
                {
                    id: "htpc.palette",
                    label: tr("htpc.settings.palette"),
                    value: safeLabel(pLabels, HtpcController.currentPaletteIndex()),
                    kind: "cycle",
                    count: pLabels.length,
                    index: HtpcController.currentPaletteIndex(),
                },
                {
                    id: "htpc.accent",
                    label: tr("htpc.settings.accent"),
                    value: safeLabel(aLabels, HtpcController.currentAccentIndex()),
                    kind: "cycle",
                    count: aLabels.length,
                    index: HtpcController.currentAccentIndex(),
                },
                {
                    id: "htpc.prompts",
                    label: tr("htpc.settings.prompts"),
                    value: safeLabel(prLabels, HtpcController.currentPromptIndex()),
                    kind: "cycle",
                    count: prLabels.length,
                    index: HtpcController.currentPromptIndex(),
                },
                {
                    id: "htpc.overscan",
                    label: tr("htpc.settings.overscan"),
                    value: HtpcController.overscanPercent + "%",
                    kind: "cycle",
                    count: 9,
                    index: HtpcController.overscanPercent,
                },
                {
                    id: "htpc.reload_themes",
                    label: tr("htpc.settings.theme_reload"),
                    value: HtpcController.themesDir(),
                    kind: "action",
                },
            ]
        }
        if (catIdx === 1) {
            // Library & Folders
            const roms = HtpcController.romsDir()
            return [
                {
                    id: "library.roms_dir",
                    label: tr("htpc.settings.roms_dir"),
                    value: roms !== "" ? roms : "—",
                    kind: "action",
                },
                {
                    id: "library.rescan",
                    label: tr("htpc.settings.rescan_library"),
                    value: library ? library.count : "",
                    kind: "action",
                },
                {
                    id: "game_names_from_file",
                    label: tr("settings.game_names_from_file"),
                    value: coreSettings.value("game_names_from_file") ? tr("controls.port.on") : tr("common.off"),
                    kind: "core_bool",
                },
            ]
        }
        if (catIdx === 2) {
            // Video & TV
            const scaleOpts = coreSettings.options("graphics.scaling")
            const resOpts = coreSettings.options("graphics.resolucao_interna")
            const aspectOpts = coreSettings.options("graphics.proporcao")
            return [
                {
                    id: "graphics.gpu_rasterizer",
                    label: tr("graphics.gpu_rasterizer"),
                    value: coreSettings.value("graphics.gpu_rasterizer") ? tr("controls.port.on") : tr("common.off"),
                    kind: "core_bool",
                },
                {
                    id: "graphics.resolucao_interna",
                    label: tr("graphics.internal_resolution"),
                    value: safeLabel(resOpts, coreSettings.value("graphics.resolucao_interna")),
                    kind: "core_cycle",
                    count: resOpts.length,
                    index: coreSettings.value("graphics.resolucao_interna"),
                },
                {
                    id: "graphics.scaling",
                    label: tr("graphics.scaling"),
                    value: safeLabel(scaleOpts, coreSettings.value("graphics.scaling")),
                    kind: "core_cycle",
                    count: scaleOpts.length,
                    index: coreSettings.value("graphics.scaling"),
                },
                {
                    id: "graphics.proporcao",
                    label: tr("graphics.aspect"),
                    value: safeLabel(aspectOpts, coreSettings.value("graphics.proporcao")),
                    kind: "core_cycle",
                    count: aspectOpts.length,
                    index: coreSettings.value("graphics.proporcao"),
                },
                {
                    id: "graphics.smooth",
                    label: tr("graphics.smooth"),
                    value: coreSettings.value("graphics.smooth") ? tr("controls.port.on") : tr("common.off"),
                    kind: "core_bool",
                },
                {
                    id: "graphics.neblina",
                    label: tr("graphics.fog"),
                    value: coreSettings.value("graphics.neblina") ? tr("controls.port.on") : tr("common.off"),
                    kind: "core_bool",
                },
                {
                    id: "debug.metricas",
                    label: tr("debug.metricas"),
                    value: coreSettings.value("debug.metricas") ? tr("controls.port.on") : tr("common.off"),
                    kind: "core_bool",
                },
            ]
        }
        if (catIdx === 3) {
            // Audio
            const vol = Math.round(coreSettings.value("audio.volume"))
            return [
                {
                    id: "audio.enabled",
                    label: tr("audio.enabled"),
                    value: coreSettings.value("audio.enabled") ? tr("controls.port.on") : tr("common.off"),
                    kind: "core_bool",
                },
                {
                    id: "audio.volume",
                    label: tr("audio.volume"),
                    value: vol + "%",
                    kind: "volume",
                    index: vol,
                },
                {
                    id: "audio.midi_effects",
                    label: tr("audio.midi_effects"),
                    value: coreSettings.value("audio.midi_effects") ? tr("controls.port.on") : tr("common.off"),
                    kind: "core_bool",
                },
            ]
        }
        if (catIdx === 4) {
            // Controllers & HTPC
            const qLabels = HtpcController.quickComboLabels()
            return [
                {
                    id: "htpc.quick_combo",
                    label: tr("htpc.settings.quick_combo"),
                    value: safeLabel(qLabels, HtpcController.currentQuickComboIndex()),
                    kind: "cycle",
                    count: qLabels.length,
                    index: HtpcController.currentQuickComboIndex(),
                },
                {
                    id: "htpc.port0_device",
                    label: tr("controls.kind"),
                    value: HtpcController.port0DeviceLabel(),
                    kind: "action",
                },
                {
                    id: "htpc.dpad_to_axes",
                    label: tr("controls.dpad_to_analog"),
                    value: HtpcController.port0DpadToAxes() ? tr("controls.port.on") : tr("common.off"),
                    kind: "action",
                },
            ]
        }
        // System
        const langOpts = coreSettings.options("language")
        return [
            {
                id: "language",
                label: tr("settings.language"),
                value: safeLabel(langOpts, coreSettings.value("language")),
                kind: "core_cycle",
                count: langOpts.length,
                index: coreSettings.value("language"),
            },
            {
                id: "htpc.quit",
                label: tr("htpc.settings.quit"),
                value: "",
                kind: "action",
            },
        ]
    }

    function applyRow(idx, dir) {
        const rows = rowsForCategory(categoryIndex)
        if (idx < 0 || idx >= rows.length)
            return
        rowIndex = idx
        const row = rows[idx]
        const step = dir < 0 ? -1 : 1

        if (row.id === "htpc.theme" && row.count > 0) {
            HtpcController.selectTheme((row.index + step + row.count) % row.count)
        } else if (row.id === "htpc.palette" && row.count > 0) {
            HtpcController.selectPalette((row.index + step + row.count) % row.count)
        } else if (row.id === "htpc.accent" && row.count > 0) {
            HtpcController.selectAccent((row.index + step + row.count) % row.count)
        } else if (row.id === "htpc.prompts" && row.count > 0) {
            HtpcController.selectPrompt((row.index + step + row.count) % row.count)
        } else if (row.id === "htpc.overscan") {
            const next = Math.max(0, Math.min(8, row.index + step))
            HtpcController.setOverscan(next)
        } else if (row.id === "htpc.reload_themes") {
            host.reloadActiveTheme()
            host.showToast(tr("htpc.settings.theme_reload"), false)
        } else if (row.id === "htpc.quick_combo" && row.count > 0) {
            HtpcController.selectQuickCombo((row.index + step + row.count) % row.count)
        } else if (row.id === "htpc.port0_device") {
            const dev = HtpcController.togglePort0Device()
            host.showToast(tr("htpc.quick.device").replace("{device}", dev), false)
        } else if (row.id === "htpc.dpad_to_axes") {
            HtpcController.togglePort0DpadToAxes()
        } else if (row.id === "library.roms_dir") {
            host.openFolderPicker()
        } else if (row.id === "library.rescan") {
            library.searchAgain()
            host.showToast(library.count, false)
        } else if (row.id === "htpc.quit") {
            Qt.quit()
        } else if (row.kind === "core_bool") {
            const cur = !!coreSettings.value(row.id)
            coreSettings.define(row.id, !cur)
            if (row.id === "game_names_from_file")
                library.rebuild()
        } else if (row.kind === "core_cycle" && row.count > 0) {
            const next = (row.index + step + row.count) % row.count
            coreSettings.define(row.id, next)
            if (row.id === "language") {
                Language.reload()
                library.rebuild()
                HtpcController.syncAfterGlobalChange()
            }
        } else if (row.kind === "volume") {
            const next = Math.max(0, Math.min(100, row.index + step * 5))
            coreSettings.define("audio.volume", next)
        }
        revision += 1
    }

    function handleCommand(code) {
        const cats = categories()
        const rows = rowsForCategory(categoryIndex)
        if (code === 8) {
            categoryIndex = (categoryIndex - 1 + cats.length) % cats.length
            rowIndex = 0
        } else if (code === 9) {
            categoryIndex = (categoryIndex + 1) % cats.length
            rowIndex = 0
        } else if (code === 0 && rows.length > 0) {
            rowIndex = (rowIndex - 1 + rows.length) % rows.length
        } else if (code === 1 && rows.length > 0) {
            rowIndex = (rowIndex + 1) % rows.length
        } else if (code === 2) {
            applyRow(rowIndex, -1)
        } else if (code === 3 || code === 4) {
            applyRow(rowIndex, 1)
        } else if (code === 5) {
            host.closeSettings()
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Top Header
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 74
            color: HtpcController.surfaceSolidColor

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 28
                anchors.rightMargin: 28
                spacing: 16

                ColumnLayout {
                    spacing: 2
                    Text {
                        text: settingsView.tr("htpc.settings.title")
                        color: HtpcController.textColor
                        font.pixelSize: 22
                        font.bold: true
                    }
                    Text {
                        text: settingsView.tr("htpc.settings.shared_hint")
                        color: HtpcController.mutedColor
                        font.pixelSize: 13
                    }
                }

                Item { Layout.fillWidth: true }

                Rectangle {
                    radius: 10
                    color: HtpcController.bgAltColor
                    border.width: 1
                    border.color: HtpcController.borderColor
                    Layout.preferredHeight: 38
                    Layout.preferredWidth: backBtnLbl.implicitWidth + 28

                    Text {
                        id: backBtnLbl
                        anchors.centerIn: parent
                        text: HtpcController.promptBack + "  ·  " + settingsView.tr("htpc.prompt.back")
                        color: HtpcController.textColor
                        font.pixelSize: 13
                        font.bold: true
                    }
                    MouseArea {
                        anchors.fill: parent
                        onClicked: host.closeSettings()
                    }
                }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            height: 1
            color: HtpcController.borderColor
        }

        // Two-column body
        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 0

            // Left Categories Sidebar
            Rectangle {
                Layout.fillHeight: true
                Layout.preferredWidth: 270
                color: HtpcController.surfaceColor

                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 18
                    spacing: 8

                    Repeater {
                        model: settingsView.dependsOn([Language.version], settingsView.categories())

                        Rectangle {
                            readonly property bool active: index === settingsView.categoryIndex
                            Layout.fillWidth: true
                            Layout.preferredHeight: 48
                            radius: 10
                            color: active ? HtpcController.accentColor : "transparent"

                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                anchors.left: parent.left
                                anchors.leftMargin: 16
                                anchors.right: parent.right
                                anchors.rightMargin: 12
                                elide: Text.ElideRight
                                text: modelData.label
                                color: active ? HtpcController.accentTextColor : HtpcController.mutedColor
                                font.pixelSize: 15
                                font.bold: active
                            }

                            MouseArea {
                                anchors.fill: parent
                                onClicked: {
                                    settingsView.categoryIndex = index
                                    settingsView.rowIndex = 0
                                }
                            }
                        }
                    }

                    Item { Layout.fillHeight: true }
                }
            }

            Rectangle {
                Layout.fillHeight: true
                width: 1
                color: HtpcController.borderColor
            }

            // Right Options Panel
            Item {
                Layout.fillWidth: true
                Layout.fillHeight: true

                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 28
                    spacing: 10

                    Repeater {
                        model: settingsView.dependsOn(
                            [Language.version, HtpcController.themeVersion, coreSettings.version, settingsView.revision, settingsView.categoryIndex],
                            settingsView.rowsForCategory(settingsView.categoryIndex)
                        )

                        Rectangle {
                            readonly property bool focused: index === settingsView.rowIndex
                            Layout.fillWidth: true
                            Layout.preferredHeight: 60
                            radius: 12
                            color: focused ? HtpcController.accentSoftColor : HtpcController.surfaceColor
                            border.width: focused ? 2 : 1
                            border.color: focused ? HtpcController.accentColor : HtpcController.borderColor

                            RowLayout {
                                anchors.fill: parent
                                anchors.leftMargin: 20
                                anchors.rightMargin: 20
                                spacing: 16

                                Text {
                                    text: modelData.label
                                    color: HtpcController.textColor
                                    font.pixelSize: 16
                                    font.bold: focused
                                    Layout.fillWidth: true
                                    elide: Text.ElideRight
                                }

                                Rectangle {
                                    visible: modelData.value !== ""
                                    radius: 8
                                    color: focused ? HtpcController.accentColor : HtpcController.bgAltColor
                                    Layout.preferredHeight: 34
                                    Layout.preferredWidth: Math.min(340, valText.implicitWidth + 28)

                                    Text {
                                        id: valText
                                        anchors.centerIn: parent
                                        width: parent.width - 16
                                        horizontalAlignment: Text.AlignHCenter
                                        elide: Text.ElideMiddle
                                        text: (modelData.kind === "cycle" || modelData.kind === "core_cycle" || modelData.kind === "volume")
                                            ? ("‹  " + modelData.value + "  ›")
                                            : modelData.value
                                        color: focused ? HtpcController.accentTextColor : HtpcController.textColor
                                        font.pixelSize: 14
                                        font.bold: true
                                    }
                                }
                            }

                            MouseArea {
                                anchors.fill: parent
                                onClicked: settingsView.applyRow(index, 1)
                            }
                        }
                    }

                    Item { Layout.fillHeight: true }
                }
            }
        }

        // Bottom Prompts Footer
        Rectangle {
            Layout.fillWidth: true
            height: 1
            color: HtpcController.borderColor
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 48
            color: HtpcController.surfaceSolidColor

            Text {
                anchors.verticalCenter: parent.verticalCenter
                anchors.left: parent.left
                anchors.leftMargin: 28
                text: HtpcController.promptOk + " " + settingsView.tr("htpc.prompt.change")
                    + "    ← / → " + settingsView.tr("htpc.prompt.change")
                    + "    " + HtpcController.promptL1 + "/" + HtpcController.promptR1 + " " + settingsView.tr("htpc.prompt.category")
                    + "    " + HtpcController.promptBack + " " + settingsView.tr("htpc.prompt.back")
                color: HtpcController.mutedColor
                font.pixelSize: 13
                font.bold: true
            }
        }
    }
}
