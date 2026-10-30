import EchodeBridge
import Foundation

/// Discrete entries and steps capture IDs on submission, then resolve values on their edit turn.
struct MixerValueBatch: Sendable {
    let document: UUID
    let tracks: [UUID]
    let anchor: UUID
    let field: MixerField
    let value: Float
    /// When present, resolve one relative step instead of the numeric value.
    var direction: Double? = nil

    /// All members share a multiplier (a dB difference) or a pan difference.
    /// Clamp the difference, not each result, to preserve the group's balance.
    func values(from originals: [Float]) -> [Float]? {
        guard !tracks.isEmpty, tracks.count == originals.count, Set(tracks).count == tracks.count,
              let index = tracks.firstIndex(of: anchor), value.isFinite,
              field == .gain || field == .pan else { return nil }
        let range: ClosedRange<Float> = field == .gain ? ModelLimits.gain : ModelLimits.pan
        guard range.contains(value), originals.allSatisfy({ $0.isFinite && range.contains($0) }) else { return nil }
        let basis = originals[index]
        let value: Float
        if let direction {
            guard direction == -1 || direction == 1 else { return nil }
            if tracks.count == 1 { return [MixScale.adjusted(field, value: basis, direction: direction)] }
            if field == .gain {
                // Group steps remain finite dB offsets, including below the single-slider floor.
                guard basis > 0 else { return direction < 0 ? originals : nil }
                value = Float(min(Double(ModelLimits.gain.upperBound), max(Double(Float.leastNonzeroMagnitude), Double(basis) * pow(10, direction / 20))))
            } else {
                value = MixScale.adjusted(.pan, value: basis, direction: direction)
            }
        } else { value = self.value }
        if tracks.count == 1 { return [value] }
        if basis == value { return originals }
        if field == .pan {
            let lower = Double(originals.min()!), upper = Double(originals.max()!)
            let delta = min(Double(ModelLimits.pan.upperBound) - upper, max(Double(ModelLimits.pan.lowerBound) - lower, Double(value) - Double(basis)))
            return originals.map { Float(Double($0) + delta) }
        }
        // Silence is an explicit endpoint, not a finite dB offset. Silent members
        // stay silent under finite changes; a silent anchor has no usable ratio.
        if value == 0 { return originals.map { _ in 0 } }
        guard basis > 0 else { return nil }
        let positive = originals.filter { $0 > 0 }
        let lower = Double(Float.leastNonzeroMagnitude) / Double(positive.min()!)
        let upper = Double(ModelLimits.gain.upperBound) / Double(positive.max()!)
        let factor = min(upper, max(lower, Double(value) / Double(basis)))
        return originals.map { Float(Double($0) * factor) }
    }
}

extension ProjectController {
    func captureMixerValue(_ track: UUID, field: MixerField, value: Float, document: UUID?) -> MixerValueBatch? {
        guard let document, documentID == document, !busy, mixer(track) != nil,
              value.isFinite, field == .gain || field == .pan,
              (field == .gain ? ModelLimits.gain.contains(value) : ModelLimits.pan.contains(value)) else { return nil }
        let targets = selectedTracks.contains(track) ? tracks.map(\.id).filter { selectedTracks.contains($0) } : [track]
        return MixerValueBatch(document: document, tracks: targets, anchor: track, field: field, value: value)
    }

    func captureMixerStep(_ track: UUID, field: MixerField, direction: Double, document: UUID?) -> MixerValueBatch? {
        guard direction == -1 || direction == 1,
              var request = captureMixerValue(track, field: field, value: 0, document: document) else { return nil }
        request.direction = direction
        return request
    }

    @discardableResult
    func enqueueMixerValue(_ request: MixerValueBatch) -> Task<Bool, Never>? {
        guard !busy, documentID == request.document else { return nil }
        // Discrete numeric entries, steps and switches share one event-order queue.
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
                if request.field == .gain, (request.direction == 1 || request.direction == nil && request.value > 0), request.tracks.count > 1,
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
