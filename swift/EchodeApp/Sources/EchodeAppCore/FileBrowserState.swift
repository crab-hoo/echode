import Foundation
import Observation

struct BrowserFile: Identifiable, Sendable {
    let url: URL
    let directory: Bool
    var id: URL { url }
    var project: Bool { url.pathExtension.lowercased() == "echode" }
    var audio: Bool {
        !directory && ["wav", "aiff", "aif", "caf", "mp3", "m4a", "aac", "flac"].contains(url.pathExtension.lowercased())
    }
    var icon: String { project || audio ? "waveform" : directory ? "folder" : "doc" }
}

@MainActor @Observable
final class FileBrowserState {
    private(set) var folder: URL?
    private(set) var files: [BrowserFile] = []
    private(set) var loading = false
    private(set) var error: String?
    private(set) var roots: [URL]
    private(set) var libraryFiles: [BrowserFile] = []
    private(set) var indexing = false
    private(set) var libraryError: String?
    var query = "" {
        didSet {
            guard query != oldValue else { return }
            selection = preservedSelection(selection, in: filtered.map(\.url))
        }
    }
    var selection: URL?
    var canOpenParent: Bool { folder.map { Self.canOpenParent(of: $0) } ?? false }
    static func canOpenParent(of folder: URL) -> Bool {
        let current = folder.standardizedFileURL
        return current.deletingLastPathComponent().standardizedFileURL != current
    }
    @ObservationIgnored private var request = UUID()
    @ObservationIgnored private var libraryRequest = UUID()
    @ObservationIgnored private var libraryTask: Task<SampleLibraryScan, Error>?
    @ObservationIgnored private let defaults: UserDefaults?
    private static let rootsKey = "workspace.sample-folders.v1"
    var searchingLibrary: Bool { !query.isEmpty && !roots.isEmpty }
    var filtered: [BrowserFile] {
        (searchingLibrary ? libraryFiles : files).filter { query.isEmpty || $0.url.lastPathComponent.localizedStandardContains(query) }
    }

    init(defaults: UserDefaults? = nil) {
        self.defaults = defaults
        var seen = Set<URL>()
        roots = (defaults?.stringArray(forKey: Self.rootsKey) ?? []).compactMap {
            let url = URL(fileURLWithPath: $0, isDirectory: true).standardizedFileURL
            return seen.insert(url).inserted ? url : nil
        }
    }

    func register(_ folder: URL) async {
        let resolved = Self.rootURL(folder)
        guard await load(resolved) else { return }
        if !roots.contains(resolved) { roots.append(resolved); saveRoots() }
        await refreshLibrary()
    }

    func removeRoot(_ folder: URL) async {
        let resolved = Self.rootURL(folder)
        roots.removeAll { $0 == resolved }; saveRoots()
        libraryFiles = []
        selection = preservedSelection(selection, in: filtered.map(\.url))
        await refreshLibrary()
    }

    func prepare() async {
        if folder == nil, let root = roots.first { await load(root) }
        await refreshLibrary()
    }

    func refresh() async {
        let query = query
        if let folder, await load(folder) { self.query = query }
        await refreshLibrary()
    }

    func refreshLibrary() async {
        let token = UUID(); libraryRequest = token
        libraryTask?.cancel()
        let roots = roots
        let task = Task.detached { try SampleLibraryScan.read(roots) }
        libraryTask = task; indexing = true; libraryError = nil
        do {
            let scan = try await withTaskCancellationHandler { try await task.value } onCancel: { task.cancel() }
            guard libraryRequest == token else { return }
            libraryFiles = scan.files
            libraryError = scan.failures.isEmpty ? nil : scan.failures.joined(separator: "\n")
            selection = preservedSelection(selection, in: filtered.map(\.url))
        } catch {
            guard libraryRequest == token else { return }
            if !(error is CancellationError) { libraryError = error.localizedDescription }
        }
        indexing = false; libraryTask = nil
    }

    private func saveRoots() { defaults?.set(roots.map(\.path), forKey: Self.rootsKey) }
    private static func rootURL(_ url: URL) -> URL {
        URL(fileURLWithPath: url.resolvingSymlinksInPath().path, isDirectory: true).standardizedFileURL
    }

    @discardableResult
    func load(_ folder: URL) async -> Bool {
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
            guard request == token else { return false }
            guard !Task.isCancelled else { loading = false; return false }
            self.folder = folder; self.files = files; self.query = ""; selection = nil; loading = false
            return true
        } catch {
            guard request == token else { return false }
            loading = false; self.error = error.localizedDescription
            return false
        }
    }
}
