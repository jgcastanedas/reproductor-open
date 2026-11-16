import AVFoundation
import Foundation

/// Abre MP4/MOV directamente con AVPlayer y los MKV a través del núcleo Rust
/// (remux a HLS servido en la red local), para poder enviarlos por AirPlay.
@MainActor
final class PlayerModel: ObservableObject {
    @Published private(set) var player: AVPlayer?
    @Published private(set) var title = ""
    @Published private(set) var info: MediaInfo?
    @Published private(set) var selectedAudio: UInt64?
    @Published private(set) var subtitleOptions: [AVMediaSelectionOption] = []
    @Published private(set) var selectedSubtitle: AVMediaSelectionOption?
    @Published private(set) var isLoading = false
    @Published private(set) var isExternal = false
    @Published private(set) var currentItemID: UUID?
    @Published var errorMessage: String?
    @Published var showImporter = false

    let library: Library

    private var server: StreamServer?
    private var scopedURL: URL?
    private var currentURL: URL?
    private var isMKV = false
    private var observations: [NSKeyValueObservation] = []
    private var timeObserver: Any?
    private var tokens: [NSObjectProtocol] = []
    private var legibleGroup: AVMediaSelectionGroup?
    /// Subtítulo elegido, para reaplicarlo al cambiar de pista de audio.
    private var wantedSubtitle: (tag: String?, forced: Bool)?

    static let mkvExtensions: Set<String> = ["mkv", "mk3d"]

    init(library: Library) {
        self.library = library
        setLogSink(sink: CoreLogSink())
    }

    // MARK: - Abrir

    func open(_ url: URL) {
        open(url, audio: nil)
    }

    func open(item: LibraryItem) {
        guard item.id != currentItemID else { return }
        guard let url = library.resolve(item) else {
            errorMessage = "No se encontró «\(item.title)». Puede que el archivo se haya movido o borrado."
            return
        }
        open(url, audio: item.audioTrack)
    }

    private func open(_ url: URL, audio: UInt64?) {
        close()
        if url.startAccessingSecurityScopedResource() {
            scopedURL = url
        }
        currentURL = url
        title = url.deletingPathExtension().lastPathComponent
        let entry = library.record(url)
        currentItemID = entry.id
        wantedSubtitle = nil
        let resume = entry.resumePosition.map { CMTime(seconds: $0, preferredTimescale: 600) }
        isMKV = Self.mkvExtensions.contains(url.pathExtension.lowercased())
        Log.player.info("Abriendo \(url.lastPathComponent, privacy: .public)")
        if isMKV {
            openMKV(url, audio: audio ?? entry.audioTrack, resumeAt: resume)
        } else {
            play(AVPlayerItem(url: url), resumeAt: resume)
        }
    }

    func selectAudio(_ id: UInt64) {
        guard let url = currentURL, id != selectedAudio else { return }
        if let itemID = currentItemID { library.setAudioTrack(itemID, id) }
        openMKV(url, audio: id, resumeAt: player?.currentTime())
    }

    func selectSubtitle(_ option: AVMediaSelectionOption?) {
        guard let item = player?.currentItem, let group = legibleGroup else { return }
        item.select(option, in: group)
        selectedSubtitle = option
        wantedSubtitle = option.map { ($0.extendedLanguageTag, $0.hasMediaCharacteristic(.containsOnlyForcedSubtitles)) }
    }

    func close() {
        saveProgress()
        teardownObservers()
        player?.pause()
        player = nil
        info = nil
        selectedAudio = nil
        subtitleOptions = []
        selectedSubtitle = nil
        legibleGroup = nil
        isExternal = false
        currentItemID = nil
        title = ""
        server?.clear()
        scopedURL?.stopAccessingSecurityScopedResource()
        scopedURL = nil
        currentURL = nil
    }

    // MARK: - MKV

    private func openMKV(_ url: URL, audio: UInt64?, resumeAt: CMTime?) {
        isLoading = true
        errorMessage = nil
        let path = url.path(percentEncoded: false)
        Task.detached(priority: .userInitiated) { [weak self] in
            do {
                let info = try probe(path: path)
                guard info.videoSupported else {
                    throw CoreError.Unsupported(detail: "El video es \(info.videoCodec). AirPlay solo acepta H.264 o HEVC sin recodificar.")
                }
                // Si ninguna pista de audio es compatible, se reproduce solo el video.
                let requested = audio.flatMap { id in info.audioTracks.first { $0.id == id && $0.supported }?.id }
                let audioId = requested ?? Self.preferredAudio(info)
                guard let server = try await self?.ensureServer() else { return }
                let hls = try server.publish(path: path, audioTrack: audioId)
                await self?.finishMKV(info: info, audio: audioId, hls: hls, resumeAt: resumeAt)
            } catch {
                await self?.fail(error)
            }
        }
    }

    nonisolated private static func preferredAudio(_ info: MediaInfo) -> UInt64? {
        let ok = info.audioTracks.filter(\.supported)
        let spanish = ok.first { ($0.language ?? "").lowercased().hasPrefix("spa") || ($0.language ?? "").lowercased().hasPrefix("es") }
        return (spanish ?? ok.first(where: \.isDefault) ?? ok.first)?.id
    }

    private func ensureServer() throws -> StreamServer {
        if let server { return server }
        let s = try StreamServer()
        server = s
        return s
    }

    private func finishMKV(info: MediaInfo, audio: UInt64?, hls: String, resumeAt: CMTime?) {
        guard let url = URL(string: hls) else { return }
        Log.player.info("HLS listo: \(hls, privacy: .public)")
        self.info = info
        self.selectedAudio = audio
        play(AVPlayerItem(url: url), resumeAt: resumeAt)
    }

