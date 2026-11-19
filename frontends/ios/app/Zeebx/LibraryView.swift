import Combine
import SwiftUI

/// The game grid.
///
/// What finds the games, reads the titles and pulls the covers out of the `.mif` files is the
/// core's `library::scan` — the same one desktop and Android use. Here we only decide how that
/// fills the screen: a card per game, the cover on top and the name below to confirm it.
///
/// With a controller connected the grid has a cursor, as on Android: the D-pad or the stick moves
/// it and button A opens the game. Reaching a game without touching the glass is the normal path
/// for whoever holds a controller, not the alternative.
struct LibraryView: View {
    @ObservedObject var model: ZeebxModel
    @State private var search = ""
    @State private var showingSettings = false
    /// Which card the controller points at, among the ones the search let through.
    @State private var selected = 0
    /// The cursor only shows once a controller has moved it: on glass alone it would be a border
    /// around the first game for no reason.
    @State private var cursorVisible = false
    /// How many columns the grid drew last time. It is the D-pad's step up and down.
    @State private var columns = 4
    /// The directions and the confirm button as of the previous look, to see only what went down.
    @State private var held: UInt32 = 0

    private let pad = Timer.publish(every: 1.0 / 30.0, on: .main, in: .common).autoconnect()

    /// Width a card asks for, in points. The same as Android's.
    private static let cardWidth: CGFloat = 170

