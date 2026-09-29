import GameController
import SwiftUI
import UIKit

/// Os índices do controle do Zeebo, resolvidos pelo nome para não copiar a tabela à mão.
struct MapaDeBotoes {
    let b1: UInt32
    let b2: UInt32
    let b3: UInt32
    let b4: UInt32
    let zl: UInt32
    let zr: UInt32
    let l2: UInt32
    let r2: UInt32
    let up: UInt32
    let down: UInt32
    let left: UInt32
    let right: UInt32
    let back: UInt32
    let start: UInt32

    static func resolver() -> MapaDeBotoes {
        func indice(_ nome: String) -> UInt32 {
            let achado = nome.withCString { zeebx_ios_botao_por_nome($0) }
            precondition(achado >= 0, "o núcleo não conhece o botão \(nome)")
            return UInt32(achado)
        }
        return MapaDeBotoes(
            b1: indice("b1"), b2: indice("b2"), b3: indice("b3"), b4: indice("b4"),
            zl: indice("zl"), zr: indice("zr"), l2: indice("l2"), r2: indice("r2"),
            up: indice("up"), down: indice("down"), left: indice("left"), right: indice("right"),
            back: indice("back"), start: indice("start")
        )
    }
}

struct JogoView: UIViewControllerRepresentable {
    @ObservedObject var modelo: ZeebxModelo

    func makeUIViewController(context: Context) -> JogoControlador {
        JogoControlador(modelo: modelo)
    }

    func updateUIViewController(_ controlador: JogoControlador, context: Context) {}

    static func dismantleUIViewController(_ controlador: JogoControlador, coordinator: ()) {
        controlador.parar()
    }
}

/// A tela do jogo: o quadro, o toque e o controle.
///
/// O `CADisplayLink` é quem chama `zeebx_ios_passo`. Ele corre na linha principal, que é a
/// mesma da lista — o ponteiro não atravessa linhas.
final class JogoControlador: UIViewController {
    let modelo: ZeebxModelo
    let mapa = MapaDeBotoes.resolver()
    let imagem = UIImageView()
    var elo: CADisplayLink?
    /// Botões segurados pelo dedo ou pelo teclado. O controle físico entra por cima, a cada volta.
    var segurando: Set<UInt32> = []
    var acabou = false

    init(modelo: ZeebxModelo) {
        self.modelo = modelo
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("o jogo não vem de storyboard")
    }

