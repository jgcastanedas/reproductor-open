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
            Color.black.ignoresSafeArea()
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
                    .padding(24)
                    .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 16))
            }
        }
        .toolbar { toolbar }
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .navigationTitle(model.title.isEmpty ? "Reproductor" : model.title)
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
            Image(systemName: "airplayvideo")
                .font(.system(size: 64, weight: .light))
                .foregroundStyle(.secondary)
            Text("Reproduce MP4 y MKV en tu Apple TV")
                .font(.title2.weight(.semibold))
                .multilineTextAlignment(.center)
            Text("Abre un video y toca el botón AirPlay. Los MKV se convierten al vuelo, sin perder calidad.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 380)
            Button {
                model.showImporter = true
            } label: {
                Label("Abrir video", systemImage: "folder")
                    .padding(.horizontal, 8)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
        }
        .padding(32)
        .foregroundStyle(.white)
    }

    private var externalBanner: some View {
        VStack(spacing: 12) {
            Image(systemName: "appletv.fill").font(.system(size: 48))
            Text("Reproduciendo en el Apple TV").font(.headline)
        }
        .foregroundStyle(.white.opacity(0.85))
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
        view.tintColor = .white
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
        return view
    }

    func updateNSView(_ view: AVRoutePickerView, context: Context) {
        view.player = player
    }
}
#endif
