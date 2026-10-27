import QtQuick
import QtQuick.Window

import zeebx

// A janela separada do jogo, a única até a 0.4.2, para quem ligou `graphics.jogo_separado`. O
// jogo em si mora no `PalcoDoJogo`, o mesmo que a janela principal usa. Ela só é mostrada depois
// de a biblioteca abrir um jogo, e fechá-la fecha o jogo.
Window {
    id: janela

    width: larguraGuardada > 0 ? larguraGuardada : 640
    height: alturaGuardada > 0 ? alturaGuardada : 512
    minimumWidth: 320
    minimumHeight: 240
    visible: false
    color: "black"
    title: "Zeebx — " + palcoDaJanela.tela.estado

    // Como na principal: o tamanho em que fechar é o tamanho em que reabrir.
    readonly property real larguraGuardada: palcoDaJanela.tela.larguraDaJanela()
    readonly property real alturaGuardada: palcoDaJanela.tela.alturaDaJanela()

    // O palco, e o que o menu da principal lê dele.
    property alias palco: palcoDaJanela
    property alias tela: palcoDaJanela.tela

    function guardaTamanho() {
        if (visibility === Window.Windowed)
            palcoDaJanela.tela.guardaTamanhoDaJanela(width, height)
    }

    onWidthChanged: filaDaGravacao.restart()
    onHeightChanged: filaDaGravacao.restart()

    Timer {
        id: filaDaGravacao
        interval: 500
        onTriggered: janela.guardaTamanho()
    }

    // A janela reaparece a cada jogo aberto: é a hora de ler o título dele.
    onVisibleChanged: if (visible) palcoDaJanela.prepara()

    // Mostra a janela no modo configurado para ela (`graphics.janela_do_jogo`).
    function abre() {
        const modo = palcoDaJanela.tela.modoDaJanela()
        if (modo === 2)
            showFullScreen()
        else if (modo === 1)
            showMaximized()
        else
            showNormal()
        liberada = false
        raise()
        requestActivate()
    }

    // Fechar pela janela pergunta antes, como o Esc. Depois da pergunta, ou de um jogo que saiu
    // sozinho, `fecha()` fecha de vez.
    property bool liberada: false
    function fecha() {
        liberada = true
        close()
    }

    function alternaTelaCheia() {
        visibility = visibility === Window.FullScreen ? Window.Windowed : Window.FullScreen
    }

    onClosing: (close) => {
        if (!liberada && palcoDaJanela.tela.confirmaAoFechar()) {
            close.accepted = false
            palcoDaJanela.pedeParaFechar()
            return
        }
        filaDaGravacao.stop()
        janela.guardaTamanho()
        palcoDaJanela.fecha()
    }
    onActiveChanged: if (!active) palcoDaJanela.tela.solta()

    PalcoDoJogo {
        id: palcoDaJanela

        anchors.fill: parent
        ativo: janela.visible
        onFechaPedido: janela.fecha()
        onTelaCheiaPedida: janela.alternaTelaCheia()
    }
}
