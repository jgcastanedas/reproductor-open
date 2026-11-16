import OSLog

/// Registros visibles en Consola.app o con:
///   log stream --predicate 'subsystem == "app.ainoa.reproductor"' --info
enum Log {
    static let server = Logger(subsystem: "app.ainoa.reproductor", category: "servidor")
    static let player = Logger(subsystem: "app.ainoa.reproductor", category: "reproductor")
}

/// Recibe del núcleo Rust cada petición HTTP (quién pide qué) y los errores.
final class CoreLogSink: LogSink {
    func log(line: String) {
        Log.server.info("\(line, privacy: .public)")
    }
}
