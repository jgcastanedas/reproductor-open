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
    @Published private(set) var isLoading = false
    @Published private(set) var isExternal = false
    @Published var errorMessage: String?
    @Published var showImporter = false

    private var server: StreamServer?
    private var scopedURL: URL?
    private var currentURL: URL?
    private var externalObservation: NSKeyValueObservation?

    static let mkvExtensions: Set<String> = ["mkv", "mk3d"]

    func open(_ url: URL) {
        close()
        if url.startAccessingSecurityScopedResource() {
            scopedURL = url
        }
        currentURL = url
        title = url.deletingPathExtension().lastPathComponent
        if Self.mkvExtensions.contains(url.pathExtension.lowercased()) {
            openMKV(url, audio: nil, resumeAt: nil)
        } else {
            play(AVPlayerItem(url: url), resumeAt: nil)
        }
    }

    func selectAudio(_ id: UInt64) {
        guard let url = currentURL, id != selectedAudio else { return }
        let time = player?.currentTime()
        openMKV(url, audio: id, resumeAt: time)
    }

    func close() {
        player?.pause()
        externalObservation = nil
        player = nil
        info = nil
        selectedAudio = nil
        isExternal = false
        server?.clear()
        scopedURL?.stopAccessingSecurityScopedResource()
        scopedURL = nil
        currentURL = nil
    }

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
                let audioId = audio ?? Self.preferredAudio(info)
                let server = try await self?.ensureServer()
                guard let server else { return }
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
        self.info = info
        self.selectedAudio = audio
        play(AVPlayerItem(url: url), resumeAt: resumeAt)
    }

    private func play(_ item: AVPlayerItem, resumeAt: CMTime?) {
        isLoading = false
        let player = self.player ?? AVPlayer()
        player.allowsExternalPlayback = true
        #if os(iOS)
        player.usesExternalPlaybackWhileExternalScreenIsActive = true
        #endif
        player.replaceCurrentItem(with: item)
        if let resumeAt {
            player.seek(to: resumeAt, toleranceBefore: .zero, toleranceAfter: .positiveInfinity)
        }
        externalObservation = player.observe(\.isExternalPlaybackActive, options: [.initial, .new]) { [weak self] p, _ in
            let active = p.isExternalPlaybackActive
            Task { @MainActor in self?.isExternal = active }
        }
        self.player = player
        player.play()
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
