import AVFoundation
import SwiftUI

@main
struct ReproductorApp: App {
    @StateObject private var library: Library
    @StateObject private var model: PlayerModel

    init() {
        let library = Library()
        _library = StateObject(wrappedValue: library)
        _model = StateObject(wrappedValue: PlayerModel(library: library))
        #if os(iOS)
        // Reproducción de video larga: habilita AirPlay en segundo plano y la pantalla bloqueada.
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback, policy: .longFormVideo)
        #endif
    }

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(model)
                .environmentObject(library)
                .onOpenURL { model.open($0) }
                .preferredColorScheme(.dark)
                .tint(Theme.accent)
        }
        #if os(macOS)
        .defaultSize(width: 1280, height: 760)
        .windowToolbarStyle(.unified)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Abrir video…") { model.showImporter = true }
                    .keyboardShortcut("o")
            }
        }
        #endif
    }
}
