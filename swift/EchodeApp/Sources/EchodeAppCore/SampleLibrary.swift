import Foundation

struct SampleLibraryScan: Sendable {
    let files: [BrowserFile]
    let failures: [String]

    /// Runs off MainActor. Packages and symbolic links are not sample roots to
    /// traverse; an explicitly registered root has already been resolved.
    static func read(_ roots: [URL]) throws -> Self {
        let manager = FileManager.default
        let keys: Set<URLResourceKey> = [.isDirectoryKey, .isSymbolicLinkKey, .isRegularFileKey, .isPackageKey]
        var pending = roots
        var visited = Set<URL>()
        var files = [URL: BrowserFile]()
        var failures = [String]()
        while let directory = pending.popLast() {
            try Task.checkCancellation()
            guard visited.insert(directory).inserted else { continue }
            do {
                // Recheck each directory: a folder can have been replaced by a
                // symlink since its parent was enumerated.
                let values = try directory.resourceValues(forKeys: keys)
                guard values.isDirectory == true, values.isSymbolicLink != true,
                    values.isPackage != true, directory.pathExtension.lowercased() != "echode"
                else { continue }
                for url in try manager.contentsOfDirectory(
                    at: directory, includingPropertiesForKeys: Array(keys), options: [.skipsHiddenFiles])
                {
                    try Task.checkCancellation()
                    let values = try url.resourceValues(forKeys: keys)
                    guard values.isSymbolicLink != true else { continue }
                    // Directory enumeration may return /private/var for a
                    // registered /var URL. Use one spelling for overlap checks.
                    let normalized = url.resolvingSymlinksInPath().standardizedFileURL
                    let file = BrowserFile(
                        url: normalized, directory: values.isDirectory == true, package: values.isPackage == true)
                    if file.project {
                        files[normalized] = file
                    } else if values.isDirectory == true {
                        if values.isPackage != true { pending.append(normalized) }
                    } else if values.isRegularFile == true, file.visible {
                        files[normalized] = file
                    }
                }
            } catch is CancellationError { throw CancellationError() } catch {
                failures.append("\(directory.path): \(error.localizedDescription)")
            }
        }
        return Self(
            files: files.values.sorted {
                let order = $0.url.lastPathComponent.localizedStandardCompare($1.url.lastPathComponent)
                return order == .orderedSame ? $0.url.path < $1.url.path : order == .orderedAscending
            }, failures: failures)
    }
}
