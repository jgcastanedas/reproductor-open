import AVKit
import SwiftUI
import UniformTypeIdentifiers

struct ContentView: View {
    @EnvironmentObject private var model: PlayerModel
    @EnvironmentObject private var library: Library
    @State private var selection: UUID?

    private static let videoTypes: [UTType] = {
        var types: [UTType] = [.movie, .mpeg4Movie, .quickTimeMovie]
        if let mkv = UTType(filenameExtension: "mkv") { types.append(mkv) }
        return types
    }()

    var body: some View {
        NavigationSplitView {
            LibrarySidebar(selection: $selection)
                .navigationSplitViewColumnWidth(min: 260, ideal: 310, max: 420)
        } detail: {
            PlayerDetail()
        }
        .onChange(of: selection) { _, id in
            guard let id, id != model.currentItemID, let item = library.item(id) else { return }
            model.open(item: item)
        }
        .onChange(of: model.currentItemID) { _, id in
            if id != nil { selection = id }
        }
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
    }
}

struct PlayerDetail: View {
    @EnvironmentObject private var model: PlayerModel

    var body: some View {
        ZStack {
            if let player = model.player {
                Color.black.ignoresSafeArea()
                PlayerSurface(player: player)
                    .ignoresSafeArea()
                if model.isExternal {
                    externalBanner
                }
            } else {
                Theme.background
                welcome
            }
            if let notice = model.notice {
                VStack {
                    Label(notice, systemImage: "tv.badge.wifi")
                        .font(.callout.weight(.medium))
                        .foregroundStyle(Theme.text)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 10)
                        .background(Theme.deep.opacity(0.95), in: Capsule())
                        .overlay(Capsule().stroke(Theme.accent.opacity(0.7), lineWidth: 1))
                        .padding(.top, 14)
                    Spacer()
                }
                .transition(.move(edge: .top).combined(with: .opacity))
                .allowsHitTesting(false)
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
        .animation(.easeInOut(duration: 0.25), value: model.notice)
        .navigationTitle(model.title)
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .toolbar { toolbar }
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
            Text("Reproduce MP4 y MKV en tu tele con AirPlay")
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.sky)
                .multilineTextAlignment(.center)
            Text(hint)
                .font(.callout)
                .foregroundStyle(Theme.sky.opacity(0.8))
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
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

    private var hint: String {
        #if os(macOS)
        "Arrastra un archivo aquí o ábrelo con ⌘O. Para verlo en la tele, usa el botón AirPlay de los controles del reproductor."
        #else
        "Abre un video y toca el botón AirPlay. Los MKV se convierten al vuelo, sin perder calidad."
        #endif
    }

    private var externalBanner: some View {
        VStack(spacing: 12) {
            Image(systemName: "tv.fill").font(.system(size: 48))
                .foregroundStyle(Theme.accent)
            Text("Reproduciendo en la tele").font(.headline)
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
                        Toggle(isOn: Binding(
                            get: { track.id == model.selectedAudio },
                            set: { if $0 { model.selectAudio(track.id) } }
                        )) {
                            Text(track.label + (track.supported ? "" : " (no compatible)"))
                        }
                        .disabled(!track.supported)
                    }
                } label: {
                    Label("Audio", systemImage: "waveform")
                }
                .help("Pista de audio")
            }
            if !model.subtitleOptions.isEmpty {
                Menu {
                    Toggle("Desactivados", isOn: Binding(
                        get: { model.selectedSubtitle == nil },
                        set: { if $0 { model.selectSubtitle(nil) } }
                    ))
                    Divider()
                    ForEach(model.subtitleOptions, id: \.self) { option in
                        Toggle(option.subtitleLabel, isOn: Binding(
                            get: { model.selectedSubtitle == option },
                            set: { if $0 { model.selectSubtitle(option) } }
                        ))
                    }
                } label: {
                    Label("Subtítulos", systemImage: "captions.bubble")
                }
                .help("Subtítulos")
            }
            Menu {
                Toggle("Modo compatible para teles Roku, Samsung, LG…", isOn: Binding(
                    get: { model.compatMode },
                    set: { model.setCompatMode($0) }
                ))
                Toggle("Enviar subtítulos a la tele", isOn: Binding(
                    get: { model.tvSubtitles },
                    set: { model.setTVSubtitles($0) }
                ))
                .disabled(!model.compatMode)
                Text("Convierte el audio Dolby Digital a AAC estéreo en piezas cortas, que es lo que esas teles aceptan por AirPlay. Si la tele rechaza los subtítulos, se quitan solos. Con Apple TV o en esta pantalla no hace falta.")
            } label: {
                Label("Tele", systemImage: model.compatMode ? "tv.badge.wifi" : "tv")
            }
            .help(model.compatMode ? "Modo compatible activado" : "Opciones para la tele")
            #if os(iOS)
            RoutePicker()
                .frame(width: 32, height: 32)
            #endif
            Button {
                model.showImporter = true
            } label: {
                Label("Abrir", systemImage: "folder")
            }
            .help("Abrir video (⌘O)")
        }
    }
}
