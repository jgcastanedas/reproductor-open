import AVFoundation
import SwiftUI

@main
struct ReproductorApp: App {
    @StateObject private var model = PlayerModel()

    init() {
        #if os(iOS)
        // Reproducción de video larga: habilita AirPlay en segundo plano y la pantalla bloqueada.
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback, policy: .longFormVideo)
        #endif
    }

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(model)
                .onOpenURL { model.open($0) }
                .preferredColorScheme(.dark)
                .tint(Theme.accent)
        }
        #if os(macOS)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Abrir video…") { model.showImporter = true }
                    .keyboardShortcut("o")
            }
        }
        #endif
    }
}
