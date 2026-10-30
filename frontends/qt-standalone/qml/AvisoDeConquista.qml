import QtQuick
import QtQuick.Layouts

// Os avisos do RetroAchievements por cima do jogo, um de cada vez. O `Image` baixa o ícone da URL.
Rectangle {
    id: aviso

    property var fila: []

    function mostra(titulo, texto, icone) {
        fila.push({ titulo: titulo, texto: texto, icone: icone })
        if (!visible)
            proximo()
    }

    function proximo() {
        const item = fila.shift()
        visible = item !== undefined
        if (!visible)
            return
        textoDoTitulo.text = item.titulo
        textoDoAviso.text = item.texto
        icone.source = item.icone
        tempo.restart()
    }

    visible: false
    width: linha.implicitWidth + 24
    height: linha.implicitHeight + 16
    radius: 6
    color: "#e6202020"
    border.color: "#505050"

    RowLayout {
        id: linha

        anchors.centerIn: parent
        spacing: 10

        Image {
            id: icone

            Layout.preferredWidth: 48
            Layout.preferredHeight: 48
            visible: status === Image.Ready
            asynchronous: true
            fillMode: Image.PreserveAspectFit
        }

        ColumnLayout {
            spacing: 2

            Text {
                id: textoDoTitulo

                color: "white"
                font.bold: true
            }
            Text {
                id: textoDoAviso

                visible: text !== ""
                color: "#cccccc"
            }
        }
    }

    Timer {
        id: tempo

        interval: 4000
        onTriggered: aviso.proximo()
    }
}
