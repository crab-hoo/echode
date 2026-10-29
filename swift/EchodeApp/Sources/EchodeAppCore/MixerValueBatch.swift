import EchodeBridge
import Foundation

/// Numeric entries capture IDs on submission, then resolve values on their edit turn.
struct MixerValueBatch: Sendable {
    let document: UUID
    let tracks: [UUID]
    let anchor: UUID
    let field: MixerField
    let value: Float

    /// All members share a multiplier (a dB difference) or a pan difference.
    /// Clamp the difference, not each result, to preserve the group's balance.
    func values(from originals: [Float]) -> [Float]? {
        guard !tracks.isEmpty, tracks.count == originals.count, Set(tracks).count == tracks.count,
              let index = tracks.firstIndex(of: anchor), value.isFinite,
              field == .gain || field == .pan else { return nil }
        let range: ClosedRange<Float> = field == .gain ? 0...4 : -1...1
        guard range.contains(value), originals.allSatisfy({ $0.isFinite && range.contains($0) }) else { return nil }
        if tracks.count == 1 { return [value] }
        let basis = originals[index]
        if basis == value { return originals }
        if field == .pan {
            let lower = Double(originals.min()!), upper = Double(originals.max()!)
            let delta = min(1 - upper, max(-1 - lower, Double(value) - Double(basis)))
            return originals.map { Float(Double($0) + delta) }
        }
        // Silence is an explicit endpoint, not a finite dB offset. Silent members
        // stay silent under finite changes; a silent anchor has no usable ratio.
        if value == 0 { return originals.map { _ in 0 } }
        guard basis > 0 else { return nil }
        let positive = originals.filter { $0 > 0 }
        let lower = Double(Float.leastNonzeroMagnitude) / Double(positive.min()!)
        let upper = 4 / Double(positive.max()!)
        let factor = min(upper, max(lower, Double(value) / Double(basis)))
        return originals.map { Float(Double($0) * factor) }
    }
}

extension ProjectController {
    func captureMixerValue(_ track: UUID, field: MixerField, value: Float, document: UUID?) -> MixerValueBatch? {
        guard let document, documentID == document, !busy, mixer(track) != nil,
              value.isFinite, field == .gain || field == .pan,
              (field == .gain ? (0...4).contains(value) : (-1...1).contains(value)) else { return nil }
        let targets = selectedTracks.contains(track) ? tracks.map(\.id).filter { selectedTracks.contains($0) } : [track]
        return MixerValueBatch(document: document, tracks: targets, anchor: track, field: field, value: value)
    }

    @discardableResult
    func enqueueMixerValue(_ request: MixerValueBatch) -> Task<Bool, Never>? {
        guard !busy, documentID == request.document else { return nil }
        // Discrete numeric entries and switches share one event-order queue.
        let previous = mixerSwitchTail
        let task = Task { [self] in
            _ = await previous?.value
            guard documentID == request.document else { return false }
            if faderPreview != nil { await finishFader(commit: true) }
            return await applyMixerValue(request)
        }
        mixerSwitchTail = task
        return task
    }

    @discardableResult
    func applyMixerValue(_ request: MixerValueBatch) async -> Bool {
        await editTransaction({ [self] in
            let mixes = request.tracks.compactMap { mixer($0) }
            guard mixes.count == request.tracks.count else { return nil }
            let originals = mixes.map { $0[request.field].value }
            guard let values = request.values(from: originals) else {
                if request.field == .gain, request.value > 0, request.tracks.count > 1,
                   let index = request.tracks.firstIndex(of: request.anchor), originals[index] == 0 {
                    errorMessage = text("Select a non-silent track to adjust the group's relative volume, or select this track alone.")
                }
                return nil
            }
            let actions: [[String: Any]] = zip(mixes, values).compactMap { mix, value in
                guard mix[request.field].value != value else { return nil }
                return ["type": "param.set", "parameter": mix[request.field].id.uuidString, "value": value]
            }
            return actions.isEmpty ? nil : actions
        }, documentID: request.document)
    }
}
