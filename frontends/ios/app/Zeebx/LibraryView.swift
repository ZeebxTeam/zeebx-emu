import AVFoundation
import SwiftUI

/// The state that the list and the game share.
///
/// The pointer is what `zeebx_ios_create` returned. It lives as long as the process: the list and
/// the frame are the same session, and destroying it in the middle of a screen switch would leave
/// the other one with a dead pointer.
final class ZeebxModel: ObservableObject {
    let app: OpaquePointer
    @Published var titles: [String] = []
    @Published var playing = false
    @Published var opening = false
    @Published var error: String?

    init() {
        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
        let created = documents.path.withCString { docs in
            support.path.withCString { support in
                zeebx_ios_create(docs, support)
            }
        }
        guard let created else {
            fatalError("zeebx_ios_create returned null with sandbox folders")
        }
        app = created
        scan()
    }

    deinit {
        zeebx_ios_destroy(app)
    }

    func scan() {
        let count = zeebx_ios_scan(app)
        titles = (0..<count).compactMap { index in
            guard let pointer = zeebx_ios_title(app, index) else { return nil }
            return String(cString: pointer)
        }
    }

    func open(_ index: Int) {
        opening = true
        // Opening the module blocks the UI thread. This hop lets the indicator appear
        // first; the session itself stays on the main thread, which is the only one that
        // touches the pointer.
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let audio = AVAudioSession.sharedInstance()
            try? audio.setCategory(.playback, mode: .default)
            try? audio.setActive(true)
            if zeebx_ios_start(self.app, Int32(index)) == 0 {
                self.error = nil
                self.playing = true
            } else if let pointer = zeebx_ios_error(self.app) {
                self.error = String(cString: pointer)
            }
            self.opening = false
        }
    }

    func close() {
        zeebx_ios_stop(app)
        playing = false
    }
}

struct LibraryView: View {
    @ObservedObject var model: ZeebxModel

    var body: some View {
        NavigationStack {
            Group {
                if model.titles.isEmpty {
                    VStack(spacing: 12) {
                        Text("No games in Documents/roms")
                            .font(.title2)
                        Text("Using the Files app, or Finder with the device connected, put the .mod or .zip files in that folder and come back here.")
                            .multilineTextAlignment(.center)
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 32)
                    }
                } else {
                    List {
                        ForEach(Array(model.titles.enumerated()), id: \.offset) { index, title in
                            Button(title) {
                                model.open(index)
                            }
                            .disabled(model.opening)
                        }
                    }
                }
            }
            .navigationTitle("Zeebx")
            .toolbar {
                Button("Refresh") { model.scan() }
            }
            .overlay {
                if model.opening {
                    ProgressView("Opening")
                        .padding(24)
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
                }
            }
        }
        .fullScreenCover(isPresented: $model.playing) {
            GameView(model: model)
        }
        .alert("Could not open it", isPresented: visibleError) {

            Button("OK", role: .cancel) { model.error = nil }
        } message: {
            Text(model.error ?? "")
        }
    }

    private var visibleError: Binding<Bool> {
        Binding(
            get: { model.error != nil && !model.playing },
            set: { if !$0 { model.error = nil } }
        )
    }
}
