import Foundation

struct PluginInsertionRequest: Sendable {
    let document: DocumentID
    let plugin: String
    let tracks: [UUID]
    /// A single add opens the plugin's window. Adding to every selected track does not.
    var opensWindow = false
}

extension ProjectController {
    /// Capture on the UI event, before scheduling asynchronous document work.
    func capturePluginInsertion(_ plugin: String, allSelected: Bool, document: DocumentID) -> PluginInsertionRequest? {
        guard acceptsInput(document), !plugin.isEmpty else { return nil }
        let targets = allSelected ? tracks.filter { selectedTracks.contains($0.id) }.map(\.id) : selection.map { [$0] } ?? []
        guard !targets.isEmpty else { return nil }
        return PluginInsertionRequest(document: document, plugin: plugin, tracks: targets, opensWindow: !allSelected)
    }

    @discardableResult
    func insertPlugin(_ request: PluginInsertionRequest) async -> Bool {
        if request.opensWindow, let track = request.tracks.first, request.tracks.count == 1 {
            return await addPluginOpeningWindow(request.plugin, on: track, documentID: request.document)
        }
        return await insertPlugin(request.plugin, on: request.tracks, documentID: request.document)
    }
}
