import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import zeebx

// A aba Conquistas das configurações: a conta do RetroAchievements. Ver `src/qt/conquistas.rs`.
ScrollView {
    id: pagina

    // O login responde depois do clique: a aba relê o estado enquanto está à mostra. As versões
    // vão como argumento para a ligação depender delas, como na `JanelaDeConfiguracoes.qml`.
    property int versao: 0

    function depende(versoes, valor) {
        return valor
    }
    function tr(chave) {
        return depende([Idioma.versao], Idioma.texto(chave))
    }

    readonly property string conta: depende([versao, Idioma.versao], Conquistas.conta())
    readonly property bool entrando: depende([versao], Conquistas.entrando())
    readonly property string erro: depende([versao], Conquistas.erro())

    contentWidth: availableWidth

    Timer {
        interval: 500
        running: pagina.visible
        repeat: true
        onTriggered: pagina.versao++
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
            text: pagina.tr("achievements.hint")
        }

        Label {
            Layout.topMargin: 8
            visible: pagina.conta !== ""
            text: pagina.conta
        }
        Button {
            visible: pagina.conta !== ""
            text: pagina.tr("achievements.logout")
            onClicked: {
                Conquistas.sai()
                pagina.versao++
            }
        }

        GridLayout {
            Layout.topMargin: 8
            visible: pagina.conta === ""
            columns: 2

            Label { text: pagina.tr("achievements.user") }
            TextField {
                id: usuario

                Layout.preferredWidth: 240
            }
            Label { text: pagina.tr("achievements.password") }
            TextField {
                id: senha

                Layout.preferredWidth: 240
                echoMode: TextInput.Password
                onAccepted: if (entrar.enabled) entrar.clicked()
            }
            Item {}
            Button {
                id: entrar

                enabled: !pagina.entrando && usuario.text !== "" && senha.text !== ""
                text: pagina.tr(pagina.entrando ? "achievements.logging_in" : "achievements.login")
                onClicked: {
                    Conquistas.entra(usuario.text, senha.text)
                    senha.text = ""
                    pagina.versao++
                }
            }
        }
        Label {
            Layout.fillWidth: true
            visible: pagina.erro !== "" && pagina.conta === ""
            wrapMode: Text.Wrap
            color: "#ff6060"
            text: pagina.erro
        }
    }
}
