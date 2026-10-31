import EchodeBridge
import Foundation

/// An input gesture captures the session, track, and live tick before yielding.
/// Queued work must never retarget itself after a selection or document change.
struct AudioImportRequest {
    let url: URL
    let document: DocumentID
    let track: UUID
    let tick: Int64
}

extension WorkspaceSession {
    /// Called synchronously by double-click, Return, and the import button.
    @discardableResult
    func importSample(_ file: BrowserFile) -> Task<Bool, Never>? {
        guard file.audio, !state.browser.loading, !controller.busy,
              let request = controller.captureAudioImport(file.url) else { return nil }
        return Task { await controller.importAudio(request) }
    }
}

extension ProjectController {
    func captureAudioImport(_ url: URL, on track: UUID? = nil) -> AudioImportRequest? {
        guard let documentID, let target = track ?? selection,
              tracks.contains(where: { $0.id == target }) else { return nil }
        let tick = playback?.position.tick ?? audio.tick
        guard tick >= 0 else { return nil }
        return AudioImportRequest(url: url, document: documentID, track: target, tick: tick)
    }

    @discardableResult
    func importAudio(_ request: AudioImportRequest) async -> Bool {
        guard isCurrent(request.document), tracks.contains(where: { $0.id == request.track }), request.tick >= 0 else { return false }
        return await performActions({ [
            .importAudio(track: request.track.uuidString, start: request.tick, path: request.url.path),
        ] }, documentID: request.document)
    }
}
