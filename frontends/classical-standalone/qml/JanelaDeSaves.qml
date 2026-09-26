import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import zeebx

// O gerenciador de saves: uma janela do sistema, como no egui. Duas listas, e a separação é a do
// console: o que o jogo escreveu na pasta dele, e o que está no sistema de arquivos do aparelho —
// que é de todos. Apagar o `zeeboiddata` tira os bonecos do Zeeboids **e** o que o Zeebo F.C. lê
// deles; a dica embaixo do título diz isso, porque a lista sozinha não diria.
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

    width: 520
    height: 420
    minimumWidth: 380
    minimumHeight: 260
    title: "Zeebx — " + tr("saves.title")

    function tr(chave) {
        return depende([Idioma.versao], Idioma.texto(chave))
    }

    // A lista toca o disco: é relida ao abrir a janela e depois de cada exclusão, e não a cada
    // desenho.
    function abre() {
        recado.text = ""
        saves.recarrega()
        show()
        raise()
        requestActivate()
    }

    onClosing: {
        confirmacao.close()
        recado.text = ""
    }

    Saves {
        id: saves
    }

    // Os índices dos saves de cada lista, na ordem em que vieram.
    function indices(doAparelho) {
        const lista = []
        const quantos = depende([saves.versao], saves.quantos())
        for (let i = 0; i < quantos; i++) {
            if (saves.doAparelho(i) === doAparelho)
                lista.push(i)
        }
        return lista
    }
    readonly property var dosJogos: indices(false)
    readonly property var doAparelho: indices(true)

    // Uma linha da lista: o que é, quanto ocupa, e o botão de excluir.
    component Linha: RowLayout {
        id: linha

        required property int modelData

        width: ListView.view ? ListView.view.width : parent.width

        Label {
            Layout.fillWidth: true
            elide: Text.ElideRight
            text: janela.depende([saves.versao], saves.titulo(linha.modelData))
        }
        Label {
            text: janela.depende([saves.versao, Idioma.versao], saves.resumo(linha.modelData))
        }
        Button {
            text: janela.tr("saves.delete")
            onClicked: confirmacao.pede(linha.modelData)
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 8
        spacing: 6

        // O que aconteceu na última exclusão.
        RowLayout {
            Layout.fillWidth: true
            visible: recado.text !== ""

            Label {
                id: recado

                Layout.fillWidth: true
                wrapMode: Text.Wrap
            }
            Button {
                text: janela.tr("saves.confirm.no")
                onClicked: recado.text = ""
            }
        }

        Label {
            Layout.fillWidth: true
            visible: janela.dosJogos.length === 0 && janela.doAparelho.length === 0
            wrapMode: Text.Wrap
            text: janela.tr("saves.none")
        }

        ScrollView {
            id: rolagem

            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth

            ColumnLayout {
                width: rolagem.availableWidth
                spacing: 4

                Label {
                    visible: janela.dosJogos.length > 0
                    font.bold: true
                    font.pixelSize: 18
                    text: janela.tr("saves.games")
                }
                Repeater {
                    model: janela.dosJogos
                    delegate: Linha {
                        Layout.fillWidth: true
                    }
                }

                Label {
                    Layout.topMargin: janela.dosJogos.length > 0 ? 12 : 0
                    visible: janela.doAparelho.length > 0
                    font.bold: true
                    font.pixelSize: 18
                    text: janela.tr("saves.device")
                }
                Label {
                    Layout.fillWidth: true
                    visible: janela.doAparelho.length > 0
                    wrapMode: Text.Wrap
                    text: janela.tr("saves.device.hint")
                }
                Repeater {
                    model: janela.doAparelho
                    delegate: Linha {
                        Layout.fillWidth: true
                    }
                }
            }
        }
    }

    // A confirmação é modal de propósito: apagar não tem volta, e um clique errado num botão de
    // lista é fácil demais.
    Dialog {
        id: confirmacao

        property int indice: -1

        function pede(indice) {
            confirmacao.indice = indice
            open()
        }

        anchors.centerIn: parent
        width: Math.min(420, janela.width - 32)
        modal: true
        title: janela.tr("saves.title")

        Label {
            width: parent.width
            wrapMode: Text.Wrap
            text: confirmacao.indice < 0 ? ""
                : janela.depende([saves.versao, Idioma.versao], saves.pergunta(confirmacao.indice))
        }

        footer: DialogButtonBox {
            Button {
                text: janela.tr("saves.confirm.yes")
                DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            }
            Button {
                text: janela.tr("saves.confirm.no")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
        }

        onAccepted: recado.text = saves.apaga(indice)
    }
}
