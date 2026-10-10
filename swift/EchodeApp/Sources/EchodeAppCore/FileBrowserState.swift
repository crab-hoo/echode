import Foundation
import Observation

struct BrowserFile: Identifiable, Sendable {
    let url: URL
    let directory: Bool
    var id: URL { url }
    var project: Bool { url.pathExtension.lowercased() == "echode" }
    var icon: String { project ? "waveform" : directory ? "folder" : "doc" }
}

@MainActor @Observable
final class FileBrowserState {
    private(set) var folder: URL?
    private(set) var files: [BrowserFile] = []
    private(set) var loading = false
    private(set) var error: String?
    var query = ""
    var selection: URL?
    @ObservationIgnored private var request = UUID()
    var filtered: [BrowserFile] { files.filter { query.isEmpty || $0.url.lastPathComponent.localizedStandardContains(query) } }

    func load(_ folder: URL) async {
        let token = UUID(); request = token
        loading = true; error = nil
        do {
            let files = try await Task.detached {
                try FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: [.isDirectoryKey], options: [.skipsHiddenFiles])
                    .map { url in BrowserFile(url: url, directory: try url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true) }
                    .sorted { $0.url.lastPathComponent.localizedStandardCompare($1.url.lastPathComponent) == .orderedAscending }
            }.value
            guard request == token else { return }
            guard !Task.isCancelled else { loading = false; return }
            self.folder = folder; self.files = files; selection = nil; query = ""; loading = false
        } catch {
            guard request == token else { return }
            loading = false; self.error = error.localizedDescription
        }
    }
}
