import AVFoundation
import ImageIO
import UniformTypeIdentifiers

/// Miniaturas para la biblioteca. Los MP4 se leen directo; para los MKV el núcleo
/// Rust devuelve un fragmento MP4 corto que AVFoundation sí sabe abrir.
enum Thumbnailer {
    static func make(for url: URL, isMKV: Bool, duration: Double, to destination: URL) async -> Bool {
        let at = duration > 0 ? min(max(duration * 0.1, 5), 600) : 30
        var assetURL = url
        var tempURL: URL?
        if isMKV {
            guard let data = try? thumbnailClip(path: url.path(percentEncoded: false), atSeconds: at) else { return false }
            let tmp = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".mp4")
            guard (try? data.write(to: tmp)) != nil else { return false }
            tempURL = tmp
            assetURL = tmp
        }
        defer { if let tempURL { try? FileManager.default.removeItem(at: tempURL) } }

        let generator = AVAssetImageGenerator(asset: AVURLAsset(url: assetURL))
        generator.appliesPreferredTrackTransform = true
        generator.maximumSize = CGSize(width: 640, height: 640)
        generator.requestedTimeToleranceBefore = .positiveInfinity
        generator.requestedTimeToleranceAfter = .positiveInfinity
        guard let (image, _) = try? await generator.image(at: CMTime(seconds: at, preferredTimescale: 600)) else {
            Log.player.error("No se pudo generar la miniatura de \(url.lastPathComponent, privacy: .public)")
            return false
        }
        return writeJPEG(image, to: destination)
    }

    private static func writeJPEG(_ image: CGImage, to url: URL) -> Bool {
        guard let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.jpeg.identifier as CFString, 1, nil) else { return false }
        CGImageDestinationAddImage(dest, image, [kCGImageDestinationLossyCompressionQuality: 0.8] as CFDictionary)
        return CGImageDestinationFinalize(dest)
    }
}
