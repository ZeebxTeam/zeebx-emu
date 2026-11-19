import Combine
import SwiftUI
import UIKit

/// The settings screen as the library describes it: the sections and the rows of the one open.
struct SettingsPage: Decodable {
    struct Tab: Decodable, Identifiable {
        let id: String
        let title: String
    }

    let tab: String
    let tabs: [Tab]
    let rows: [SettingsRow]
}

/// One row. Which fields are present depends on `kind`; see `zeebx_ios_settings_json`.
struct SettingsRow: Decodable {
    let kind: String
    let id: String?
    let label: String?
    let hint: String?
    let enabled: Bool?
    let on: Bool?
    let options: [String]?
    let selected: Int?
    let value: Int?
    let min: Int?
    let max: Int?
    let format: String?
    let detail: String?
    let role: String?
    let warn: Bool?
}

/// Settings.
///
/// **This screen knows no option by name.** The library hands over a list of rows — a switch, a
/// choice, a slider, a row that does something — over the same settings file, with the same
/// labels, as desktop and Android. What is here is how iOS draws each kind of row, and nothing
/// about which rows exist: a new option is added on the other side and shows up here.
///
/// The sections sit in a rail on the left, as on Android: the device is locked to landscape, and
/// in landscape width is what there is to spare.
struct SettingsView: View {
    @ObservedObject var model: ZeebxModel
    @Environment(\.dismiss) private var dismiss
    @State private var tab = "general"
    @State private var page: SettingsPage?
    /// The library's revision this page was drawn from.
    @State private var drawn: UInt32 = 0
    /// Which row's hint is open. The help texts are long, and all of them open at once become a
    /// wall: only one at a time.
    @State private var openHint: String?
    @State private var editingLayout = false

    /// The controller is read here too: a row that waits for a button has to see it, and the
    /// answer comes back as a new revision.
    private let poll = Timer.publish(every: 1.0 / 20.0, on: .main, in: .common).autoconnect()

    var body: some View {
        HStack(spacing: 0) {
            rail
            Divider()
            rows
        }
        .background(Color(.systemBackground))
        .onAppear(perform: reload)
        .onReceive(poll) { _ in
            guard !editingLayout else { return }
            ControllerReading.current().send(to: model.app)
            if zeebx_ios_revision(model.app) != drawn {
                reload()
            }
        }
        .fullScreenCover(isPresented: $editingLayout, onDismiss: reload) {
            TouchEditorView(model: model)
                .preferredColorScheme(.dark)
        }
    }

