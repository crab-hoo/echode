import Foundation

struct TrackInsertionRequest: Sendable {
    let document: DocumentID
    let after: UUID?
    let name: String
    var intoFolder = true
}

extension ProjectController {
    /// Hold the document and focus from the UI event through asynchronous work.
    func captureTrackInsertion(atEnd: Bool, collapsedTracks: Set<UUID> = []) -> TrackInsertionRequest? {
        guard let documentID, !busy else { return nil }
        return TrackInsertionRequest(
            document: documentID, after: atEnd ? nil : selection,
            name: "\(text("Track")) \(tracks.count + 1)",
            intoFolder: selection.map { !collapsedTracks.contains($0) } ?? true)
    }
}

extension WorkspaceSession {
    @discardableResult
    func addTrack(atEnd: Bool = false) -> Task<Void, Never>? {
        guard enabled(.addTrack),
            let request = controller.captureTrackInsertion(atEnd: atEnd, collapsedTracks: state.collapsedTracks)
        else { return nil }
        return Task { _ = await controller.insertTrack(request) }
    }
}
