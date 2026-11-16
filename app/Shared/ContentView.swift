import AVKit
import SwiftUI
import UniformTypeIdentifiers

struct ContentView: View {
    @EnvironmentObject private var model: PlayerModel

    private static let videoTypes: [UTType] = {
        var types: [UTType] = [.movie, .mpeg4Movie, .quickTimeMovie]
        if let mkv = UTType(filenameExtension: "mkv") { types.append(mkv) }
        return types
    }()

    var body: some View {
        ZStack {
            if model.player == nil {
                Theme.background
            } else {
                Color.black.ignoresSafeArea()
            }
            if let player = model.player {
                VideoPlayer(player: player)
                    .ignoresSafeArea(edges: .bottom)
                if model.isExternal {
                    externalBanner
                }
            } else {
                welcome
            }
            if model.isLoading {
                ProgressView("Preparando video…")
                    .tint(Theme.accent)
                    .foregroundStyle(Theme.text)
                    .padding(24)
                    .background(Theme.deep.opacity(0.92), in: RoundedRectangle(cornerRadius: 18))
                    .overlay(RoundedRectangle(cornerRadius: 18).stroke(Theme.slate, lineWidth: 1))
            }
        }
        .toolbar { toolbar }
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .navigationTitle(model.title)
        .fileImporter(isPresented: $model.showImporter, allowedContentTypes: Self.videoTypes) { result in
            if case .success(let url) = result { model.open(url) }
        }
        .dropDestination(for: URL.self) { urls, _ in
            guard let url = urls.first else { return false }
            model.open(url)
            return true
        }
        .alert("No se pudo reproducir", isPresented: Binding(
            get: { model.errorMessage != nil },
            set: { if !$0 { model.errorMessage = nil } }
        )) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(model.errorMessage ?? "")
        }
        .wrapInNavigation()
    }

    private var welcome: some View {
        VStack(spacing: 20) {
            Image("Logo")
                .resizable()
                .interpolation(.high)
                .frame(width: 112, height: 112)
                .shadow(color: Theme.accent.opacity(0.45), radius: 28, y: 8)
            Text("Reproductor Open")
                .font(.largeTitle.weight(.bold))
                .foregroundStyle(Theme.text)
            Text("Reproduce MP4 y MKV en tu Apple TV")
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.sky)
                .multilineTextAlignment(.center)
            Text("Abre un video y toca el botón AirPlay. Los MKV se convierten al vuelo, sin perder calidad.")
                .font(.callout)
                .foregroundStyle(Theme.sky.opacity(0.8))
                .multilineTextAlignment(.center)
                .frame(maxWidth: 380)
            Button {
                model.showImporter = true
            } label: {
                Label("Abrir video", systemImage: "folder.fill")
                    .font(.headline)
                    .padding(.horizontal, 22)
                    .padding(.vertical, 12)
                    .foregroundStyle(Theme.abyss)
                    .background(
                        LinearGradient(colors: [Theme.sky, Theme.accent], startPoint: .topLeading, endPoint: .bottomTrailing),
                        in: Capsule()
                    )
                    .shadow(color: Theme.accent.opacity(0.5), radius: 16, y: 6)
            }
            .buttonStyle(.plain)
            .padding(.top, 8)
        }
        .padding(32)
    }

    private var externalBanner: some View {
        VStack(spacing: 12) {
            Image(systemName: "appletv.fill").font(.system(size: 48))
                .foregroundStyle(Theme.accent)
            Text("Reproduciendo en el Apple TV").font(.headline)
                .foregroundStyle(Theme.text)
        }
        .padding(28)
        .background(Theme.deep.opacity(0.9), in: RoundedRectangle(cornerRadius: 22))
        .allowsHitTesting(false)
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .primaryAction) {
            if let tracks = model.info?.audioTracks, tracks.count > 1 {
                Menu {
                    ForEach(tracks, id: \.id) { track in
                        Button {
                            model.selectAudio(track.id)
                        } label: {
                            if track.id == model.selectedAudio {
                                Label(track.label, systemImage: "checkmark")
                            } else {
                                Text(track.label + (track.supported ? "" : " (no compatible)"))
                            }
                        }
                        .disabled(!track.supported)
                    }
                } label: {
                    Label("Audio", systemImage: "waveform")
                }
            }
            RoutePicker(player: model.player)
                .frame(width: 32, height: 32)
            Button {
                model.showImporter = true
            } label: {
                Label("Abrir", systemImage: "folder")
            }
        }
    }
}

private extension View {
    @ViewBuilder
    func wrapInNavigation() -> some View {
        #if os(iOS)
        NavigationStack { self }
        #else
        self.frame(minWidth: 640, minHeight: 400)
        #endif
    }
}

/// Botón del selector de AirPlay (Apple TV, altavoces…).
#if os(iOS)
struct RoutePicker: UIViewRepresentable {
    let player: AVPlayer?

    func makeUIView(context: Context) -> AVRoutePickerView {
        let view = AVRoutePickerView()
        view.prioritizesVideoDevices = true
        view.tintColor = UIColor(Theme.sky)
        view.activeTintColor = UIColor(Theme.accent)
        return view
    }

    func updateUIView(_ view: AVRoutePickerView, context: Context) {}
}
#else
struct RoutePicker: NSViewRepresentable {
    let player: AVPlayer?

    func makeNSView(context: Context) -> AVRoutePickerView {
        let view = AVRoutePickerView()
        view.isRoutePickerButtonBordered = false
        view.player = player
        view.setRoutePickerButtonColor(NSColor(Theme.sky), for: .normal)
        view.setRoutePickerButtonColor(NSColor(Theme.accent), for: .active)
        return view
    }

    func updateNSView(_ view: AVRoutePickerView, context: Context) {
        view.player = player
    }
}
#endif
