import GameController
import SwiftUI
import UIKit

/// The Zeebo controller indices, resolved by name so the table is not copied by hand.
struct ButtonMap {
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

    static func resolver() -> ButtonMap {
        func index(_ name: String) -> UInt32 {
            let found = name.withCString { zeebx_ios_button_by_name($0) }
            precondition(found >= 0, "the core does not know the button \(name)")
            return UInt32(found)
        }
        return ButtonMap(
            b1: index("b1"), b2: index("b2"), b3: index("b3"), b4: index("b4"),
            zl: index("zl"), zr: index("zr"), l2: index("l2"), r2: index("r2"),
            up: index("up"), down: index("down"), left: index("left"), right: index("right"),
            back: index("back"), start: index("start")
        )
    }
}

struct GameView: UIViewControllerRepresentable {
    @ObservedObject var model: ZeebxModel

    func makeUIViewController(context: Context) -> GameViewController {
        GameViewController(model: model)
    }

    func updateUIViewController(_ controller: GameViewController, context: Context) {}

    static func dismantleUIViewController(_ controller: GameViewController, coordinator: ()) {
        controller.stop()
    }
}

/// The game screen: the frame, touch and the controller.
///
/// `CADisplayLink` is what calls `zeebx_ios_step`. It runs on the main thread, which is the
/// same as the list's — the pointer does not cross threads.
final class GameViewController: UIViewController {
    let model: ZeebxModel
    let map = ButtonMap.resolver()
    let image = UIImageView()
    var displayLink: CADisplayLink?
    /// Buttons held by a finger or the keyboard. The physical controller goes on top, every turn.
    var holding: Set<UInt32> = []
    var finished = false

    init(model: ZeebxModel) {
        self.model = model
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("the game does not come from a storyboard")
    }

