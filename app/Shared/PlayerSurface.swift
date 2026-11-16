import AVKit
import SwiftUI

#if os(macOS)
/// Reproductor nativo de macOS: controles flotantes, pantalla completa, PiP,
/// atajos de teclado (espacio, ←/→) y su propio botón de AirPlay para video.
struct PlayerSurface: NSViewRepresentable {
    let player: AVPlayer

    func makeNSView(context: Context) -> AVPlayerView {
        let view = AVPlayerView()
        view.player = player
        view.controlsStyle = .floating
        view.showsFullScreenToggleButton = true
        view.allowsPictureInPicturePlayback = true
        view.showsSharingServiceButton = false
        view.videoGravity = .resizeAspect
        return view
    }

    func updateNSView(_ view: AVPlayerView, context: Context) {
        if view.player !== player { view.player = player }
    }
}
/// Botón de AirPlay de la barra de herramientas, atado al reproductor de video
/// (sin `player`, macOS enviaría solo el audio de la app).
struct MacRoutePicker: NSViewRepresentable {
    let player: AVPlayer

    func makeNSView(context: Context) -> AVRoutePickerView {
        let view = AVRoutePickerView()
        view.player = player
        view.isRoutePickerButtonBordered = false
        view.setRoutePickerButtonColor(NSColor(Theme.sky), for: .normal)
        view.setRoutePickerButtonColor(NSColor(Theme.accent), for: .active)
        return view
    }

    func updateNSView(_ view: AVRoutePickerView, context: Context) {
        if view.player !== player { view.player = player }
    }
}
#else
struct PlayerSurface: View {
    let player: AVPlayer

    var body: some View {
        VideoPlayer(player: player)
    }
}

/// Botón del selector de AirPlay en iOS (en macOS lo trae AVPlayerView).
struct RoutePicker: UIViewRepresentable {
    func makeUIView(context: Context) -> AVRoutePickerView {
        let view = AVRoutePickerView()
        view.prioritizesVideoDevices = true
        view.tintColor = UIColor(Theme.sky)
        view.activeTintColor = UIColor(Theme.accent)
        return view
    }

    func updateUIView(_ view: AVRoutePickerView, context: Context) {}
}
#endif
