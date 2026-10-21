import QtQuick
import QtQuick.Controls

import zeebx

// A biblioteca em grade: cartões em pé, na proporção das caixas que a Z-Wheel traz, quantos
// couberem na largura. É a grade que a interface do egui tinha (`src/ui/app/vitrine.rs`, até a
// 0.4.2).
GridView {
    id: grade

    required property var biblioteca
    // Um cartão foi escolhido para jogar: pelo clique, pelo Enter ou pelo controle.
    signal abre(int linha)
    // O menu do clique direito: as configurações próprias do jogo, ou os detalhes dele.
    signal configura(int linha)
    signal inspeciona(int linha)
    // O teclado anda pela grade. Falso com outra janela por cima — ver `Principal.sobreposta`.
    property bool escutando: true

    // O cartão tem 136 de largura; a imagem cabe num quadro de 112×145 — o `QUADRO_DA_CAPA` do
    // `biblioteca.rs`, que decide quando ampliar sem interpolar —, e o título ocupa duas linhas.
    readonly property int larguraDoCartao: 136
    readonly property int alturaDoCartao: 145 + 36 + 24

    clip: true
    cellWidth: larguraDoCartao + 8
    cellHeight: alturaDoCartao + 8
    model: biblioteca
    focus: true
    // A grade não dá a volta: descer da última linha fica na última.
    keyNavigationWraps: false
    keyNavigationEnabled: escutando
    highlightFollowsCurrentItem: true
    ScrollBar.vertical: ScrollBar {}

    Keys.onReturnPressed: if (escutando) abre(currentIndex)
    Keys.onEnterPressed: if (escutando) abre(currentIndex)
    Keys.onSpacePressed: if (escutando) abre(currentIndex)
    // O X e o C são as configurações e os detalhes, como o b2 e o b3 do controle — no teclado de
    // fábrica é onde eles ficam. A tecla Menu e o Shift+F10 abrem o menu do cartão escolhido,
    // como o clique direito.
    Keys.onPressed: (evento) => {
        evento.accepted = false
        if (!escutando || evento.isAutoRepeat || count === 0)
            return
        if (evento.key === Qt.Key_X) {
            configura(currentIndex)
        } else if (evento.key === Qt.Key_C) {
            inspeciona(currentIndex)
        } else if (evento.key === Qt.Key_Menu
                   || (evento.key === Qt.Key_F10 && (evento.modifiers & Qt.ShiftModifier))) {
            menuDoJogo.linha = currentIndex
            if (currentItem)
                menuDoJogo.popup(currentItem, currentItem.width / 2, currentItem.height / 2)
            else
                menuDoJogo.popup()
        } else {
            return
        }
        evento.accepted = true
    }

    // Devolve `valor`, e faz a ligação depender das `versoes`: ver o `depende` do slider.
    function depende(versoes, valor) {
        return valor
    }
    function tr(chave) {
        return depende([Idioma.versao], Idioma.texto(chave))
    }

    // Um só menu para a grade toda, aberto sobre o cartão do clique.
    Menu {
        id: menuDoJogo

        property int linha: -1

        MenuItem {
            text: grade.tr("library.menu.configure") + "  (X)"
            onTriggered: grade.configura(menuDoJogo.linha)
        }
        MenuItem {
            text: grade.tr("library.menu.details") + "  (C)"
            onTriggered: grade.inspeciona(menuDoJogo.linha)
        }
    }

    // O título e a descrição entram em texto rico na dica: um "&" ou um "<" no nome não pode
    // virar marcação.
    function escapa(texto) {
        return texto.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    }

    // O que o controle pede, pela `Biblioteca.comandos`: as direções andam, e abrir joga.
    function comando(codigo) {
        switch (codigo) {
        case 0: moveCurrentIndexUp(); break
        case 1: moveCurrentIndexDown(); break
        case 2: moveCurrentIndexLeft(); break
        case 3: moveCurrentIndexRight(); break
        case 4: abre(currentIndex); break
        }
    }

    delegate: Item {
        id: cartao

        required property int index
        required property string titulo
        required property string descricao
        required property string classe
        required property string capa
        required property bool capaNitida

        readonly property bool escolhido: GridView.isCurrentItem

        width: grade.cellWidth
        height: grade.cellHeight

        Rectangle {
            id: fundo

            anchors.centerIn: parent
            width: grade.larguraDoCartao
            height: grade.alturaDoCartao
            radius: 8
            color: toque.containsMouse ? palette.midlight : palette.button
            border.color: cartao.escolhido ? palette.highlight : palette.mid
            border.width: cartao.escolhido ? 2 : 1

            // A imagem cabe no quadro sem esticar: um ícone de 65×42 deformado até virar
            // quadrado fica pior que um com sobra dos lados. E um ícone pequeno amplia sem
            // interpolar, que é o bloco quadrado que o console mostrava.
            Image {
                id: imagem

                x: (parent.width - 112) / 2
                y: 12
                width: 112
                height: 145
                fillMode: Image.PreserveAspectFit
                source: cartao.capa
                smooth: !cartao.capaNitida
                mipmap: !cartao.capaNitida
            }

            Text {
                anchors.top: imagem.bottom
                anchors.topMargin: 8
                anchors.horizontalCenter: parent.horizontalCenter
                width: parent.width - 16
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                maximumLineCount: 2
                elide: Text.ElideRight
                font.pixelSize: 12
                color: palette.buttonText
                text: cartao.titulo
            }

            // O escolhido ganha o preenchimento translúcido, como a seleção da Z-Wheel.
            Rectangle {
                anchors.fill: parent
                radius: parent.radius
                visible: cartao.escolhido
                color: palette.highlight
                opacity: 0.16
            }

            MouseArea {
                id: toque

                anchors.fill: parent
                hoverEnabled: true
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                // O clique devolve o teclado à grade: vindo da busca, as setas continuavam no campo.
                // O direito escolhe o cartão e abre o menu dele, sem jogar.
                onClicked: (mouse) => {
                    grade.forceActiveFocus()
                    grade.currentIndex = cartao.index
                    if (mouse.button === Qt.RightButton) {
                        menuDoJogo.linha = cartao.index
                        menuDoJogo.popup()
                    } else {
                        grade.abre(cartao.index)
                    }
                }
            }

            // O cartão corta o título comprido, então o nome inteiro fica à espera do ponteiro,
            // com a descrição da Z-Wheel quando ela conhece o jogo. A largura tem teto, como os
            // 320 pontos do egui: sem ele o texto quebrava numa palavra por linha.
            ToolTip {
                visible: toque.containsMouse
                delay: 500
                width: Math.min(implicitWidth, 320)
                contentItem: Label {
                    // A cor da dica, e não a do texto da janela: num tema de dica escura com
                    // janela clara, o texto sumia.
                    color: palette.toolTipText
                    wrapMode: Text.Wrap
                    text: "<b>" + grade.escapa(cartao.titulo) + "</b>"
                          + (cartao.descricao !== "" ? "<br><br>" + grade.escapa(cartao.descricao) : "")
                          + (cartao.classe !== "" ? "<br><br><small>" + cartao.classe + "</small>" : "")
                }
            }
        }
    }
}
