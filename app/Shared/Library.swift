import Foundation

struct LibraryItem: Codable, Identifiable, Hashable {
    var id: UUID
    var title: String
    var path: String
    var bookmark: Data
    var position: Double = 0
    var duration: Double = 0
    var lastOpened: Date
    var audioTrack: UInt64?
    var hasThumbnail = false

    var progress: Double { duration > 0 ? min(position / duration, 1) : 0 }
    var isFinished: Bool { duration > 0 && position > duration - 120 }

    /// Dónde retomar, si se dejó a medias.
    var resumePosition: Double? { position > 15 && !isFinished ? position : nil }

    var isMKV: Bool { ["mkv", "mk3d"].contains((path as NSString).pathExtension.lowercased()) }

    var remainingText: String? {
        guard duration > 0 else { return nil }
        if isFinished { return "Visto" }
        if position < 15 { return Self.format(duration) }
        return "Quedan \(Self.format(duration - position))"
    }

    private static func format(_ secs: Double) -> String {
        let m = Int(secs / 60)
        return m >= 60 ? "\(m / 60) h \(m % 60) min" : "\(max(m, 1)) min"
    }
}

/// Videos abiertos recientemente, con su posición para «seguir viendo».
/// Se guarda en Application Support/ReproductorOpen/biblioteca.json.
@MainActor
final class Library: ObservableObject {
    @Published private(set) var items: [LibraryItem] = []

    private let directory: URL
    private var fileURL: URL { directory.appendingPathComponent("biblioteca.json") }
    private var thumbnailsDir: URL { directory.appendingPathComponent("Miniaturas", isDirectory: true) }

    init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        directory = base.appendingPathComponent("ReproductorOpen", isDirectory: true)
        try? FileManager.default.createDirectory(at: thumbnailsDir, withIntermediateDirectories: true)
        if let data = try? Data(contentsOf: fileURL),
           let saved = try? JSONDecoder().decode([LibraryItem].self, from: data) {
            items = saved.sorted { $0.lastOpened > $1.lastOpened }
        }
    }

    var continueWatching: [LibraryItem] { items.filter { $0.resumePosition != nil } }
    var others: [LibraryItem] { items.filter { $0.resumePosition == nil } }

    func item(_ id: UUID) -> LibraryItem? { items.first { $0.id == id } }

    func thumbnailURL(for id: UUID) -> URL {
        thumbnailsDir.appendingPathComponent("\(id.uuidString).jpg")
    }

    /// Registra (o actualiza) un archivo recién abierto.
    @discardableResult
    func record(_ url: URL) -> LibraryItem {
        let path = url.standardizedFileURL.path(percentEncoded: false)
        let bookmark = Self.bookmark(for: url) ?? Data()
        if let i = items.firstIndex(where: { $0.path == path }) {
            items[i].lastOpened = .now
            if !bookmark.isEmpty { items[i].bookmark = bookmark }
            let item = items[i]
            items.sort { $0.lastOpened > $1.lastOpened }
            save()
            return item
        }
        let item = LibraryItem(
            id: UUID(),
            title: url.deletingPathExtension().lastPathComponent,
            path: path,
            bookmark: bookmark,
            lastOpened: .now
        )
        items.insert(item, at: 0)
        save()
        return item
    }

    /// URL del archivo a partir del marcador (con permiso de lectura en el sandbox).
    func resolve(_ item: LibraryItem) -> URL? {
        var stale = false
        guard let url = try? URL(resolvingBookmarkData: item.bookmark, options: Self.resolveOptions,
                                 relativeTo: nil, bookmarkDataIsStale: &stale) else { return nil }
        if stale, let fresh = Self.bookmark(for: url), let i = items.firstIndex(where: { $0.id == item.id }) {
            items[i].bookmark = fresh
            save()
        }
        return url
    }

    func updateProgress(_ id: UUID, position: Double, duration: Double) {
        guard let i = items.firstIndex(where: { $0.id == id }), position.isFinite else { return }
        items[i].position = position
        if duration.isFinite, duration > 0 { items[i].duration = duration }
        save()
    }

    func setAudioTrack(_ id: UUID, _ track: UInt64?) {
        guard let i = items.firstIndex(where: { $0.id == id }) else { return }
        items[i].audioTrack = track
        save()
    }

    func markThumbnail(_ id: UUID) {
        guard let i = items.firstIndex(where: { $0.id == id }) else { return }
        items[i].hasThumbnail = true
        save()
    }

    func remove(_ id: UUID) {
        items.removeAll { $0.id == id }
        try? FileManager.default.removeItem(at: thumbnailURL(for: id))
        save()
    }

    private func save() {
        guard let data = try? JSONEncoder().encode(items) else { return }
        try? data.write(to: fileURL, options: .atomic)
    }

    #if os(macOS)
    private static let resolveOptions: URL.BookmarkResolutionOptions = [.withSecurityScope]
    private static func bookmark(for url: URL) -> Data? {
        try? url.bookmarkData(options: [.withSecurityScope, .securityScopeAllowOnlyReadAccess],
                              includingResourceValuesForKeys: nil, relativeTo: nil)
    }
    #else
    private static let resolveOptions: URL.BookmarkResolutionOptions = []
    private static func bookmark(for url: URL) -> Data? {
        try? url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
    }
    #endif
}
