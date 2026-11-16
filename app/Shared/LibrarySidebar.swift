import SwiftUI

struct LibrarySidebar: View {
    @EnvironmentObject private var model: PlayerModel
    @EnvironmentObject private var library: Library
    @Binding var selection: UUID?

    var body: some View {
        List(selection: $selection) {
            Section {
                Button {
                    model.showImporter = true
                } label: {
                    Label("Abrir video…", systemImage: "plus.circle.fill")
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(Theme.accent)
                }
                .buttonStyle(.plain)
                .padding(.vertical, 4)
            }

            if library.items.isEmpty {
                Text("Los videos que abras aparecerán aquí, listos para seguir viéndolos.")
                    .font(.callout)
                    .foregroundStyle(Theme.sky.opacity(0.8))
                    .padding(.vertical, 8)
            }

            if !library.continueWatching.isEmpty {
                Section("Seguir viendo") {
                    ForEach(library.continueWatching) { item in
                        LibraryRow(item: item).tag(item.id)
                    }
                }
            }
            if !library.others.isEmpty {
                Section("Biblioteca") {
                    ForEach(library.others) { item in
                        LibraryRow(item: item).tag(item.id)
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .background(Theme.abyss)
        .navigationTitle("Reproductor Open")
    }
}

struct LibraryRow: View {
    @EnvironmentObject private var library: Library
    @EnvironmentObject private var model: PlayerModel
    let item: LibraryItem

    var body: some View {
        HStack(spacing: 12) {
            Thumbnail(url: library.thumbnailURL(for: item.id), available: item.hasThumbnail, isMKV: item.isMKV)
                .frame(width: 104, height: 58)
                .overlay(alignment: .bottomLeading) {
                    if item.progress > 0, !item.isFinished {
                        GeometryReader { geo in
                            Capsule().fill(Theme.accent)
                                .frame(width: max(4, geo.size.width * item.progress), height: 3)
                                .frame(maxHeight: .infinity, alignment: .bottom)
                        }
                        .padding(.horizontal, 4)
                        .padding(.bottom, 3)
                    }
                }
                .overlay {
                    if model.currentItemID == item.id {
                        RoundedRectangle(cornerRadius: 8).stroke(Theme.accent, lineWidth: 2)
                    }
                }
            VStack(alignment: .leading, spacing: 4) {
                Text(item.title.replacingOccurrences(of: ".", with: " "))
                    .font(.callout.weight(.medium))
                    .foregroundStyle(Theme.text)
                    .lineLimit(2)
                HStack(spacing: 6) {
                    if model.currentItemID == item.id {
                        Image(systemName: "play.fill").font(.caption2).foregroundStyle(Theme.accent)
                    }
                    if let text = item.remainingText {
                        Text(text).font(.caption).foregroundStyle(Theme.sky)
                    }
                    Text(item.isMKV ? "MKV" : (item.path as NSString).pathExtension.uppercased())
                        .font(.caption2.weight(.semibold))
                        .padding(.horizontal, 5).padding(.vertical, 1)
                        .background(Theme.slate.opacity(0.7), in: Capsule())
                        .foregroundStyle(Theme.sky)
                }
            }
        }
        .padding(.vertical, 4)
        .contextMenu {
            Button("Reproducir") { model.open(item: item) }
            #if os(macOS)
            Button("Mostrar en el Finder") {
                if let url = library.resolve(item) {
                    NSWorkspace.shared.activateFileViewerSelecting([url])
                }
            }
            #endif
            Divider()
            Button("Quitar de la biblioteca", role: .destructive) { library.remove(item.id) }
        }
    }
}

struct Thumbnail: View {
    let url: URL
    let available: Bool
    let isMKV: Bool

    var body: some View {
        ZStack {
            LinearGradient(colors: [Theme.deep, Theme.abyss], startPoint: .topLeading, endPoint: .bottomTrailing)
            if available, let image = load() {
                image.resizable().aspectRatio(contentMode: .fill)
            } else {
                Image(systemName: "film").font(.title3).foregroundStyle(Theme.slate)
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Theme.slate.opacity(0.6), lineWidth: 1))
    }

    private func load() -> Image? {
        #if os(macOS)
        NSImage(contentsOf: url).map { Image(nsImage: $0) }
        #else
        UIImage(contentsOfFile: url.path).map { Image(uiImage: $0) }
        #endif
    }
}
