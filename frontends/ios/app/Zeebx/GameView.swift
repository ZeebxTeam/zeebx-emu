import GameController
import SwiftUI
import UIKit

/// The Zeebo controller indices a keyboard presses, resolved by name so the table is not copied
/// by hand.
struct KeyboardMap {
    let b1: UInt32
    let b2: UInt32
    let b3: UInt32
    let b4: UInt32
    let zl: UInt32
    let zr: UInt32
    let up: UInt32
    let down: UInt32
    let left: UInt32
    let right: UInt32
    let back: UInt32

    static func resolver() -> KeyboardMap {
        func index(_ name: String) -> UInt32 {
            let found = zeebx_ios_button_by_name(name)
            precondition(found >= 0, "the core does not know the button \(name)")
            return UInt32(found)
        }
        return KeyboardMap(
            b1: index("b1"), b2: index("b2"), b3: index("b3"), b4: index("b4"),
            zl: index("zl"), zr: index("zr"),
            up: index("up"), down: index("down"), left: index("left"), right: index("right"),
            back: index("back")
        )
    }
}

/// The game, and the pause menu over it when it is up.
struct GameScreen: View {
    @ObservedObject var model: ZeebxModel

    var body: some View {
        ZStack {
            GameView(model: model)
                .ignoresSafeArea()
            if model.menuOpen {
                PauseMenuView(model: model)
            }
        }
        .preferredColorScheme(.dark)
        // The home indicator sits where the thumbs rest: dimmed, a slide along the edge during
        // play does not leave the game on the first try.
        .persistentSystemOverlays(.hidden)
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

/// The game screen: the frame, the controls drawn over it, and the turn of the loop.
///
/// `CADisplayLink` is what calls `zeebx_ios_step`. It runs on the main thread, which is the same
/// as the library's and the settings' — the pointer does not cross threads.
///
/// What this screen does not do is decide: the size of the frame, which pieces show and what is
/// pressed on them, what the physical controller's buttons mean and what is written over the
/// game all come from the library, turn by turn. Here they are painted.
final class GameViewController: UIViewController {
    let model: ZeebxModel
    let keyboard = KeyboardMap.resolver()
    let image = UIImageView()
    let overlay = TouchOverlayView()
    let menuButton = UIButton(type: .system)
    let indicator = Badge()
    let debugLine = Badge()
    var displayLink: CADisplayLink?
    var finished = false

    /// What is already on screen, to redo only what changed.
    var frameSerial: UInt32?
    var overlayRevision: UInt32?
    var frameSize = CGSize.zero
    var smooth: Bool?
    var indicatorText = ""
    var debugText = ""

    /// The fingers on the glass, by the number the library knows each one by.
    var fingers: [UITouch: Int64] = [:]
    var nextFinger: Int64 = 0

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
        view.addSubview(image)

        overlay.frame = view.bounds
        overlay.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        overlay.onTouch = { [weak self] touch, phase in
            self?.finger(touch, phase)
        }
        view.addSubview(overlay)

        buildChrome()
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(leftTheScreen),
            name: UIApplication.willResignActiveNotification,
            object: nil
        )
        let displayLink = CADisplayLink(target: self, selector: #selector(tick))
        displayLink.add(to: .main, forMode: .common)
        self.displayLink = displayLink
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        becomeFirstResponder()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        let bounds = view.bounds
        let safe = bounds.inset(by: view.safeAreaInsets)
        zeebx_ios_layout(
            model.app,
            Float(bounds.width), Float(bounds.height),
            Float(safe.minX), Float(safe.minY), Float(safe.width), Float(safe.height),
            Float(traitCollection.displayScale)
        )
        placeFrame()
    }

    func stop() {
        displayLink?.invalidate()
        displayLink = nil
        NotificationCenter.default.removeObserver(self)
    }

    /// The menu button, and the two lines written over the game.
    ///
    /// **The button sits beside ZL, not in the corner.** The corners belong to the triggers, and
    /// the top middle to HOME and the two shortcuts beside it; between them is the only stretch
    /// of the top edge no piece takes out of the box.
    private func buildChrome() {
        let margin = view.safeAreaLayoutGuide
        var look = UIButton.Configuration.plain()
        look.image = UIImage(systemName: "line.3.horizontal")
        look.baseForegroundColor = UIColor(white: 1, alpha: 0.7)
        look.background.backgroundColor = UIColor(white: 0.1, alpha: 0.45)
        look.background.cornerRadius = 18
        menuButton.configuration = look
        menuButton.accessibilityLabel = model.tr("play.pause")
        menuButton.addTarget(self, action: #selector(openMenu), for: .touchUpInside)
        menuButton.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(menuButton)

        indicator.translatesAutoresizingMaskIntoConstraints = false
        debugLine.translatesAutoresizingMaskIntoConstraints = false
        debugLine.label.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        view.addSubview(indicator)
        view.addSubview(debugLine)

        NSLayoutConstraint.activate([
            menuButton.leadingAnchor.constraint(equalTo: margin.leadingAnchor, constant: 132),
            menuButton.topAnchor.constraint(equalTo: margin.topAnchor, constant: 10),
            menuButton.widthAnchor.constraint(equalToConstant: 36),
            menuButton.heightAnchor.constraint(equalToConstant: 36),
            indicator.leadingAnchor.constraint(equalTo: menuButton.trailingAnchor, constant: 10),
            indicator.centerYAnchor.constraint(equalTo: menuButton.centerYAnchor),
            debugLine.centerXAnchor.constraint(equalTo: margin.centerXAnchor),
            debugLine.bottomAnchor.constraint(equalTo: margin.bottomAnchor, constant: -4),
        ])
    }

    @objc func tick() {
        guard !finished else { return }
        // With the menu up the fingers and the controller belong to it; the turn still runs, to
        // keep the game still and the frame current after a load.
        ControllerReading.current().send(to: model.app)
        let code = zeebx_ios_step(model.app)
        // Stopped, or no session at all: either way there is no game left to show.
        if code == ZEEBX_STOPPED || code < 0 {
            finished = true
            stop()
            model.takeError()
            model.menuOpen = false
            model.playing = false
            return
        }

        var state = ZeebxView()
        zeebx_ios_view(model.app, &state)
        if frameSerial != state.frame_serial {
            frameSerial = state.frame_serial
            var width: UInt32 = 0
            var height: UInt32 = 0
            if let pointer = zeebx_ios_frame(model.app, &width, &height) {
                image.image = makeImage(bgra: pointer, width: Int(width), height: Int(height))
            }
        }
        let size = CGSize(width: CGFloat(state.frame_width), height: CGFloat(state.frame_height))
        if size != frameSize {
            frameSize = size
            placeFrame()
        }
        if smooth != (state.smooth != 0) {
            smooth = state.smooth != 0
            let filter: CALayerContentsFilter = state.smooth != 0 ? .linear : .nearest
            image.layer.magnificationFilter = filter
            image.layer.minificationFilter = filter
        }
        if overlayRevision != state.overlay_revision {
            overlayRevision = state.overlay_revision
            overlay.opacity = CGFloat(state.overlay_opacity) / 100
            overlay.pieces = Self.pieces(of: model.app)
        }
        write(zeebx_ios_indicator(model.app), to: indicator, known: &indicatorText)
        write(zeebx_ios_debug_line(model.app), to: debugLine, known: &debugText)
    }

    /// The pieces of the on-screen controls, as the library wants them drawn now.
    static func pieces(of app: OpaquePointer) -> [ZeebxElement] {
        var buffer = [ZeebxElement](repeating: ZeebxElement(), count: 16)
        let count = buffer.withUnsafeMutableBufferPointer { pointer in
            zeebx_ios_elements(app, pointer.baseAddress, Int32(pointer.count))
        }
        return Array(buffer.prefix(Int(max(count, 0))))
    }

    /// The frame, at the size the library fitted it, in the middle of the screen.
    private func placeFrame() {
        guard frameSize.width > 0, frameSize.height > 0 else { return }
        image.bounds = CGRect(origin: .zero, size: frameSize)
        image.center = CGPoint(x: view.bounds.midX, y: view.bounds.midY)
    }

    /// Updates a line over the game only when what it says changed: the turn runs sixty times a
    /// second, and the text changes a few times a minute.
    private func write(_ pointer: UnsafePointer<CChar>?, to badge: Badge, known: inout String) {
        let text = pointer.map { String(cString: $0) } ?? ""
        guard text != known else { return }
        known = text
        badge.label.text = text
        badge.isHidden = text.isEmpty
    }

    private func finger(_ touch: UITouch, _ phase: Int32) {
        let id: Int64
        if let known = fingers[touch] {
            id = known
        } else {
            nextFinger += 1
            id = nextFinger
            fingers[touch] = id
        }
        let point = touch.location(in: view)
        zeebx_ios_touch(model.app, id, phase, Float(point.x), Float(point.y))
        if phase == ZEEBX_TOUCH_UP {
            fingers[touch] = nil
        }
    }

    @objc func openMenu() {
        model.openMenu()
    }

    /// The app is leaving the screen — a call, the app switcher, the control center. A button
    /// still held will never be seen lifting, and the game would run on with nobody watching: the
    /// input is released and the menu goes up.
    @objc func leftTheScreen() {
        fingers.removeAll()
        zeebx_ios_release_input(model.app)
        model.openMenu()
    }

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var handled = false
        for press in presses {
            if press.key?.keyCode == .keyboardEscape {
                model.openMenu()
                handled = true
            } else if let index = keyboardIndex(press) {
                zeebx_ios_button(model.app, index, 1)
                handled = true
            }
        }
        if !handled {
            super.pressesBegan(presses, with: event)
        }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        for press in presses {
            if let index = keyboardIndex(press) {
                zeebx_ios_button(model.app, index, 0)
            }
        }
        super.pressesEnded(presses, with: event)
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        pressesEnded(presses, with: event)
    }

    /// The same map as the desktop window: arrows on the d-pad, Z/X/C/V on the four buttons,
    /// Q and W on the triggers, H/Backspace/Enter on HOME. Esc opens the menu.
    func keyboardIndex(_ press: UIPress) -> UInt32? {
        guard let key = press.key else { return nil }
        switch key.keyCode {
        case .keyboardUpArrow: return keyboard.up
        case .keyboardDownArrow: return keyboard.down
        case .keyboardLeftArrow: return keyboard.left
        case .keyboardRightArrow: return keyboard.right
        case .keyboardZ: return keyboard.b1
        case .keyboardX, .keyboardSpacebar: return keyboard.b2
        case .keyboardC: return keyboard.b3
        case .keyboardV: return keyboard.b4
        case .keyboardQ: return keyboard.zl
        case .keyboardW: return keyboard.zr
        case .keyboardH, .keyboardDeleteOrBackspace, .keyboardReturnOrEnter: return keyboard.back
        default: return nil
        }
    }
}

/// A short text on a dark pill, for what is written over the game. It never takes a touch: the
/// game is underneath.
final class Badge: UIView {
    let label = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        isHidden = true
        backgroundColor = UIColor(white: 0, alpha: 0.78)
        layer.cornerRadius = 6
        label.textColor = .white
        label.font = .boldSystemFont(ofSize: 14)
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            label.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            label.topAnchor.constraint(equalTo: topAnchor, constant: 5),
            label.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -5),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("the badge does not come from a storyboard")
    }
}
