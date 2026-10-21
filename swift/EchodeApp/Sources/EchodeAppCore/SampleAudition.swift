import EchodeBridge
import Foundation

/// The browser plays a selected sample once, over the project and without its
/// FX or faders (ADR 0135). A newer selection replaces the sound, which fades
/// out; selecting something that is not audio stops it.
extension WorkspaceSession {
    /// Follows a new browser selection. Returns the file that was requested,
    /// or nil when the audition was stopped or no document is open.
    @discardableResult
    func auditionSelection(_ url: URL?) -> URL? {
        let file = url.flatMap { url in state.browser.filtered.first { $0.url == url } }
        let sample = file.flatMap { $0.audio ? $0.url : nil }
        return controller.audition(sample) ? sample : nil
    }
}

extension ProjectController {
    /// Plays `url` once, or fades the audition out for nil. The request goes
    /// straight to the document's audition thread, so a queued edit or a
    /// plugin reply does not delay it. False when no document is open.
    @discardableResult
    func audition(_ url: URL?) -> Bool {
        guard let document else { return false }
        sendAudition(document, url)
        return true
    }
}
