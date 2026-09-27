import QtQuick
import QtQuick.Window

import zeebx

// A janela do jogo: uma janela do sistema, e não um painel dentro da principal, como no egui.
// Ela só é mostrada depois de a biblioteca abrir um jogo, e fechá-la fecha o jogo.
Window {
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

    width: 960
    height: 760
    minimumWidth: 320
    minimumHeight: 240
    visible: false
    color: "black"
    title: "Zeebx — " + tela.estado

    // A janela reaparece a cada jogo aberto: é a hora de ler o título dele.
    onVisibleChanged: if (visible) tela.atualiza()

    // Mostra a janela no modo configurado para ela (`graphics.janela_do_jogo`).
    function abre() {
        const modo = tela.modoDaJanela()
        if (modo === 2)
            showFullScreen()
        else if (modo === 1)
            showMaximized()
        else
            showNormal()
        raise()
        requestActivate()
    }

    // Os textos se refazem sozinhos quando o idioma muda: a ligação que chama isto lê a versão.
    function tr(chave) {
        return depende([Idioma.versao], Idioma.texto(chave))
    }

    function alternaTelaCheia() {
        visibility = visibility === Window.FullScreen ? Window.Windowed : Window.FullScreen
    }
    onClosing: tela.fecha()
    onActiveChanged: if (!active) tela.solta()

    TelaDoJogo {
        id: tela

        anchors.fill: parent
        // O painel de depuração é uma faixa, e não uma sobreposição: reservar espaço encolhe o
        // quadro do jogo em vez de tapá-lo, como no egui.
        anchors.bottomMargin: 24 + (painel.visible ? painel.height : 0)
        focus: true

        // Esc fecha e P pausa, como na janela do egui. Nenhuma das duas colide com o controle do
        // Zeebo, que usa setas, Z, X, C, V, Q, W, F, G, H, Backspace e Enter. F11 e Alt+Enter
        // alternam a tela cheia. O screenshot (F9 por padrão, trocável) é perguntado antes do
        // jogo: um botão mapeado na mesma tecla não a recebe. Ver
        // `docs/implementacao/22-screenshots.md`.
        Keys.onPressed: (evento) => {
            if (tela.ehAtalhoDeScreenshot(evento.key)) {
                // Segurar a tecla tira um screenshot, e não trinta.
                if (!evento.isAutoRepeat)
                    tela.screenshot(Qt.formatDateTime(new Date(), "yyyy-MM-dd HH-mm-ss"))
            } else if (evento.key === Qt.Key_Escape) {
                janela.close()
            } else if (evento.key === Qt.Key_P && !evento.isAutoRepeat) {
                pausado.visible = tela.pausa()
            } else if (evento.key === Qt.Key_F11
                       || (evento.key === Qt.Key_Return && (evento.modifiers & Qt.AltModifier))) {
                janela.alternaTelaCheia()
            } else if (!evento.isAutoRepeat) {
                tecla(evento.key, true)
            }
            evento.accepted = true
        }
        Keys.onReleased: (evento) => {
            if (!evento.isAutoRepeat && !tela.ehAtalhoDeScreenshot(evento.key)) {
                tecla(evento.key, false)
            }
            evento.accepted = true
        }
        onActiveFocusChanged: if (!activeFocus) solta()
        // Um jogo que sai sozinho sem Z-Wheel para onde voltar fecha a janela, como no egui.
        onFechou: janela.close()
    }

    Text {
        id: pausado

        anchors.centerIn: tela
        visible: false
        color: "white"
        style: Text.Outline
        font.pixelSize: 32
        text: janela.tr("play.paused")
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

    // O aviso do screenshot, no canto de cima, por dois segundos. É o Qt que o desenha por cima
    // do quadro, e por isso ele não entra no PNG, que vem do núcleo. Clicar abre a pasta do jogo.
    Rectangle {
        id: avisoDoScreenshot

        anchors.right: tela.right
        anchors.top: tela.top
        anchors.margins: 16
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

    // O painel de depuração: os números e, se couber, a linha do tempo. Ver `ui/depuracao.rs`.
    Rectangle {
        id: painel

        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 24
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
        anchors.bottomMargin: 24 + (painel.visible ? painel.height : 0)
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
                text: janela.tr("play.stop.hint")
            }
        }
    }

    Text {
        anchors.bottom: parent.bottom
        anchors.left: parent.left
        anchors.margins: 4
        color: "#bbbbbb"
        text: tela.estado
    }

    // Um passo por quadro da janela, no ritmo do vsync — como o `request_repaint` do egui. Um
    // `Timer` de intervalo zero deixaria a CPU a 100% à toa. Só com a janela à mostra: fechada,
    // não há jogo para andar.
    FrameAnimation {
        running: janela.visible
        onTriggered: tela.passo()
    }
}
