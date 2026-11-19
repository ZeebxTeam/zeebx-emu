import SwiftUI
import UIKit

private struct SlotsPayload: Decodable {
    struct Slot: Decodable {
        let used: Bool
        let size: String
        let thumbnail: Bool
    }

    let available: Bool
    let paused: Bool
    let slots: [Slot]
}

/// The pause menu: back to the game, pause, restart, stop, and the save state slots.
///
/// While it is up the game is still and silent — the library sees to that — and the curtain
/// behind it eats the touch: a finger outside the panel must not land on the game.
struct PauseMenuView: View {
    @ObservedObject var model: ZeebxModel
    @State private var payload: SlotsPayload?
    @State private var thumbnails: [Int: UIImage] = [:]
    /// An occupied slot that asked for a second tap before being replaced.
    @State private var overwrite: Int?
    /// Result of the last operation, shown without closing the menu.
    @State private var message: String?

    private var available: Bool { payload?.available ?? false }
    private var paused: Bool { payload?.paused ?? false }

    var body: some View {
        ZStack {
            Color.black.opacity(0.7)
                .ignoresSafeArea()
            VStack(alignment: .leading, spacing: 10) {
                Text(model.tr("play.state.title"))
                    .font(.headline)
                if payload != nil, !available {
                    Text(model.tr("play.state.unavailable"))
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                if let message {
                    Text(message)
                        .font(.footnote)
                }
                ScrollView {
                    VStack(spacing: 6) {
                        ForEach(Array((payload?.slots ?? []).enumerated()), id: \.offset) { index, slot in
                            row(index, slot)
                        }
                    }
                }
                Divider()
                HStack(spacing: 10) {
                    // "Back to the game" comes first: it is what someone who opened the menu by
                    // accident will want to press.
                    Button(model.tr("play.continue")) {
                        model.closeMenu(paused: false)
                    }
                    .buttonStyle(.borderedProminent)
                    Button(paused ? model.tr("play.resume") : model.tr("play.pause")) {
                        model.closeMenu(paused: !paused)
                    }
                    Button(model.tr("play.restart")) {
                        model.restart()
                    }
                    Spacer()
                    Button(model.tr("play.stop"), role: .destructive) {
                        model.close()
                    }
                }
                .buttonStyle(.bordered)
                Text(model.tr("play.close.warning"))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .padding(16)
            .frame(maxWidth: 640)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16))
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
        }
        .onAppear(perform: reload)
    }

    private func row(_ index: Int, _ slot: SlotsPayload.Slot) -> some View {
        HStack(spacing: 12) {
            ZStack {
                RoundedRectangle(cornerRadius: 4)
                    .fill(Color.black.opacity(0.5))
                if let thumbnail = thumbnails[index] {
                    Image(uiImage: thumbnail)
                        .resizable()
                        .interpolation(.medium)
                        .aspectRatio(contentMode: .fit)
                }
            }
            .frame(width: 80, height: 60)
            VStack(alignment: .leading, spacing: 2) {
                Text(model.tr("play.state.slot", ["slot": String(index + 1)]))
                    .bold()
                Text(slot.used ? model.tr("play.state.used", ["size": slot.size]) : model.tr("play.state.empty"))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                if overwrite == index {
                    Text(model.tr("play.state.overwrite.hint"))
                        .font(.caption)
                }
            }
            Spacer(minLength: 8)
            Button(saveLabel(index, slot)) {
                save(index, slot)
            }
            .disabled(!available)
            Button(model.tr("play.state.load")) {
                load(index)
            }
            .disabled(!available || !slot.used)
        }
        .buttonStyle(.bordered)
        .padding(8)
        .background(Color.white.opacity(0.06), in: RoundedRectangle(cornerRadius: 10))
    }

    private func saveLabel(_ index: Int, _ slot: SlotsPayload.Slot) -> String {
        if overwrite == index { return model.tr("play.state.confirm") }
        return slot.used ? model.tr("play.state.overwrite") : model.tr("play.state.save")
    }

    private func reload() {
        payload = takeJSON(SlotsPayload.self, zeebx_ios_slots_json(model.app))
        var pictures: [Int: UIImage] = [:]
        for (index, slot) in (payload?.slots ?? []).enumerated() where slot.thumbnail {
            var width: UInt32 = 0
            var height: UInt32 = 0
            if let pointer = zeebx_ios_slot_thumbnail(model.app, Int32(index), &width, &height) {
                pictures[index] = makeImage(bgra: pointer, width: Int(width), height: Int(height))
            }
        }
        thumbnails = pictures
    }

    private func save(_ index: Int, _ slot: SlotsPayload.Slot) {
        // An occupied slot asks twice: a save state is the one thing here that cannot be redone.
        if slot.used, overwrite != index {
            overwrite = index
            return
        }
        overwrite = nil
        if zeebx_ios_save_state(model.app, Int32(index)) == 0 {
            message = model.tr("play.state.saved", ["slot": String(index + 1)])
        } else {
            message = failure()
        }
        reload()
    }

    private func load(_ index: Int) {
        overwrite = nil
        if zeebx_ios_load_state(model.app, Int32(index)) == 0 {
            // The library already took the menu down and set the game running.
            model.menuOpen = false
        } else {
            message = failure()
        }
    }

    private func failure() -> String {
        zeebx_ios_error(model.app).map { String(cString: $0) } ?? ""
    }
}
