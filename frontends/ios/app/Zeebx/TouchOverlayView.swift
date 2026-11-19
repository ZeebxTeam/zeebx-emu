import UIKit

/// The controls drawn on screen: the pieces, and the fingers on them.
///
/// Where each piece sits, what is pressed and where a stick's knob is all come from the library,
/// already decided — the same computation Android uses. This view paints what it is told and
/// reports every finger; it does not know which button is which.
///
/// The drawing follows Android's piece by piece, so the two look like the same controller.
final class TouchOverlayView: UIView {
    /// The pieces as they are to be drawn now.
    var pieces: [ZeebxElement] = [] {
        didSet { setNeedsDisplay() }
    }

    /// From 0 to 1.
    var opacity: CGFloat = 0.55 {
        didSet { setNeedsDisplay() }
    }

    /// A finger touched down, moved or lifted. The phase is one of `ZEEBX_TOUCH_*`.
    var onTouch: ((UITouch, Int32) -> Void)?

    override init(frame: CGRect) {
        super.init(frame: frame)
        isOpaque = false
        backgroundColor = .clear
        // Playing needs two fingers at once: one thumb holding the D-pad and the other pressing 1.
        isMultipleTouchEnabled = true
        contentMode = .redraw
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("the overlay does not come from a storyboard")
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches { onTouch?(touch, ZEEBX_TOUCH_DOWN) }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches { onTouch?(touch, ZEEBX_TOUCH_MOVED) }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches { onTouch?(touch, ZEEBX_TOUCH_UP) }
    }

