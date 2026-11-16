import AVFoundation

/// Convierte Dolby Digital (AC-3 / E-AC-3) a AAC-LC estéreo con AudioToolbox, para las
/// teles AirPlay de otras marcas (Roku, Samsung, LG…), que no decodifican AC-3.
///
/// El núcleo Rust lo llama segmento a segmento (cada uno por separado y en paralelo),
/// así que los frames AAC se alinean a una rejilla global de 1024 muestras: el paquete i
/// cubre las muestras [(firstFrameIndex + i)·1024, …+1024) de la línea de tiempo.
final class AppleAudioTranscoder: AudioTranscoder {
    private static let aacFrame: Int64 = 1024
    /// Retardo del codificador AAC de Apple (TN2258), si el conversor no informa otro.
    private static let defaultPriming: Int64 = 2112

    func toAac(codec: String, sampleRate: UInt32, channels: UInt32, firstSample: Int64, frames: [Data]) throws -> TranscodedAudio {
        let rate = Double(sampleRate)
        guard !frames.isEmpty else { return TranscodedAudio(firstFrameIndex: 0, packets: []) }

        // 1. Dolby Digital → PCM con los canales originales.
        var inDesc = AudioStreamBasicDescription(
            mSampleRate: rate,
            mFormatID: codec == "ec-3" ? kAudioFormatEnhancedAC3 : kAudioFormatAC3,
            mFormatFlags: 0, mBytesPerPacket: 0, mFramesPerPacket: 1536, mBytesPerFrame: 0,
            mChannelsPerFrame: channels, mBitsPerChannel: 0, mReserved: 0
        )
        let layout = Self.layout(for: channels)
        let nativeFormat = AVAudioFormat(standardFormatWithSampleRate: rate, channelLayout: layout)
        guard let compressedFormat = AVAudioFormat(streamDescription: &inDesc, channelLayout: layout),
              let stereoFormat = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2),
              let decoder = AVAudioConverter(from: compressedFormat, to: nativeFormat)
        else { throw Self.fail("no se pudo crear el decodificador de \(codec)") }
        decoder.primeMethod = .none

        let maxPacket = frames.map(\.count).max() ?? 0
        let compressed = AVAudioCompressedBuffer(format: compressedFormat, packetCapacity: AVAudioPacketCount(frames.count),
                                                 maximumPacketSize: maxPacket)
        var offset = 0
        for (i, frame) in frames.enumerated() {
            frame.withUnsafeBytes { raw in
                (compressed.data + offset).copyMemory(from: raw.baseAddress!, byteCount: frame.count)
            }
            compressed.packetDescriptions![i] = AudioStreamPacketDescription(
                mStartOffset: Int64(offset), mVariableFramesInPacket: 0, mDataByteSize: UInt32(frame.count))
            offset += frame.count
        }
        compressed.packetCount = AVAudioPacketCount(frames.count)
        compressed.byteLength = UInt32(offset)

        let capacity = AVAudioFrameCount(frames.count * 1536 + 4096)
        guard let native = AVAudioPCMBuffer(pcmFormat: nativeFormat, frameCapacity: capacity) else {
            throw Self.fail("sin memoria para el audio decodificado")
        }
        var fed = false
        var error: NSError?
        let status = decoder.convert(to: native, error: &error) { _, outStatus in
            if fed { outStatus.pointee = .endOfStream; return nil }
            fed = true
            outStatus.pointee = .haveData
            return compressed
        }
        if status == .error { throw Self.fail("error al decodificar \(codec): \(error?.localizedDescription ?? "-")") }

        // 2. Mezcla a estéreo (5.1 → 2.0 con los coeficientes estándar de Apple).
        guard let stereo = AVAudioPCMBuffer(pcmFormat: stereoFormat, frameCapacity: max(native.frameLength, 1)) else {
            throw Self.fail("sin memoria para el audio estéreo")
        }
        if channels == 2 {
            Self.copy(native, into: stereo, from: 0)
        } else {
            guard let mixer = AVAudioConverter(from: nativeFormat, to: stereoFormat) else { throw Self.fail("no se pudo crear el mezclador") }
            mixer.downmix = true
            try mixer.convert(to: stereo, from: native)
        }

