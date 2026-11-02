import EchodeBridge
import Foundation

extension ProjectController {
    /// Capture before a drag starts, not after selection or document publication changes.
    func captureFaderActions(_ actions: inout MixActions, track: UUID, field: MixerField, document: DocumentID?) {
        actions.capture = nil
        guard let request = captureMixerValue(track, field: field, value: MixScale.defaultValue(field), document: document) else {
            actions.begin = { _, _ in false }
            return
        }
        var members = Dictionary(uniqueKeysWithValues: request.tracks.compactMap { id -> (UUID, FaderMember)? in
            guard let mix = mixer(id) else { return nil }
            // A different press settles the preceding reset/drag. Capture its shown result,
            // then require that exact result before rebasing the new gesture's originals.
            let shown = session.faderPreview?.values(on: id)
            return (id, FaderMember(original: mix, gain: shown?.gain ?? mix.gain.value, pan: shown?.pan ?? mix.pan.value))
        })
        actions.begin = { [self] id, requestedField in
            guard id == track, requestedField == field, members.count == request.tracks.count,
                  members.allSatisfy({ id, member in
                      guard let current = mixer(id) else { return false }
                      return current.gain.id == member.original.gain.id && current.pan.id == member.original.pan.id
                          && current.mute == member.original.mute && current.solo == member.original.solo
                          && current.gain.value == member.gain && current.pan.value == member.pan
                  }), beginFader(track, field: field, documentID: request.document) else { return false }
            for id in request.tracks {
                if let mix = mixer(id) { members[id] = FaderMember(original: mix, gain: mix.gain.value, pan: mix.pan.value) }
            }
            if members.count > 1 { session.faderPreview?.members = members }
            return true
        }
        actions.dragged = { [self] fraction in
            guard fraction.isFinite, let original = members[track]?.original[field].value else { return }
            let value: Float
            if field == .gain, members.count > 1, original > 0, fraction != 0 {
                // Group travel is a finite dB offset even below the display floor.
                let scaled = Double(original) * pow(10, fraction * (60 + MixScale.maximumGainDecibels) / 20)
                value = Float(min(Double(ModelLimits.gain.upperBound), max(Double(Float.leastNonzeroMagnitude), scaled)))
            } else { value = MixScale.dragged(field, from: original, by: fraction) }
            changeFader(value, documentID: request.document)
        }
    }

    func updateFaderMembers(_ preview: inout FaderPreview, value: Float) -> Bool {
        guard !preview.members.isEmpty else { return true }
        let tracks = Array(preview.members.keys)
        let originals = tracks.compactMap { preview.members[$0]?.original[preview.field].value }
        let request = MixerValueBatch(document: preview.document, tracks: tracks, anchor: preview.track, field: preview.field, value: value)
        guard let values = request.values(from: originals) else {
            errorMessage = text("Select a non-silent track to adjust the group's relative volume, or select this track alone.")
            return false
        }
        for (track, value) in zip(tracks, values) {
            if preview.field == .gain { preview.members[track]?.gain = value }
            else { preview.members[track]?.pan = value }
        }
        guard let anchor = preview.members[preview.track] else { return false }
        preview.gain = anchor.gain; preview.pan = anchor.pan
        return true
    }

    /// Validate the complete captured set inside the edit turn; never partially commit.
    func faderActions(_ preview: FaderPreview) -> [EditAction]? {
        if preview.members.isEmpty {
            guard let recorded = mixer(preview.track)?[preview.field], recorded.id == preview.parameter,
                  recorded.value != preview.value else { return nil }
            return [.setParameter(parameter: preview.parameter.uuidString, value: preview.value)]
        }
        var actions: [EditAction] = []
        for (track, member) in preview.members {
            guard let current = mixer(track), current == member.original else { return nil }
            let value = preview.field == .gain ? member.gain : member.pan
            if current[preview.field].value != value {
                actions.append(.setParameter(parameter: current[preview.field].id.uuidString, value: value))
            }
        }
        return actions.isEmpty ? nil : actions
    }
}
