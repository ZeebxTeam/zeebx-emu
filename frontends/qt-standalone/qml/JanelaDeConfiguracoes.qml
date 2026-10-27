import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

import zeebx

// A janela de configurações: uma janela do sistema, e não um painel dentro da principal, como no
// egui. As opções são as do `settings.json`, lidas e gravadas pela chave — ver
// `src/qt/configuracoes.rs`. Cada gravação salva o arquivo e vale na hora.
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

    // A biblioteca da janela principal, para refazer a lista quando a pasta, a Z-Wheel ou o
    // idioma mudam.
    required property var biblioteca
    // A janela principal, para aplicar o modo dela na hora.
    required property var principal

    // O objeto das configurações, para a principal reler o que depende delas.
    property alias cfg: cfg

    // **O modo do jogo.** Com `jogo` preenchido, a janela edita as configurações daquele jogo
    // só: ficam as abas de controles, gráficos, velocidade e áudio, e o que é da janela sai.
    // Ver `Nucleo::com_o_jogo`.
    property string jogo: ""
    property string nomeDoJogo: ""
    readonly property bool doJogo: jogo !== ""
    readonly property bool personalizado: doJogo && v("jogo.personalizado")

    width: 900
    height: 660
    minimumWidth: 600
    minimumHeight: 400
    title: "Zeebx — " + (doJogo ? tr("game_settings.title").replace("{game}", nomeDoJogo)
                                : tr("settings.title"))

    // Os textos e os valores se refazem sozinhos: a `versao` do idioma e a das configurações são
    // lidas aqui, e uma ligação que chama estas funções depende delas.
    function tr(chave) {
        return depende([Idioma.versao], Idioma.texto(chave))
    }
    function v(chave) {
        return depende([cfg.versao], cfg.valor(chave))
    }
    function define(chave, valor) {
        cfg.define(chave, valor)
    }

    // Mostra a janela na aba pedida: 1 é a dos controles, que o aviso de abertura manda
    // configurar antes de jogar; -1 fica na que estava.
    function abre(aba) {
        if (aba >= 0)
            abas.currentIndex = aba
        show()
        raise()
        requestActivate()
    }

    // Uma captura começou: a tecla tem de chegar a `teclado`, o FocusScope da aba.
    //
    // **No clique, e não só quando a aba aparece.** O foco era pedido ao mostrar a aba, e a
    // captura ficava em "Aguardando" para sempre quando ele não estava lá: a janela aberta pelo
    // aviso de boas-vindas, que no Wayland pode nascer sem o teclado, ou uma lista desta aba que
    // ficou com o foco depois de usada. Os botões de atribuir não pegam o foco de propósito — com
    // ele, o espaço capturado clicaria o botão de novo —, então quem o devolve é isto.
    function escutaTeclas(teclado) {
        janela.requestActivate()
        teclado.forceActiveFocus()
    }

    // Abre no modo do jogo de `chave`, na aba dos controles.
    function abreDoJogo(chave, nome) {
        jogo = chave
        nomeDoJogo = nome
        cfg.editaOJogo(chave)
        abre(1)
    }

    // A procura por versão nova e o Discord respondem de outra thread: o estado deles é relido
    // de tempos em tempos enquanto a janela está aberta.
    property int tique: 0
    Timer {
        interval: 250
        repeat: true
        running: janela.visible
        onTriggered: janela.tique += 1
    }

    Configuracoes {
        id: cfg

        onIdiomaMudou: Idioma.recarrega()
        onTemaMudou: Tema.recarrega()
        onBibliotecaMudou: (varrer) => {
            if (varrer)
                janela.biblioteca.procuraDeNovo()
            else
                janela.biblioteca.refaz()
        }
        onJanelaPrincipalMudou: janela.principal.aplicaModo()
    }

    // **A explicação de cada opção mora no rodapé**, como no PCSX2: a da opção sob o ponteiro,
    // ou com o foco do teclado. Embaixo de cada opção, ela alongava as páginas até elas não
    // caberem na janela. O que muda com o estado (a porta desligada, a captura esperando, nenhum
    // controle ligado) continua junto da opção: é aviso, e não explicação.
    //
    // Quem marca um grupo é um `Titulo`, com a `descricao` do que vem embaixo dele até o próximo
    // título, e uma `Opcao`, com a `dica` dela. A página acha o marcador mais próximo acima do
    // ponteiro (`descreve`). Uma opção sem dica mostra a do título do grupo dela.
    property string descricao: ""

    function descreve(pagina, y) {
        // Por índice, e não com `for...of`: no Qt 6.4 a lista de filhos de um item não é
        // iterável, e o laço dava `TypeError`.
        // Uma `Explica` vale a partir do filho de antes dela, que é o que ela explica.
        const acima = []
        const filhos = pagina.children
        for (let i = 0; i < filhos.length; i++) {
            const filho = filhos[i]
            if (!filho.visible || filho.marcaDeDescricao === undefined)
                continue
            let inicio = filho.y
            if (filho.descreveOAnterior) {
                for (let j = i - 1; j >= 0; j--) {
                    if (filhos[j].visible) {
                        inicio = filhos[j].y
                        break
                    }
                }
            }
            if (inicio <= y)
                acima.push({ y: inicio, marca: filho })
        }
        acima.sort((a, b) => b.y - a.y)
        for (const { marca } of acima) {
            if (marca.marcaDeDescricao !== "")
                return mostraDescricao(marca.marcaDeDescricao)
            if (marca.ehTitulo)
                break
        }
        mostraDescricao(pagina.descricaoDaPagina)
    }
    function mostraDescricao(texto) {
        descricao = texto
    }

    // O foco do teclado também descreve: quem anda pelas opções com o Tab lê o que cada uma faz.
    onActiveFocusItemChanged: {
        for (let item = activeFocusItem; item; item = item.parent) {
            if (item.paginaDeConfiguracao === true) {
                const ponto = activeFocusItem.mapToItem(item, 0, 0)
                descreve(item, ponto.y)
                return
            }
        }
    }

    // A coluna de uma página: o que o ponteiro sobrevoa nela vira descrição.
    component Pagina: ColumnLayout {
        id: pagina

        readonly property bool paginaDeConfiguracao: true
        // O que o rodapé diz acima do primeiro título: a página toda, ou nada.
        property string descricaoDaPagina: ""

        HoverHandler {
            onPointChanged: janela.descreve(pagina, point.position.y)
        }
    }

    // Um liga-desliga. A explicação dele vai para o rodapé.
    component Opcao: ColumnLayout {
        id: opcao

        required property string chave
        required property string rotulo
        property string dica: ""
        readonly property string marcaDeDescricao: dica !== "" ? janela.tr(dica) : ""
        readonly property bool ehTitulo: false

        Layout.fillWidth: true
        spacing: 0

        CheckBox {
            text: janela.tr(opcao.rotulo)
            checked: janela.v(opcao.chave)
            onToggled: janela.define(opcao.chave, checked)
        }
    }

    // Uma escolha de lista: o rótulo à esquerda e a caixa.
    component Escolha: RowLayout {
        id: escolha

        required property string chave
        required property string rotulo

        Layout.fillWidth: true

        Label {
            text: janela.tr(escolha.rotulo)
        }
        ComboBox {
            Layout.preferredWidth: 280
            model: janela.depende([Idioma.versao], cfg.opcoes(escolha.chave))
            currentIndex: janela.v(escolha.chave)
            onActivated: (indice) => janela.define(escolha.chave, indice)
        }
    }

    component Dica: Label {
        Layout.fillWidth: true
        wrapMode: Text.Wrap
        opacity: 0.6
    }

    // A explicação do que está logo acima dela, para o rodapé: uma caixa de escolha, uma linha
    // de botões. Não ocupa lugar. A margem negativa devolve o espaçamento que o layout poria por
    // ela ser um filho a mais.
    component Explica: Item {
        property string text: ""
        readonly property string marcaDeDescricao: text
        readonly property bool ehTitulo: false
        readonly property bool descreveOAnterior: true

        Layout.preferredHeight: 0
        Layout.topMargin: -(parent && parent.spacing ? parent.spacing : 0)
    }

    // O título de um grupo de opções, com a explicação do grupo para o rodapé.
    component Titulo: Label {
        property string descricao: ""
        readonly property string marcaDeDescricao: descricao
        readonly property bool ehTitulo: true

        Layout.topMargin: 12
        font.bold: true
    }

    header: ColumnLayout {
        spacing: 0

        // No modo do jogo, a opção que liga as configurações dele. Desligada, as abas mostram as
        // gerais, sem deixar mexer: é o que o jogo está usando.
        Pane {
            Layout.fillWidth: true
            visible: janela.doJogo

            ColumnLayout {
                width: parent.width
                spacing: 0

                CheckBox {
                    text: janela.tr("game_settings.custom")
                    checked: janela.personalizado
                    onToggled: janela.define("jogo.personalizado", checked)
                }
                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: 32
                    wrapMode: Text.Wrap
                    opacity: 0.6
                    text: janela.tr("game_settings.custom.hint")
                }
            }
        }
    }

    // O rodapé com a explicação da opção sob o ponteiro. A altura é fixa: uma que seguisse o
    // texto faria a janela pular a cada opção sobrevoada.
    footer: Pane {
        padding: 10

        Label {
            width: parent.width
            height: 64
            wrapMode: Text.Wrap
            elide: Text.ElideRight
            maximumLineCount: 4
            opacity: janela.descricao !== "" ? 0.85 : 0.5
            text: janela.descricao !== "" ? janela.descricao : janela.tr("settings.description.empty")
        }
    }

    RowLayout {
        anchors.fill: parent
        spacing: 0

        // As categorias, como a coluna do PCSX2. **Mantém o `id: abas` e o `currentIndex`**:
        // quem abre a janela numa categoria (`abre`) e as páginas que leem o controle enquanto
        // estão à vista perguntam por eles. No modo do jogo, as que não valem por jogo somem.
        ListView {
            id: abas

            Layout.preferredWidth: 190
            Layout.fillHeight: true
            Layout.topMargin: 6
            interactive: false
            currentIndex: 0
            model: [
                { chave: "settings.tab.general", icone: "settings", geral: true },
                { chave: "settings.tab.controls", icone: "gamepad-2", geral: false },
                { chave: "settings.tab.shortcuts", icone: "keyboard", geral: true },
                { chave: "settings.tab.graphics", icone: "monitor", geral: false },
                { chave: "settings.tab.speed", icone: "gauge", geral: false },
                { chave: "settings.tab.audio", icone: "volume-2", geral: false },
                { chave: "settings.tab.debug", icone: "bug", geral: true }
            ]

            delegate: ItemDelegate {
                id: categoria

                required property var modelData
                required property int index

                width: ListView.view.width
                visible: !modelData.geral || !janela.doJogo
                height: visible ? implicitHeight : 0
                // Como os botões das outras janelas: o clique não leva o foco do teclado, que a
                // captura de teclas das páginas de controles e de atalhos precisa.
                focusPolicy: Qt.NoFocus
                highlighted: ListView.isCurrentItem
                onClicked: abas.currentIndex = index

                contentItem: RowLayout {
                    spacing: 10

                    Icone {
                        Layout.preferredWidth: 20
                        nome: categoria.modelData.icone
                        tamanho: 18
                        color: categoria.highlighted ? categoria.palette.highlightedText : categoria.palette.windowText
                    }
                    Label {
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                        text: janela.tr(categoria.modelData.chave)
                        color: categoria.highlighted ? categoria.palette.highlightedText : categoria.palette.windowText
                    }
                }
            }
        }

        Rectangle {
            Layout.preferredWidth: 1
            Layout.fillHeight: true
            color: palette.mid
        }

        // Toda página rola: a gráfica já não cabe na altura padrão da janela, e uma opção que some
        // embaixo da borda é uma opção que não existe.
        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: abas.currentIndex
            enabled: !janela.doJogo || janela.personalizado
            // A página trocou: a descrição da anterior não vale mais.
            onCurrentIndexChanged: janela.mostraDescricao("")

            // Geral
            ScrollView {
                contentWidth: availableWidth

                Pagina {
                    width: parent.width - 32
                    x: 16
                    spacing: 6

                    Titulo {
                        text: janela.tr("settings.roms_folder")
                        descricao: janela.tr("settings.roms_folder.hint")
                    }
                    RowLayout {
                        Label {
                            Layout.fillWidth: true
                            elide: Text.ElideMiddle
                            font.family: "monospace"
                            text: janela.v("roms_dir")
                        }
                        Button {
                            text: janela.tr("settings.browse")
                            onClicked: cfg.escolhePastaDeRoms()
                        }
                    }
                    Label {
                        visible: janela.depende([cfg.versao], cfg.pastaDeRomsFalta())
                        color: Tema.aviso
                        text: janela.tr("common.folder_missing")
                    }

                    // A padrão mora em `~/.config`, que é escondida: por isso o caminho aparece mesmo
                    // sem escolha, e há o botão de abrir. Ver `docs/implementacao/22-screenshots.md`.
                    Titulo {
                        text: janela.tr("settings.screenshots_folder")
                        descricao: janela.tr("settings.screenshots_folder.hint")
                    }
                    RowLayout {
                        Label {
                            Layout.fillWidth: true
                            elide: Text.ElideMiddle
                            font.family: "monospace"
                            text: janela.depende([cfg.versao], cfg.pastaDeScreenshots())
                        }
                        Button {
                            text: janela.tr("settings.browse")
                            onClicked: cfg.escolhePastaDeScreenshots()
                        }
                        Button {
                            visible: !janela.depende([cfg.versao], cfg.screenshotsNaPastaPadrao())
                            text: janela.tr("settings.screenshots_folder.default")
                            onClicked: cfg.usaPastaPadraoDeScreenshots()
                        }
                        Button {
                            text: janela.tr("settings.screenshots_folder.open")
                            onClicked: Qt.openUrlExternally(cfg.enderecoDaPastaDeScreenshots())
                        }
                    }

                    Titulo {
                        text: janela.tr("settings.language")
                        descricao: janela.tr("settings.language.hint")
                    }
                    ComboBox {
                        Layout.preferredWidth: 280
                        model: cfg.opcoes("language")
                        currentIndex: janela.v("language")
                        onActivated: (indice) => janela.define("language", indice)
                    }

                    // A paleta muda na hora, em todas as janelas: ver `docs/implementacao/26-temas.md`.
                    Titulo {
                        text: janela.tr("settings.theme")
                        descricao: janela.tr("settings.theme.hint")
                    }
                    ComboBox {
                        Layout.preferredWidth: 280
                        model: janela.depende([Idioma.versao], cfg.opcoes("tema"))
                        currentIndex: janela.v("tema")
                        onActivated: (indice) => janela.define("tema", indice)
                    }

                    Titulo {
                        text: janela.tr("settings.library_view")
                        descricao: janela.tr("settings.library_view.hint")
                    }
                    ComboBox {
                        Layout.preferredWidth: 280
                        model: janela.depende([Idioma.versao], cfg.opcoes("biblioteca"))
                        currentIndex: janela.v("biblioteca")
                        onActivated: (indice) => janela.define("biblioteca", indice)
                    }

                    Titulo {
                        text: janela.tr("settings.z_wheel")
                        descricao: janela.tr("settings.z_wheel.hint")
                    }
                    RowLayout {
                        Label {
                            Layout.fillWidth: true
                            elide: Text.ElideMiddle
                            font.family: "monospace"
                            text: janela.v("z_wheel_path")
                        }
                        Button {
                            text: janela.tr("settings.browse")
                            onClicked: cfg.escolheZWheel(false)
                        }
                        Button {
                            text: janela.tr("settings.z_wheel.folder")
                            onClicked: cfg.escolheZWheel(true)
                        }
                        Button {
                            text: janela.tr("settings.z_wheel.detect")
                            onClicked: cfg.detectaZWheel()
                        }
                    }
                    Label {
                        Layout.fillWidth: true
                        wrapMode: Text.Wrap
                        opacity: janela.depende([cfg.versao], cfg.zWheelAchada()) ? 0.6 : 1
                        color: janela.depende([cfg.versao], cfg.zWheelAchada()) ? palette.windowText : Tema.aviso
                        text: janela.depende([cfg.versao, Idioma.versao], cfg.estadoDaZWheel())
                    }
                    Opcao {
                        chave: "z_wheel.fim_de_vida"
                        rotulo: "settings.z_wheel_eol"
                        dica: "settings.z_wheel_eol.hint"
                    }
                    Opcao {
                        chave: "z_wheel.transicoes_sempre"
                        rotulo: "settings.z_wheel_transitions"
                        dica: "settings.z_wheel_transitions.hint"
                    }

                    Titulo { text: janela.tr("settings.discord") }
                    Opcao {
                        chave: "discord.ativo"
                        rotulo: "settings.discord.on"
                    }
                    Label {
                        Layout.leftMargin: 32
                        enabled: janela.v("discord.ativo")
                        text: janela.depende([janela.tique], cfg.discordConectado())
                              ? janela.tr("settings.discord.connected")
                              : janela.tr("settings.discord.waiting")
                    }

                    Titulo { text: janela.tr("settings.updates") }
                    Dica {
                        text: janela.tr("settings.updates.version").replace("{version}", cfg.versaoDoEmulador())
                    }
                    Opcao {
                        chave: "atualizacoes.ao_abrir"
                        rotulo: "settings.updates.on_start"
                    }
                    Opcao {
                        chave: "atualizacoes.pre_lancamentos"
                        rotulo: "settings.updates.prereleases"
                    }
                    RowLayout {
                        Button {
                            text: janela.tr("settings.updates.check")
                            enabled: !janela.depende([janela.tique], cfg.procurandoAtualizacao())
                            onClicked: cfg.procuraAtualizacao()
                        }
                        Label {
                            text: janela.depende([janela.tique, Idioma.versao], cfg.estadoDaAtualizacao())
                        }
                        Button {
                            readonly property string pagina: janela.depende([janela.tique], cfg.paginaDaAtualizacao())
                            readonly property bool sozinho: cfg.atualizaSozinho()
                            visible: pagina !== ""
                            // Quem troca sozinho volta ao aviso da janela principal, que é onde o
                            // andamento da instalação aparece.
                            text: janela.tr(sozinho ? "update.install" : "update.download")
                            onClicked: sozinho ? cfg.mostraAvisoDeAtualizacao() : Qt.openUrlExternally(pagina)
                        }
                    }

                    Dica {
                        Layout.topMargin: 16
                        Layout.bottomMargin: 16
                        text: janela.depende([Idioma.versao], cfg.ondeFicam())
                    }
                }
            }

            // Controles
            ScrollView {
                id: abaDeControles

                contentWidth: availableWidth

                // O controle não gera evento no Qt, como não gerava no egui: enquanto a aba está à
                // vista, ele é lido aqui — o desenho acende, a captura por botão de controle anda e a
                // calibração junta leituras. O `tique` refaz o que é ao vivo; a `versao` das
                // configurações, o que foi gravado.
                property int tique: 0
                readonly property bool aVista: janela.visible && abas.currentIndex === 1
                Timer {
                    interval: 33
                    repeat: true
                    running: abaDeControles.aVista
                    onTriggered: {
                        cfg.leControle()
                        abaDeControles.tique += 1
                    }
                }
                onAVistaChanged: if (aVista) teclado.forceActiveFocus()

                // O teclado desta janela acende o desenho e, com uma captura aberta, vira a origem do
                // botão. Os botões da aba não pegam o foco: com ele, o espaço capturado clicaria o
                // "atribuir" de novo e desistiria da captura.
                FocusScope {
                    id: teclado

                    width: abaDeControles.availableWidth
                    implicitHeight: controles.implicitHeight + 32
                    focus: true
                    Keys.onPressed: (evento) => {
                        if (!evento.isAutoRepeat)
                            cfg.tecla(evento.key, evento.modifiers, true)
                        evento.accepted = true
                    }
                    Keys.onReleased: (evento) => {
                        if (!evento.isAutoRepeat)
                            cfg.tecla(evento.key, evento.modifiers, false)
                        evento.accepted = true
                    }

                    Pagina {
                        id: controles

                        width: parent.width - 32
                        x: 16
                        y: 12
                        spacing: 6

                        readonly property bool ligada: janela.depende([cfg.versao], cfg.portaLigada())
                        readonly property int aparelho: janela.depende([cfg.versao], cfg.aparelho())

                        // A porta: as duas USB do console são portas de verdade, cada uma com o seu
                        // mapeamento e o seu aparelho.
                        RowLayout {
                            Label { text: janela.tr("controls.port") }
                            Repeater {
                                model: 2

                                Button {
                                    required property int index
                                    focusPolicy: Qt.NoFocus
                                    checkable: true
                                    checked: cfg.portaEditada === index
                                    text: janela.tr("controls.port.n").replace("{n}", index + 1)
                                    onClicked: cfg.editaPorta(index)
                                }
                            }
                        }
                        RowLayout {
                            CheckBox {
                                focusPolicy: Qt.NoFocus
                                text: janela.tr("controls.port.on")
                                checked: controles.ligada
                                onToggled: cfg.ligaPorta(checked)
                            }
                            Label {
                                enabled: controles.ligada
                                text: janela.tr("controls.kind")
                            }
                            // O Z-Pad e o Dragon são um botão só: os botões, os eixos e o desenho são os
                            // mesmos. O modelo vai na caixa ao lado, porque o console vê o identificador
                            // USB de cada um, e os jogos da Boomerang Sports mudam o tratamento por ele.
                            // Os índices continuam os do `cfg.aparelho()`: 0 Z-Pad, 1 Dragon,
                            // 2 Boomerang, 3 teclado.
                            Repeater {
                                model: [["controls.kind.pad", 1], ["controls.kind.boomerang", 2],
                                        ["controls.kind.keyboard", 3]]

                                Button {
                                    required property var modelData
                                    readonly property bool ehControle: modelData[1] === 1
                                    focusPolicy: Qt.NoFocus
                                    enabled: controles.ligada
                                    checkable: true
                                    // Como botões de rádio: clicar no Controle já escolhido não o
                                    // desmarca. Sem isto o clique alternava o `checked`, e como o
                                    // aparelho não mudava, a ligação não voltava a marcá-lo.
                                    autoExclusive: true
                                    checked: ehControle ? controles.aparelho <= 1
                                                        : controles.aparelho === modelData[1]
                                    text: janela.tr(modelData[0])
                                    // Já num controle, o clique não troca o modelo escolhido. Vindo de
                                    // outro aparelho, é o Dragon, que é o padrão do emulador.
                                    onClicked: cfg.defineAparelho(ehControle && controles.aparelho <= 1
                                                                  ? controles.aparelho : modelData[1])
                                }
                            }
                            Label {
                                visible: controles.aparelho <= 1
                                enabled: controles.ligada
                                Layout.leftMargin: 8
                                text: janela.tr("controls.kind.model")
                            }
                            ComboBox {
                                visible: controles.aparelho <= 1
                                enabled: controles.ligada
                                focusPolicy: Qt.NoFocus
                                model: [janela.tr("controls.kind.zpad"), janela.tr("controls.kind.dragon")]
                                currentIndex: controles.aparelho
                                onActivated: (indice) => cfg.defineAparelho(indice)
                            }
                        }
                        Dica {
                            text: janela.tr(controles.aparelho === 3 ? "controls.kind.hint"
                                            : controles.aparelho === 2 ? "controls.kind.boomerang.hint"
                                            : "controls.kind.pad.hint")
                        }
                        Explica { text: janela.tr("controls.hint") }

                        // Uma porta livre não tem o que configurar, e mostrar o desenho do controle
                        // nela seria convidar a mapear um aparelho que o console não vai enumerar.
                        Dica {
                            visible: !controles.ligada
                            text: janela.tr("controls.port.off_hint")
                        }

                        // O Boomerang: a imagem inclina com o sensor, e embaixo ficam o que está
                        // apertado, a aceleração e a calibração.
                        ColumnLayout {
                            Layout.fillWidth: true
                            visible: controles.ligada && controles.aparelho === 2
                            spacing: 6

                            Opcao {
                                chave: "movimento.aviso_de_calibracao"
                                rotulo: "calibration.toast.setting"
                            }
                            Item {
                                Layout.alignment: Qt.AlignHCenter
                                Layout.preferredWidth: 360
                                Layout.preferredHeight: 324

                                Image {
                                    anchors.centerIn: parent
                                    width: 360
                                    fillMode: Image.PreserveAspectFit
                                    source: "qrc:/zeebx/boomerang.png"
                                    rotation: janela.depende([abaDeControles.tique], cfg.giro())
                                    smooth: true
                                }
                            }
                            Label {
                                Layout.alignment: Qt.AlignHCenter
                                text: janela.depende([abaDeControles.tique], cfg.sensor())
                            }
                            // O caso mais comum de um controle com sensor que não mexe o Boomerang: o
                            // nó do sensor sem permissão. Um clique grava a regra que resolve, pedindo
                            // a senha.
                            ColumnLayout {
                                Layout.fillWidth: true
                                visible: janela.depende([abaDeControles.tique], cfg.sensorSemPermissao())
                                readonly property int estado: janela.depende([abaDeControles.tique], cfg.estadoDaLiberacao())

                                Button {
                                    focusPolicy: Qt.NoFocus
                                    visible: parent.estado !== 1
                                    text: janela.tr("controls.boomerang.unlock")
                                    onClicked: cfg.liberaSensores()
                                }
                                Dica {
                                    text: janela.tr(parent.estado === 1 ? "controls.boomerang.unlock_waiting"
                                                    : "controls.boomerang.unlock_hint")
                                }
                                // A regra entrou, e a leitura volta na próxima tentativa, dentro de um
                                // segundo.
                                Dica {
                                    visible: parent.estado === 2
                                    text: janela.tr("controls.boomerang.unlock_done")
                                }
                                Label {
                                    Layout.fillWidth: true
                                    visible: parent.estado === 3
                                    wrapMode: Text.Wrap
                                    color: Tema.aviso
                                    text: janela.depende([abaDeControles.tique], cfg.falhaDaLiberacao())
                                }
                                TextField {
                                    Layout.fillWidth: true
                                    visible: parent.estado === 3
                                    readOnly: true
                                    font.family: "monospace"
                                    text: cfg.regraDoUdev()
                                }
                            }
                            Label {
                                Layout.alignment: Qt.AlignHCenter
                                font.family: "monospace"
                                text: janela.depende([abaDeControles.tique], cfg.linhaDoMovimento())
                            }
                            RowLayout {
                                Layout.alignment: Qt.AlignHCenter

                                Button {
                                    focusPolicy: Qt.NoFocus
                                    enabled: janela.depende([abaDeControles.tique], cfg.comLeitura() && !cfg.calibrando())
                                    text: janela.tr("controls.boomerang.calibrate")
                                    onClicked: cfg.calibra()
                                }
                                Button {
                                    focusPolicy: Qt.NoFocus
                                    text: janela.tr("controls.boomerang.calibrate_reset")
                                    onClicked: cfg.restauraCalibracao()
                                }
                            }
                            Label {
                                Layout.alignment: Qt.AlignHCenter
                                visible: janela.depende([abaDeControles.tique], cfg.calibracaoRecusada() && !cfg.calibrando())
                                color: Tema.aviso
                                text: janela.tr("controls.boomerang.calibrate_refused")
                            }
                            Dica {
                                horizontalAlignment: Text.AlignHCenter
                                text: janela.tr(janela.depende([abaDeControles.tique], cfg.calibrando())
                                                ? "controls.boomerang.calibrating"
                                                : "controls.boomerang.calibrate_hint")
                            }
                        }

                        // O desenho do controle: a arte, e por cima a silhueta de cada botão — acesa
                        // quando apertado, azulada sob o cursor, pulsando à espera da captura. O clique
                        // é testado pela silhueta, e não por uma caixa.
                        Item {
                            id: desenho

                            readonly property real proporcao: cfg.proporcaoDaArte()
                            // No máximo 500 de largura e 210 de altura, como no egui: a janela também
                            // precisa caber a lista de botões.
                            readonly property real largura: Math.min(Math.min(parent.width, 500), 210 * proporcao)
                            property string sob: ""

                            Layout.fillWidth: true
                            Layout.preferredHeight: proporcao > 0 ? largura / proporcao : 0
                            visible: controles.ligada && controles.aparelho !== 2 && proporcao > 0

                            Item {
                                id: arte

                                anchors.centerIn: parent
                                width: desenho.largura
                                height: desenho.proporcao > 0 ? width / desenho.proporcao : 0

                                Image {
                                    anchors.fill: parent
                                    source: "image://zeebx/controle/base"
                                    smooth: true
                                }

                                Repeater {
                                    model: cfg.partes()

                                    Image {
                                        id: silhueta

                                        required property int index
                                        required property string modelData
                                        readonly property rect limites: cfg.limitesDaParte(index)
                                        readonly property bool esperando: janela.depende([abaDeControles.tique], cfg.capturando() === modelData)
                                        readonly property bool aceso: janela.depende([abaDeControles.tique], cfg.apertado(modelData))

                                        x: limites.x * arte.width
                                        y: limites.y * arte.height
                                        width: limites.width * arte.width
                                        height: limites.height * arte.height
                                        smooth: true
                                        visible: esperando || aceso || desenho.sob === modelData
                                        // As cores do egui: aceso verde, sob o cursor azulado, e a
                                        // captura laranja pulsando no relógio da interface.
                                        source: "image://zeebx/controle/parte/" + index + "/"
                                                + (esperando ? "ffa028ff" : aceso ? "2fd68ab4" : "6c9cff50")
                                        opacity: esperando ? (100 + 110 * (0.5 + 0.5 * Math.sin(abaDeControles.tique * 0.2))) / 255 : 1
                                    }
                                }

                                // O ponto de cada manche, dentro do círculo desenhado: mostra que um
                                // analógico está mesmo chegando ao emulador, e com quanto curso.
                                Repeater {
                                    model: [["lthumb", 0, 1], ["rthumb", 2, 3]]

                                    Rectangle {
                                        required property var modelData
                                        readonly property int parte: cfg.partes().indexOf(modelData[0])
                                        readonly property rect limites: parte >= 0 ? cfg.limitesDaParte(parte) : Qt.rect(0, 0, 0, 0)
                                        readonly property real raio: Math.min(limites.width * arte.width, limites.height * arte.height) / 2

                                        visible: parte >= 0
                                        width: Math.max(raio * 0.44, 4)
                                        height: width
                                        radius: width / 2
                                        color: "#d02f6bd6"
                                        x: (limites.x + limites.width / 2) * arte.width
                                           + janela.depende([abaDeControles.tique], cfg.eixo(modelData[1])) * raio * 0.6 - width / 2
                                        y: (limites.y + limites.height / 2) * arte.height
                                           + janela.depende([abaDeControles.tique], cfg.eixo(modelData[2])) * raio * 0.6 - height / 2
                                    }
                                }

                                MouseArea {
                                    anchors.fill: parent
                                    hoverEnabled: true
                                    cursorShape: desenho.sob !== "" ? Qt.PointingHandCursor : Qt.ArrowCursor
                                    onPositionChanged: (mouse) => desenho.sob = cfg.botaoEm(mouse.x / width, mouse.y / height)
                                    onExited: desenho.sob = ""
                                    // Clicar na peça é o mesmo que clicar em "atribuir" na linha dela.
                                    onClicked: if (desenho.sob !== "") {
                                        cfg.captura(desenho.sob)
                                        janela.escutaTeclas(teclado)
                                    }
                                }
                            }
                        }
                        Dica {
                            visible: desenho.visible
                            horizontalAlignment: Text.AlignHCenter
                            text: janela.tr("controls.art_hint")
                        }

                        // O resto só existe com a porta ligada.
                        ColumnLayout {
                            Layout.fillWidth: true
                            visible: controles.ligada
                            spacing: 6

                            // Escolha do controle. O teclado nunca sai: quem liga um controle continua
                            // podendo usar as teclas.
                            RowLayout {
                                id: escolhaDoControle

                                property int procuras: 0

                                Label { text: janela.tr("controls.device") }
                                ComboBox {
                                    id: listaDeControles

                                    Layout.preferredWidth: 320
                                    focusPolicy: Qt.NoFocus
                                    model: janela.depende([cfg.versao, escolhaDoControle.procuras], cfg.controles())
                                    currentIndex: janela.depende([cfg.versao, escolhaDoControle.procuras], cfg.controleAtual())
                                    onActivated: (indice) => cfg.escolheControle(indice)
                                }
                                Button {
                                    focusPolicy: Qt.NoFocus
                                    text: janela.tr("controls.rescan")
                                    onClicked: {
                                        cfg.leControle()
                                        escolhaDoControle.procuras += 1
                                    }
                                }
                            }
                            Dica {
                                visible: listaDeControles.count <= 1
                                text: janela.tr("controls.no_devices")
                            }
                            Button {
                                focusPolicy: Qt.NoFocus
                                text: janela.tr("controls.reset")
                                onClicked: cfg.restauraMapeamento()
                            }

                            // Os botões do Zeebo: de onde cada um vem, atribuir e limpar.
                            GridLayout {
                                Layout.fillWidth: true
                                Layout.topMargin: 8
                                columns: 4

                                Repeater {
                                    model: cfg.botoes()

                                    Label {
                                        required property int index
                                        required property string modelData
                                        Layout.row: index
                                        Layout.column: 0
                                        text: janela.tr("button." + modelData)
                                    }
                                }
                                Repeater {
                                    model: cfg.botoes()

                                    Label {
                                        required property int index
                                        required property string modelData
                                        Layout.row: index
                                        Layout.column: 1
                                        Layout.fillWidth: true
                                        elide: Text.ElideRight
                                        text: janela.depende([cfg.versao, cfg.portaEditada, Idioma.versao], cfg.origensDoBotao(modelData))
                                    }
                                }
                                Repeater {
                                    model: cfg.botoes()

                                    Button {
                                        required property int index
                                        required property string modelData
                                        readonly property bool esperando: janela.depende([abaDeControles.tique], cfg.capturando() === modelData)
                                        Layout.row: index
                                        Layout.column: 2
                                        focusPolicy: Qt.NoFocus
                                        checkable: true
                                        checked: esperando
                                        text: janela.tr(esperando ? "controls.assigning" : "controls.assign")
                                        onClicked: {
                                            cfg.captura(modelData)
                                            janela.escutaTeclas(teclado)
                                        }
                                    }
                                }
                                Repeater {
                                    model: cfg.botoes()

                                    Button {
                                        required property int index
                                        required property string modelData
                                        Layout.row: index
                                        Layout.column: 3
                                        focusPolicy: Qt.NoFocus
                                        text: janela.tr("controls.clear")
                                        onClicked: cfg.limpaBotao(modelData)
                                    }
                                }
                            }

                            // O turbo da porta: o modo e o botão padrão são do jogador; os toques por
                            // segundo, de todos. A tecla de turbo é a última linha da grade acima. Ver
                            // `docs/implementacao/24-velocidade.md`.
                            Titulo {
                                text: janela.tr("turbo")
                                descricao: janela.tr("turbo.hint")
                            }
                            Repeater {
                                model: [["turbo.mode", "turbo.mode"], ["turbo.button", "turbo.button"], ["turbo.rate", "turbo.rate"]]

                                RowLayout {
                                    required property var modelData

                                    Label {
                                        Layout.preferredWidth: 180
                                        text: janela.tr(modelData[1])
                                    }
                                    ComboBox {
                                        Layout.preferredWidth: 280
                                        focusPolicy: Qt.NoFocus
                                        model: janela.depende([Idioma.versao], cfg.opcoes(modelData[0]))
                                        currentIndex: janela.depende([cfg.versao, cfg.portaEditada], cfg.valor(modelData[0]))
                                        onActivated: (indice) => janela.define(modelData[0], indice)
                                    }
                                }
                            }

                            Label {
                                readonly property string motivo: janela.depende([cfg.versao, abaDeControles.tique], cfg.recusa(false))
                                Layout.fillWidth: true
                                visible: motivo !== ""
                                wrapMode: Text.Wrap
                                color: Tema.aviso
                                text: motivo
                            }
                            Dica {
                                readonly property string dica: janela.depende([abaDeControles.tique], cfg.dicaDeCaptura())
                                visible: dica !== ""
                                text: dica
                            }

                            // Os eixos: um eixo não é um botão, tem curso, e por isso a origem é uma só
                            // e ganha um sentido. O valor ao vivo separa "não mapeado" de "mapeado no
                            // eixo errado": um manche que não chega aparece como um zero teimoso.
                            Titulo {
                                text: janela.tr("controls.axes")
                                descricao: janela.tr("controls.axes.hint")
                            }
                            Label {
                                font.family: "monospace"
                                function linha() {
                                    const nomes = cfg.eixos()
                                    let linha = []
                                    for (let i = 0; i < nomes.length; i++)
                                        linha.push(nomes[i] + ": " + (cfg.eixo(i) >= 0 ? "+" : "") + cfg.eixo(i).toFixed(2))
                                    return linha.join("   ")
                                }
                                text: janela.depende([abaDeControles.tique], linha())
                            }
                            Repeater {
                                model: cfg.eixos()

                                RowLayout {
                                    required property string modelData
                                    readonly property int origem: janela.depende([cfg.versao, cfg.portaEditada], cfg.origemDoEixo(modelData))

                                    Label {
                                        Layout.preferredWidth: 180
                                        text: janela.tr("axis." + modelData)
                                    }
                                    ComboBox {
                                        Layout.preferredWidth: 200
                                        focusPolicy: Qt.NoFocus
                                        model: janela.depende([Idioma.versao], cfg.origensDeEixo())
                                        currentIndex: parent.origem
                                        onActivated: (indice) => cfg.defineOrigemDoEixo(parent.modelData, indice)
                                    }
                                    CheckBox {
                                        focusPolicy: Qt.NoFocus
                                        visible: parent.origem > 0
                                        text: janela.tr("controls.invert")
                                        checked: janela.depende([cfg.versao, cfg.portaEditada], cfg.eixoInvertido(parent.modelData))
                                        onToggled: cfg.inverteEixo(parent.modelData, checked)
                                    }
                                }
                            }

                            Dica {
                                Layout.topMargin: 12
                                Layout.bottomMargin: 16
                                text: janela.tr("controls.players_note")
                            }
                        }
                    }
                }
            }

            // Atalhos: todas as teclas da janela do jogo num lugar só, como o "Hotkey Settings" do
            // Dolphin — as trocáveis, e as fixas só para ler. A tecla vem do teclado desta janela, e o
            // botão de controle de um relógio próprio: o controle não gera evento no Qt.
            ScrollView {
                id: abaDeAtalhos

                readonly property bool aVista: janela.visible && abas.currentIndex === 2
                property int tique: 0

                contentWidth: availableWidth

                Timer {
                    interval: 33
                    repeat: true
                    running: abaDeAtalhos.aVista
                    onTriggered: {
                        cfg.leAtalhoDoControle()
                        abaDeAtalhos.tique += 1
                    }
                }
                onAVistaChanged: if (aVista) tecladoDosAtalhos.forceActiveFocus()

                // O mesmo teclado da aba de controles: com uma captura aberta, a tecla vira o atalho.
                FocusScope {
                    id: tecladoDosAtalhos

                    width: abaDeAtalhos.availableWidth
                    implicitHeight: atalhos.implicitHeight + 32
                    focus: true
                    Keys.onPressed: (evento) => {
                        if (!evento.isAutoRepeat)
                            cfg.tecla(evento.key, evento.modifiers, true)
                        evento.accepted = true
                    }
                    Keys.onReleased: (evento) => {
                        if (!evento.isAutoRepeat)
                            cfg.tecla(evento.key, evento.modifiers, false)
                        evento.accepted = true
                    }

                    Pagina {
                        id: atalhos

                        width: parent.width - 32
                        x: 16
                        y: 12
                        spacing: 6
                        descricaoDaPagina: janela.tr("controls.shortcuts.hint") + " "
                                           + janela.tr("controls.shortcuts.combo.hint")


                        Repeater {
                            model: [
                                ["screenshot", "controls.shortcut.screenshot", "controls.shortcut.waiting", "controls.shortcut.reset"],
                                ["avancar", "controls.shortcut.fast_forward", "controls.shortcut.waiting_any", "controls.shortcut.reset_ff"],
                                ["voltar", "controls.shortcut.rewind", "controls.shortcut.waiting_any", "controls.shortcut.reset_rewind"],
                                ["reiniciar", "controls.shortcut.restart", "controls.shortcut.waiting", "controls.shortcut.reset_restart"],
                                // Os do save state: o de fábrica muda com o sistema, e o botão o diz.
                                ["salvar_estado", "controls.shortcut.save_state", "controls.shortcut.waiting_combo", ""],
                                ["carregar_estado", "controls.shortcut.load_state", "controls.shortcut.waiting_combo", ""],
                                ["proximo_slot", "controls.shortcut.next_slot", "controls.shortcut.waiting_combo", ""],
                                ["slot_anterior", "controls.shortcut.previous_slot", "controls.shortcut.waiting_combo", ""],
                                ["desfazer_carregar", "controls.shortcut.undo_load", "controls.shortcut.waiting_combo", ""]
                            ]

                            RowLayout {
                                required property var modelData
                                readonly property bool esperando: janela.depende([abaDeAtalhos.tique], cfg.capturandoAtalho(modelData[0]))

                                Label {
                                    Layout.preferredWidth: 180
                                    text: janela.tr(modelData[1])
                                }
                                Label {
                                    Layout.preferredWidth: 160
                                    elide: Text.ElideRight
                                    font.family: "monospace"
                                    text: janela.depende([cfg.versao], cfg.atalho(modelData[0]))
                                }
                                Button {
                                    focusPolicy: Qt.NoFocus
                                    checkable: true
                                    checked: parent.esperando
                                    text: janela.tr(parent.esperando ? modelData[2] : "controls.shortcut.change")
                                    onClicked: {
                                        cfg.capturaAtalho(modelData[0])
                                        janela.escutaTeclas(tecladoDosAtalhos)
                                    }
                                }
                                Button {
                                    focusPolicy: Qt.NoFocus
                                    text: modelData[3] !== ""
                                          ? janela.tr(modelData[3])
                                          : janela.tr("controls.shortcut.reset_key").replace("{key}", cfg.padraoDoAtalho(modelData[0]))
                                    onClicked: cfg.restauraAtalho(modelData[0])
                                }
                            }
                        }
                        Label {
                            readonly property string motivo: janela.depende([cfg.versao, abaDeAtalhos.tique], cfg.recusa(true))
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            color: Tema.aviso
                            text: motivo
                        }
                        Dica {
                            readonly property string dica: janela.depende([abaDeAtalhos.tique], cfg.dicaDeCaptura())
                            visible: dica !== ""
                            text: dica
                        }

                        Titulo { text: janela.tr("controls.shortcut.fixed") }
                        Repeater {
                            model: [
                                ["controls.shortcut.quit", "Esc"],
                                ["controls.shortcut.pause", "P"],
                                ["controls.shortcut.fullscreen", "F11 · Alt+Enter"]
                            ]

                            RowLayout {
                                required property var modelData

                                Label {
                                    Layout.preferredWidth: 180
                                    text: janela.tr(modelData[0])
                                }
                                Label {
                                    font.family: "monospace"
                                    opacity: 0.6
                                    text: modelData[1]
                                }
                            }
                        }
                    }
                }
            }

            // Gráficos
            ScrollView {
                contentWidth: availableWidth

                Pagina {
                    width: parent.width - 32
                    x: 16
                    spacing: 6
                    descricaoDaPagina: janela.doJogo ? "" : janela.tr("graphics.window.hint")

                    // As janelas são do emulador, e não do jogo: no modo do jogo, saem.
                    Escolha {
                        Layout.topMargin: 12
                        visible: !janela.doJogo
                        chave: "graphics.janela"
                        rotulo: "graphics.window.main"
                    }
                    Escolha {
                        visible: !janela.doJogo
                        chave: "graphics.janela_do_jogo"
                        rotulo: "graphics.window.game"
                    }
                    // Como o "Render to Separate Window" do PCSX2: desligado, o jogo é desenhado na
                    // principal, no lugar da biblioteca.
                    Opcao {
                        visible: !janela.doJogo
                        chave: "graphics.jogo_separado"
                        rotulo: "graphics.separate_window"
                        dica: "graphics.separate_window.hint"
                    }
                    // Como o "Confirm Shutdown" do PCSX2. Global, como a janela: não muda por jogo.
                    Opcao {
                        visible: !janela.doJogo
                        chave: "graphics.confirmar_ao_fechar"
                        rotulo: "graphics.confirm_close"
                        dica: "graphics.confirm_close.hint"
                    }

                    Titulo { text: janela.tr("graphics.scaling") }
                    Repeater {
                        model: janela.depende([Idioma.versao], cfg.opcoes("graphics.scaling"))

                        RadioButton {
                            required property int index
                            required property string modelData

                            text: modelData
                            checked: janela.v("graphics.scaling") === index
                            onClicked: janela.define("graphics.scaling", index)
                        }
                    }
                    Dica {
                        // "Inteira" é a primeira opção.
                        visible: janela.v("graphics.scaling") === 0
                        text: janela.tr("graphics.scaling.integer.hint")
                    }

                    Opcao {
                        Layout.topMargin: 12
                        chave: "graphics.smooth"
                        rotulo: "graphics.smooth"
                    }
                    Opcao {
                        chave: "graphics.keep_aspect"
                        rotulo: "graphics.keep_aspect"
                    }
                    // O "pôr o quadro na tela pelo GL" do egui não aparece aqui: a janela Qt sempre
                    // põe o quadro pelo scene graph, com a textura da placa quando há uma.
                    Opcao {
                        Layout.topMargin: 12
                        chave: "graphics.neblina"
                        rotulo: "graphics.fog"
                        dica: "graphics.fog.hint"
                    }
                    Opcao {
                        Layout.topMargin: 12
                        chave: "graphics.gpu_rasterizer"
                        rotulo: "graphics.gpu_rasterizer"
                        dica: "graphics.gpu_rasterizer.hint"
                    }

                    // O que depende do rasterizador na placa.
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.topMargin: 12
                        enabled: janela.v("graphics.gpu_rasterizer")
                        spacing: 6

                        Escolha {
                            chave: "graphics.resolucao_interna"
                            rotulo: "graphics.internal_resolution"
                        }
                        Dica { text: janela.tr("graphics.internal_resolution.hint") }
                        Escolha {
                            chave: "graphics.proporcao"
                            rotulo: "graphics.aspect"
                        }
                        Label {
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            color: Tema.aviso
                            text: janela.tr("graphics.aspect.hint")
                        }
                        Escolha {
                            chave: "graphics.antialias"
                            rotulo: "graphics.antialias"
                        }
                        Dica { text: janela.tr("graphics.antialias.hint") }
                        Escolha {
                            chave: "graphics.anisotropico"
                            rotulo: "graphics.anisotropic"
                        }
                        Dica {
                            Layout.bottomMargin: 16
                            text: janela.tr("graphics.anisotropic.hint")
                        }
                    }
                }
            }

            // Velocidade: o ritmo do jogo contra o relógio do mundo. Ver
            // `docs/implementacao/24-velocidade.md`.
            ScrollView {
                contentWidth: availableWidth

                Pagina {
                    width: parent.width - 32
                    x: 16
                    spacing: 6

                    Escolha {
                        Layout.topMargin: 12
                        chave: "speed.fps_limit"
                        rotulo: "speed.fps_limit"
                    }
                    Explica { text: janela.tr("speed.fps_limit.hint") }
                    Escolha {
                        Layout.topMargin: 12
                        chave: "speed.frameskip"
                        rotulo: "speed.frameskip"
                    }
                    Explica { text: janela.tr("speed.frameskip.hint") }

                    Titulo {
                        text: janela.tr("speed.ff")
                        descricao: janela.tr("speed.ff.hint")
                    }
                    Escolha {
                        chave: "speed.ff.ratio"
                        rotulo: "speed.ff.ratio"
                    }
                    Escolha {
                        chave: "speed.ff.mode"
                        rotulo: "speed.ff.mode"
                    }
                    Opcao {
                        chave: "speed.ff.mute"
                        rotulo: "speed.ff.mute"
                        dica: "speed.ff.mute.hint"
                    }

                    Titulo { text: janela.tr("speed.rewind") }
                    Opcao {
                        chave: "speed.rewind.on"
                        rotulo: "speed.rewind.on"
                        dica: "speed.rewind.on.hint"
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.bottomMargin: 16
                        enabled: janela.v("speed.rewind.on")
                        spacing: 6

                        Escolha {
                            chave: "speed.rewind.interval"
                            rotulo: "speed.rewind.interval"
                        }
                        Escolha {
                            chave: "speed.rewind.memory"
                            rotulo: "speed.rewind.memory"
                        }
                        Dica { text: janela.tr("speed.rewind.memory.hint") }
                    }
                }
            }

            // Áudio
            ScrollView {
                contentWidth: availableWidth

                Pagina {
                    width: parent.width - 32
                    x: 16
                    spacing: 6
                    descricaoDaPagina: janela.tr("audio.hint")

                    Opcao {
                        Layout.topMargin: 12
                        chave: "audio.enabled"
                        rotulo: "audio.enabled"
                    }
                    RowLayout {
                        enabled: janela.v("audio.enabled")

                        Slider {
                            Layout.preferredWidth: 280
                            from: 0
                            to: 100
                            stepSize: 1
                            value: janela.v("audio.volume")
                            onMoved: janela.define("audio.volume", Math.round(value))
                        }
                        Label {
                            text: janela.v("audio.volume") + " — " + janela.tr("audio.volume")
                        }
                    }

                    // O banco é aberto quando o jogo abre: a dica diz que a troca vale para o próximo.
                    Titulo {
                        text: janela.tr("audio.soundfont")
                        descricao: janela.tr("audio.soundfont.hint")
                    }
                    RowLayout {
                        Label {
                            Layout.fillWidth: true
                            elide: Text.ElideMiddle
                            font.family: "monospace"
                            text: janela.depende([cfg.versao], cfg.soundfont())
                        }
                        Button {
                            text: janela.tr("settings.browse")
                            onClicked: cfg.escolheSoundfont()
                        }
                        Button {
                            visible: !janela.depende([cfg.versao], cfg.soundfontAutomatico())
                            text: janela.tr("audio.soundfont.clear")
                            onClicked: cfg.usaSoundfontAutomatico()
                        }
                    }
                    Opcao {
                        Layout.topMargin: 12
                        chave: "audio.midi_effects"
                        rotulo: "audio.midi_effects"
                        dica: "audio.midi_effects.hint"
                    }
                }
            }

            // Depuração
            ScrollView {
                contentWidth: availableWidth

                Pagina {
                    width: parent.width - 32
                    x: 16
                    spacing: 6
                    descricaoDaPagina: janela.tr("debug.hint")

                    Opcao {
                        Layout.topMargin: 8
                        chave: "debug.overlay"
                        rotulo: "debug.overlay"
                    }
                    // O que segue só existe dentro do painel; sem ele, marcar não faria efeito nenhum.
                    ColumnLayout {
                        Layout.leftMargin: 24
                        enabled: janela.v("debug.overlay")
                        spacing: 0

                        Opcao { chave: "debug.speed"; rotulo: "debug.speed" }
                        Opcao { chave: "debug.clock"; rotulo: "debug.clock" }
                        Opcao { chave: "debug.memory"; rotulo: "debug.memory" }
                        Opcao { chave: "debug.timeline"; rotulo: "debug.timeline" }
                    }
                    Opcao {
                        Layout.topMargin: 8
                        chave: "debug.metricas"
                        rotulo: "debug.metricas"
                        dica: "debug.metricas.hint"
                    }
                    Opcao {
                        Layout.topMargin: 8
                        chave: "debug.log"
                        rotulo: "debug.log"
                        dica: "debug.log.hint"
                    }
                    Opcao {
                        Layout.topMargin: 8
                        chave: "debug.gravar_sessao"
                        rotulo: "debug.gravar_sessao"
                        dica: "debug.gravar_sessao.hint"
                    }
                    Button {
                        Layout.leftMargin: 24
                        text: janela.tr("debug.gravar_sessao.open")
                        onClicked: Qt.openUrlExternally(cfg.enderecoDaPastaDeSessoes())
                    }
                }
            }
        }
    }
}