        // 3. PCM → AAC alineado a la rejilla global.
        let aacSettings: [String: Any] = [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: rate,
            AVNumberOfChannelsKey: 2,
        ]
        guard let aacFormat = AVAudioFormat(settings: aacSettings),
              let encoder = AVAudioConverter(from: stereoFormat, to: aacFormat)
        else { throw Self.fail("no se pudo crear el codificador AAC") }
        encoder.bitRate = 192_000
        let reported = Int64(encoder.primeInfo.leadingFrames)
        let priming = reported > 0 ? reported : Self.defaultPriming

        // Recortar d muestras para que (firstSample + d - priming) caiga en la rejilla.
        let d = ((priming - firstSample) % Self.aacFrame + Self.aacFrame) % Self.aacFrame
        guard Int64(stereo.frameLength) > d,
              let trimmed = AVAudioPCMBuffer(pcmFormat: stereoFormat, frameCapacity: stereo.frameLength - AVAudioFrameCount(d))
        else { return TranscodedAudio(firstFrameIndex: 0, packets: []) }
        Self.copy(stereo, into: trimmed, from: Int(d))
        let firstFrameIndex = (firstSample + d - priming) / Self.aacFrame

        var packets: [Data] = []
        var pcmFed = false
        while true {
            let out = AVAudioCompressedBuffer(format: aacFormat, packetCapacity: 64, maximumPacketSize: encoder.maximumOutputPacketSize)
            let st = encoder.convert(to: out, error: &error) { _, outStatus in
                if pcmFed { outStatus.pointee = .endOfStream; return nil }
                pcmFed = true
                outStatus.pointee = .haveData
                return trimmed
            }
            for i in 0..<Int(out.packetCount) {
                let desc = out.packetDescriptions![i]
                packets.append(Data(bytes: out.data + Int(desc.mStartOffset), count: Int(desc.mDataByteSize)))
            }
            if st == .error { throw Self.fail("error al codificar AAC: \(error?.localizedDescription ?? "-")") }
            if st == .endOfStream || (out.packetCount == 0 && st != .haveData) { break }
        }
        return TranscodedAudio(firstFrameIndex: firstFrameIndex, packets: packets)
    }

    private static func layout(for channels: UInt32) -> AVAudioChannelLayout {
        let tag: AudioChannelLayoutTag
        switch channels {
        case 1: tag = kAudioChannelLayoutTag_Mono
        case 2: tag = kAudioChannelLayoutTag_Stereo
        case 3: tag = kAudioChannelLayoutTag_AC3_3_0
        case 4: tag = kAudioChannelLayoutTag_AC3_2_1_1
        case 5: tag = kAudioChannelLayoutTag_MPEG_5_0_C   // L C R Ls Rs (orden Dolby)
        case 6: tag = kAudioChannelLayoutTag_MPEG_5_1_C   // L C R Ls Rs LFE
        case 8: tag = kAudioChannelLayoutTag_MPEG_7_1_C
        default: tag = kAudioChannelLayoutTag_DiscreteInOrder | channels
        }
        return AVAudioChannelLayout(layoutTag: tag)!
    }

    /// Copia `src` a partir de la muestra `start` (formato float no entrelazado).
    private static func copy(_ src: AVAudioPCMBuffer, into dst: AVAudioPCMBuffer, from start: Int) {
        let count = min(Int(src.frameLength) - start, Int(dst.frameCapacity))
        guard count > 0, let s = src.floatChannelData, let t = dst.floatChannelData else { return }
        for ch in 0..<Int(dst.format.channelCount) {
            (t[ch]).update(from: s[min(ch, Int(src.format.channelCount) - 1)] + start, count: count)
        }
        dst.frameLength = AVAudioFrameCount(count)
    }

    private static func fail(_ detail: String) -> TranscodeError {
        Log.player.error("Conversor de audio: \(detail, privacy: .public)")
        return TranscodeError.Failed(detail: detail)
    }
}
