import Foundation

struct PluginInsertionRequest: Sendable {
    let document: UUID
    let plugin: String
    let tracks: [UUID]
}

extension ProjectController {
    /// Capture on the UI event, before scheduling asynchronous document work.
    func capturePluginInsertion(_ plugin: String, allSelected: Bool, document: UUID) -> PluginInsertionRequest? {
        guard documentID == document, !busy, !plugin.isEmpty else { return nil }
        let targets = allSelected ? tracks.filter { selectedTracks.contains($0.id) }.map(\.id) : selection.map { [$0] } ?? []
        guard !targets.isEmpty else { return nil }
        return PluginInsertionRequest(document: document, plugin: plugin, tracks: targets)
    }

    @discardableResult
    func insertPlugin(_ request: PluginInsertionRequest) async -> Bool {
        await insertPlugin(request.plugin, on: request.tracks, documentID: request.document)
    }
}
