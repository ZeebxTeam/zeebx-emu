import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import zeebx

// The settings' Achievements tab: the RetroAchievements account. See `src/qt/achievements.rs`.
ScrollView {
    id: page

    // The login answers after the click: the tab rereads the state while it is showing. The
    // versions go as an argument so the binding depends on them, as in `SettingsWindow.qml`.
    property int version: 0

    function dependsOn(versions, value) {
        return value
    }
    function tr(keyId) {
        return dependsOn([Language.version], Language.text(keyId))
    }

    readonly property string account: dependsOn([version, Language.version], Achievements.account())
    readonly property bool loggingIn: dependsOn([version], Achievements.loggingIn())
    readonly property string error: dependsOn([version], Achievements.error())

    contentWidth: availableWidth

    Timer {
        interval: 500
        running: page.visible
        repeat: true
        onTriggered: page.version++
    }

    ColumnLayout {
        width: parent.width - 32
        x: 16
        spacing: 6

        Label {
            Layout.topMargin: 12
            font.bold: true
            text: "RetroAchievements"
        }
        Label {
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            opacity: 0.6
            text: page.tr("achievements.hint")
        }

        Label {
            Layout.topMargin: 8
            visible: page.account !== ""
            text: page.account
        }
        Button {
            visible: page.account !== ""
            text: page.tr("achievements.logout")
            onClicked: {
                Achievements.logout()
                page.version++
            }
        }

        GridLayout {
            Layout.topMargin: 8
            visible: page.account === ""
            columns: 2

            Label { text: page.tr("achievements.user") }
            TextField {
                id: username

                Layout.preferredWidth: 240
            }
            Label { text: page.tr("achievements.password") }
            TextField {
                id: password

                Layout.preferredWidth: 240
                echoMode: TextInput.Password
                onAccepted: if (loginButton.enabled) loginButton.clicked()
            }
            Item {}
            Button {
                id: loginButton

                enabled: !page.loggingIn && username.text !== "" && password.text !== ""
                text: page.tr(page.loggingIn ? "achievements.logging_in" : "achievements.login")
                onClicked: {
                    Achievements.login(username.text, password.text)
                    password.text = ""
                    page.version++
                }
            }
        }
        Label {
            Layout.fillWidth: true
            visible: page.error !== "" && page.account === ""
            wrapMode: Text.Wrap
            color: "#ff6060"
            text: page.error
        }
    }
}
