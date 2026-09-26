import QtQuick

// A biblioteca no jeito da Z-Wheel: o rolo de logos girando como cilindro, o nome, as caixas
// deslizando, e a classificação com a descrição embaixo. As medidas e as curvas são as do
// `src/ui/app/vitrine.rs`, que desenha o mesmo slider no egui.
Item {
    id: slider

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

    required property var biblioteca
    signal abre(int linha)
    // O teclado anda pelo slider. Falso com outra janela por cima — ver `Principal.sobreposta`.
    property bool escutando: true

    // O jogo escolhido, sem limite: o slider dá a volta, e a posição na lista é o resto da
    // divisão. Assim a animação da última para a primeira anda um passo, e não a lista toda.
    property int cursor: 0
    // Onde o slider está desenhado agora, correndo atrás do cursor.
    property real posicao: 0
    // Quantos jogos a lista tem, e um número que muda a cada vez que ela é refeita: o `linhas`
    // e o `item` são chamadas, e uma ligação não sabe sozinha que o modelo mudou por baixo.
    property int total: 0
    property int versao: 0

    Component.onCompleted: total = biblioteca.linhas()
    Connections {
        target: slider.biblioteca
        function onModelReset() {
            slider.total = slider.biblioteca.linhas()
            slider.versao += 1
        }
    }

    function linha(k) {
        return total > 0 ? ((k % total) + total) % total : 0
    }
    // A busca mudou: a escolha volta ao primeiro resultado, sem a animação atravessar a lista.
    function reinicia() {
        cursor = 0
        posicao = 0
    }
    function comando(codigo) {
        switch (codigo) {
        case 2: cursor -= 1; break
        case 3: cursor += 1; break
        case 4: abre(linha(cursor)); break
        }
    }
    // Um clique no escolhido abre; num vizinho, traz ele para o meio.
    // O clique devolve o teclado ao slider: vindo da busca, as setas continuavam no campo.
    function clica(k) {
        forceActiveFocus()
        if (k === cursor)
            abre(linha(cursor))
        else
            cursor = k
    }

    focus: true
    clip: true
    Keys.onLeftPressed: if (escutando) cursor -= 1
    Keys.onRightPressed: if (escutando) cursor += 1
    Keys.onReturnPressed: if (escutando) abre(linha(cursor))
    Keys.onEnterPressed: if (escutando) abre(linha(cursor))
    Keys.onSpacePressed: if (escutando) abre(linha(cursor))

    // A posição corre atrás do cursor com uma mola amortecida: rápida no começo, macia no fim.
    FrameAnimation {
        running: slider.visible && slider.posicao !== slider.cursor
        onTriggered: {
            const dt = Math.min(frameTime, 0.1)
            slider.posicao += (slider.cursor - slider.posicao) * (1 - Math.exp(-dt * 12))
            if (Math.abs(slider.cursor - slider.posicao) < 0.002)
                slider.posicao = slider.cursor
        }
    }

    // A roda anda um jogo por entalhe, de qualquer eixo e de qualquer aparelho.
    //
    // **Uma `MouseArea`, e não um `WheelHandler`.** O `WheelHandler` só aceita o mouse por padrão
    // (`acceptedDevices` 1, conferido no Qt 6.11) e só um eixo: num notebook, a rolagem de dois
    // dedos do touchpad não andava o slider. A `MouseArea` sem botões recebe a roda de tudo, nos
    // dois eixos; os painéis e as caixas por cima não tratam a roda, e ela chega até aqui.
    property real roda: 0
    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.NoButton
        onWheel: (evento) => {
            // Um entalhe de mouse é 120 em `angleDelta`; o touchpad manda o mesmo movimento em
            // pedaços menores, que se somam até valer um passo.
            slider.roda += (evento.angleDelta.x - evento.angleDelta.y) / 2
            while (Math.abs(slider.roda) >= 60) {
                slider.cursor += Math.sign(slider.roda)
                slider.roda -= 60 * Math.sign(slider.roda)
            }
        }
    }

    readonly property real alturaDoRolo: Math.min(Math.max(height * 0.17, 56), 104)
    readonly property real alturaDaFicha: Math.min(Math.max(height * 0.2, 72), 130)
    readonly property real topoDoPalco: 8 + alturaDoRolo + 6 + 40 + 4
    readonly property real alturaDoPalco: height - alturaDaFicha - topoDoPalco
    readonly property var atual: slider.depende([versao], total > 0 ? biblioteca.item(linha(cursor)) : ({}))

    // O rolo: um cilindro de painéis, cada um num ângulo. O da frente é o do jogo escolhido.
    Rectangle {
        id: rolo

        x: 12
        y: 8
        width: parent.width - 24
        height: slider.alturaDoRolo
        radius: height / 2
        color: palette.dark
    }

    Repeater {
        model: 11

        Rectangle {
            id: painel

            readonly property int k: Math.round(slider.posicao) - 5 + index
            readonly property real angulo: (k - slider.posicao) * 0.42
            readonly property real frente: Math.cos(angulo)
            readonly property var jogo: slider.depende([slider.versao], slider.total > 0 ? slider.biblioteca.item(slider.linha(k)) : ({}))
            readonly property bool escolhido: k === slider.cursor
                                              && Math.abs(slider.posicao - slider.cursor) < 0.5

            visible: slider.total > 0 && Math.abs(angulo) < Math.PI / 2 * 0.95
            z: 10 - Math.abs(angulo)
            width: slider.alturaDoRolo * 1.45 * frente
            height: slider.alturaDoRolo * 0.78
            x: slider.width / 2 + slider.width * 0.42 * Math.sin(angulo) - width / 2
            y: rolo.y + (rolo.height - height) / 2
            radius: 6
            color: Qt.rgba(236 / 255, 236 / 255, 236 / 255, 1)
            opacity: 0.25 + 0.75 * frente * frente
            border.width: escolhido ? 2 : 0
            border.color: palette.highlight

            // O logo é quadrado (64×64), mas no palco da Z-Wheel ele é esticado sobre um painel
            // retangular: foi desenhado já comprimido na largura para isso. Sem cena no palco, o
            // painel leva o nome, para o rolo não ficar com buracos.
            Image {
                anchors.fill: parent
                anchors.margins: 3
                visible: painel.jogo.logo !== undefined && painel.jogo.logo !== ""
                source: visible ? painel.jogo.logo : ""
                smooth: true
            }
            Text {
                anchors.fill: parent
                anchors.margins: 4
                visible: painel.jogo.logo === undefined || painel.jogo.logo === ""
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
                wrapMode: Text.Wrap
                elide: Text.ElideRight
                font.pixelSize: 11 + 2 * painel.frente
                color: "#282828"
                text: painel.jogo.titulo !== undefined ? painel.jogo.titulo : ""
            }
            Rectangle {
                anchors.fill: parent
                radius: parent.radius
                visible: painel.escolhido
                color: palette.highlight
                opacity: 0.18
            }
            MouseArea {
                anchors.fill: parent
                onClicked: slider.clica(painel.k)
            }
        }
    }

    // O nome do jogo escolhido, em cima das caixas.
    Text {
        x: 0
        y: rolo.y + rolo.height + 6
        width: parent.width
        height: 40
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
        font.pixelSize: 24
        font.bold: true
        color: palette.windowText
        text: slider.atual.titulo !== undefined ? slider.atual.titulo : ""
    }

    // As caixas: a escolhida grande no meio, as vizinhas menores e apagadas dos lados.
    Repeater {
        model: 9

        Item {
            id: caixa

            readonly property int k: Math.round(slider.posicao) - 4 + index
            readonly property real d: k - slider.posicao
            readonly property real escala: 1 / (1 + 0.38 * Math.abs(d))
            readonly property real alturaBase: slider.alturaDoPalco * 0.92
            readonly property var jogo: slider.depende([slider.versao], slider.total > 0 ? slider.biblioteca.item(slider.linha(k)) : ({}))
            readonly property real aspecto: capa.sourceSize.height > 0
                                            ? capa.sourceSize.width / capa.sourceSize.height : 0.77
            readonly property real passo: alturaBase * 0.62

            visible: slider.total > 0 && Math.abs(d) < 3.6
            z: 20 - Math.abs(d)
            height: alturaBase * escala
            width: Math.min(height * aspecto, slider.width * 0.5)
            x: slider.width / 2 + passo * Math.sign(d) * Math.pow(Math.abs(d) * 1.35, 0.8) - width / 2
            y: slider.topoDoPalco + (slider.alturaDoPalco - height) / 2
            opacity: Math.max(0, Math.min(1, 1 - 0.28 * Math.abs(d)))

            // A sombra dá o chão: sem ela as caixas parecem coladas no fundo.
            Rectangle {
                x: -2
                y: 6 * caixa.escala - 2
                width: parent.width + 4
                height: parent.height + 4
                radius: 6
                color: Qt.rgba(0, 0, 0, 90 / 255)
            }
            Image {
                id: capa

                anchors.fill: parent
                source: caixa.jogo.capa !== undefined ? caixa.jogo.capa : ""
                smooth: !caixa.jogo.capaNitida
                mipmap: !caixa.jogo.capaNitida
            }
            Rectangle {
                anchors.fill: parent
                anchors.margins: -3
                radius: 6
                color: "transparent"
                visible: Math.abs(caixa.d) < 0.5
                border.width: 2
                border.color: palette.highlight
            }
            MouseArea {
                anchors.fill: parent
                onClicked: slider.clica(caixa.k)
            }
        }
    }

    // A ficha: classificação e descrição.
    Item {
        x: 16
        y: slider.height - slider.alturaDaFicha + 8
        width: slider.width - 32
        height: slider.alturaDaFicha - 12
        clip: true

        Image {
            id: classificacao

            height: Math.min(parent.height - 8, sourceSize.height)
            fillMode: Image.PreserveAspectFit
            source: slider.atual.classificacao !== undefined ? slider.atual.classificacao : ""
            visible: source !== ""
        }
        Text {
            x: classificacao.visible ? classificacao.width + 12 : 0
            width: parent.width - x
            wrapMode: Text.Wrap
            font.pixelSize: 13
            color: palette.windowText
            text: slider.atual.descricao !== undefined ? slider.atual.descricao : ""
        }
    }
}
