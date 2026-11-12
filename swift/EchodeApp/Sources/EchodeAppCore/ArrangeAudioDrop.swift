import AppKit
import EchodeBridge

/// One file gesture, captured before any asynchronous document work starts.
public struct AudioFileDrop: Sendable {
    let document: DocumentID
    let url: URL
    let track: UUID?  // Nil appends a top-level track, in the same undo group.
    let tick: Int64

    static func audioURL(from board: NSPasteboard) -> URL? {
        guard
            let urls = board.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL],
            urls.count == 1, let url = urls.first, url.isFileURL,
            BrowserFile(url: url, directory: false).audio
        else { return nil }
        return url
    }
}

extension ArrangeView {
    func audioDrop(_ board: NSPasteboard, at point: CGPoint, flags: NSEvent.ModifierFlags, document: DocumentID)
        -> AudioFileDrop?
    {
        let input = renderInput
        let origin = input.header.map { $0.width + 1 } ?? 0
        guard audioDocument == document, onDropAudio != nil, let snapshot = input.snapshot,
            point.x.isFinite, point.y.isFinite, bounds.contains(point), point.x >= origin, point.y >= input.arrangeTop,
            let url = AudioFileDrop.audioURL(from: board)
        else { return nil }
        let beat = input.camera.originBeat + (point.x - origin) / input.camera.pixelsPerBeat
        // The arrange grid currently draws quarter-note lines. Shift preserves
        // the pointer's tick rather than snapping to those visible lines.
        guard let tick = Tick.ticks(flags.contains(.shift) ? beat : beat.rounded()), tick >= 0 else { return nil }
        let rows = input.camera.rows(trackCount: snapshot.trackCount, height: input.arrangeHeight)
        let track = rows.index(at: point.y - input.arrangeTop).flatMap { snapshot.trackID(at: $0) }
        return AudioFileDrop(document: document, url: url, track: track, tick: tick)
    }

    @discardableResult
    func acceptAudio(_ board: NSPasteboard, at point: CGPoint, flags: NSEvent.ModifierFlags, document: DocumentID)
        -> Bool
    {
        guard let drop = audioDrop(board, at: point, flags: flags, document: document) else { return false }
        return onDropAudio?(drop) ?? false
    }
}

extension WorkspaceSession {
    @discardableResult
    func importDroppedAudio(_ drop: AudioFileDrop) -> Task<Bool, Never>? {
        guard controller.acceptsInput(drop.document) else { return nil }
        return Task { await controller.importDroppedAudio(drop) }
    }
}

extension ProjectController {
    @discardableResult
    func importDroppedAudio(_ drop: AudioFileDrop) async -> Bool {
        guard drop.url.isFileURL, BrowserFile(url: drop.url, directory: false).audio, drop.tick >= 0 else {
            return false
        }
        if let track = drop.track {
            return await importAudio(
                AudioImportRequest(url: drop.url, document: drop.document, track: track, tick: drop.tick))
        }
        return await performActions(
            referencing: [
                .addTrack(name: .value(drop.url.deletingPathExtension().lastPathComponent)),
                .importAudio(track: .result(action: 0, field: "id"), start: drop.tick, path: .value(drop.url.path)),
            ], documentID: drop.document, selectingAddedTrack: true)
    }
}
