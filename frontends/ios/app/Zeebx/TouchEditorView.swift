import SwiftUI
import UIKit

/// The screen for moving and resizing the on-screen controls.
///
/// Dragging a piece moves it; touching it selects it, and the bar in the middle grows and
/// shrinks the selected one. The library saves on release and at every size step.
///
/// The bar sits in the middle because it is the only place no piece takes out of the box — at
/// the top it would cover HOME, and HOME could not be grabbed.
struct TouchEditorView: View {
    @ObservedObject var model: ZeebxModel
    @Environment(\.dismiss) private var dismiss
    /// What the size bar says, like "D-pad  120%". Nil with nothing selected.
    @State private var selection: String?
    /// Bumped to make the canvas ask the library for the pieces again.
    @State private var redraw = 0

    var body: some View {
        ZStack {
            EditorCanvas(model: model, selection: $selection, redraw: redraw)
                .ignoresSafeArea()
            VStack(spacing: 12) {
                Text(model.tr("touch.editor.hint"))
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .allowsHitTesting(false)
                if let selection {
                    HStack(spacing: 16) {
                        Button {
                            resize(-1)
                        } label: {
                            Image(systemName: "minus").frame(width: 28, height: 28)
                        }
                        Text(selection)
                            .bold()
                            .monospacedDigit()
                        Button {
                            resize(1)
                        } label: {
                            Image(systemName: "plus").frame(width: 28, height: 28)
                        }
                    }
                    .buttonStyle(.bordered)
                }
                HStack(spacing: 12) {
                    Button(model.tr("touch.editor.reset")) {
                        zeebx_ios_editor_reset(model.app)
                        redraw += 1
                    }
                    .buttonStyle(.bordered)
                    Button(model.tr("touch.editor.done")) {
                        dismiss()
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
        }
        .persistentSystemOverlays(.hidden)
        .onAppear {
            zeebx_ios_editor(model.app, 1)
            redraw += 1
        }
        .onDisappear {
            zeebx_ios_editor(model.app, 0)
            model.sync()
        }
    }

    private func resize(_ steps: Int32) {
        zeebx_ios_editor_resize(model.app, steps)
        redraw += 1
    }
}

private struct EditorCanvas: UIViewRepresentable {
    @ObservedObject var model: ZeebxModel
    @Binding var selection: String?
    let redraw: Int

    func makeUIView(context: Context) -> EditorCanvasView {
        let view = EditorCanvasView(app: model.app)
        view.onSelection = { text in
            // The canvas reports from inside a layout pass or a touch: the state changes on the
            // next turn of the run loop, not in the middle of SwiftUI's update.
            DispatchQueue.main.async {
                if selection != text { selection = text }
            }
        }
        return view
    }

    func updateUIView(_ view: EditorCanvasView, context: Context) {
        view.refresh()
    }
}

/// The pieces, drawn by the same view as in the game, and the one finger that drags them.
final class EditorCanvasView: UIView {
    let app: OpaquePointer
    private let overlay = TouchOverlayView()
    private let frameOutline = UIView()
    var onSelection: ((String?) -> Void)?

    init(app: OpaquePointer) {
        self.app = app
        super.init(frame: .zero)
        backgroundColor = UIColor(red: 16 / 255, green: 16 / 255, blue: 20 / 255, alpha: 1)
        // The outline of the game frame, 4:3 and centered, as it appears fitted to the screen:
        // it is the reference for how much each piece will cover.
        frameOutline.backgroundColor = UIColor(red: 34 / 255, green: 38 / 255, blue: 48 / 255, alpha: 1)
        frameOutline.isUserInteractionEnabled = false
        addSubview(frameOutline)

        overlay.isMultipleTouchEnabled = false
        overlay.onTouch = { [weak self] touch, phase in
            guard let self else { return }
            let point = touch.location(in: self)
            zeebx_ios_editor_touch(self.app, phase, Float(point.x), Float(point.y))
            self.refresh()
        }
        addSubview(overlay)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("the editor does not come from a storyboard")
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        overlay.frame = bounds
        let height = bounds.height
        let width = min(bounds.width, height * 4 / 3)
        frameOutline.frame = CGRect(x: bounds.midX - width / 2, y: 0, width: width, height: height)
        let safe = bounds.inset(by: safeAreaInsets)
        zeebx_ios_layout(
            app,
            Float(bounds.width), Float(bounds.height),
            Float(safe.minX), Float(safe.minY), Float(safe.width), Float(safe.height),
            Float(traitCollection.displayScale)
        )
        refresh()
    }

    override func safeAreaInsetsDidChange() {
        super.safeAreaInsetsDidChange()
        setNeedsLayout()
    }

    /// Asks the library what the pieces look like now, and which one is selected.
    func refresh() {
        var state = ZeebxView()
        zeebx_ios_view(app, &state)
        overlay.opacity = CGFloat(state.overlay_opacity) / 100
        overlay.pieces = GameViewController.pieces(of: app)
        onSelection?(takeText(zeebx_ios_editor_selection(app)))
    }
}