    override var prefersStatusBarHidden: Bool { true }
    override var prefersHomeIndicatorAutoHidden: Bool { true }
    override var canBecomeFirstResponder: Bool { true }
    override var supportedInterfaceOrientations: UIInterfaceOrientationMask { .landscape }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        imagem.contentMode = .scaleAspectFit
        imagem.layer.magnificationFilter = .nearest
        imagem.layer.minificationFilter = .nearest
        imagem.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(imagem)
        NSLayoutConstraint.activate([
            imagem.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            imagem.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            imagem.topAnchor.constraint(equalTo: view.topAnchor),
            imagem.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
        montaControles()
        let elo = CADisplayLink(target: self, selector: #selector(volta))
        elo.add(to: .main, forMode: .common)
        self.elo = elo
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        becomeFirstResponder()
    }

    func parar() {
        elo?.invalidate()
        elo = nil
    }

    @objc func volta() {
        guard !acabou else { return }
        aplicaEntrada()
        let codigo = zeebx_ios_passo(modelo.app)
        if codigo == ZEEBX_PAROU {
            acabou = true
            parar()
            let mensagem = zeebx_ios_erro(modelo.app).map { String(cString: $0) } ?? ""
            modelo.erro = mensagem.isEmpty ? nil : mensagem
            modelo.jogando = false
            return
        }
        var largura: UInt32 = 0
        var altura: UInt32 = 0
        if let ponteiro = zeebx_ios_quadro(modelo.app, &largura, &altura) {
            imagem.image = Self.imagem(de: ponteiro, largura: Int(largura), altura: Int(altura))
        }
    }

    func aplicaEntrada() {
        let fisico = GCController.controllers().lazy.compactMap(\.extendedGamepad).first
        for (indice, apertado) in leitura(fisico) {
            let ligado = apertado || segurando.contains(indice)
            zeebx_ios_botao(modelo.app, indice, ligado ? 1 : 0)
        }
        let (x, y) = eixos(fisico)
        zeebx_ios_eixo(modelo.app, 0, x)
        zeebx_ios_eixo(modelo.app, 1, y)
        if let direito = fisico?.rightThumbstick {
            zeebx_ios_eixo(modelo.app, 2, Self.curso(direito.xAxis.value))
            zeebx_ios_eixo(modelo.app, 3, Self.curso(-direito.yAxis.value))
        }
    }

    /// O controle moderno no mapa do Android: sul no `b1`, leste no `b2`, oeste no `b3`,
    /// norte no `b4`. O Y do GameController cresce para cima; o do console, para baixo.
    func leitura(_ jogo: GCExtendedGamepad?) -> [(UInt32, Bool)] {
        guard let jogo else {
            return [
                (mapa.b1, false), (mapa.b2, false), (mapa.b3, false), (mapa.b4, false),
                (mapa.zl, false), (mapa.zr, false), (mapa.l2, false), (mapa.r2, false),
                (mapa.up, false), (mapa.down, false), (mapa.left, false), (mapa.right, false),
                (mapa.back, false), (mapa.start, false),
            ]
        }
        return [
            (mapa.b1, jogo.buttonA.isPressed),
            (mapa.b2, jogo.buttonB.isPressed),
            (mapa.b3, jogo.buttonX.isPressed),
            (mapa.b4, jogo.buttonY.isPressed),
            (mapa.zl, jogo.leftShoulder.isPressed),
            (mapa.zr, jogo.rightShoulder.isPressed),
            (mapa.l2, jogo.leftTrigger.isPressed),
            (mapa.r2, jogo.rightTrigger.isPressed),
            (mapa.up, jogo.dpad.up.isPressed),
            (mapa.down, jogo.dpad.down.isPressed),
            (mapa.left, jogo.dpad.left.isPressed),
            (mapa.right, jogo.dpad.right.isPressed),
            (mapa.back, jogo.buttonMenu.isPressed),
            (mapa.start, jogo.buttonOptions?.isPressed ?? false),
        ]
    }

    func eixos(_ jogo: GCExtendedGamepad?) -> (Int32, Int32) {
        guard let esquerdo = jogo?.leftThumbstick else {
            return (0, 0)
        }
        let x = Self.curso(esquerdo.xAxis.value)
        let y = Self.curso(-esquerdo.yAxis.value)
        return (x, y)
    }

    static func curso(_ cru: Float) -> Int32 {
        let valor = abs(cru) < 0.15 ? 0 : cru
        return Int32((valor * 128).rounded())
    }

    static func imagem(de ponteiro: UnsafePointer<UInt8>, largura: Int, altura: Int) -> UIImage? {
        let bytesPorLinha = largura * 4
        guard largura > 0, altura > 0 else { return nil }
        let dados = Data(bytes: ponteiro, count: bytesPorLinha * altura)
        guard let provedor = CGDataProvider(data: dados as CFData) else { return nil }
        let ordem = CGBitmapInfo.byteOrder32Little.union(
            CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue)
        )
        guard let cg = CGImage(
            width: largura,
            height: altura,
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: bytesPorLinha,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: ordem,
            provider: provedor,
            decode: nil,
            shouldInterpolate: false,
            intent: .defaultIntent
        ) else { return nil }
        return UIImage(cgImage: cg)
    }

    func montaControles() {
        let margem = view.safeAreaLayoutGuide
        let sair = UIButton(type: .system)
        sair.setTitle("Sair", for: .normal)
        sair.tintColor = .white
        sair.addTarget(self, action: #selector(sairDoJogo), for: .touchUpInside)
        sair.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(sair)
        NSLayoutConstraint.activate([
            sair.leadingAnchor.constraint(equalTo: margem.leadingAnchor, constant: 12),
            sair.topAnchor.constraint(equalTo: margem.topAnchor, constant: 8),
        ])

        let cruz = cruzDirecional()
        let losango = losangoDeFaces()
        cruz.translatesAutoresizingMaskIntoConstraints = false
        losango.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(cruz)
        view.addSubview(losango)
        NSLayoutConstraint.activate([
            cruz.leadingAnchor.constraint(equalTo: margem.leadingAnchor, constant: 16),
            cruz.bottomAnchor.constraint(equalTo: margem.bottomAnchor, constant: -16),
            losango.trailingAnchor.constraint(equalTo: margem.trailingAnchor, constant: -16),
            losango.bottomAnchor.constraint(equalTo: margem.bottomAnchor, constant: -16),
        ])
    }

    func cruzDirecional() -> UIView {
        let caixa = UIStackView()
        caixa.axis = .vertical
        caixa.alignment = .center
        caixa.spacing = 4
        caixa.addArrangedSubview(botao("L", mapa.zl))
        let cima = botao("↑", mapa.up)
        let meio = UIStackView()
        meio.axis = .horizontal
        meio.spacing = 4
        meio.addArrangedSubview(botao("←", mapa.left))
        meio.addArrangedSubview(botao("→", mapa.right))
        let baixo = botao("↓", mapa.down)
        caixa.addArrangedSubview(cima)
        caixa.addArrangedSubview(meio)
        caixa.addArrangedSubview(baixo)
        return caixa
    }

    /// Sul é o 1, leste o 2, oeste o 3, norte o 4 — o mesmo mapa do Android.
    func losangoDeFaces() -> UIView {
        let caixa = UIStackView()
        caixa.axis = .vertical
        caixa.alignment = .center
        caixa.spacing = 4
        caixa.addArrangedSubview(botao("R", mapa.zr))
        let norte = botao("4", mapa.b4)
        let meio = UIStackView()
        meio.axis = .horizontal
        meio.spacing = 36
        meio.addArrangedSubview(botao("3", mapa.b3))
        meio.addArrangedSubview(botao("2", mapa.b2))
        let sul = botao("1", mapa.b1)
        caixa.addArrangedSubview(norte)
        caixa.addArrangedSubview(meio)
        caixa.addArrangedSubview(sul)
        return caixa
    }

    func botao(_ titulo: String, _ indice: UInt32) -> UIButton {
        let botao = UIButton(type: .custom)
        botao.setTitle(titulo, for: .normal)
        botao.setTitleColor(.white, for: .normal)
        botao.titleLabel?.font = .boldSystemFont(ofSize: 18)
        botao.backgroundColor = UIColor(white: 0.2, alpha: 0.72)
        botao.layer.cornerRadius = 28
        botao.tag = Int(indice)
        botao.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            botao.widthAnchor.constraint(equalToConstant: 56),
            botao.heightAnchor.constraint(equalToConstant: 56),
        ])
        botao.addTarget(self, action: #selector(desceu(_:)), for: .touchDown)
        botao.addTarget(self, action: #selector(subiu(_:)), for: [.touchUpInside, .touchUpOutside, .touchCancel])
        return botao
    }

    @objc func desceu(_ botao: UIButton) {
        segurando.insert(UInt32(botao.tag))
    }

    @objc func subiu(_ botao: UIButton) {
        segurando.remove(UInt32(botao.tag))
    }

    @objc func sairDoJogo() {
        parar()
        modelo.fecha()
    }

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var tratado = false
        for toque in presses {
            if let indice = indiceDoTeclado(toque) {
                segurando.insert(indice)
                tratado = true
            }
        }
        if !tratado {
            super.pressesBegan(presses, with: event)
        }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        for toque in presses {
            if let indice = indiceDoTeclado(toque) {
                segurando.remove(indice)
            }
        }
        super.pressesEnded(presses, with: event)
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        pressesEnded(presses, with: event)
    }

    /// O mesmo mapa da janela de desktop: setas no direcional, Z/X/C/V nos quatro botões,
    /// Q e W nos gatilhos, H/Backspace/Enter no HOME.
    func indiceDoTeclado(_ toque: UIPress) -> UInt32? {
        guard let tecla = toque.key else { return nil }
        switch tecla.keyCode {
        case .keyboardUpArrow: return mapa.up
        case .keyboardDownArrow: return mapa.down
        case .keyboardLeftArrow: return mapa.left
        case .keyboardRightArrow: return mapa.right
        case .keyboardZ: return mapa.b1
        case .keyboardX, .keyboardSpacebar: return mapa.b2
        case .keyboardC: return mapa.b3
        case .keyboardV: return mapa.b4
        case .keyboardQ: return mapa.zl
        case .keyboardW: return mapa.zr
        case .keyboardH, .keyboardDeleteOrBackspace, .keyboardReturnOrEnter: return mapa.back
        default: return nil
        }
    }
}
