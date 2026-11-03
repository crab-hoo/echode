import EchodeBridge
import Foundation

struct SampleAuditionPreferences: Codable {
    var automatic = true
    var gain: Float = 1
    var isValid: Bool { gain.isFinite && (0...1).contains(gain) }
}

/// The browser plays a selected sample once, over the project and without its
/// FX or faders (ADR 0135). A newer selection replaces the sound, which fades
/// out; selecting something that is not audio stops it. Only the user's own
/// click or key changes the sound (ADR 0143).
extension WorkspaceSession {
    /// The user selected `url` in the browser's list, or cleared it. A
    /// selection the browser moves itself after a search or rescan does not
    /// come here and stays silent. Returns the file that was requested.
    @discardableResult
    func chooseInBrowser(_ url: URL?, origin: AuditionOrigin) -> URL? {
        guard state.browser.selection != url else { return nil }
        state.browser.selection = url
        guard state.auditionPreferences.automatic else {
            controller.audition(nil, origin: origin)
            return nil
        }
        return auditionSelection(url, origin: origin)
    }

    /// Follows a new browser selection. Returns the file that was requested,
    /// or nil when the audition was stopped or no document is open.
    @discardableResult
    func auditionSelection(_ url: URL?, origin: AuditionOrigin = .other) -> URL? {
        let file = url.flatMap { url in state.browser.filtered.first { $0.url == url } }
        let sample = file.flatMap { $0.audio ? $0.url : nil }
        return controller.audition(sample, origin: origin) ? sample : nil
    }

    /// Manual audition always works, including when automatic audition is off.
    @discardableResult
    func auditionSelected(origin: AuditionOrigin = .other, from frame: UInt64 = 0, expected: URL? = nil) -> URL? {
        guard expected == nil || expected == state.browser.selection else { return nil }
        guard let selected = state.browser.filtered.first(where: { $0.url == state.browser.selection }), selected.audio else { return nil }
        return controller.audition(selected.url, from: frame, origin: origin) ? selected.url : nil
    }

    func setAutomaticAudition(_ enabled: Bool) {
        guard state.auditionPreferences.automatic != enabled else { return }
        state.auditionPreferences.automatic = enabled
        state.saveAuditionPreferences()
        if !enabled { controller.audition(nil) }
    }

    func setAuditionGain(_ gain: Float) {
        guard gain.isFinite, (0...1).contains(gain) else { return }
        state.auditionPreferences.gain = gain
        state.saveAuditionPreferences()
        synchronizeAuditionOutput()
    }

    /// Called by workspace visibility and document changes, never by audio polling.
    func synchronizeAuditionOutput(visible: Bool? = nil) {
        guard let document = controller.document else { return }
        controller.sendAuditionSettings(document, visible ?? state.layout.browser, state.auditionPreferences.gain)
    }
}

extension ProjectController {
    /// Plays `url` once, or fades the audition out for nil. The request goes
    /// straight to the document's audition thread, so a queued edit or a
    /// plugin reply does not delay it. False when no document is open.
    @discardableResult
    func audition(_ url: URL?, from frame: UInt64 = 0, origin: AuditionOrigin = .other) -> Bool {
        guard let document else { return false }
        if let url, frame > 0 { sendAuditionAt(document, url, frame, origin) }
        else { sendAudition(document, url, origin) }
        return true
    }
}

/// Writes each audition request once its startup stages settle or a newer
/// request replaces it, so `make debug-report` shows where a sound waited after the
/// snapshot's short history has moved on. Nothing is written until the app
/// chooses a file (ADR 0143).
@MainActor
final class AuditionLog {
    private(set) var logged: UInt64 = 0
    /// Receives each settled entry. Tests read it instead of a file.
    var write: (AuditionEntry) -> Void = { _ in }

    func record(_ status: AuditionStatus?) {
        guard let status else { logged = 0; return }
        // Another document's audition numbers its requests from 1 again.
        if let last = status.history.last?.request, last < logged { logged = 0 }
        for entry in status.history where entry.request > logged {
            // Only the newest request still gains stages; an older one waits no longer.
            guard entry.settled || entry.request != status.request else { break }
            write(entry)
            logged = entry.request
        }
    }

    /// Appends each entry to `file` as one JSON line.
    func keep(in file: LineLog) {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        write = { entry in
            guard var line = try? encoder.encode(entry) else { return }
            line.append(0x0A)
            file.append(line)
        }
    }
}

/// Appends lines to a file on a background queue. Past `limit` bytes the
/// file becomes `<name>.1.<extension>` and a new one starts. The limit bounds
/// this diagnostic record only.
final class LineLog: Sendable {
    static var auditions: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/Echode/audition.ndjson")
    }
    static var panics: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/Echode/panic.ndjson")
    }
    let url: URL
    let limit: Int
    private let queue = DispatchQueue(label: "io.echode.line-log", qos: .utility)

    init(url: URL, limit: Int = 1 << 20) { self.url = url; self.limit = limit }

    var previous: URL { url.deletingPathExtension().appendingPathExtension("1").appendingPathExtension(url.pathExtension) }

    func append(_ line: Data) {
        queue.async { [url, limit, previous] in
            let manager = FileManager.default
            try? manager.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            let size = (try? manager.attributesOfItem(atPath: url.path)[.size] as? Int) ?? nil
            if let size, size > 0, size + line.count > limit {
                try? manager.removeItem(at: previous)
                try? manager.moveItem(at: url, to: previous)
            }
            if let handle = try? FileHandle(forWritingTo: url) {
                defer { try? handle.close() }
                _ = try? handle.seekToEnd()
                try? handle.write(contentsOf: line)
            } else {
                try? line.write(to: url)
            }
        }
    }

    /// Waits for every line appended so far.
    func flush() { queue.sync {} }
}

extension ProjectController {
    /// Keeps a record of audition requests in `file`. The app calls this;
    /// tests and tools that never call it write nothing.
    public func keepAuditionLog(at file: URL? = nil) {
        auditionLog.keep(in: LineLog(url: file ?? LineLog.auditions))
    }

    /// Lets the Rust core append its panics beside the audition log
    /// (ADR 0238). Once per process; tests and tools that never call it
    /// write nothing.
    public static func keepPanicLog(at file: URL? = nil) {
        PanicLog.keep(at: file ?? LineLog.panics)
    }
}
