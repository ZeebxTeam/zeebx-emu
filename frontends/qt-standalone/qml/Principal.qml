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

    width: larguraGuardada > 0 ? larguraGuardada : 960
    height: alturaGuardada > 0 ? alturaGuardada : 720
    minimumWidth: 480
    minimumHeight: 360
    title: jogandoNaPrincipal ? "Zeebx — " + palcoDaPrincipal.tela.estado : "Zeebx"

    // O tamanho em que a janela foi fechada é o tamanho em que ela abre. Chamadas de
    // função não têm sinal de mudança: avaliam uma vez, na montagem, que é quando valem.
    readonly property real larguraGuardada: biblioteca.larguraDaJanela()
    readonly property real alturaGuardada: biblioteca.alturaDaJanela()

    // E o contrário: arrasta, a janela guarda. Em tela cheia ou maximizada, o tamanho é o da
    // tela, e não escolha do usuário — aí não se guarda.
    function guardaTamanho() {
        if (visibility === Window.Windowed)
            biblioteca.guardaTamanhoDaJanela(width, height)
    }

    // Arrastar a borda passa por dezenas de tamanhos por segundo, e cada um escreveria o
    // `settings.json` inteiro. A gravação espera a mão parar.
    onWidthChanged: filaDaGravacao.restart()
    onHeightChanged: filaDaGravacao.restart()

    Timer {
        id: filaDaGravacao
        interval: 500
        onTriggered: principal.guardaTamanho()
    }

    // A janela perdeu o foco com o jogo nela: nenhuma tecla continua apertada.
    onActiveChanged: if (!active && jogandoNaPrincipal) palcoDaPrincipal.tela.solta()

    // Fechar a biblioteca encerra tudo, com ou sem jogo aberto — como no egui.
    onClosing: {
        filaDaGravacao.stop()
        principal.guardaTamanho()
        Qt.quit()
    }

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
        // Com o jogo nela, vale o modo do jogo; o da principal volta quando ele fechar.
        if (jogandoNaPrincipal)
            return
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
    //
    // **Onde o jogo aparece só se decide sem jogo rodando.** Um jogo aberto por cima de outro (a
    // biblioteca usada com a janela separada, ou a Z-Wheel) fica onde o anterior estava: com os
    // dois palcos ativos, o núcleo andaria duas vezes por quadro. A opção vale no próximo jogo.
    function mostra(erro) {
        aviso.text = erro
        if (erro !== "")
            return
        if (jogo.visible)
            jogo.abre()
        else if (jogandoNaPrincipal || !configuracoes.cfg.valor("graphics.jogo_separado"))
            jogaNaPrincipal()
        else
            jogo.abre()
    }

    // O jogo na principal, no lugar da biblioteca. A janela fica no modo configurado para o jogo
    // (`graphics.janela_do_jogo`) enquanto ele roda, e volta ao que era quando ele fecha.
    property bool jogandoNaPrincipal: false
    property int visibilidadeAntesDoJogo: Window.Windowed
    // O palco onde o jogo está, para o menu, a barra de ferramentas e a de status.
    readonly property var palco: jogandoNaPrincipal ? palcoDaPrincipal : jogo.palco
    readonly property bool jogando: jogo.visible || jogandoNaPrincipal

    function jogaNaPrincipal() {
        if (!jogandoNaPrincipal) {
            visibilidadeAntesDoJogo = visibility
            jogandoNaPrincipal = true
            const modo = palcoDaPrincipal.tela.modoDaJanela()
            if (modo === 2)
                showFullScreen()
            else if (modo === 1)
                showMaximized()
            else
                showNormal()
        }
        palcoDaPrincipal.prepara()
        palcoDaPrincipal.foca()
        raise()
        requestActivate()
    }

    // Fecha o jogo de onde ele estiver. Na principal, a biblioteca volta, com a janela como era.
    function fechaJogo() {
        if (jogo.visible) {
            jogo.close()
            return
        }
        if (!jogandoNaPrincipal)
            return
        palcoDaPrincipal.fecha()
        jogandoNaPrincipal = false
        if (visibilidadeAntesDoJogo === Window.FullScreen)
            showFullScreen()
        else if (visibilidadeAntesDoJogo === Window.Maximized)
            showMaximized()
        else
            showNormal()
        principal.vista.forceActiveFocus()
    }

    // O jogo escolhido agora, pela linha da lista.
    function escolhido() {
        return emSlider ? slider.linha(slider.cursor) : grade.currentIndex
    }

    // As configurações próprias do jogo da linha: a mesma janela de configurações, só com as abas
    // que valem por jogo. Ver `Nucleo::com_o_jogo`.
    function configuraJogo(linha) {
        const chave = biblioteca.chave(linha)
        if (chave !== "")
            configuracoesDoJogo.abreDoJogo(chave, biblioteca.item(linha).titulo)
    }

    // O que se sabe do jogo da linha, num diálogo.
    function inspecionaJogo(linha) {
        if (biblioteca.linhas() === 0)
            return
        detalhes.linha = linha
        detalhes.dados = biblioteca.detalhes(linha)
        detalhes.open()
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

    // As configurações de um jogo só. Uma janela à parte, e não a de cima no modo do jogo: as
    // duas podem estar abertas, e cada uma edita o que é seu.
    JanelaDeConfiguracoes {
        id: configuracoesDoJogo

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

        transientParent: principal.jogandoNaPrincipal ? principal : jogo

        pedida: principal.depende([configuracoes.cfg.versao, janelaDeLog.dispensas],
                                  principal.jogando && janelaDeLog.log.ativo())
    }

    Avisos {
        id: avisos
    }

    // **Com outra janela por cima, a biblioteca não escuta o controle nem o teclado** — nem se a
    // principal voltar a ter o foco. Mapeando o controle nas configurações, o botão apertado
    // abriria um jogo sem querer. O mouse continua valendo, como no egui. Toda janela que abrir
    // por cima da principal entra aqui.
    readonly property bool sobreposta: principal.jogando || configuracoes.visible || saves.visible
                                       || configuracoesDoJogo.visible || detalhes.visible
                                       || boasVindas.visible || novaVersao.visible
                                       || sobre.visible || respostaDaProcura.visible
                                       || barraDeMenus.aberto

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
                    novaVersao.pagina = avisos.paginaDaAtualizacao()
                    novaVersao.open()
                }
            }
            // A procura pelo menu Ajuda responde mesmo sem versão nova.
            if (!respostaDaProcura.visible) {
                const resposta = avisos.resultadoDaProcura()
                if (resposta !== "") {
                    respostaDaProcura.texto = resposta
                    respostaDaProcura.open()
                }
            }
            const escutando = principal.active && !principal.sobreposta
            for (const codigo of biblioteca.comandos(escutando)) {
                if (codigo === 5)
                    principal.mostra(biblioteca.abreZWheel())
                else if (codigo === 6)
                    principal.configuraJogo(principal.escolhido())
                else if (codigo === 7)
                    principal.inspecionaJogo(principal.escolhido())
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
        enabled: !principal.jogandoNaPrincipal
        onActivated: busca.forceActiveFocus()
    }

    // Mostrar a barra de ferramentas e a de status: o menu Exibir liga e desliga, e fica gravado.
    readonly property bool comFerramentas: depende([configuracoes.cfg.versao], configuracoes.cfg.valor("barras.ferramentas"))
    readonly property bool comStatus: depende([configuracoes.cfg.versao], configuracoes.cfg.valor("barras.status"))
    // Estreita demais para os rótulos, a barra de ferramentas fica só com os ícones, e o nome vai
    // para a dica. A largura é a que os rótulos em português pedem, os mais longos dos quatro.
    readonly property bool barraCompacta: width < 1200

    // O que o menu Sistema mostra dos slots, lido quando ele abre: cada leitura toca o disco.
    property var slotsDoMenu: []
    property var ocupadosDoMenu: []
    property int slotDoMenu: 0

    function atalho(nome) {
        return depende([configuracoes.cfg.versao], configuracoes.cfg.atalho(nome))
    }

    // A tela cheia da barra e do menu: a do jogo, se ele está aberto, e a da biblioteca, se não.
    function alternaTelaCheia() {
        if (jogo.visible)
            jogo.alternaTelaCheia()
        else
            principal.visibility = principal.visibility === Window.FullScreen ? Window.Windowed : Window.FullScreen
    }

    // Um item de menu com o ícone na frente e a tecla do atalho à direita. **A tecla é só texto**:
    // um `shortcut` de verdade valeria na janela inteira, e o P digitado na busca pausaria o jogo.
    // Quem atende as teclas é a janela do jogo, como sempre.
    component ItemDeMenu: MenuItem {
        id: itemDeMenu

        property string icone: ""
        property string atalho: ""

        // O indicador do item marcável ocupa a frente: a linha começa depois dele.
        readonly property real recuo: checkable && indicator ? indicator.width + spacing : 0

        contentItem: Item {
            implicitWidth: linha.implicitWidth + itemDeMenu.recuo
            implicitHeight: linha.implicitHeight

            RowLayout {
                id: linha

                anchors.fill: parent
                anchors.leftMargin: itemDeMenu.recuo
                spacing: 8
                opacity: itemDeMenu.enabled ? 1 : 0.45

                // Vazio, o ícone ainda ocupa o lugar: os textos do menu ficam alinhados.
                Icone {
                    Layout.preferredWidth: 16
                    visible: itemDeMenu.icone !== "" || !itemDeMenu.checkable
                    nome: itemDeMenu.icone
                    tamanho: 15
                    color: itemDeMenu.highlighted ? itemDeMenu.palette.highlightedText : itemDeMenu.palette.windowText
                }
                Label {
                    Layout.fillWidth: true
                    text: itemDeMenu.text
                    color: itemDeMenu.highlighted ? itemDeMenu.palette.highlightedText : itemDeMenu.palette.windowText
                }
                Label {
                    Layout.leftMargin: 24
                    visible: itemDeMenu.atalho !== ""
                    opacity: 0.65
                    text: itemDeMenu.atalho
                    color: itemDeMenu.highlighted ? itemDeMenu.palette.highlightedText : itemDeMenu.palette.windowText
                }
            }
        }
    }

    // Um slot no menu Sistema: `acao` 0 salva e 1 carrega. O slot atual mostra a tecla.
    component ItemDeSlot: ItemDeMenu {
        required property int slot
        property int acao: 0

        text: principal.slotsDoMenu[slot] !== undefined ? principal.slotsDoMenu[slot] : ""
        enabled: acao === 0 || principal.ocupadosDoMenu[slot] === 1
        atalho: slot === principal.slotDoMenu
                ? principal.atalho(acao === 0 ? "salvar_estado" : "carregar_estado") : ""
        onTriggered: principal.palco.tela.estadoPeloMenu(acao, slot)
    }

    // Um botão da barra de ferramentas: o ícone, e o nome ao lado enquanto couber. **Não pega o
    // foco do teclado**: clicado, um botão do Qt Quick ficava com ele, e as setas deixavam de ir
    // para a grade ou para o slider. No egui um clique não prendia o teclado.
    component BotaoDeFerramenta: ToolButton {
        id: botao

        property string icone
        property string dica: ""

        focusPolicy: Qt.NoFocus
        contentItem: RowLayout {
            spacing: 6
            opacity: botao.enabled ? 1 : 0.4

            Icone {
                Layout.preferredWidth: 20
                nome: botao.icone
                tamanho: 18
                color: botao.palette.buttonText
            }
            Label {
                visible: !principal.barraCompacta
                text: botao.text
                color: botao.palette.buttonText
            }
        }
        ToolTip.visible: hovered && (principal.barraCompacta || dica !== "")
        ToolTip.delay: 500
        ToolTip.text: dica !== "" ? dica : text
    }

    component Separador: Rectangle {
        Layout.preferredWidth: 1
        Layout.preferredHeight: 22
        Layout.leftMargin: 4
        Layout.rightMargin: 4
        color: palette.mid
    }

    // Com o jogo na principal em tela cheia, as três barras saem, como no PCSX2: a tela é do jogo.
    readonly property bool soOJogo: jogandoNaPrincipal && visibility === Window.FullScreen

    menuBar: MenuBar {
        id: barraDeMenus

        visible: !principal.soOJogo

        // Com um menu aberto, o controle não navega a biblioteca por baixo dele.
        readonly property bool aberto: menuArquivo.visible || menuSistema.visible
                                       || menuConfiguracoes.visible || menuExibir.visible
                                       || menuFerramentas.visible || menuAjuda.visible

        Menu {
            id: menuArquivo

            title: tr("menu.file")

            ItemDeMenu {
                icone: "disc-3"
                text: tr("menu.file.z_wheel")
                enabled: principal.depende([configuracoes.cfg.versao], biblioteca.temZWheel())
                onTriggered: principal.mostra(biblioteca.abreZWheel())
            }
            ItemDeMenu {
                icone: "refresh-cw"
                text: tr("menu.file.rescan")
                onTriggered: {
                    biblioteca.procuraDeNovo()
                    principal.recomeca()
                }
            }
            MenuSeparator {}
            ItemDeMenu {
                icone: "log-out"
                text: tr("menu.file.quit")
                onTriggered: principal.close()
            }
        }

        // O que se faz com o jogo aberto. Sem jogo, tudo desabilitado. O rewind não está aqui:
        // voltar só faz sentido segurando a tecla, e um item de menu não se segura.
        Menu {
            id: menuSistema

            title: tr("menu.system")
            onAboutToShow: {
                principal.slotsDoMenu = principal.jogando ? principal.palco.tela.slotsDeEstado() : []
                principal.ocupadosDoMenu = principal.jogando ? principal.palco.tela.slotsOcupados() : []
                principal.slotDoMenu = principal.jogando ? principal.palco.tela.slotAtual() : 0
                itemDoAvanco.checked = principal.jogando && principal.palco.tela.avancoFixo()
                itemDoDesfazer.enabled = principal.jogando && principal.palco.tela.podeDesfazerCarregar()
            }

            ItemDeMenu {
                icone: principal.palco.pausado ? "play" : "pause"
                text: tr(principal.palco.pausado ? "menu.system.resume" : "menu.system.pause")
                atalho: "P"
                enabled: principal.jogando
                onTriggered: principal.palco.pausa()
            }
            ItemDeMenu {
                icone: "rotate-ccw"
                text: tr("menu.system.restart")
                atalho: principal.atalho("reiniciar")
                enabled: principal.jogando
                onTriggered: principal.palco.reinicia()
            }
            MenuSeparator {}
            ItemDeMenu {
                id: itemDoAvanco

                checkable: true
                text: tr("menu.system.fast_forward")
                atalho: principal.atalho("avancar")
                enabled: principal.jogando
                onTriggered: checked = principal.palco.tela.alternaAvanco()
            }
            ItemDeMenu {
                icone: "camera"
                text: tr("menu.system.screenshot")
                atalho: principal.atalho("screenshot")
                enabled: principal.jogando
                onTriggered: principal.palco.screenshot()
            }
            MenuSeparator {}
            Menu {
                title: tr("menu.system.save_state")
                // Largura que caiba o slot ocupado: o Menu mede os itens na primeira vez que abre,
                // e não cresce quando um slot vazio passa a ocupado.
                width: Math.max(implicitWidth, 340)
                enabled: principal.jogando

                ItemDeSlot { slot: 0; icone: "save" }
                ItemDeSlot { slot: 1; icone: "save" }
                ItemDeSlot { slot: 2; icone: "save" }
                ItemDeSlot { slot: 3; icone: "save" }
                ItemDeSlot { slot: 4; icone: "save" }
                ItemDeSlot { slot: 5; icone: "save" }
                ItemDeSlot { slot: 6; icone: "save" }
                ItemDeSlot { slot: 7; icone: "save" }
                ItemDeSlot { slot: 8; icone: "save" }
                ItemDeSlot { slot: 9; icone: "save" }
            }
            Menu {
                title: tr("menu.system.load_state")
                // Largura que caiba o slot ocupado: o Menu mede os itens na primeira vez que abre,
                // e não cresce quando um slot vazio passa a ocupado.
                width: Math.max(implicitWidth, 340)
                enabled: principal.jogando

                ItemDeSlot { slot: 0; acao: 1; icone: "folder-open" }
                ItemDeSlot { slot: 1; acao: 1; icone: "folder-open" }
                ItemDeSlot { slot: 2; acao: 1; icone: "folder-open" }
                ItemDeSlot { slot: 3; acao: 1; icone: "folder-open" }
                ItemDeSlot { slot: 4; acao: 1; icone: "folder-open" }
                ItemDeSlot { slot: 5; acao: 1; icone: "folder-open" }
                ItemDeSlot { slot: 6; acao: 1; icone: "folder-open" }
                ItemDeSlot { slot: 7; acao: 1; icone: "folder-open" }
                ItemDeSlot { slot: 8; acao: 1; icone: "folder-open" }
                ItemDeSlot { slot: 9; acao: 1; icone: "folder-open" }
            }
            ItemDeMenu {
                id: itemDoDesfazer

                icone: "undo-2"
                text: tr("menu.system.undo_load")
                atalho: principal.atalho("desfazer_carregar")
                onTriggered: principal.palco.tela.estadoPeloMenu(2, -1)
            }
            MenuSeparator {}
            ItemDeMenu {
                icone: "square"
                text: tr("menu.system.close")
                atalho: "Esc"
                enabled: principal.jogando
                onTriggered: principal.fechaJogo()
            }
        }

        // Cada categoria abre a janela de configurações já nela, na ordem das abas.
        Menu {
            id: menuConfiguracoes

            title: tr("menu.settings")

            ItemDeMenu { icone: "settings"; text: tr("settings.tab.general"); onTriggered: configuracoes.abre(0) }
            ItemDeMenu { icone: "gamepad-2"; text: tr("settings.tab.controls"); onTriggered: configuracoes.abre(1) }
            ItemDeMenu { icone: "keyboard"; text: tr("settings.tab.shortcuts"); onTriggered: configuracoes.abre(2) }
            ItemDeMenu { icone: "monitor"; text: tr("settings.tab.graphics"); onTriggered: configuracoes.abre(3) }
            ItemDeMenu { icone: "gauge"; text: tr("settings.tab.speed"); onTriggered: configuracoes.abre(4) }
            ItemDeMenu { icone: "volume-2"; text: tr("settings.tab.audio"); onTriggered: configuracoes.abre(5) }
            ItemDeMenu { icone: "bug"; text: tr("settings.tab.debug"); onTriggered: configuracoes.abre(6) }
            MenuSeparator {}
            ItemDeMenu {
                text: tr("menu.settings.game")
                atalho: "X"
                enabled: biblioteca.vazio === ""
                onTriggered: principal.configuraJogo(principal.escolhido())
            }
        }

        // Os marcáveis são relidos quando o menu abre, e não amarrados: um clique num marcável
        // quebra a ligação dele, e uma troca feita nas configurações não apareceria aqui.
        Menu {
            id: menuExibir

            title: tr("menu.view")
            onAboutToShow: {
                itemDaGrade.checked = !principal.emSlider
                itemDoSlider.checked = principal.emSlider
                itemDasFerramentas.checked = principal.comFerramentas
                itemDoStatus.checked = principal.comStatus
            }

            ItemDeMenu {
                id: itemDaGrade

                checkable: true
                icone: "layout-grid"
                text: tr("settings.library_view.grid")
                onTriggered: configuracoes.cfg.define("biblioteca", 0)
            }
            ItemDeMenu {
                id: itemDoSlider

                checkable: true
                icone: "gallery-horizontal"
                text: tr("settings.library_view.slider")
                onTriggered: configuracoes.cfg.define("biblioteca", 1)
            }
            MenuSeparator {}
            ItemDeMenu {
                id: itemDasFerramentas

                checkable: true
                text: tr("menu.view.toolbar")
                onTriggered: configuracoes.cfg.define("barras.ferramentas", checked)
            }
            ItemDeMenu {
                id: itemDoStatus

                checkable: true
                text: tr("menu.view.statusbar")
                onTriggered: configuracoes.cfg.define("barras.status", checked)
            }
            MenuSeparator {}
            ItemDeMenu {
                icone: "maximize"
                text: tr("controls.shortcut.fullscreen")
                atalho: "F11"
                onTriggered: principal.alternaTelaCheia()
            }
        }

        Menu {
            id: menuFerramentas

            title: tr("menu.tools")
            onAboutToShow: itemDoLog.checked = configuracoes.cfg.valor("debug.log")

            ItemDeMenu {
                icone: "hard-drive"
                text: tr("menu.tools.saves")
                onTriggered: saves.abre()
            }
            ItemDeMenu {
                id: itemDoLog

                checkable: true
                text: tr("debug.log")
                onTriggered: configuracoes.cfg.define("debug.log", checked)
            }
            MenuSeparator {}
            // O Zeeboids só deixa sincronizar uma vez por dia, e a trava é dele: guarda a data no
            // próprio banco. Testar rede com isso custa um dia por tentativa, então o item recua
            // a data em um dia.
            ItemDeMenu {
                icone: "calendar-clock"
                text: tr("menu.tools.unlock_sync")
                onTriggered: recado.text = biblioteca.liberaSincronizacao()
            }
        }

        Menu {
            id: menuAjuda

            title: tr("menu.help")

            ItemDeMenu {
                icone: "external-link"
                text: tr("about.repository")
                onTriggered: Qt.openUrlExternally(configuracoes.cfg.endereco("repositorio"))
            }
            ItemDeMenu {
                icone: "message-circle"
                text: tr("about.discord")
                onTriggered: Qt.openUrlExternally(configuracoes.cfg.endereco("discord"))
            }
            MenuSeparator {}
            ItemDeMenu {
                icone: "download"
                text: tr("menu.help.check_update")
                onTriggered: avisos.procuraAtualizacao()
            }
            ItemDeMenu {
                icone: "info"
                text: tr("menu.help.about")
                onTriggered: sobre.open()
            }
        }
    }

    // A barra de ferramentas: o que se faz na biblioteca, o que se faz com o jogo aberto, as
    // configurações, e à direita a vista e a busca, como no PCSX2.
    header: ToolBar {
        visible: principal.comFerramentas && !principal.soOJogo

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 4
            anchors.rightMargin: 6
            spacing: 2

            BotaoDeFerramenta {
                id: botaoZWheel

                icone: "disc-3"
                text: tr("nav.z_wheel")
                dica: tr(enabled ? "nav.z_wheel.hint" : "nav.z_wheel.missing")
                enabled: principal.depende([configuracoes.cfg.versao], biblioteca.temZWheel())
                onClicked: principal.mostra(biblioteca.abreZWheel())
            }
            BotaoDeFerramenta {
                icone: "refresh-cw"
                text: tr("library.rescan")
                onClicked: {
                    biblioteca.procuraDeNovo()
                    principal.recomeca()
                }
            }
            Separador {}
            BotaoDeFerramenta {
                icone: principal.palco.pausado ? "play" : "pause"
                text: tr(principal.palco.pausado ? "menu.system.resume" : "menu.system.pause")
                enabled: principal.jogando
                onClicked: principal.palco.pausa()
            }
            BotaoDeFerramenta {
                icone: "rotate-ccw"
                text: tr("menu.system.restart")
                enabled: principal.jogando
                onClicked: principal.palco.reinicia()
            }
            BotaoDeFerramenta {
                icone: "square"
                text: tr("toolbar.close")
                enabled: principal.jogando
                onClicked: principal.fechaJogo()
            }
            Separador {}
            BotaoDeFerramenta {
                icone: "settings"
                text: tr("nav.settings")
                onClicked: configuracoes.abre(-1)
            }
            BotaoDeFerramenta {
                icone: "gamepad-2"
                text: tr("settings.tab.controls")
                onClicked: configuracoes.abre(1)
            }
            Separador {}
            BotaoDeFerramenta {
                icone: "maximize"
                text: tr("controls.shortcut.fullscreen")
                onClicked: principal.alternaTelaCheia()
            }

            Item {
                Layout.fillWidth: true
            }

            // A vista da biblioteca, como o par grade/lista do PCSX2. **Não marcáveis**: o clique
            // de um marcável troca o `checked` e quebra a ligação dele com a configuração, e uma
            // troca feita pelo menu ou pelas configurações não apareceria aqui.
            ToolButton {
                focusPolicy: Qt.NoFocus
                visible: !principal.jogandoNaPrincipal
                checked: !principal.emSlider
                contentItem: Icone { nome: "layout-grid"; tamanho: 18; color: palette.buttonText }
                onClicked: configuracoes.cfg.define("biblioteca", 0)
                ToolTip.visible: hovered
                ToolTip.delay: 500
                ToolTip.text: tr("settings.library_view.grid")
            }
            ToolButton {
                focusPolicy: Qt.NoFocus
                visible: !principal.jogandoNaPrincipal
                checked: principal.emSlider
                contentItem: Icone { nome: "gallery-horizontal"; tamanho: 18; color: palette.buttonText }
                onClicked: configuracoes.cfg.define("biblioteca", 1)
                ToolTip.visible: hovered
                ToolTip.delay: 500
                ToolTip.text: tr("settings.library_view.slider")
            }

            // A busca: filtra a lista pelo nome, sem ligar para acentos. Esc limpa; o Enter joga
            // o escolhido, que é o primeiro resultado enquanto ninguém mexe na escolha.
            TextField {
                id: busca
                visible: !principal.jogandoNaPrincipal

                Layout.preferredWidth: 240
                Layout.leftMargin: 6
                leftPadding: 28
                rightPadding: limpaBusca.visible ? 28 : 8
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
                ToolTip.visible: hovered && !activeFocus
                ToolTip.delay: 500
                ToolTip.text: tr("nav.search.hint")

                Icone {
                    anchors.left: parent.left
                    anchors.leftMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    nome: "search"
                    tamanho: 14
                    color: busca.palette.placeholderText
                }
                // Como o Esc da busca: a lista volta a ser das setas.
                ToolButton {
                    id: limpaBusca

                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    width: 26
                    height: 26
                    focusPolicy: Qt.NoFocus
                    visible: busca.text !== ""
                    contentItem: Icone { nome: "x"; tamanho: 14; color: busca.palette.text }
                    onClicked: {
                        busca.text = ""
                        principal.vista.forceActiveFocus()
                    }
                    ToolTip.visible: hovered
                    ToolTip.text: tr("nav.search.clear")
                }
            }
        }
    }

    // A barra de status: a contagem e o que apertar à esquerda; com um jogo aberto, à direita,
    // o nome dele, onde ele desenha, e a velocidade da última amostra, como no PCSX2.
    footer: ToolBar {
        position: ToolBar.Footer
        visible: principal.comStatus && !principal.soOJogo

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8
            anchors.rightMargin: 8
            spacing: 8

            Label {
                text: biblioteca.contagem
            }
            Label {
                Layout.fillWidth: true
                elide: Text.ElideRight
                opacity: 0.6
                text: tr("library.controls_hint") + " · " + tr("library.pad_game")
            }
            Label {
                visible: principal.jogando && text !== ""
                text: principal.palco.tela.estado + (principal.palco.tela.velocidade !== "" ? "  ·  " + principal.palco.tela.velocidade : "")
            }
        }
    }

    // O jogo, quando ele roda na principal: no lugar da biblioteca, sem as margens dela.
    PalcoDoJogo {
        id: palcoDaPrincipal

        anchors.fill: parent
        visible: principal.jogandoNaPrincipal
        ativo: principal.jogandoNaPrincipal
        faixaDeEstado: false
        onFechaPedido: principal.fechaJogo()
        onTelaCheiaPedida: principal.alternaTelaCheia()
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 8
        spacing: 6
        visible: !principal.jogandoNaPrincipal

        // O recado da última ação que tem o que dizer, como o liberar a sincronização.
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

        // O que deu errado na última tentativa de abrir um jogo.
        Label {
            id: aviso

            Layout.fillWidth: true
            visible: text !== ""
            color: Tema.erro
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
            onConfigura: (linha) => principal.configuraJogo(linha)
            onInspeciona: (linha) => principal.inspecionaJogo(linha)
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
            onConfigura: (linha) => principal.configuraJogo(linha)
            onInspeciona: (linha) => principal.inspecionaJogo(linha)
        }
    }

    // Os detalhes de um jogo: o que a biblioteca sabe dele, e o caminho para as configurações dele.
    Dialog {
        id: detalhes

        property int linha: -1
        property var dados: ({})

        function campo(nome) {
            return dados[nome] !== undefined ? dados[nome] : ""
        }

        anchors.centerIn: parent
        width: Math.min(560, principal.width - 32)
        modal: true
        title: principal.tr("details.title").replace("{game}", campo("titulo"))

        // **Sem layouts aqui dentro.** Texto que quebra linha dentro de layouts aninhados faz a
        // altura depender da largura e a largura da altura: o diálogo não terminava de se
        // arrumar e saía estreito, com o texto vazando. Com as larguras dadas, não há volta.
        Item {
            width: parent.width
            implicitHeight: Math.max(capaDosDetalhes.height, fichaDosDetalhes.implicitHeight)

            Image {
                id: capaDosDetalhes

                width: 112
                height: 145
                fillMode: Image.PreserveAspectFit
                source: detalhes.campo("capa")
                smooth: !detalhes.campo("capaNitida")
            }

            Column {
                id: fichaDosDetalhes

                x: capaDosDetalhes.width + 16
                width: parent.width - x
                spacing: 8

                Repeater {
                    // O rótulo em cima do valor, e não ao lado: o valor ganha a largura inteira.
                    // O caminho quebra em qualquer lugar, porque não tem espaços onde quebrar.
                    model: [
                        { rotulo: "details.classid", valor: detalhes.campo("classe") || "—" },
                        { rotulo: "details.module", valor: detalhes.campo("modulo") || "—" },
                        { rotulo: "details.file", valor: detalhes.campo("caminho") },
                        { rotulo: "details.size", valor: detalhes.campo("tamanho") },
                        { rotulo: "details.custom",
                          valor: principal.tr(detalhes.campo("personalizado") ? "details.custom.yes"
                                                                              : "details.custom.no") },
                    ]

                    Column {
                        required property var modelData

                        width: fichaDosDetalhes.width

                        Label {
                            opacity: 0.6
                            font.pixelSize: 11
                            text: principal.tr(modelData.rotulo)
                        }
                        Label {
                            width: parent.width
                            wrapMode: Text.WrapAnywhere
                            text: modelData.valor
                        }
                    }
                }

                Image {
                    height: Math.min(64, sourceSize.height)
                    fillMode: Image.PreserveAspectFit
                    source: detalhes.campo("classificacao")
                    visible: source !== ""
                }
                Label {
                    width: parent.width
                    wrapMode: Text.Wrap
                    visible: text !== ""
                    text: detalhes.campo("descricao")
                }
            }
        }

        footer: DialogButtonBox {
            Button {
                text: principal.tr("library.menu.configure")
                onClicked: {
                    detalhes.close()
                    principal.configuraJogo(detalhes.linha)
                }
            }
            Button {
                text: principal.tr("details.close")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
        }
    }

    // Ajuda > Sobre o Zeebx: o que era a aba Sobre das configurações, num diálogo como o do
    // PCSX2.
    Dialog {
        id: sobre

        anchors.centerIn: parent
        width: Math.min(420, principal.width - 32)
        modal: true
        title: principal.tr("menu.help.about")

        ColumnLayout {
            width: parent.width
            spacing: 6

            Image {
                Layout.alignment: Qt.AlignHCenter
                Layout.preferredWidth: 96
                Layout.preferredHeight: 96
                fillMode: Image.PreserveAspectFit
                source: "qrc:/zeebx/zeebx.png"
                smooth: true
                mipmap: true
            }
            Label {
                Layout.alignment: Qt.AlignHCenter
                font.pixelSize: 22
                font.bold: true
                text: principal.tr("app.name")
            }
            Label {
                Layout.alignment: Qt.AlignHCenter
                text: principal.tr("about.version").replace("{version}", configuracoes.cfg.versaoDoEmulador())
            }
            Label {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                opacity: 0.7
                text: principal.tr("about.tagline")
            }
            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: 8

                Button {
                    text: principal.tr("about.repository")
                    onClicked: Qt.openUrlExternally(configuracoes.cfg.endereco("repositorio"))
                }
                Button {
                    text: principal.tr("about.discord")
                    onClicked: Qt.openUrlExternally(configuracoes.cfg.endereco("discord"))
                }
            }
        }

        footer: DialogButtonBox {
            Button {
                text: principal.tr("details.close")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
        }
    }

    // A resposta da procura por versão nova feita pelo menu, quando não é versão nova: em dia, ou
    // por que não deu. A versão nova abre o aviso de baixo, o de sempre.
    Dialog {
        id: respostaDaProcura

        property string texto: ""

        anchors.centerIn: parent
        width: Math.min(420, principal.width - 32)
        modal: true
        title: principal.tr("menu.help.check_update")

        Label {
            width: parent.width
            wrapMode: Text.Wrap
            text: respostaDaProcura.texto
        }

        footer: DialogButtonBox {
            Button {
                text: principal.tr("details.close")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
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

    // O aviso de versão nova, se a procura ao abrir achou uma. Nas cópias que se trocam sozinhas
    // (AppImage, instalador do Windows, `.app`) é também onde a instalação anda: a etapa é lida a
    // cada volta do relógio acima, enquanto o diálogo está aberto.
    Dialog {
        id: novaVersao

        property string texto: ""
        // Guardada na abertura: o `onClosed` dispensa o aviso, e depois disso o núcleo não diz
        // mais qual é a página. Se o diálogo fechasse antes do `onClicked`, o botão abria "".
        property string pagina: ""
        readonly property bool sozinho: avisos.atualizaSozinho()
        // 0 nenhuma, 1 baixando, 2 instalando, 3 pronta, 4 falhou. Ver `etapaDaAtualizacao`.
        property int etapa: 0
        property real fracao: -1
        property string andamento: ""
        readonly property bool ocupado: etapa === 1 || etapa === 2

        function acompanha() {
            etapa = avisos.etapaDaAtualizacao()
            fracao = avisos.fracaoDaAtualizacao()
            andamento = avisos.textoDaAtualizacao()
        }

        anchors.centerIn: parent
        width: Math.min(460, principal.width - 32)
        modal: true
        title: principal.tr("update.title")
        // No meio do download, clicar fora ou apertar Esc não fecha: a instalação seguiria sem
        // ninguém ver, e o emulador fecharia sozinho no Windows.
        closePolicy: ocupado ? Popup.NoAutoClose : Popup.CloseOnEscape

        Timer {
            interval: 100
            repeat: true
            running: novaVersao.visible
            onTriggered: novaVersao.acompanha()
        }

        ColumnLayout {
            width: parent.width
            spacing: 12

            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: novaVersao.etapa === 0 ? novaVersao.texto : novaVersao.andamento
            }

            ProgressBar {
                Layout.fillWidth: true
                visible: novaVersao.ocupado
                indeterminate: novaVersao.etapa === 2 || novaVersao.fracao < 0
                value: Math.max(0, novaVersao.fracao)
            }

            RowLayout {
                Layout.alignment: Qt.AlignRight
                visible: !novaVersao.ocupado

                Button {
                    visible: novaVersao.sozinho && novaVersao.etapa === 0
                    text: principal.tr("update.install")
                    onClicked: {
                        avisos.instalaAtualizacao()
                        novaVersao.acompanha()
                    }
                }
                Button {
                    visible: novaVersao.etapa === 3
                    text: principal.tr("update.restart")
                    onClicked: avisos.reiniciaNaVersaoNova()
                }
                Button {
                    // Sem troca sozinha, ou se ela falhou, o caminho é a página da release.
                    visible: novaVersao.etapa === 0 || novaVersao.etapa === 4
                    text: principal.tr(novaVersao.sozinho && novaVersao.etapa === 0
                                       ? "update.release_page" : "update.download")
                    onClicked: {
                        Qt.openUrlExternally(novaVersao.pagina)
                        if (!novaVersao.sozinho || novaVersao.etapa === 4)
                            novaVersao.close()
                    }
                }
                Button {
                    text: principal.tr("update.later")
                    onClicked: novaVersao.close()
                }
            }
        }

        onOpened: acompanha()
        onClosed: avisos.dispensaAtualizacao()
    }
}