    // MARK: - Reproducción

    private func play(_ item: AVPlayerItem, resumeAt: CMTime?) {
        isLoading = false
        teardownObservers()
        let player = self.player ?? AVPlayer()
        player.allowsExternalPlayback = true
        #if os(iOS)
        player.usesExternalPlaybackWhileExternalScreenIsActive = true
        #endif
        player.replaceCurrentItem(with: item)
        if let resumeAt {
            player.seek(to: resumeAt, toleranceBefore: .zero, toleranceAfter: .positiveInfinity)
        }
        observe(player: player, item: item)
        self.player = player
        player.play()
    }

    private func observe(player: AVPlayer, item: AVPlayerItem) {
        observations.append(player.observe(\.isExternalPlaybackActive, options: [.initial, .new]) { [weak self] p, _ in
            let active = p.isExternalPlaybackActive
            Log.player.info("Reproducción externa (AirPlay de video): \(active ? "activa" : "inactiva", privacy: .public)")
            Task { @MainActor in self?.isExternal = active }
        })
        observations.append(item.observe(\.status, options: [.new]) { [weak self] item, _ in
            Task { @MainActor in self?.itemStatusChanged(item) }
        })
        let center = NotificationCenter.default
        tokens.append(center.addObserver(forName: AVPlayerItem.newErrorLogEntryNotification, object: item, queue: .main) { note in
            guard let event = (note.object as? AVPlayerItem)?.errorLog()?.events.last else { return }
            Log.player.error("Error HLS \(event.errorStatusCode, privacy: .public) [\(event.errorDomain, privacy: .public)]: \(event.errorComment ?? "-", privacy: .public) — \(event.uri ?? "-", privacy: .public)")
        })
        tokens.append(center.addObserver(forName: AVPlayerItem.failedToPlayToEndTimeNotification, object: item, queue: .main) { [weak self] note in
            let error = note.userInfo?[AVPlayerItemFailedToPlayToEndTimeErrorKey] as? Error
            Log.player.error("Falló la reproducción: \(String(describing: error), privacy: .public)")
            Task { @MainActor in self?.errorMessage = error?.localizedDescription ?? "La reproducción se detuvo por un error." }
        })
        timeObserver = player.addPeriodicTimeObserver(forInterval: CMTime(seconds: 5, preferredTimescale: 1), queue: .main) { [weak self] _ in
            Task { @MainActor in self?.saveProgress() }
        }
    }

    private func itemStatusChanged(_ item: AVPlayerItem) {
        switch item.status {
        case .failed:
            let error = item.error
            Log.player.error("El video no se pudo cargar: \(String(describing: error), privacy: .public)")
            errorMessage = error?.localizedDescription ?? "No se pudo cargar el video."
        case .readyToPlay:
            Task { await loadSubtitles(for: item) }
            makeThumbnailIfNeeded(duration: item.duration.seconds)
        default:
            break
        }
    }

    private func loadSubtitles(for item: AVPlayerItem) async {
        guard let group = try? await item.asset.loadMediaSelectionGroup(for: .legible) else { return }
        legibleGroup = group
        subtitleOptions = group.options
        if let wanted = wantedSubtitle,
           let match = group.options.first(where: {
               $0.extendedLanguageTag == wanted.tag && $0.hasMediaCharacteristic(.containsOnlyForcedSubtitles) == wanted.forced
           }) {
            item.select(match, in: group)
        }
        selectedSubtitle = item.currentMediaSelection.selectedMediaOption(in: group)
    }

    private func makeThumbnailIfNeeded(duration: Double) {
        guard let id = currentItemID, let url = currentURL, library.item(id)?.hasThumbnail == false else { return }
        let total = info?.durationSeconds ?? (duration.isFinite ? duration : 0)
        let destination = library.thumbnailURL(for: id)
        let mkv = isMKV
        Task.detached(priority: .utility) { [weak self] in
            if await Thumbnailer.make(for: url, isMKV: mkv, duration: total, to: destination) {
                await self?.library.markThumbnail(id)
            }
        }
    }

    private func saveProgress() {
        guard let id = currentItemID, let player, let item = player.currentItem else { return }
        let duration = info?.durationSeconds ?? item.duration.seconds
        library.updateProgress(id, position: player.currentTime().seconds, duration: duration)
    }

    private func teardownObservers() {
        observations.forEach { $0.invalidate() }
        observations = []
        tokens.forEach { NotificationCenter.default.removeObserver($0) }
        tokens = []
        if let timeObserver, let player { player.removeTimeObserver(timeObserver) }
        timeObserver = nil
    }

    private func fail(_ error: Error) {
        isLoading = false
        if let e = error as? CoreError {
            switch e {
            case .Io(let d), .InvalidFile(let d), .Unsupported(let d), .Server(let d):
                errorMessage = d
            }
        } else {
            errorMessage = error.localizedDescription
        }
        Log.player.error("Error al abrir: \(self.errorMessage ?? "-", privacy: .public)")
    }
}

extension AudioTrackInfo {
    var label: String {
        var parts: [String] = []
        if let language { parts.append(Locale.current.localizedString(forLanguageCode: language)?.capitalized ?? language) }
        parts.append(codec)
        if channels > 2 { parts.append(channels == 6 ? "5.1" : channels == 8 ? "7.1" : "\(channels) canales") }
        if let name, !name.lowercased().contains("www.") { parts.append("«\(name)»") }
        return parts.joined(separator: " · ")
    }
}

extension AVMediaSelectionOption {
    var subtitleLabel: String {
        let name = displayName(with: Locale.current).capitalized
        return hasMediaCharacteristic(.containsOnlyForcedSubtitles) ? "\(name) (forzados)" : name
    }
}
