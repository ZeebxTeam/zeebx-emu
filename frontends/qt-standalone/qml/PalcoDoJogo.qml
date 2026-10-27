import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Shapes

import zeebx

// O jogo na tela: o quadro, os avisos por cima dele, o teclado e o passo a cada quadro. Mora num
// componente, e não numa janela, porque o jogo pode estar em dois lugares: na janela principal, no
// lugar da biblioteca, que é o padrão, ou numa janela só dele (`graphics.jogo_separado`). Cada
// uma tem o seu palco, e só o de onde o jogo está anda: o núcleo é um só. Ver
// `docs/implementacao/28-jogo-na-janela-principal.md`.
//
// O que é da janela, como fechar ou pôr em tela cheia, sai daqui como sinal: quem hospeda decide.
Item {
    id: palco

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
    function tr(chave) {
        return depende([Idioma.versao], Idioma.texto(chave))
    }

    // Se o jogo está neste palco: só o ativo anda a cada quadro.
    //
    // **Não há linha de estado embaixo do quadro.** A janela separada tinha uma, de 24 px, com o
    // título e onde o jogo desenha, e ela não saía por opção nenhuma. O título está no da janela,
    // e onde o jogo desenha está na caixa de métricas, que se liga em Depuração.
    property bool ativo: false
    // Se o jogo está parado pela pausa. O menu e a barra de ferramentas da principal leem.
    property bool pausado: false
    property alias tela: tela

    // O jogo deve fechar agora: confirmado na pergunta, sem pergunta a fazer, ou o jogo saiu
    // sozinho sem Z-Wheel para onde voltar.
    signal fechaPedido()
    // Sair do Zeebx, confirmado na pergunta. Ver `pedeParaSair`.
    signal saidaPedida()
    // F11, ou Alt+Enter.
    signal telaCheiaPedida()

    // Um jogo começou neste palco: o título dele, e o símbolo da pausa do jogo de antes some. A
    // partida nova começa andando.
    function prepara() {
        tela.atualiza()
        simbolo.opacity = 0
        pausado = false
    }
    function foca() {
        tela.forceActiveFocus()
    }
    // Fecha o jogo de vez: a partida sai do núcleo.
    function fecha() {
        tela.fecha()
    }

    // O Esc, o Fechar do menu e da barra e o fechar da janela passam por aqui: com a opção
    // ligada, o jogo pausa e pergunta antes. Ver `graphics.confirmar_ao_fechar`.
    function pedeParaFechar() {
        if (tela.confirmaAoFechar())
            confirmacao.abre(false)
        else
            fechaPedido()
    }
    // O mesmo, para sair do Zeebx com o jogo aberto.
    function pedeParaSair() {
        if (tela.confirmaAoFechar())
            confirmacao.abre(true)
        else
            saidaPedida()
    }

    // O que o teclado e o menu Sistema pedem, pelo mesmo caminho: o símbolo no meio da tela
    // aparece dos dois jeitos.
    function pausa() {
        pausado = tela.pausa()
        simbolo.mostra(pausado ? "pausa" : "play", pausado)
    }
    function reinicia() {
        if (tela.reinicia()) {
            pausado = false
            simbolo.mostra("reinicio", false)
        }
    }
    function screenshot() {
        tela.screenshot(Qt.formatDateTime(new Date(), "yyyy-MM-dd HH-mm-ss"))
    }

    // O fundo de trás do quadro, quando ele não ocupa tudo.
    Rectangle {
        anchors.fill: parent
        color: "black"
    }

    TelaDoJogo {
        id: tela

        anchors.fill: parent
        // O painel de depuração é uma faixa, e não uma sobreposição: reservar espaço encolhe o
        // quadro do jogo em vez de tapá-lo, como no egui.
        anchors.bottomMargin: painel.visible ? painel.height : 0
        focus: true

        // Esc fecha e P pausa, como na janela do egui. Nenhuma das duas colide com o controle do
        // Zeebo, que usa setas, Z, X, C, V, Q, W, F, G, H, Backspace e Enter. F11 e Alt+Enter
        // alternam a tela cheia. O screenshot (F9 por padrão) e o reiniciar (R por padrão),
        // trocáveis, são perguntados antes do jogo: um botão mapeado na mesma tecla não a recebe.
        // Ver `docs/implementacao/22-screenshots.md`. Os do save state vêm antes de todos: são
        // combinações, que batem só com os mesmos modificadores, e o Shift+F9 de um deles não
        // pode virar o F9 do screenshot. Ver `docs/implementacao/25-save-state-no-desktop.md`.
        Keys.onPressed: (evento) => {
            if (tela.ehAtalhoDeEstado(evento.key, evento.modifiers)) {
                // Segurar não grava nem carrega trinta vezes. O aviso chega pela `estadoSerie`.
                if (!evento.isAutoRepeat)
                    tela.atalhoDeEstado(evento.key, evento.modifiers)
            } else if (tela.ehAtalhoDeScreenshot(evento.key)) {
                // Segurar a tecla tira um screenshot, e não trinta.
                if (!evento.isAutoRepeat)
                    palco.screenshot()
            } else if (tela.ehAtalhoDeReinicio(evento.key)) {
                // Segurar não reinicia trinta vezes: o jogo recomeçaria a cada repetição.
                if (!evento.isAutoRepeat)
                    palco.reinicia()
            } else if (evento.key === Qt.Key_Escape) {
                // Segurado, o Esc repetido fecharia a pergunta que ele mesmo abriu.
                if (!evento.isAutoRepeat)
                    palco.pedeParaFechar()
            } else if (evento.key === Qt.Key_P && !evento.isAutoRepeat) {
                palco.pausa()
            } else if (evento.key === Qt.Key_F11
                       || (evento.key === Qt.Key_Return && (evento.modifiers & Qt.AltModifier))) {
                palco.telaCheiaPedida()
            } else if (!evento.isAutoRepeat) {
                tecla(evento.key, true)
            }
            evento.accepted = true
        }
        Keys.onReleased: (evento) => {
            if (!evento.isAutoRepeat && !tela.ehAtalhoDeScreenshot(evento.key)
                    && !tela.ehAtalhoDeReinicio(evento.key)) {
                tecla(evento.key, false)
            }
            evento.accepted = true
        }
        onActiveFocusChanged: if (!activeFocus) solta()
        // Um jogo que sai sozinho sem Z-Wheel para onde voltar fecha, como no egui.
        onFechou: palco.fechaPedido()
    }

    // O símbolo do meio da tela, como num tocador de vídeo: ⏸ enquanto o jogo está pausado; ▶
    // por um instante quando ele continua; ↻ quando o R o reiniciou. Desenhados, e não letras:
    // nem toda fonte tem o ⏸ e o ↻, e a janela não sabe qual fonte o sistema vai escolher.
    Item {
        id: simbolo

        // "pausa", "play" ou "reinicio".
        property string forma: ""

        // O desenhista de curvas do Qt 6.6, de borda suave sem multiamostragem. **Atribuído só
        // onde existe**: escrito como propriedade da `Shape`, o Qt 6.4 do Ubuntu 24.04 recusava o
        // arquivo inteiro, e com ele a janela principal, que não abria. Lá fica o desenhista
        // padrão.
        function curvasSuaves(forma) {
            if (forma.preferredRendererType !== undefined)
                forma.preferredRendererType = Shape.CurveRenderer
        }

        // Mostra `qual`. O que não `fica` some sozinho, esmaecendo.
        function mostra(qual, fica) {
            saida.stop()
            forma = qual
            opacity = 1
            if (!fica)
                saida.restart()
        }

        anchors.centerIn: tela
        width: 112
        height: 112
        opacity: 0
        visible: opacity > 0

        SequentialAnimation {
            id: saida

            PauseAnimation { duration: 350 }
            NumberAnimation {
                target: simbolo
                property: "opacity"
                to: 0
                duration: 450
                easing.type: Easing.InQuad
            }
        }

        Rectangle {
            anchors.fill: parent
            radius: width / 2
            color: "#a0000000"
            border.color: "#60ffffff"
            border.width: 2
        }

        Row {
            anchors.centerIn: parent
            spacing: 14
            visible: simbolo.forma === "pausa"

            Repeater {
                model: 2

                Rectangle {
                    width: 14
                    height: 44
                    radius: 3
                    color: "white"
                }
            }
        }

        // O triângulo do play, um pouco à direita do centro: o centro de massa dele não é o da
        // caixa, e centrado pela caixa ele parece puxado para a esquerda.
        Shape {
            anchors.centerIn: parent
            anchors.horizontalCenterOffset: 5
            width: 44
            height: 48
            visible: simbolo.forma === "play"
            Component.onCompleted: simbolo.curvasSuaves(this)

            ShapePath {
                fillColor: "white"
                strokeColor: "transparent"
                startX: 0
                startY: 0
                PathLine { x: 44; y: 24 }
                PathLine { x: 0; y: 48 }
                PathLine { x: 0; y: 0 }
            }
        }

        // A seta circular do reinício: um arco de 300 graus, no sentido do relógio, com a ponta
        // no fim dele.
        Shape {
            anchors.centerIn: parent
            width: 56
            height: 56
            visible: simbolo.forma === "reinicio"
            Component.onCompleted: simbolo.curvasSuaves(this)

            ShapePath {
                fillColor: "transparent"
                strokeColor: "white"
                strokeWidth: 7
                capStyle: ShapePath.RoundCap
                PathAngleArc {
                    centerX: 28
                    centerY: 28
                    radiusX: 21
                    radiusY: 21
                    startAngle: -50
                    sweepAngle: 280
                }
            }
            // A ponta, no fim do arco (230 graus), apontando para onde ele segue.
            ShapePath {
                fillColor: "white"
                strokeColor: "transparent"
                startX: 28 + 21 * Math.cos(230 * Math.PI / 180) + 12 * Math.cos(320 * Math.PI / 180)
                startY: 28 + 21 * Math.sin(230 * Math.PI / 180) + 12 * Math.sin(320 * Math.PI / 180)
                PathLine {
                    x: 28 + 21 * Math.cos(230 * Math.PI / 180) + 9 * Math.cos(230 * Math.PI / 180)
                    y: 28 + 21 * Math.sin(230 * Math.PI / 180) + 9 * Math.sin(230 * Math.PI / 180)
                }
                PathLine {
                    x: 28 + 21 * Math.cos(230 * Math.PI / 180) - 9 * Math.cos(230 * Math.PI / 180)
                    y: 28 + 21 * Math.sin(230 * Math.PI / 180) - 9 * Math.sin(230 * Math.PI / 180)
                }
                PathLine {
                    x: 28 + 21 * Math.cos(230 * Math.PI / 180) + 12 * Math.cos(320 * Math.PI / 180)
                    y: 28 + 21 * Math.sin(230 * Math.PI / 180) + 12 * Math.sin(320 * Math.PI / 180)
                }
            }
        }
    }

    // A palavra embaixo do ⏸, para quem não reconhece o símbolo.
    Text {
        anchors.top: simbolo.bottom
        anchors.topMargin: 8
        anchors.horizontalCenter: simbolo.horizontalCenter
        visible: simbolo.forma === "pausa" && simbolo.visible
        color: "white"
        style: Text.Outline
        font.pixelSize: 20
        text: palco.tr("play.paused")
    }

    // O aviso de calibração do Boomerang, no canto de baixo: o modelo inclina como o controle, e o
    // texto diz se ele está parado. É uma área flutuante, que não tira espaço do quadro. Ver
    // `src/ui/calibracao.rs`.
    Rectangle {
        anchors.right: tela.right
        anchors.bottom: tela.bottom
        anchors.margins: 16
        width: conteudo.implicitWidth + 24
        height: conteudo.implicitHeight + 16
        visible: tela.avisoTitulo !== ""
        radius: 6
        color: "#e6202020"
        border.color: "#505050"

        Row {
            id: conteudo

            anchors.centerIn: parent
            spacing: 12

            Item {
                width: 110
                height: 77

                Image {
                    anchors.centerIn: parent
                    width: 110
                    fillMode: Image.PreserveAspectFit
                    source: "qrc:/zeebx/boomerang.png"
                    rotation: tela.avisoGiro
                    smooth: true
                }
            }

            Column {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 4

                Text {
                    color: "white"
                    font.bold: true
                    text: tela.avisoTitulo
                }

                Text {
                    visible: text !== ""
                    color: tela.avisoParado ? "#d0d0d0" : "#ffb040"
                    text: tela.avisoEstado
                }
            }
        }
    }

    // O fast-forward e o turbo ligado, no canto de cima à esquerda, enquanto valem: um alternar
    // esquecido ligado parece defeito do jogo. Ver `docs/implementacao/24-velocidade.md`.
    Rectangle {
        anchors.left: tela.left
        anchors.top: tela.top
        anchors.margins: 16
        width: textoDosIndicadores.implicitWidth + 24
        height: textoDosIndicadores.implicitHeight + 16
        visible: tela.indicadores !== ""
        radius: 6
        color: "#e6202020"
        border.color: "#505050"

        Text {
            id: textoDosIndicadores

            anchors.centerIn: parent
            color: "white"
            font.bold: true
            text: tela.indicadores
        }
    }

    // As métricas, no canto de cima à direita, com a opção de Depuração: os quadros por segundo
    // em destaque, a velocidade e onde o jogo desenha. Por cima do quadro, como os outros avisos,
    // e não uma faixa: o painel de baixo encolhe o jogo, e isto é para deixar ligado jogando.
    Rectangle {
        id: metricas

        anchors.right: tela.right
        anchors.top: tela.top
        anchors.margins: 16
        width: Math.min(linhasDasMetricas.implicitWidth + 24, tela.width / 2)
        height: linhasDasMetricas.implicitHeight + 16
        visible: tela.metricas.length > 0
        radius: 6
        color: "#c0202020"
        border.color: "#505050"

        // Um layout, e não uma `Column`: a largura natural dele é a do texto mais largo, e não a
        // que ele recebeu, então a caixa pode depender dela sem fechar um laço.
        ColumnLayout {
            id: linhasDasMetricas

            anchors.centerIn: parent
            width: parent.width - 24
            spacing: 2

            Repeater {
                model: tela.metricas

                Text {
                    required property int index
                    required property string modelData

                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignRight
                    elide: Text.ElideRight
                    color: index === 0 ? "white" : "#d0d0d0"
                    font.bold: index === 0
                    font.pixelSize: index === 0 ? 18 : 12
                    text: modelData
                }
            }
        }
    }

    // O aviso do screenshot, no canto de cima, por dois segundos. É o Qt que o desenha por cima
    // do quadro, e por isso ele não entra no PNG, que vem do núcleo. Clicar abre a pasta do jogo.
    // Embaixo das métricas, quando elas estão à mostra.
    Rectangle {
        id: avisoDoScreenshot

        anchors.right: tela.right
        anchors.top: metricas.visible ? metricas.bottom : tela.top
        anchors.topMargin: metricas.visible ? 8 : 16
        anchors.rightMargin: 16
        width: textoDoScreenshot.implicitWidth + 24
        height: textoDoScreenshot.implicitHeight + 16
        visible: false
        radius: 6
        color: "#e6202020"
        border.color: "#505050"

        Text {
            id: textoDoScreenshot

            anchors.centerIn: parent
            color: tela.screenshotFalhou ? "#ffb040" : "white"
            text: tela.screenshotTexto
        }

        MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: Qt.openUrlExternally(tela.screenshotPasta)
        }

        Timer {
            id: someODoScreenshot

            interval: 2000
            onTriggered: avisoDoScreenshot.visible = false
        }

        Connections {
            target: tela
            function onScreenshotSerieChanged() {
                avisoDoScreenshot.visible = true
                someODoScreenshot.restart()
            }
        }
    }

    // O aviso do save state, no mesmo canto, pelo mesmo tempo: o slot escolhido, o estado salvo
    // ou carregado, ou por que não deu. Embaixo do do screenshot, para os dois caberem juntos.
    Rectangle {
        id: avisoDoEstado

        anchors.right: tela.right
        anchors.top: avisoDoScreenshot.visible ? avisoDoScreenshot.bottom
                     : metricas.visible ? metricas.bottom : tela.top
        anchors.topMargin: avisoDoScreenshot.visible || metricas.visible ? 8 : 16
        anchors.rightMargin: 16
        width: textoDoEstado.implicitWidth + 24
        height: textoDoEstado.implicitHeight + 16
        visible: false
        radius: 6
        color: "#e6202020"
        border.color: "#505050"

        Text {
            id: textoDoEstado

            anchors.centerIn: parent
            color: tela.estadoFalhou ? "#ffb040" : "white"
            text: tela.estadoTexto
        }

        Timer {
            id: someODoEstado

            interval: 2000
            onTriggered: avisoDoEstado.visible = false
        }

        Connections {
            target: tela
            function onEstadoSerieChanged() {
                avisoDoEstado.visible = true
                someODoEstado.restart()
            }
        }
    }

    // O painel de depuração: os números e, se couber, a linha do tempo. Ver `ui/depuracao.rs`.
    Rectangle {
        id: painel

        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: 48
        visible: tela.depuracao !== ""
        color: "#1c1c1c"

        Text {
            anchors.left: parent.left
            anchors.right: grafico.visible ? grafico.left : parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.margins: 8
            elide: Text.ElideRight
            color: "#d0d0d0"
            font.family: "monospace"
            text: tela.depuracao
        }

        // Velocidade (azul, até 200%) e quadros por segundo (verde, até 60) no último minuto, com
        // a linha dos 100% no meio: acima dela o jogo está no ritmo do console, abaixo, devendo.
        // A largura é fixa pelo mesmo motivo do egui: uma que mudasse com a janela faria a mesma
        // queda parecer outra a cada redimensionamento.
        Canvas {
            id: grafico

            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.rightMargin: 8
            width: 220
            height: 40
            visible: tela.linhaDoTempo.length > 0 && painel.width >= width + 200

            Connections {
                target: tela
                function onLinhaDoTempoChanged() { grafico.requestPaint() }
            }

            onPaint: {
                const g = getContext("2d")
                const pontos = tela.linhaDoTempo
                const amostras = pontos.length / 2
                g.reset()
                g.fillStyle = "rgba(0, 0, 0, 0.47)"
                g.fillRect(0, 0, width, height)
                g.strokeStyle = "rgba(255, 255, 255, 0.16)"
                g.beginPath()
                g.moveTo(0, height / 2)
                g.lineTo(width, height / 2)
                g.stroke()
                const passo = width / Math.max(amostras, 2)
                for (const [serie, cor, teto] of [[0, "rgb(120, 200, 255)", 200], [1, "rgb(160, 255, 160)", 60]]) {
                    g.strokeStyle = cor
                    g.beginPath()
                    for (let i = 0; i < amostras; i++) {
                        const y = height - height * Math.min(pontos[2 * i + serie] / teto, 1)
                        if (i === 0)
                            g.moveTo(0, y)
                        else
                            g.lineTo(i * passo, y)
                    }
                    g.stroke()
                }
            }
        }
    }

    // Um jogo que parou continua com o último quadro à mostra, mas o motivo precisa aparecer em
    // algum lugar: uma faixa embaixo, como no egui, com o que fazer em seguida.
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.bottomMargin: painel.visible ? painel.height : 0
        height: faixa.implicitHeight + 12
        visible: tela.parou !== ""
        color: "#c8000000"

        Column {
            id: faixa

            anchors.verticalCenter: parent.verticalCenter
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.margins: 8
            spacing: 2

            Text {
                width: parent.width
                wrapMode: Text.Wrap
                color: "#ffb040"
                text: tela.parou
            }

            Text {
                color: "#9a9a9a"
                text: palco.tr("play.stop.hint")
            }
        }
    }

    // A pergunta antes de fechar o jogo, ou de sair do Zeebx com ele aberto. Mora no palco, e não na
    // janela principal, para aparecer onde o jogo está: na janela separada, um diálogo da principal
    // abriria atrás dela. O jogo fica pausado enquanto ela está aberta, e desistir o devolve como
    // estava.
    Dialog {
        id: confirmacao

        // Se a pergunta é a de sair do Zeebx, e não só a de fechar o jogo.
        property bool saindo: false
        // Se foi a pergunta que pausou o jogo: um jogo que já estava pausado continua assim.
        property bool pausouOJogo: false
        property bool confirmada: false

        function abre(sair) {
            saindo = sair
            confirmada = false
            pausouOJogo = !palco.pausado
            if (pausouOJogo)
                palco.pausa()
            open()
        }

        anchors.centerIn: parent
        width: Math.min(420, palco.width - 32)
        modal: true
        title: palco.tr(saindo ? "play.confirm_quit.title" : "play.confirm_close.title")

        Label {
            width: parent.width
            wrapMode: Text.Wrap
            text: palco.tr(confirmacao.saindo ? "play.confirm_quit.text" : "play.confirm_close.text")
        }

        footer: DialogButtonBox {
            Button {
                text: palco.tr("play.confirm.cancel")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
            Button {
                text: palco.tr(confirmacao.saindo ? "play.confirm_quit.ok" : "play.confirm_close.ok")
                DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            }
        }

        onAccepted: confirmada = true
        // Pelo `closed`, e não pelo `rejected`: o Esc e o clique fora fecham sem passar por ele.
        onClosed: {
            if (confirmada) {
                if (saindo)
                    palco.saidaPedida()
                else
                    palco.fechaPedido()
                return
            }
            if (pausouOJogo && palco.pausado)
                palco.pausa()
            palco.foca()
        }
    }

    // Um passo por quadro da janela, no ritmo do vsync — como o `request_repaint` do egui. Um
    // `Timer` de intervalo zero deixaria a CPU a 100% à toa. Só no palco do jogo: o outro não anda.
    FrameAnimation {
        running: palco.ativo
        onTriggered: tela.passo()
    }
}
