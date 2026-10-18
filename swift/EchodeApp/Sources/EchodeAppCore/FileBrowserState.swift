import Foundation
import Observation

struct BrowserFile: Identifiable, Sendable {
    let url: URL
    let directory: Bool
    var id: URL { url }
    var project: Bool { url.pathExtension.lowercased() == "echode" }
    var audio: Bool {
        ["wav", "aiff", "aif", "caf", "mp3", "m4a", "aac", "flac"].contains(url.pathExtension.lowercased())
    }
    var icon: String { project || audio ? "waveform" : directory ? "folder" : "doc" }
}

@MainActor @Observable
final class FileBrowserState {
    private(set) var folder: URL?
    private(set) var files: [BrowserFile] = []
    private(set) var loading = false
    private(set) var error: String?
    var query = "" {
        didSet {
            guard query != oldValue else { return }
            selection = retainedSelection(selection, in: filtered.map(\.url))
        }
    }
    var selection: URL?
    var canOpenParent: Bool { folder.map { Self.canOpenParent(of: $0) } ?? false }
    static func canOpenParent(of folder: URL) -> Bool {
        let current = folder.standardizedFileURL
        return current.deletingLastPathComponent().standardizedFileURL != current
    }
    @ObservationIgnored private var request = UUID()
    var filtered: [BrowserFile] { files.filter { query.isEmpty || $0.url.lastPathComponent.localizedStandardContains(query) } }

    func load(_ folder: URL) async {
        let token = UUID(); request = token
        loading = true; error = nil
        do {
            let files = try await Task.detached {
                try FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: [.isDirectoryKey], options: [.skipsHiddenFiles])
                    .map { url in BrowserFile(url: url, directory: try url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true) }
                    .sorted { left, right in
                        if left.directory != right.directory { return left.directory }
                        return left.url.lastPathComponent.localizedStandardCompare(right.url.lastPathComponent) == .orderedAscending
                    }
            }.value
            guard request == token else { return }
            guard !Task.isCancelled else { loading = false; return }
            self.folder = folder; self.files = files; self.query = ""; selection = nil; loading = false
        } catch {
            guard request == token else { return }
            loading = false; self.error = error.localizedDescription
        }
    }
}