    /// A cancelled touch — the system took it for a gesture of its own — is a finger that lifted:
    /// otherwise its button would stay down with nobody holding it.
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches { onTouch?(touch, ZEEBX_TOUCH_UP) }
    }

    override func draw(_ rect: CGRect) {
        guard let context = UIGraphicsGetCurrentContext(), !pieces.isEmpty else { return }
        let alpha = min(max(opacity, 0), 1)
        let colors = Colors(
            background: UIColor(red: 20 / 255, green: 20 / 255, blue: 24 / 255, alpha: alpha * 0.75),
            lit: UIColor(red: 0, green: 110 / 255, blue: 190 / 255, alpha: alpha),
            ink: UIColor(white: 1, alpha: alpha),
            idleKnob: UIColor(red: 200 / 255, green: 200 / 255, blue: 210 / 255, alpha: alpha / 2),
            highlight: UIColor(red: 1, green: 200 / 255, blue: 0, alpha: 1)
        )
        for piece in pieces {
            switch piece.kind {
            case ZEEBX_PIECE_LEFT_STICK, ZEEBX_PIECE_RIGHT_STICK:
                drawStick(piece, colors, in: context)
            case ZEEBX_PIECE_DPAD:
                drawDpad(piece, colors, in: context)
            default:
                drawButton(piece, colors, in: context)
            }
        }
    }

    private struct Colors {
        let background: UIColor
        let lit: UIColor
        let ink: UIColor
        let idleKnob: UIColor
        let highlight: UIColor
    }

    /// The outline of a piece: the editor's selection is thicker and yellow.
    private func setOutline(_ piece: ZeebxElement, _ colors: Colors, in context: CGContext) {
        let selected = piece.highlighted != 0
        context.setStrokeColor((selected ? colors.highlight : colors.ink).cgColor)
        context.setLineWidth(selected ? 3 : 2)
    }

    /// The base and the knob. Pushed against the edge, the knob still sits entirely inside the
    /// base: the maximum offset is the base's radius minus its own.
    private func drawStick(_ piece: ZeebxElement, _ colors: Colors, in context: CGContext) {
        let center = CGPoint(x: CGFloat(piece.center_x), y: CGFloat(piece.center_y))
        let radius = CGFloat(piece.half_width)
        let base = CGRect(x: center.x - radius, y: center.y - radius, width: radius * 2, height: radius * 2)
        context.setFillColor(colors.background.cgColor)
        context.fillEllipse(in: base)
        setOutline(piece, colors, in: context)
        context.strokeEllipse(in: base.insetBy(dx: 1, dy: 1))

        let knob = radius * 0.45
        let inUse = piece.knob_x != 0 || piece.knob_y != 0
        let at = CGPoint(
            x: center.x + CGFloat(piece.knob_x) * (radius - knob),
            y: center.y + CGFloat(piece.knob_y) * (radius - knob)
        )
        let circle = CGRect(x: at.x - knob, y: at.y - knob, width: knob * 2, height: knob * 2)
        context.setFillColor((inUse ? colors.lit : colors.idleKnob).cgColor)
        context.fillEllipse(in: circle)
        context.setStrokeColor(colors.ink.cgColor)
        context.setLineWidth(2)
        context.strokeEllipse(in: circle)
    }

    /// A single cross, like the controller's, with each arm lit on its own: the diagonals light
    /// two. The center and the arms do not overlap — with a see-through color, a crossing would
    /// come out darker than the rest.
    private func drawDpad(_ piece: ZeebxElement, _ colors: Colors, in context: CGContext) {
        let center = CGPoint(x: CGFloat(piece.center_x), y: CGFloat(piece.center_y))
        let half = CGFloat(piece.half_width)
        let arm = half * 0.34
        let tip = half * 0.95
        func box(_ x0: CGFloat, _ y0: CGFloat, _ x1: CGFloat, _ y1: CGFloat) -> CGRect {
            CGRect(x: center.x + x0, y: center.y + y0, width: x1 - x0, height: y1 - y0)
        }
        func fill(_ rect: CGRect, lit: Bool) {
            context.setFillColor((lit ? colors.lit : colors.background).cgColor)
            context.fill(rect)
        }
        // The arms in the order of the library's bits: up, down, left, right.
        fill(box(-arm, -arm, arm, arm), lit: false)
        fill(box(-arm, -tip, arm, -arm), lit: piece.pressed & 1 != 0)
        fill(box(-arm, arm, arm, tip), lit: piece.pressed & 2 != 0)
        fill(box(-tip, -arm, -arm, arm), lit: piece.pressed & 4 != 0)
        fill(box(arm, -arm, tip, arm), lit: piece.pressed & 8 != 0)

        // The outline is the whole cross's, in a single line.
        let corners: [(CGFloat, CGFloat)] = [
            (-arm, -tip), (arm, -tip), (arm, -arm), (tip, -arm), (tip, arm), (arm, arm),
            (arm, tip), (-arm, tip), (-arm, arm), (-tip, arm), (-tip, -arm), (-arm, -arm),
        ]
        context.beginPath()
        for (index, corner) in corners.enumerated() {
            let point = CGPoint(x: center.x + corner.0, y: center.y + corner.1)
            if index == 0 {
                context.move(to: point)
            } else {
                context.addLine(to: point)
            }
        }
        context.closePath()
        setOutline(piece, colors, in: context)
        context.strokePath()

        // The arrows at the tips, so the cross reads as a D-pad and not as a "+".
        context.setFillColor(colors.ink.cgColor)
        let directions: [(CGFloat, CGFloat)] = [(0, -1), (0, 1), (-1, 0), (1, 0)]
        for (dx, dy) in directions {
            let apex = CGPoint(x: center.x + dx * tip * 0.82, y: center.y + dy * tip * 0.82)
            let base = CGPoint(x: center.x + dx * tip * 0.55, y: center.y + dy * tip * 0.55)
            let side = CGPoint(x: -dy * arm * 0.55, y: dx * arm * 0.55)
            context.beginPath()
            context.move(to: apex)
            context.addLine(to: CGPoint(x: base.x + side.x, y: base.y + side.y))
            context.addLine(to: CGPoint(x: base.x - side.x, y: base.y - side.y))
            context.closePath()
            context.fillPath()
        }
    }

    /// A face button, a trigger, HOME or a shortcut: a circle or a pill, with what the Zeebo
    /// controller has printed on it.
    private func drawButton(_ piece: ZeebxElement, _ colors: Colors, in context: CGContext) {
        let halfWidth = CGFloat(piece.half_width)
        let halfHeight = CGFloat(piece.half_height)
        let frame = CGRect(
            x: CGFloat(piece.center_x) - halfWidth,
            y: CGFloat(piece.center_y) - halfHeight,
            width: halfWidth * 2,
            height: halfHeight * 2
        )
        // The outline is drawn inside the piece, so a pressed neighbour does not eat half of it.
        let outline = frame.insetBy(dx: 1, dy: 1)
        let shape = piece.round != 0
            ? UIBezierPath(ovalIn: frame)
            : UIBezierPath(roundedRect: frame, cornerRadius: halfHeight)
        let edge = piece.round != 0
            ? UIBezierPath(ovalIn: outline)
            : UIBezierPath(roundedRect: outline, cornerRadius: halfHeight - 1)
        context.setFillColor((piece.pressed != 0 ? colors.lit : colors.background).cgColor)
        context.addPath(shape.cgPath)
        context.fillPath()
        setOutline(piece, colors, in: context)
        context.addPath(edge.cgPath)
        context.strokePath()

        guard let pointer = zeebx_ios_element_label(piece.kind) else { return }
        let label = String(cString: pointer) as NSString
        // HOME is a long word on a short piece, and gets a smaller letter than the rest.
        let size = halfHeight * (label.isEqual(to: "HOME") ? 0.75 : 0.85)
        let attributes: [NSAttributedString.Key: Any] = [
            .font: UIFont.systemFont(ofSize: size, weight: .semibold),
            .foregroundColor: colors.ink,
        ]
        let bounds = label.size(withAttributes: attributes)
        label.draw(
            at: CGPoint(
                x: CGFloat(piece.center_x) - bounds.width / 2,
                y: CGFloat(piece.center_y) - bounds.height / 2
            ),
            withAttributes: attributes
        )
    }
}
