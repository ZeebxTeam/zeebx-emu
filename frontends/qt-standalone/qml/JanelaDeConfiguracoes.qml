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

    width: 720
    height: 640
    minimumWidth: 420
    minimumHeight: 320
    title: "Zeebx — " + tr("settings.title")

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
        onBibliotecaMudou: (varrer) => {
            if (varrer)
                janela.biblioteca.procuraDeNovo()
            else
                janela.biblioteca.refaz()
        }
        onJanelaPrincipalMudou: janela.principal.aplicaModo()
    }

    // Um liga-desliga com a explicação embaixo, apagada: o jeito do egui.
    component Opcao: ColumnLayout {
        id: opcao

        required property string chave
        required property string rotulo
        property string dica: ""

        Layout.fillWidth: true
        spacing: 0

        CheckBox {
            text: janela.tr(opcao.rotulo)
            checked: janela.v(opcao.chave)
            onToggled: janela.define(opcao.chave, checked)
        }
        Label {
            Layout.fillWidth: true
            Layout.leftMargin: 32
            visible: opcao.dica !== ""
            wrapMode: Text.Wrap
            opacity: 0.6
            text: opcao.dica !== "" ? janela.tr(opcao.dica) : ""
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

    component Titulo: Label {
        Layout.topMargin: 12
        font.bold: true
    }

    header: TabBar {
        id: abas

        TabButton { text: janela.tr("settings.tab.general") }
        TabButton { text: janela.tr("settings.tab.controls") }
        TabButton { text: janela.tr("settings.tab.graphics") }
        TabButton { text: janela.tr("settings.tab.audio") }
        TabButton { text: janela.tr("settings.tab.debug") }
        TabButton { text: janela.tr("settings.tab.about") }
    }

    // Toda aba rola: a gráfica já não cabe na altura padrão da janela, e uma opção que some
    // embaixo da borda é uma opção que não existe.
    StackLayout {
        anchors.fill: parent
        currentIndex: abas.currentIndex

        // Geral
        ScrollView {
            contentWidth: availableWidth

            ColumnLayout {
                width: parent.width - 32
                x: 16
                spacing: 6

                Titulo { text: janela.tr("settings.roms_folder") }
                Dica { text: janela.tr("settings.roms_folder.hint") }
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
                    color: "#e0a030"
                    text: janela.tr("common.folder_missing")
                }

                // A padrão mora em `~/.config`, que é escondida: por isso o caminho aparece mesmo
                // sem escolha, e há o botão de abrir. Ver `docs/implementacao/22-screenshots.md`.
                Titulo { text: janela.tr("settings.screenshots_folder") }
                Dica { text: janela.tr("settings.screenshots_folder.hint") }
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

                Titulo { text: janela.tr("settings.language") }
                Dica { text: janela.tr("settings.language.hint") }
                ComboBox {
                    Layout.preferredWidth: 280
                    model: cfg.opcoes("language")
                    currentIndex: janela.v("language")
                    onActivated: (indice) => janela.define("language", indice)
                }

                Titulo { text: janela.tr("settings.library_view") }
                Dica { text: janela.tr("settings.library_view.hint") }
                ComboBox {
                    Layout.preferredWidth: 280
                    model: janela.depende([Idioma.versao], cfg.opcoes("biblioteca"))
                    currentIndex: janela.v("biblioteca")
                    onActivated: (indice) => janela.define("biblioteca", indice)
                }

                Titulo { text: janela.tr("settings.z_wheel") }
                Dica { text: janela.tr("settings.z_wheel.hint") }
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
                    color: janela.depende([cfg.versao], cfg.zWheelAchada()) ? palette.windowText : "#e0a030"
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
                        visible: pagina !== ""
                        text: janela.tr("update.download")
                        onClicked: Qt.openUrlExternally(pagina)
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
                        cfg.tecla(evento.key, true)
                    evento.accepted = true
                }
                Keys.onReleased: (evento) => {
                    if (!evento.isAutoRepeat)
                        cfg.tecla(evento.key, false)
                    evento.accepted = true
                }

                ColumnLayout {
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
                        Repeater {
                            model: ["controls.kind.zpad", "controls.kind.dragon",
                                    "controls.kind.boomerang", "controls.kind.keyboard"]

                            Button {
                                required property int index
                                required property string modelData
                                focusPolicy: Qt.NoFocus
                                enabled: controles.ligada
                                checkable: true
                                checked: controles.aparelho === index
                                text: janela.tr(modelData)
                                onClicked: cfg.defineAparelho(index)
                            }
                        }
                    }
                    Dica {
                        text: janela.tr(controles.aparelho === 3 ? "controls.kind.hint"
                                        : controles.aparelho === 2 ? "controls.kind.boomerang.hint"
                                        : "controls.kind.pad.hint")
                    }
                    Dica { text: janela.tr("controls.hint") }

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
                                color: "#e0a030"
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
                            color: "#e0a030"
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
                                onClicked: if (desenho.sob !== "") cfg.captura(desenho.sob)
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
                                    onClicked: cfg.captura(modelData)
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

                        Label {
                            readonly property string motivo: janela.depende([cfg.versao, abaDeControles.tique], cfg.recusa(false))
                            Layout.fillWidth: true
                            visible: motivo !== ""
                            wrapMode: Text.Wrap
                            color: "#e0a030"
                            text: motivo
                        }

                        // Os eixos: um eixo não é um botão, tem curso, e por isso a origem é uma só
                        // e ganha um sentido. O valor ao vivo separa "não mapeado" de "mapeado no
                        // eixo errado": um manche que não chega aparece como um zero teimoso.
                        Titulo { text: janela.tr("controls.axes") }
                        Dica { text: janela.tr("controls.axes.hint") }
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
                            text: janela.tr("controls.players_note")
                        }

                        // Os atalhos da janela do jogo. A tecla vem pelo mesmo teclado desta aba;
                        // a captura recusa o que já tem dono. Ver `docs/implementacao/22-screenshots.md`.
                        Titulo { text: janela.tr("controls.shortcuts") }
                        Dica { text: janela.tr("controls.shortcuts.hint") }
                        RowLayout {
                            readonly property bool esperando: janela.depende([abaDeControles.tique], cfg.capturandoAtalho())

                            Label {
                                Layout.preferredWidth: 180
                                text: janela.tr("controls.shortcut.screenshot")
                            }
                            Label {
                                Layout.preferredWidth: 80
                                font.family: "monospace"
                                text: janela.depende([cfg.versao], cfg.atalhoDeScreenshot())
                            }
                            Button {
                                focusPolicy: Qt.NoFocus
                                checkable: true
                                checked: parent.esperando
                                text: janela.tr(parent.esperando ? "controls.shortcut.waiting" : "controls.shortcut.change")
                                onClicked: cfg.capturaAtalho()
                            }
                            Button {
                                focusPolicy: Qt.NoFocus
                                text: janela.tr("controls.shortcut.reset")
                                onClicked: cfg.restauraAtalho()
                            }
                        }
                        Label {
                            readonly property string motivo: janela.depende([cfg.versao, abaDeControles.tique], cfg.recusa(true))
                            Layout.fillWidth: true
                            Layout.bottomMargin: 16
                            wrapMode: Text.Wrap
                            color: "#e0a030"
                            text: motivo
                        }
                    }
                }
            }
        }

        // Gráficos
        ScrollView {
            contentWidth: availableWidth

            ColumnLayout {
                width: parent.width - 32
                x: 16
                spacing: 6

                Escolha {
                    Layout.topMargin: 12
                    chave: "graphics.janela"
                    rotulo: "graphics.window.main"
                }
                Escolha {
                    chave: "graphics.janela_do_jogo"
                    rotulo: "graphics.window.game"
                }
                Dica { text: janela.tr("graphics.window.hint") }

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
                Opcao {
                    chave: "graphics.speed_limit"
                    rotulo: "graphics.speed_limit"
                    dica: "graphics.speed_limit.hint"
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
                        color: "#e0a030"
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

        // Áudio
        ScrollView {
            contentWidth: availableWidth

            ColumnLayout {
                width: parent.width - 32
                x: 16
                spacing: 6

                Dica {
                    Layout.topMargin: 12
                    text: janela.tr("audio.hint")
                }
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
            }
        }

        // Depuração
        ScrollView {
            contentWidth: availableWidth

            ColumnLayout {
                width: parent.width - 32
                x: 16
                spacing: 6

                Dica {
                    Layout.topMargin: 12
                    text: janela.tr("debug.hint")
                }
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
                    chave: "debug.log"
                    rotulo: "debug.log"
                    dica: "debug.log.hint"
                }
            }
        }

        // Sobre
        ScrollView {
            contentWidth: availableWidth

            ColumnLayout {
                width: parent.width - 32
                x: 16
                spacing: 6

                Label {
                    Layout.topMargin: 12
                    font.pixelSize: 22
                    font.bold: true
                    text: janela.tr("app.name")
                }
                Label {
                    text: janela.tr("about.version").replace("{version}", cfg.versaoDoEmulador())
                }
                Dica { text: janela.tr("about.tagline") }
                Button {
                    Layout.topMargin: 16
                    flat: true
                    text: janela.tr("about.repository")
                    onClicked: Qt.openUrlExternally(cfg.endereco("repositorio"))
                }
                Button {
                    flat: true
                    text: "💬 " + janela.tr("about.discord")
                    onClicked: Qt.openUrlExternally(cfg.endereco("discord"))
                }
            }
        }
    }
}
