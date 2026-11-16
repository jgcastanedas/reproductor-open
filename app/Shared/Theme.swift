import SwiftUI

/// Paleta de Reproductor Open (tonos petróleo y turquesa).
enum Theme {
    /// #05161A — fondo más profundo
    static let abyss = Color(hex: 0x05161A)
    /// #072E33 — fondo secundario / superficies
    static let deep = Color(hex: 0x072E33)
    /// #0C7075 — superficies activas, bordes
    static let teal = Color(hex: 0x0C7075)
    /// #0F969C — color de acento (botones, progreso)
    static let accent = Color(hex: 0x0F969C)
    /// #6DA5C0 — texto secundario, íconos
    static let sky = Color(hex: 0x6DA5C0)
    /// #294D61 — tarjetas y separadores
    static let slate = Color(hex: 0x294D61)

    static let text = Color(hex: 0xE6F2F5)

    static var background: some View {
        ZStack {
            LinearGradient(colors: [deep, abyss], startPoint: .topLeading, endPoint: .bottomTrailing)
            RadialGradient(colors: [accent.opacity(0.28), teal.opacity(0.08), .clear],
                           center: UnitPoint(x: 0.5, y: 0.62), startRadius: 0, endRadius: 520)
        }
        .ignoresSafeArea()
    }
}

extension Color {
    init(hex: UInt32) {
        self.init(.sRGB,
                  red: Double((hex >> 16) & 0xFF) / 255,
                  green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255)
    }
}
