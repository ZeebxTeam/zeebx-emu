import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Window

import zeebx

// A janela principal: a biblioteca, em grade ou no slider. O jogo, as configurações, os saves e o
// log abrem cada um na sua janela, como no egui; os avisos de abertura e de versão nova são
// diálogos por cima da biblioteca. Ver `docs/implementacao/21-migracao-para-qt.md`.
// É uma `ApplicationWindow`, e não uma `Window`, para o fundo e os textos soltos usarem a mesma
// paleta dos botões e campos: com a cor da janela tirada do sistema e a dos controles do estilo,
// o texto saía escuro sobre fundo escuro num tema escuro.
ApplicationWindow {
    id: principal

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

    // O jogo pedido na linha de comando, se houve: abre direto, sem passar pela lista.
    required property string jogoInicial

    width: 960
    height: 720
    minimumWidth: 480
    minimumHeight: 360
    title: "Zeebx"

    // Fechar a biblioteca encerra tudo, com ou sem jogo aberto — como no egui.
    onClosing: Qt.quit()

    Component.onCompleted: {
        aplicaModo()
        if (avisos.deAbertura())
            boasVindas.open()
        // Depois de a montagem acabar, e não dentro dela: no Qt 6.4 do Ubuntu 24.04, mostrar a
        // janela do jogo daqui a deixava sem aparecer — o `visible` voltava falso e a janela nunca
        // era mapeada. No 6.11 passava. Aberto pela biblioteca, já montada, os dois funcionam.
        if (jogoInicial !== "")
            Qt.callLater(() => principal.mostra(biblioteca.abreCaminho(jogoInicial)))
    }

    // O modo configurado para a janela principal (`graphics.janela`), como no egui. As
    // configurações chamam isto de novo quando ele muda: na principal, vale na hora.
    function aplicaModo() {
        const modo = biblioteca.modoDaJanela()
        if (modo === 2)
            showFullScreen()
        else if (modo === 1)
            showMaximized()
        else
            showNormal()
    }

    // Os textos se refazem sozinhos quando o idioma muda: a ligação que chama isto lê a versão.
    function tr(chave) {
        return depende([Idioma.versao], Idioma.texto(chave))
    }

    // `erro` vazio é jogo aberto.
    function mostra(erro) {
        aviso.text = erro
        if (erro === "")
            jogo.abre()
    }

    // O jogo escolhido agora, pela linha da lista.
    function escolhido() {
        return emSlider ? slider.linha(slider.cursor) : grade.currentIndex
    }

    // A busca ou a varredura refizeram a lista: a escolha volta ao primeiro, sem a animação do
    // slider atravessar a lista.
    function recomeca() {
        grade.currentIndex = 0
        slider.reinicia()
    }

    // O modo da biblioteca vem das configurações: 0 grade, 1 slider. Relido quando elas mudam.
    readonly property bool emSlider: principal.depende([configuracoes.cfg.versao], biblioteca.modoDaBiblioteca() === 1)
    readonly property Item vista: emSlider ? slider : grade
    // Trocado o modo nas configurações, as setas passam à vista nova.
    onVistaChanged: vista.forceActiveFocus()

    Biblioteca {
        id: biblioteca
    }

    // **O jogo não é filho da principal.** Uma janela declarada dentro de outra ganha ela como
    // `transientParent`, e o gerenciador de janelas mantém a filha sempre acima do pai: clicar na
    // principal durante o jogo, para mexer em alguma coisa, não a trazia para a frente. No egui
    // as duas também são independentes.
    Jogo {
        id: jogo

        transientParent: null
    }

    JanelaDeConfiguracoes {
        id: configuracoes

        biblioteca: biblioteca
        principal: principal
        visible: false
    }

    JanelaDeSaves {
        id: saves

        visible: false
    }

    // O log acompanha o jogo, com a opção ligada. Liga-se e desliga-se no meio do jogo, pelas
    // configurações, como no egui. É filho da janela do jogo, e não da principal: fica sempre por
    // cima dele, e não atrás.
    JanelaDeLog {
        id: janelaDeLog

        transientParent: jogo

        pedida: principal.depende([configuracoes.cfg.versao, janelaDeLog.dispensas],
                                  jogo.visible && janelaDeLog.log.ativo())
    }

    Avisos {
        id: avisos
    }

    // **Com outra janela por cima, a biblioteca não escuta o controle nem o teclado** — nem se a
    // principal voltar a ter o foco. Mapeando o controle nas configurações, o botão apertado
    // abriria um jogo sem querer. O mouse continua valendo, como no egui. Toda janela que abrir
    // por cima da principal entra aqui.
    readonly property bool sobreposta: jogo.visible || configuracoes.visible || saves.visible
                                       || boasVindas.visible || novaVersao.visible

    // O controle não gera evento no Qt, como não gerava no egui: enquanto a biblioteca está à
    // vista, ele é lido aqui. Sem o foco, ou com outra janela por cima, o controle não é dela — e
    // escrevendo na busca ele continua navegando, como no egui. Ao voltar a escutar, o que já
    // estava apertado não conta: a navegação é silenciada enquanto não escuta.
    Timer {
        interval: 33
        repeat: true
        running: true
        onTriggered: {
            // A resposta da procura por versão nova é lida pelos `comandos`: o aviso dela vem
            // depois do de abertura, nunca junto.
            if (!boasVindas.visible && !novaVersao.visible) {
                const texto = avisos.deAtualizacao()
                if (texto !== "") {
                    novaVersao.texto = texto
                    novaVersao.open()
                }
            }
            const escutando = principal.active && !principal.sobreposta
            for (const codigo of biblioteca.comandos(escutando)) {
                if (codigo === 5)
                    principal.mostra(biblioteca.abreZWheel())
                else
                    principal.vista.comando(codigo)
            }
        }
    }

    // F11, ou Alt+Enter, põe e tira a biblioteca da tela cheia, como no egui. O Alt+Enter só vale
    // com o Alt: o Enter sozinho é botão do controle no teclado.
    Shortcut {
        sequences: ["F11", "Alt+Return", "Alt+Enter"]
        onActivated: principal.visibility = principal.visibility === Window.FullScreen
                     ? Window.Windowed : Window.FullScreen
    }

    // Ctrl+F leva à busca, de qualquer lugar da janela. Em `sequences`, e não em `sequence`: onde
    // o sistema dá mais de uma tecla ao "procurar" (Ctrl+F e F3), o Qt 6.11 liga só a primeira e
    // avisa no terminal.
    Shortcut {
        sequences: [StandardKey.Find]
        onActivated: busca.forceActiveFocus()
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 8
        spacing: 6

        // A barra de cima. **Os botões dela não pegam o foco do teclado**, nem os da linha da
        // contagem e o do recado: clicado, um botão do Qt Quick fica com o foco, e as setas
        // passavam a ir para ele, e não para a grade ou para o slider. No egui um clique não
        // prendia o teclado.
        RowLayout {
            Layout.fillWidth: true
            spacing: 8

            Label {
                text: "Zeebx"
                font.pixelSize: 20
                font.bold: true
            }

            Button {
                focusPolicy: Qt.NoFocus
                text: "▶ " + tr("nav.z_wheel")
                enabled: principal.depende([configuracoes.cfg.versao], biblioteca.temZWheel())
                onClicked: principal.mostra(biblioteca.abreZWheel())
                ToolTip.visible: hovered
                ToolTip.delay: 500
                ToolTip.text: tr(enabled ? "nav.z_wheel.hint" : "nav.z_wheel.missing")
            }

            // A busca: filtra a lista pelo nome, sem ligar para acentos. Esc limpa; o Enter joga
            // o escolhido, que é o primeiro resultado enquanto ninguém mexe na escolha.
            TextField {
                id: busca

                Layout.preferredWidth: 220
                placeholderText: tr("nav.search")
                onTextChanged: {
                    biblioteca.busca(text)
                    principal.recomeca()
                }
                Keys.onEscapePressed: {
                    text = ""
                    principal.vista.forceActiveFocus()
                }
                Keys.onReturnPressed: if (!principal.sobreposta) principal.mostra(biblioteca.abre(principal.escolhido()))
                Keys.onEnterPressed: if (!principal.sobreposta) principal.mostra(biblioteca.abre(principal.escolhido()))
                ToolTip.visible: hovered
                ToolTip.delay: 500
                ToolTip.text: tr("nav.search.hint")
            }

            ToolButton {
                focusPolicy: Qt.NoFocus
                text: "✕"
                visible: busca.text !== ""
                // Como o Esc da busca: a lista volta a ser das setas.
                onClicked: {
                    busca.text = ""
                    principal.vista.forceActiveFocus()
                }
                ToolTip.visible: hovered
                ToolTip.text: tr("nav.search.clear")
            }

            Item {
                Layout.fillWidth: true
            }

            Button {
                focusPolicy: Qt.NoFocus
                text: tr("nav.saves")
                onClicked: saves.abre()
            }

            Button {
                focusPolicy: Qt.NoFocus
                text: tr("nav.settings")
                onClicked: configuracoes.abre(-1)
            }

            // O Zeeboids só deixa sincronizar uma vez por dia, e a trava é dele: guarda a data no
            // próprio banco. Testar rede com isso custa um dia por tentativa, então o botão recua
            // a data em um dia.
            Button {
                focusPolicy: Qt.NoFocus
                text: tr("nav.unlock_sync")
                onClicked: recado.text = biblioteca.liberaSincronizacao()
                ToolTip.visible: hovered
                ToolTip.delay: 500
                ToolTip.text: tr("nav.unlock_sync.hint")
            }
        }

        RowLayout {
            Layout.fillWidth: true
            visible: recado.text !== ""

            Label {
                id: recado

                Layout.fillWidth: true
                wrapMode: Text.Wrap
            }
            Button {
                focusPolicy: Qt.NoFocus
                text: tr("nav.unlock_sync.ok")
                onClicked: recado.text = ""
            }
        }

        // A contagem, o procurar de novo e o que apertar.
        RowLayout {
            Layout.fillWidth: true

            Label {
                text: biblioteca.contagem
            }
            Button {
                focusPolicy: Qt.NoFocus
                text: tr("library.rescan")
                onClicked: {
                    biblioteca.procuraDeNovo()
                    principal.recomeca()
                }
            }
            Label {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignRight
                elide: Text.ElideLeft
                opacity: 0.6
                text: tr("library.controls_hint")
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 1
            color: palette.mid
        }

        // O que deu errado na última tentativa de abrir um jogo.
        Label {
            id: aviso

            Layout.fillWidth: true
            visible: text !== ""
            color: "#d04040"
            wrapMode: Text.Wrap
        }

        // A lista vazia diz por quê: sem pasta, pasta sem jogos, ou busca sem resultado.
        Label {
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: biblioteca.vazio !== ""
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignTop
            topPadding: 48
            wrapMode: Text.Wrap
            text: biblioteca.vazio
        }

        GradeDaBiblioteca {
            id: grade

            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: !principal.emSlider && biblioteca.vazio === ""
            // **O foco é de quem está à vista.** As duas declaravam `focus: true`, e o Qt dá o foco
            // a um só, o primeiro: no modo slider, as setas andavam a grade escondida e o slider
            // ficava parado — medido pelo `currentIndex` da grade subindo com o `cursor` em zero.
            focus: !principal.emSlider
            escutando: !principal.sobreposta
            biblioteca: biblioteca
            onAbre: (linha) => principal.mostra(biblioteca.abre(linha))
        }

        SliderDaBiblioteca {
            id: slider

            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: principal.emSlider && biblioteca.vazio === ""
            focus: principal.emSlider
            escutando: !principal.sobreposta
            biblioteca: biblioteca
            onAbre: (linha) => principal.mostra(biblioteca.abre(linha))
        }
    }

    // O aviso de abertura: o emulador ainda em desenvolvimento, e o controle a configurar antes
    // de jogar. A caixa marcada guarda a versão, e a próxima versão mostra o aviso de novo.
    Dialog {
        id: boasVindas

        anchors.centerIn: parent
        width: Math.min(460, principal.width - 32)
        modal: true
        title: principal.tr("welcome.title")

        ColumnLayout {
            width: parent.width
            spacing: 8

            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: principal.tr("welcome.development")
            }
            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: principal.tr("welcome.controls")
            }
            CheckBox {
                id: naoMostrar

                Layout.topMargin: 4
                text: principal.tr("welcome.dont_show")
            }
        }

        footer: DialogButtonBox {
            Button {
                text: principal.tr("welcome.controls.open")
                onClicked: {
                    boasVindas.close()
                    configuracoes.abre(1)
                }
            }
            Button {
                text: principal.tr("welcome.dismiss")
                DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            }
        }

        // Fechado por qualquer caminho — um dos botões ou o Esc —, o aviso sai da frente.
        onClosed: avisos.dispensaAbertura(naoMostrar.checked)
    }

    // O aviso de versão nova, se a procura ao abrir achou uma.
    Dialog {
        id: novaVersao

        property string texto: ""

        anchors.centerIn: parent
        width: Math.min(460, principal.width - 32)
        modal: true
        title: principal.tr("update.title")

        Label {
            width: parent.width
            wrapMode: Text.Wrap
            text: novaVersao.texto
        }

        footer: DialogButtonBox {
            Button {
                text: principal.tr("update.download")
                onClicked: {
                    Qt.openUrlExternally(avisos.paginaDaAtualizacao())
                    novaVersao.close()
                }
            }
            Button {
                text: principal.tr("update.later")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
        }

        onClosed: avisos.dispensaAtualizacao()
    }
}
