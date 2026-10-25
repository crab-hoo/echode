import EchodeBridge
import Foundation
import Observation

/// Whole-file overviews of browser samples (ADR 0163). A selection asks for
/// its file at once, whether or not it is auditioned, and the result stays
/// after the sound stops. Recent files are kept so switching back shows them
/// immediately. Display data only: never saved and never part of Undo.
@MainActor @Observable
final class SampleOverviews {
    enum Entry: Equatable {
        case loading
        case ready(AuditionWaveform)
        case failed(String)
    }

    private(set) var entries: [URL: Entry] = [:]
    /// Ready overviews kept for switching back. A display cache size, not a
    /// limit on how many files the browser lists.
    @ObservationIgnored let keep: Int
    @ObservationIgnored private var recent: [URL] = []
    @ObservationIgnored private var stamps: [URL: FileStamp] = [:]
    /// The read each loading file waits for.
    @ObservationIgnored private var reading: [URL: UInt64] = [:]
    @ObservationIgnored private let read: @Sendable (URL, UInt64, UInt64) -> SampleOverview
    /// This browser's reads. A larger request stops older ones on the same
    /// channel only, so another window's browser keeps reading.
    @ObservationIgnored let channel: UInt64
    @ObservationIgnored private var lastRequest: UInt64 = 0
    private static var channels: UInt64 = 0

    init(keep: Int = 256, read: @escaping @Sendable (URL, UInt64, UInt64) -> SampleOverview = SampleOverview.read) {
        self.keep = max(1, keep)
        self.read = read
        Self.channels &+= 1
        channel = Self.channels
    }

    deinit { SampleOverview.end(channel: channel) }

    func entry(for url: URL) -> Entry? { entries[url] }

    /// Shows a kept overview at once, then reads again only when the file
    /// changed since. Reading happens off the main actor.
    func load(_ url: URL) async {
        if let request = reading[url], request == lastRequest { return }
        let stamp = await Task.detached { FileStamp(url) }.value
        if case .ready = entries[url], stamps[url] == stamp, stamp != nil {
            touch(url)
            return
        }
        if let request = reading[url], request == lastRequest { return }
        lastRequest &+= 1
        let request = lastRequest
        reading[url] = request
        if entries[url] == nil { entries[url] = .loading }
        let read = read, channel = channel
        let result = await Task.detached { read(url, channel, request) }.value
        switch result {
        case .waveform(let waveform):
            entries[url] = .ready(waveform)
            stamps[url] = stamp
            touch(url)
        case .failed(let message) where reading[url] == request:
            entries[url] = .failed(message)
        case .cancelled where reading[url] == request:
            // Another selection took over. Asking for this file again reads it.
            if case .loading = entries[url] { entries[url] = nil }
        default: break
        }
        if reading[url] == request { reading[url] = nil }
    }

    private func touch(_ url: URL) {
        recent.removeAll { $0 == url }
        recent.append(url)
        while recent.count > keep {
            let old = recent.removeFirst()
            entries[old] = nil
            stamps[old] = nil
        }
    }
}

/// Size and modification date: a changed file is read again.
struct FileStamp: Equatable, Sendable {
    let size: Int
    let modified: Date

    /// Reads the file system each time; `URL.resourceValues` would return
    /// the values cached on that URL.
    init?(_ url: URL) {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
              let size = (attributes[.size] as? NSNumber)?.intValue,
              let modified = attributes[.modificationDate] as? Date else { return nil }
        self.size = size
        self.modified = modified
    }
}