    private var rail: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                Button {
                    dismiss()
                } label: {
                    Label(model.tr("common.back"), systemImage: "chevron.left")
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.vertical, 12)
                        .padding(.horizontal, 12)
                }
                ForEach(page?.tabs ?? []) { item in
                    Button {
                        tab = item.id
                        // Switching sections closes the open hint: it belonged to another screen.
                        openHint = nil
                        reload()
                    } label: {
                        Label(item.title, systemImage: Self.icon(item.id))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.vertical, 11)
                            .padding(.horizontal, 12)
                            .background(
                                item.id == tab ? Color.accentColor.opacity(0.25) : Color.clear,
                                in: RoundedRectangle(cornerRadius: 10)
                            )
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(8)
        }
        .frame(width: 210)
        .background(Color(.secondarySystemBackground))
    }

    private var rows: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Text(page?.tabs.first(where: { $0.id == tab })?.title ?? "")
                    .font(.title2.bold())
                    .padding(.bottom, 8)
                ForEach(Array((page?.rows ?? []).enumerated()), id: \.offset) { _, row in
                    view(of: row)
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// An icon recognized from afar is worth more than a name read up close.
    private static func icon(_ tab: String) -> String {
        switch tab {
        case "general": return "gearshape"
        case "graphics": return "display"
        case "speed": return "forward"
        case "audio": return "speaker.wave.2"
        case "controls": return "gamecontroller"
        case "debug": return "stopwatch"
        default: return "info.circle"
        }
    }

    @ViewBuilder private func view(of row: SettingsRow) -> some View {
        let enabled = row.enabled ?? true
        switch row.kind {
        case "section":
            Text(row.label ?? "")
                .font(.headline)
                .padding(.top, 22)
                .padding(.bottom, 4)
        case "note":
            Text(row.label ?? "")
                .font(.footnote)
                .foregroundStyle((row.warn ?? false) ? Color.orange : Color.secondary)
                .padding(.vertical, 6)
        case "toggle":
            Toggle(
                isOn: Binding(
                    get: { row.on ?? false },
                    set: { set(row, $0 ? 1 : 0) }
                )
            ) {
                title(of: row)
            }
            .disabled(!enabled)
            .padding(.vertical, 8)
            hint(of: row)
        case "choice":
            HStack {
                title(of: row)
                Spacer(minLength: 12)
                Picker(
                    row.label ?? "",
                    selection: Binding(
                        get: { row.selected ?? 0 },
                        set: { set(row, $0) }
                    )
                ) {
                    ForEach(Array((row.options ?? []).enumerated()), id: \.offset) { index, option in
                        Text(option).tag(index)
                    }
                }
                .pickerStyle(.menu)
                .labelsHidden()
            }
            .disabled(!enabled)
            .padding(.vertical, 6)
            hint(of: row)
        case "slider":
            SliderRow(row: row, title: title(of: row)) { set(row, $0) }
                // A value changed from elsewhere is a new slider: its thumb is state of its own.
                .id("\(row.id ?? "")-\(row.value ?? 0)")
                .disabled(!enabled)
                .padding(.vertical, 6)
            hint(of: row)
        case "action":
            Button {
                act(row)
            } label: {
                HStack {
                    Text(row.label ?? "")
                    Spacer(minLength: 12)
                    Text(row.role == "url" ? "" : (row.detail ?? ""))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    Image(systemName: row.role == "url" ? "arrow.up.right" : "chevron.right")
                        .font(.footnote)
                        .foregroundStyle(.tertiary)
                }
                .padding(.vertical, 12)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(!enabled)
            .opacity(enabled ? 1 : 0.4)
        default:
            EmptyView()
        }
        if row.kind != "section" {
            Divider()
        }
    }

    /// The label, with the `?` that opens the hint when the row has one.
    private func title(of row: SettingsRow) -> some View {
        HStack(spacing: 8) {
            Text(row.label ?? "")
            if row.hint != nil, let id = row.id {
                Button {
                    openHint = openHint == id ? nil : id
                } label: {
                    Image(systemName: openHint == id ? "questionmark.circle.fill" : "questionmark.circle")
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
            }
        }
    }

    @ViewBuilder private func hint(of row: SettingsRow) -> some View {
        if let hint = row.hint, let id = row.id, openHint == id {
            Text(hint)
                .font(.footnote)
                .foregroundStyle(.secondary)
                .padding(.bottom, 8)
        }
    }

    private func reload() {
        drawn = zeebx_ios_revision(model.app)
        page = takeJSON(SettingsPage.self, zeebx_ios_settings_json(model.app, tab))
        model.sync()
    }

    private func set(_ row: SettingsRow, _ value: Int) {
        guard let id = row.id else { return }
        zeebx_ios_settings_set(model.app, id, Int32(value))
        // "Scan again" is a row here, and the scan is watched from the model.
        model.watchScan()
        reload()
    }

    private func act(_ row: SettingsRow) {
        switch row.role {
        case "editor":
            editingLayout = true
        case "url":
            if let url = URL(string: row.detail ?? "") {
                UIApplication.shared.open(url)
            }
        default:
            set(row, 0)
        }
    }
}

/// A slider with its value beside it.
///
/// The thumb moves on state of its own and the setting is written when the finger lifts: a
/// change is saved to disk as it is made, and a drag is dozens of them.
private struct SliderRow<Title: View>: View {
    let row: SettingsRow
    let title: Title
    let commit: (Int) -> Void
    @State private var value: Double

    init(row: SettingsRow, title: Title, commit: @escaping (Int) -> Void) {
        self.row = row
        self.title = title
        self.commit = commit
        _value = State(initialValue: Double(row.value ?? 0))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                title
                Spacer(minLength: 12)
                Text((row.format ?? "{n}").replacingOccurrences(of: "{n}", with: String(Int(value))))
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
            Slider(
                value: $value,
                in: Double(row.min ?? 0)...Double(Swift.max(row.max ?? 100, (row.min ?? 0) + 1)),
                step: 1
            ) { dragging in
                if !dragging {
                    commit(Int(value))
                }
            }
        }
    }
}
