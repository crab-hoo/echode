import Foundation

struct TrackDuplicationRequest: Sendable {
    let document: DocumentID
    /// Selected tracks without a selected ancestor, in arrange order.
    let sources: [UUID]
}

extension ProjectController {
    /// Capture on the UI event. A selected descendant is already copied with
    /// its selected ancestor, so it is not copied a second time.
    func captureTrackDuplication() -> TrackDuplicationRequest? {
        guard let documentID, !busy, let snapshot else { return nil }
        var parents: [UUID: UUID] = [:]
        for row in 0..<snapshot.trackCount {
            if let id = snapshot.trackID(at: row), let parent = snapshot.trackHierarchy(at: row)?.parent { parents[id] = parent }
        }
        let sources = tracks.map(\.id).filter { id in
            guard selectedTracks.contains(id) else { return false }
            var ancestor = parents[id]
            while let current = ancestor {
                if selectedTracks.contains(current) { return false }
                ancestor = parents[current]
            }
            return true
        }
        guard !sources.isEmpty else { return nil }
        return TrackDuplicationRequest(document: documentID, sources: sources)
    }
}

extension WorkspaceSession {
    @discardableResult
    func duplicateTracks() -> Task<Void, Never>? {
        guard enabled(.duplicateTrack), let request = controller.captureTrackDuplication() else { return nil }
        return Task { _ = await controller.duplicateTracks(request) }
    }
}
