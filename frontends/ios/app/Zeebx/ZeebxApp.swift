import SwiftUI

@main
struct ZeebxApp: App {
    @StateObject private var model = ZeebxModel()

    var body: some Scene {
        WindowGroup {
            LibraryView(model: model)
                .preferredColorScheme(.dark)
        }
    }
}