    override var prefersStatusBarHidden: Bool { true }
    override var prefersHomeIndicatorAutoHidden: Bool { true }
    override var canBecomeFirstResponder: Bool { true }
    override var supportedInterfaceOrientations: UIInterfaceOrientationMask { .landscape }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        image.contentMode = .scaleAspectFit
        image.layer.magnificationFilter = .nearest
        image.layer.minificationFilter = .nearest
        image.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(image)
        NSLayoutConstraint.activate([
            image.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            image.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            image.topAnchor.constraint(equalTo: view.topAnchor),
            image.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
        buildControls()
        let displayLink = CADisplayLink(target: self, selector: #selector(tick))
        displayLink.add(to: .main, forMode: .common)
        self.displayLink = displayLink
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        becomeFirstResponder()
    }

    func stop() {
        displayLink?.invalidate()
        displayLink = nil
    }

    @objc func tick() {
        guard !finished else { return }
        applyInput()
        let code = zeebx_ios_step(model.app)
        if code == ZEEBX_STOPPED {
            finished = true
            stop()
            let message = zeebx_ios_error(model.app).map { String(cString: $0) } ?? ""
            model.error = message.isEmpty ? nil : message
            model.playing = false
            return
        }
        var width: UInt32 = 0
        var height: UInt32 = 0
        if let pointer = zeebx_ios_frame(model.app, &width, &height) {
            image.image = Self.image(from: pointer, width: Int(width), height: Int(height))
        }
    }

    func applyInput() {
        let physical = GCController.controllers().lazy.compactMap(\.extendedGamepad).first
        for (index, pressed) in reading(physical) {
            let enabled = pressed || holding.contains(index)
            zeebx_ios_button(model.app, index, enabled ? 1 : 0)
        }
        let (x, y) = axes(physical)
        zeebx_ios_axis(model.app, 0, x)
        zeebx_ios_axis(model.app, 1, y)
        if let right = physical?.rightThumbstick {
            zeebx_ios_axis(model.app, 2, Self.travel(right.xAxis.value))
            zeebx_ios_axis(model.app, 3, Self.travel(-right.yAxis.value))
        }
    }

    /// The modern controller on the Android map: south on `b1`, east on `b2`, west on `b3`,
    /// north on `b4`. GameController's Y grows upward; the console's, downward.
    func reading(_ game: GCExtendedGamepad?) -> [(UInt32, Bool)] {
        guard let game else {
            return [
                (map.b1, false), (map.b2, false), (map.b3, false), (map.b4, false),
                (map.zl, false), (map.zr, false), (map.l2, false), (map.r2, false),
                (map.up, false), (map.down, false), (map.left, false), (map.right, false),
                (map.back, false), (map.start, false),
            ]
        }
        return [
            (map.b1, game.buttonA.isPressed),
            (map.b2, game.buttonB.isPressed),
            (map.b3, game.buttonX.isPressed),
            (map.b4, game.buttonY.isPressed),
            (map.zl, game.leftShoulder.isPressed),
            (map.zr, game.rightShoulder.isPressed),
            (map.l2, game.leftTrigger.isPressed),
            (map.r2, game.rightTrigger.isPressed),
            (map.up, game.dpad.up.isPressed),
            (map.down, game.dpad.down.isPressed),
            (map.left, game.dpad.left.isPressed),
            (map.right, game.dpad.right.isPressed),
            (map.back, game.buttonMenu.isPressed),
            (map.start, game.buttonOptions?.isPressed ?? false),
        ]
    }

    func axes(_ game: GCExtendedGamepad?) -> (Int32, Int32) {
        guard let left = game?.leftThumbstick else {
            return (0, 0)
        }
        let x = Self.travel(left.xAxis.value)
        let y = Self.travel(-left.yAxis.value)
        return (x, y)
    }

    static func travel(_ raw: Float) -> Int32 {
        let value = abs(raw) < 0.15 ? 0 : raw
        return Int32((value * 128).rounded())
    }

    static func image(from pointer: UnsafePointer<UInt8>, width: Int, height: Int) -> UIImage? {
        let rowBytes = width * 4
        guard width > 0, height > 0 else { return nil }
        let data = Data(bytes: pointer, count: rowBytes * height)
        guard let provider = CGDataProvider(data: data as CFData) else { return nil }
        let order = CGBitmapInfo.byteOrder32Little.union(
            CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue)
        )
        guard let cg = CGImage(
            width: width,
            height: height,
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: rowBytes,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: order,
            provider: provider,
            decode: nil,
            shouldInterpolate: false,
            intent: .defaultIntent
        ) else { return nil }
        return UIImage(cgImage: cg)
    }

    func buildControls() {
        let margin = view.safeAreaLayoutGuide
        let quit = UIButton(type: .system)
        quit.setTitle("Quit", for: .normal)
        quit.tintColor = .white
        quit.addTarget(self, action: #selector(exitGame), for: .touchUpInside)
        quit.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(quit)
        NSLayoutConstraint.activate([
            quit.leadingAnchor.constraint(equalTo: margin.leadingAnchor, constant: 12),
            quit.topAnchor.constraint(equalTo: margin.topAnchor, constant: 8),
        ])

        let cross = dpadCross()
        let diamond = faceDiamond()
        cross.translatesAutoresizingMaskIntoConstraints = false
        diamond.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(cross)
        view.addSubview(diamond)
        NSLayoutConstraint.activate([
            cross.leadingAnchor.constraint(equalTo: margin.leadingAnchor, constant: 16),
            cross.bottomAnchor.constraint(equalTo: margin.bottomAnchor, constant: -16),
            diamond.trailingAnchor.constraint(equalTo: margin.trailingAnchor, constant: -16),
            diamond.bottomAnchor.constraint(equalTo: margin.bottomAnchor, constant: -16),
        ])
    }

    func dpadCross() -> UIView {
        let container = UIStackView()
        container.axis = .vertical
        container.alignment = .center
        container.spacing = 4
        container.addArrangedSubview(button("L", map.zl))
        let up = button("↑", map.up)
        let middle = UIStackView()
        middle.axis = .horizontal
        middle.spacing = 4
        middle.addArrangedSubview(button("←", map.left))
        middle.addArrangedSubview(button("→", map.right))
        let down = button("↓", map.down)
        container.addArrangedSubview(up)
        container.addArrangedSubview(middle)
        container.addArrangedSubview(down)
        return container
    }

    /// South is 1, east 2, west 3, north 4 — the same map as Android.
    func faceDiamond() -> UIView {
        let container = UIStackView()
        container.axis = .vertical
        container.alignment = .center
        container.spacing = 4
        container.addArrangedSubview(button("R", map.zr))
        let north = button("4", map.b4)
        let middle = UIStackView()
        middle.axis = .horizontal
        middle.spacing = 36
        middle.addArrangedSubview(button("3", map.b3))
        middle.addArrangedSubview(button("2", map.b2))
        let south = button("1", map.b1)
        container.addArrangedSubview(north)
        container.addArrangedSubview(middle)
        container.addArrangedSubview(south)
        return container
    }

    func button(_ title: String, _ index: UInt32) -> UIButton {
        let button = UIButton(type: .custom)
        button.setTitle(title, for: .normal)
        button.setTitleColor(.white, for: .normal)
        button.titleLabel?.font = .boldSystemFont(ofSize: 18)
        button.backgroundColor = UIColor(white: 0.2, alpha: 0.72)
        button.layer.cornerRadius = 28
        button.tag = Int(index)
        button.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            button.widthAnchor.constraint(equalToConstant: 56),
            button.heightAnchor.constraint(equalToConstant: 56),
        ])
        button.addTarget(self, action: #selector(went_down(_:)), for: .touchDown)
        button.addTarget(self, action: #selector(went_up(_:)), for: [.touchUpInside, .touchUpOutside, .touchCancel])
        return button
    }

    @objc func went_down(_ button: UIButton) {
        holding.insert(UInt32(button.tag))
    }

    @objc func went_up(_ button: UIButton) {
        holding.remove(UInt32(button.tag))
    }

    @objc func exitGame() {
        stop()
        model.close()
    }

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var handled = false
        for touch in presses {
            if let index = keyboardIndex(touch) {
                holding.insert(index)
                handled = true
            }
        }
        if !handled {
            super.pressesBegan(presses, with: event)
        }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        for touch in presses {
            if let index = keyboardIndex(touch) {
                holding.remove(index)
            }
        }
        super.pressesEnded(presses, with: event)
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        pressesEnded(presses, with: event)
    }

    /// The same map as the desktop window: arrows on the d-pad, Z/X/C/V on the four buttons,
    /// Q and W on the triggers, H/Backspace/Enter on HOME.

    func keyboardIndex(_ touch: UIPress) -> UInt32? {
        guard let key = touch.key else { return nil }
        switch key.keyCode {
        case .keyboardUpArrow: return map.up
        case .keyboardDownArrow: return map.down
        case .keyboardLeftArrow: return map.left
        case .keyboardRightArrow: return map.right
        case .keyboardZ: return map.b1
        case .keyboardX, .keyboardSpacebar: return map.b2
        case .keyboardC: return map.b3
        case .keyboardV: return map.b4
        case .keyboardQ: return map.zl
        case .keyboardW: return map.zr
        case .keyboardH, .keyboardDeleteOrBackspace, .keyboardReturnOrEnter: return map.back
        default: return nil
        }
    }
}
