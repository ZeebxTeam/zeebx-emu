import SwiftUI

@main
struct ZeebxApp: App {
    @StateObject private var modelo = ZeebxModelo()

    var body: some Scene {
        WindowGroup {
            BibliotecaView(modelo: modelo)
                .preferredColorScheme(.dark)
        }
    }
}
