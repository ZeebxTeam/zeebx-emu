import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import zeebx

// A janela de log da execução. Separada da do jogo de propósito, como no egui: quem está lendo
// log quer as duas coisas ao mesmo tempo, e um painel dentro da janela do jogo roubaria espaço do
// quadro. Ela acompanha o jogo: só existe enquanto há execução para registrar.
ApplicationWindow {
    id: janela

    // Devolve `valor`, e faz a ligação que chama isto depender das `versoes`.
    //
    // **As versões vão como argumento, e não numa expressão solta.** O QML é compilado
    // antecipadamente (qmlcachegen), e o compilador descarta uma leitura cujo valor não é usado:
    // `(cfg.versao, cfg.aparelho())` perdia a leitura da versão, a ligação deixava de depender
    // dela, e trocar o aparelho de Boomerang para Z-Pad não trocava a tela. Interpretado, como no
    // qmltestrunner, funcionava — por isso os testes não pegaram.
    function depende(versoes, valor) {
        return valor
    }

    // Se a janela deve estar à mostra: o jogo aberto, a opção ligada, e não fechada nesta
    // execução. Quem liga é a principal, que sabe do jogo e das configurações.
    property bool pedida: false
    // Quantas vezes a janela foi fechada, para a ligação de `pedida` reler o `Log`.
    property int dispensas: 0
    // O que deu a última exportação.
    property string recado: ""
    readonly property alias log: log

    width: 640
    height: 420
    minimumWidth: 360
    minimumHeight: 200
    visible: false
    // **O log não pega o teclado.** O KWin dava o foco a ele ao aparecer, e no Wayland o foco só
    // volta a outra janela em resposta a uma ação do usuário: pedir de volta para o jogo não
    // adiantava, e o teclado ficava no log. O mouse continua valendo — rolar, exportar, fechar.
    flags: Qt.Window | Qt.WindowDoesNotAcceptFocus
    title: "Zeebx — " + tr("debug.log.title")

    function tr(chave) {
        return depende([Idioma.versao], Idioma.texto(chave))
    }

    onPedidaChanged: {
        if (pedida) {
            recado = ""
            presa = true
            releLinhas()
            if (transientParent === null || transientParent.active)
                mostra()
        } else {
            hide()
        }
    }

    // **Só depois de a janela do jogo existir no compositor.** O log é filho da janela do jogo, e
    // é isso que o mantém por cima dela. Mostrado no mesmo instante que ela, o log chegava ao KWin
    // antes de a janela do jogo existir, sem pai: o KWin o via como janela solta
    // (`transient=false`, medido por um script dele que lista a pilha) e o deixava atrás do jogo.
    // O jogo ficar ativo é o sinal de que a janela dele já está lá.
    Connections {
        target: janela.transientParent
        function onActiveChanged() {
            if (janela.pedida && !janela.visible && janela.transientParent.active)
                janela.mostra()
        }
    }

    // Por cima do jogo, e com o teclado devolvido a ele: o log é para ler, e o jogo continua sendo
    // jogado.
    function mostra() {
        showNormal()
        if (transientParent)
            Qt.callLater(() => janela.transientParent.requestActivate())
    }

    // Fechar a janela dispensa o log **desta** execução, e não a preferência: gravar isso
    // desligaria a opção sozinha, e a janela abriria uma vez e nunca mais.
    onClosing: {
        log.dispensa()
        dispensas += 1
    }

    Log {
        id: log
    }

    // As linhas mudam enquanto o jogo roda, e o log não avisa: é relido de tempos em tempos.
    // Relido só se mudou, para não perder a rolagem de quem está lendo o meio.
    property var linhas: []
    // Preso no fim: log que não acompanha o que acabou de acontecer não serve. Quem sobe para
    // ler solta a lista, e quem desce até o fim prende de novo. Só o usuário muda isto: a troca
    // das linhas mexe na rolagem, e é marcada como `ajustando` para não contar.
    property bool presa: true
    property bool ajustando: false
    function releLinhas() {
        const novas = log.linhas()
        if (novas.length === linhas.length
                && (novas.length === 0 || novas[novas.length - 1] === linhas[linhas.length - 1]))
            return
        const altura = lista.contentY
        ajustando = true
        linhas = novas
        if (presa)
            lista.positionViewAtEnd()
        else
            lista.contentY = altura
        ajustando = false
        // Na primeira leitura a janela ainda nem apareceu, e a lista não tem altura: o fim só
        // existe depois do layout.
        if (presa)
            Qt.callLater(janela.vaiAoFim)
    }
    function vaiAoFim() {
        ajustando = true
        lista.positionViewAtEnd()
        ajustando = false
    }
    Timer {
        interval: 250
        repeat: true
        running: janela.visible
        onTriggered: janela.releLinhas()
    }

    header: ToolBar {
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8
            anchors.rightMargin: 8

            Button {
                text: janela.tr("debug.log.export")
                onClicked: {
                    const recado = log.exporta()
                    if (recado !== "")
                        janela.recado = recado
                }
            }
            // O que deu a exportação, ou, antes dela, o caminho fixo à vista: quem quer o arquivo
            // não precisa exportar nada.
            Label {
                Layout.fillWidth: true
                elide: Text.ElideMiddle
                opacity: 0.6
                text: janela.recado !== "" ? janela.recado : janela.pedida ? log.caminho() : ""
            }
        }
    }

    Label {
        anchors.centerIn: parent
        visible: janela.linhas.length === 0
        opacity: 0.6
        text: janela.tr("debug.log.empty")
    }

    ListView {
        id: lista

        anchors.fill: parent
        anchors.margins: 8
        clip: true
        model: janela.linhas
        boundsBehavior: Flickable.StopAtBounds
        onContentYChanged: if (!janela.ajustando) janela.presa = atYEnd
        ScrollBar.vertical: ScrollBar {}

        delegate: Text {
            required property string modelData

            width: ListView.view.width
            wrapMode: Text.WrapAnywhere
            color: palette.text
            font.family: "monospace"
            text: modelData
        }
    }
}