    private var filtered: [GameEntry] {
        let query = search.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return model.games }
        return model.games.filter {
            $0.title.range(of: query, options: [.caseInsensitive, .diacriticInsensitive]) != nil
        }
    }

    private var countText: String {
        let total = String(model.games.count)
        let shown = filtered.count
        if shown == model.games.count {
            return model.tr("library.count", ["count": total])
        }
        return model.tr("library.count_filtered", ["shown": String(shown), "count": total])
    }

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Zeebx")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .navigationBarLeading) {
                        Text(countText)
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                    ToolbarItemGroup(placement: .navigationBarTrailing) {
                        if model.scanning {
                            ProgressView()
                        }
                        Button {
                            model.scan()
                        } label: {
                            Image(systemName: "arrow.clockwise")
                        }
                        .disabled(model.scanning)
                        .accessibilityLabel(model.tr("library.rescan"))
                        Button {
                            showingSettings = true
                        } label: {
                            Image(systemName: "gearshape")
                        }
                        .accessibilityLabel(model.tr("nav.settings"))
                    }
                }
                .searchable(text: $search, prompt: Text(model.tr("nav.search")))
                .fullScreenCover(isPresented: $showingSettings) {
                    SettingsView(model: model)
                        .preferredColorScheme(.dark)
                }
        }
        .overlay {
            if model.opening {
                ProgressView(model.tr("ios.opening"))
                    .padding(24)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
            }
        }
        .fullScreenCover(isPresented: $model.playing) {
            GameScreen(model: model)
        }
        .alert("Zeebx", isPresented: visibleError) {
        } message: {
            Text(model.error ?? "")
        }
        .onChange(of: search) { _ in
            selected = 0
        }
        .onReceive(pad) { _ in
            navigate()
        }
    }

    @ViewBuilder private var content: some View {
        if model.games.isEmpty {
            emptyLibrary
        } else if filtered.isEmpty {
            Text(model.tr("library.no_match", ["query": search]))
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            grid
        }
    }

    /// The screen with no games: still looking, or nothing in the folder.
    private var emptyLibrary: some View {
        VStack(spacing: 12) {
            if model.scanning {
                ProgressView()
                Text(model.tr("library.scanning"))
                    .foregroundStyle(.secondary)
            } else {
                Text(model.tr("library.empty", ["folder": "Zeebx/roms"]))
                    .font(.title3)
                Text(model.tr("ios.library.hint"))
                    .multilineTextAlignment(.center)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 32)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var grid: some View {
        GeometryReader { proxy in
            // How many columns fit, never fewer than two: one card per row becomes a list, and
            // the grid stops being a grid.
            let count = max(2, Int(proxy.size.width / Self.cardWidth))
            let games = filtered
            ScrollViewReader { reader in
                ScrollView {
                    LazyVGrid(
                        columns: Array(repeating: GridItem(.flexible(), spacing: 12), count: count),
                        spacing: 12
                    ) {
                        ForEach(Array(games.enumerated()), id: \.element.id) { position, game in
                            Button {
                                model.open(game.id)
                            } label: {
                                GameCard(
                                    title: game.title,
                                    cover: model.cover(game),
                                    highlighted: cursorVisible && position == selected
                                )
                            }
                            .buttonStyle(.plain)
                            .disabled(model.opening)
                            .id(game.id)
                        }
                    }
                    .padding(12)
                }
                .onChange(of: selected) { position in
                    guard cursorVisible, games.indices.contains(position) else { return }
                    withAnimation(.easeOut(duration: 0.15)) {
                        reader.scrollTo(games[position].id, anchor: .center)
                    }
                }
            }
            .onAppear { columns = count }
            .onChange(of: count) { columns = $0 }
        }
    }

    private var visibleError: Binding<Bool> {
        Binding(
            get: { model.error != nil && !model.playing },
            set: { if !$0 { model.error = nil } }
        )
    }

    /// The controller walking the grid. Only while the grid is what is on screen: in a game the
    /// same buttons belong to the player.
    private func navigate() {
        guard !model.playing, !model.opening, !showingSettings else {
            held = 0
            return
        }
        let reading = ControllerReading.current()
        // The stick counts as a direction only when pushed most of the way: whoever picks a game
        // with it pushes it all the way, and a bump must not move the cursor.
        let threshold: Float = 0.6
        var now: UInt32 = 0
        if reading.holds(ZEEBX_PAD_DPAD_UP) || reading.leftY > threshold { now |= 1 }
        if reading.holds(ZEEBX_PAD_DPAD_DOWN) || reading.leftY < -threshold { now |= 2 }
        if reading.holds(ZEEBX_PAD_DPAD_LEFT) || reading.leftX < -threshold { now |= 4 }
        if reading.holds(ZEEBX_PAD_DPAD_RIGHT) || reading.leftX > threshold { now |= 8 }
        if reading.holds(ZEEBX_PAD_SOUTH) { now |= 16 }
        let pressed = now & ~held
        held = now
        let games = filtered
        guard pressed != 0, !games.isEmpty else { return }

        // The first press only shows where the cursor is.
        guard cursorVisible else {
            cursorVisible = true
            selected = min(selected, games.count - 1)
            return
        }
        var position = min(selected, games.count - 1)
        if pressed & 1 != 0 { position -= columns }
        if pressed & 2 != 0 { position += columns }
        if pressed & 4 != 0 { position -= 1 }
        if pressed & 8 != 0 { position += 1 }
        selected = min(max(position, 0), games.count - 1)
        if pressed & 16 != 0 {
            model.open(games[selected].id)
        }
    }
}

/// A card: the cover, or the initial in a frame when the game has none, and the title.
private struct GameCard: View {
    let title: String
    let cover: UIImage?
    let highlighted: Bool

    var body: some View {
        VStack(spacing: 8) {
            ZStack {
                if let cover {
                    // The covers are enlarged `.mif` icons: interpolating is what avoids the giant
                    // blocks. A `.mif` icon is wide and a real cover is tall, and both have to fit
                    // in the same box without stretching.
                    Image(uiImage: cover)
                        .resizable()
                        .interpolation(.medium)
                        .aspectRatio(contentMode: .fit)
                } else {
                    // No cover, the initial in a rectangle: the card stays the same size, and the
                    // grid gets no holes.
                    RoundedRectangle(cornerRadius: 8)
                        .fill(Color.black.opacity(0.35))
                    Text(String(title.prefix(1)).uppercased())
                        .font(.system(size: 34))
                        .foregroundStyle(.secondary)
                }
            }
            .frame(height: 104)
            .frame(maxWidth: .infinity)
            // A long title is cut with an ellipsis, not wrapped onto two lines: cards of different
            // heights on the same row would ruin the grid.
            Text(title)
                .font(.system(size: 15))
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .padding(8)
        .background(Color(.secondarySystemBackground), in: RoundedRectangle(cornerRadius: 12))
        .overlay {
            // The one the controller points at gets a border: with no finger on screen, it is the
            // only way to know which game the button will open.
            if highlighted {
                RoundedRectangle(cornerRadius: 12)
                    .stroke(Color.accentColor, lineWidth: 2.5)
            }
        }
        .contentShape(RoundedRectangle(cornerRadius: 12))
    }
}
