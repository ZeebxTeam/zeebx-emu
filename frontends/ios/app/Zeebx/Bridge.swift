import AVFoundation
import GameController
import SwiftUI
import UIKit

/// Copies a text the library handed over and gives the memory back.
func takeText(_ pointer: UnsafeMutablePointer<CChar>?) -> String? {
    guard let pointer else { return nil }
    defer { zeebx_ios_free(pointer) }
    return String(cString: pointer)
}

/// Reads one of the library's JSON answers.
func takeJSON<T: Decodable>(_ type: T.Type, _ pointer: UnsafeMutablePointer<CChar>?) -> T? {
    guard let text = takeText(pointer), let data = text.data(using: .utf8) else { return nil }
    return try? JSONDecoder().decode(type, from: data)
}

/// An image out of the library's BGRA8. The bytes are copied: the pointer only lives until the
/// next call into the library.
func makeImage(bgra pointer: UnsafePointer<UInt8>, width: Int, height: Int) -> UIImage? {
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

/// The physical controller at one instant, in the shape `zeebx_ios_controller` takes.
///
/// Which Zeebo button each of these presses is not decided here: the library reads the same
/// settings the remapping and the shortcuts are stored in. Swift only says what is held.
struct ControllerReading {
    var connected = false
    var buttons: UInt32 = 0
    var leftX: Float = 0
    var leftY: Float = 0
    var rightX: Float = 0
    var rightY: Float = 0

    static func current() -> ControllerReading {
        guard let pad = GCController.controllers().lazy.compactMap(\.extendedGamepad).first else {
            return ControllerReading()
        }
        var buttons: UInt32 = 0
        func hold(_ pressed: Bool, _ bit: Int32) {
            if pressed { buttons |= 1 << UInt32(bit) }
        }
        hold(pad.buttonA.isPressed, ZEEBX_PAD_SOUTH)
        hold(pad.buttonB.isPressed, ZEEBX_PAD_EAST)
        hold(pad.buttonX.isPressed, ZEEBX_PAD_WEST)
        hold(pad.buttonY.isPressed, ZEEBX_PAD_NORTH)
        hold(pad.leftShoulder.isPressed, ZEEBX_PAD_LEFT_TRIGGER)
        hold(pad.rightShoulder.isPressed, ZEEBX_PAD_RIGHT_TRIGGER)
        hold(pad.leftTrigger.isPressed, ZEEBX_PAD_LEFT_TRIGGER2)
        hold(pad.rightTrigger.isPressed, ZEEBX_PAD_RIGHT_TRIGGER2)
        hold(pad.leftThumbstickButton?.isPressed ?? false, ZEEBX_PAD_LEFT_THUMB)
        hold(pad.rightThumbstickButton?.isPressed ?? false, ZEEBX_PAD_RIGHT_THUMB)
        hold(pad.buttonMenu.isPressed, ZEEBX_PAD_START)
        hold(pad.buttonOptions?.isPressed ?? false, ZEEBX_PAD_SELECT)
        hold(pad.buttonHome?.isPressed ?? false, ZEEBX_PAD_MODE)
        hold(pad.dpad.up.isPressed, ZEEBX_PAD_DPAD_UP)
        hold(pad.dpad.down.isPressed, ZEEBX_PAD_DPAD_DOWN)
        hold(pad.dpad.left.isPressed, ZEEBX_PAD_DPAD_LEFT)
        hold(pad.dpad.right.isPressed, ZEEBX_PAD_DPAD_RIGHT)
        return ControllerReading(
            connected: true,
            buttons: buttons,
            leftX: pad.leftThumbstick.xAxis.value,
            leftY: pad.leftThumbstick.yAxis.value,
            rightX: pad.rightThumbstick.xAxis.value,
            rightY: pad.rightThumbstick.yAxis.value
        )
    }

    func send(to app: OpaquePointer) {
        zeebx_ios_controller(app, connected ? 1 : 0, buttons, leftX, leftY, rightX, rightY)
    }

    func holds(_ bit: Int32) -> Bool {
        buttons & (1 << UInt32(bit)) != 0
    }
}

/// A game of the library, as the grid shows it.
struct GameEntry: Identifiable {
    /// The position in the library's list: it is what `zeebx_ios_start` takes.
    let id: Int
    let title: String
    let hasCover: Bool
}

private struct LibraryPayload: Decodable {
    struct Game: Decodable {
        let title: String
        let cover: Bool
    }

    let scanning: Bool
    let games: [Game]
}

/// The state that the screens share.
///
/// The pointer is what `zeebx_ios_create` returned. It lives as long as the process: the library,
/// the settings and the game are the same app on the other side, and destroying it in the middle
/// of a screen switch would leave the others with a dead pointer.
final class ZeebxModel: ObservableObject {
    let app: OpaquePointer
    @Published var games: [GameEntry] = []
    @Published var scanning = false
    @Published var playing = false
    @Published var opening = false
    @Published var menuOpen = false
    @Published var error: String?

    /// The interface texts already asked for. Emptied when the library's revision moves, which
    /// is what a change of language does.
    private var texts: [String: String] = [:]
    private var covers: [Int: UIImage] = [:]
    private var revision: UInt32 = 0
    private var scanTimer: Timer?

    init() {
        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
        // The device's language, in the `pt-BR` shape the catalog matches against.
        let language = Locale.preferredLanguages.first ?? "en"
        let created = zeebx_ios_create(documents.path, support.path, language)
        guard let created else {
            fatalError("zeebx_ios_create returned null with sandbox folders")
        }
        app = created
        revision = zeebx_ios_revision(app)
        watchScan()
    }

    deinit {
        scanTimer?.invalidate()
        zeebx_ios_destroy(app)
    }

    /// An interface text, from the same catalog as desktop and Android.
    func tr(_ key: String) -> String {
        if let known = texts[key] { return known }
        let text = takeText(zeebx_ios_tr(app, key)) ?? key
        texts[key] = text
        return text
    }

    /// A text with its `{name}`s filled in.
    func tr(_ key: String, _ values: [String: String]) -> String {
        var text = tr(key)
        for (name, value) in values {
            text = text.replacingOccurrences(of: "{\(name)}", with: value)
        }
        return text
    }

    /// Notices that something changed on the other side. Every screen that changes a setting
    /// calls this afterwards; the texts are asked for again, in case the language was it.
    func sync() {
        let now = zeebx_ios_revision(app)
        guard now != revision else { return }
        revision = now
        texts.removeAll()
        objectWillChange.send()
    }

    func scan() {
        zeebx_ios_scan(app)
        watchScan()
    }

    /// Reads the list and, while a scan is running, keeps asking whether it finished. The scan
    /// itself is on a thread of the library's; this only looks.
    func watchScan() {
        loadLibrary()
        guard scanning, scanTimer == nil else { return }
        scanTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            guard let self else { return }
            if zeebx_ios_scan_poll(self.app) != 0 {
                self.loadLibrary()
            }
            if !self.scanning {
                self.scanTimer?.invalidate()
                self.scanTimer = nil
            }
        }
    }

    private func loadLibrary() {
        guard let payload = takeJSON(LibraryPayload.self, zeebx_ios_library_json(app)) else { return }
        covers.removeAll()
        games = payload.games.enumerated().map { index, game in
            GameEntry(id: index, title: game.title, hasCover: game.cover)
        }
        scanning = payload.scanning
        sync()
    }

    /// The cover of a game, converted once per scan.
    func cover(_ game: GameEntry) -> UIImage? {
        guard game.hasCover else { return nil }
        if let known = covers[game.id] { return known }
        var width: UInt32 = 0
        var height: UInt32 = 0
        guard let pointer = zeebx_ios_cover(app, Int32(game.id), &width, &height),
              let image = makeImage(bgra: pointer, width: Int(width), height: Int(height))
        else { return nil }
        covers[game.id] = image
        return image
    }

    func open(_ index: Int) {
        guard !opening, !playing else { return }
        opening = true
        // Opening the module blocks the UI thread. This hop lets the indicator appear first; the
        // session itself stays on the main thread, which is the only one that touches the
        // pointer.
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let audio = AVAudioSession.sharedInstance()
            try? audio.setCategory(.playback, mode: .default)
            try? audio.setActive(true)
            if zeebx_ios_start(self.app, Int32(index)) == 0 {
                self.error = nil
                self.menuOpen = false
                self.playing = true
            } else {
                self.takeError()
            }
            self.opening = false
        }
    }

    /// Moves the library's last failure, if there is one, to where the alert reads it.
    func takeError() {
        let message = zeebx_ios_error(app).map { String(cString: $0) } ?? ""
        error = message.isEmpty ? nil : message
    }

    /// The pause menu goes up: the game stops and touch goes to the menu.
    func openMenu() {
        guard playing, !menuOpen else { return }
        zeebx_ios_set_menu(app, 1)
        menuOpen = true
    }

    /// Back to the game, paused or not.
    func closeMenu(paused: Bool) {
        zeebx_ios_set_paused(app, paused ? 1 : 0)
        zeebx_ios_set_menu(app, 0)
        menuOpen = false
    }

    func restart() {
        if zeebx_ios_restart(app) == 0 {
            menuOpen = false
        } else {
            // The game that was open is closed and the new one did not come up: there is nothing
            // left to show but the reason.
            takeError()
            menuOpen = false
            playing = false
        }
    }

    func close() {
        zeebx_ios_stop(app)
        menuOpen = false
        playing = false
    }
}
