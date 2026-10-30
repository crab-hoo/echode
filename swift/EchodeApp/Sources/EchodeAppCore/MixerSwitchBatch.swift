import EchodeBridge
import Foundation

/// Transient event targets. A toggle resolves the anchor's latest state on its edit turn.
struct MixerSwitchBatch: Sendable {
    let document: UUID
    let tracks: [UUID]
    let anchor: UUID
    let field: MixerField
    let value: Float?
}

extension ProjectController {
    func captureMixerSwitch(_ track: UUID, field: MixerField, value: Float? = nil, document: UUID?) -> MixerSwitchBatch? {
        guard let document, documentID == document, !busy, field == .mute || field == .solo,
              value == nil || value == 0 || value == 1, mixer(track) != nil else { return nil }
        let targets = selectedTracks.contains(track) ? tracks.map(\.id).filter { selectedTracks.contains($0) } : [track]
        return MixerSwitchBatch(document: document, tracks: targets, anchor: track, field: field, value: value)
    }

    @discardableResult
    func enqueueMixerSwitch(_ request: MixerSwitchBatch) -> Task<Bool, Never>? {
        guard !busy, documentID == request.document else { return nil }
        let previous = mixerSwitchTail
        let task = Task { [self] in
            _ = await previous?.value
            guard documentID == request.document else { return false }
            if faderPreview != nil { await finishFader(commit: true) }
            return await applyMixerSwitch(request)
        }
        mixerSwitchTail = task
        return task
    }

    @discardableResult
    func applyMixerSwitch(_ request: MixerSwitchBatch) async -> Bool {
        await performActions({ [self] in
            guard request.field == .mute || request.field == .solo,
                  request.value == nil || request.value == 0 || request.value == 1,
                  !request.tracks.isEmpty, Set(request.tracks).count == request.tracks.count,
                  request.tracks.contains(request.anchor), let anchor = mixer(request.anchor) else { return nil }
            let value = request.value ?? (anchor[request.field].value == 0 ? 1 : 0)
            var actions: [EditAction] = []
            for track in request.tracks {
                guard let mix = mixer(track) else { return nil }
                if mix[request.field].value != value {
                    actions.append(.setParameter(parameter: mix[request.field].id.uuidString, value: value))
                }
            }
            return actions.isEmpty ? nil : actions
        }, documentID: request.document)
    }

    /// Switches, numeric entries and relative steps capture groups; live previews remain single-track.
    func arrangeMixActions(document: UUID?) -> MixActions {
        MixActions(begin: { [self] track, field in
            guard let document, documentID == document else { return false }
            return beginFader(track, field: field)
        }, change: { [self] value in
            guard documentID == document else { return }
            changeFader(value)
        }, finish: { [self] commit in
            guard documentID == document else { return }
            Task {
                guard documentID == document else { return }
                await finishFader(commit: commit)
            }
        }, set: { [self] track, field, value in
            guard let document, documentID == document else { return }
            if field == .mute || field == .solo {
                if let request = captureMixerSwitch(track, field: field, value: value, document: document) { enqueueMixerSwitch(request) }
            } else {
                Task {
                    guard documentID == document else { return }
                    if faderPreview != nil { await finishFader(commit: true) }
                    await setMixer(track, field: field, value: value, documentID: document)
                }
            }
        }, toggle: { [self] track, field in
            if let request = captureMixerSwitch(track, field: field, document: document) { enqueueMixerSwitch(request) }
        }, enter: { [self] track, field, value in
            if let request = captureMixerValue(track, field: field, value: value, document: document) { enqueueMixerValue(request) }
        }, adjust: { [self] track, field, direction in
            if let request = captureMixerStep(track, field: field, direction: direction, document: document) { enqueueMixerValue(request) }
        })
    }
}
