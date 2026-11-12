import EchodeBridge
import Foundation

/// Transient event targets. A toggle resolves the anchor's latest state on its edit turn.
struct MixerSwitchBatch: Sendable {
    let document: DocumentID
    let tracks: [UUID]
    let anchor: UUID
    let field: MixerField
    let value: Float?
}

extension ProjectController {
    func captureMixerSwitch(_ track: UUID, field: MixerField, value: Float? = nil, document: DocumentID?)
        -> MixerSwitchBatch?
    {
        guard let document, acceptsInput(document), field == .mute || field == .solo,
            value == nil || value == 0 || value == 1, mixer(track) != nil
        else { return nil }
        let targets = selectedTracks.contains(track) ? tracks.map(\.id).filter { selectedTracks.contains($0) } : [track]
        return MixerSwitchBatch(document: document, tracks: targets, anchor: track, field: field, value: value)
    }

    @discardableResult
    func enqueueMixerSwitch(_ request: MixerSwitchBatch) -> Task<Bool, Never>? {
        guard let session = inputSession(for: request.document) else { return nil }
        let previous = session.mixerSwitchTail
        let task = Task { [self] in
            _ = await previous?.value
            guard isCurrent(request.document) else { return false }
            if session.faderPreview != nil { await finishFader(commit: true, in: session) }
            return await applyMixerSwitch(request)
        }
        session.mixerSwitchTail = task
        return task
    }

    @discardableResult
    func applyMixerSwitch(_ request: MixerSwitchBatch) async -> Bool {
        await performActions(
            { [self] in
                guard request.field == .mute || request.field == .solo,
                    request.value == nil || request.value == 0 || request.value == 1,
                    !request.tracks.isEmpty, Set(request.tracks).count == request.tracks.count,
                    request.tracks.contains(request.anchor), let anchor = mixer(request.anchor)
                else { return nil }
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

    /// Discrete edits and live gestures share target capture and relative group limits.
    func arrangeMixActions(document: DocumentID?) -> MixActions {
        var actions = MixActions(
            begin: { [self] track, field in
                guard let document else { return false }
                return beginFader(track, field: field, documentID: document)
            },
            change: { [self] value in
                guard let document else { return }
                changeFader(value, documentID: document)
            },
            finish: { [self] commit in
                guard let document, isCurrent(document) else { return }
                Task { await finishFader(commit: commit, documentID: document) }
            },
            set: { [self] track, field, value in
                guard let document, isCurrent(document) else { return }
                if field == .mute || field == .solo {
                    if let request = captureMixerSwitch(track, field: field, value: value, document: document) {
                        enqueueMixerSwitch(request)
                    }
                } else {
                    Task {
                        guard let session = session(for: document) else { return }
                        if session.faderPreview != nil { await finishFader(commit: true, in: session) }
                        await setMixer(track, field: field, value: value, documentID: document)
                    }
                }
            },
            toggle: { [self] track, field in
                if let request = captureMixerSwitch(track, field: field, document: document) {
                    enqueueMixerSwitch(request)
                }
            },
            enter: { [self] track, field, value in
                if let request = captureMixerValue(track, field: field, value: value, document: document) {
                    enqueueMixerValue(request)
                }
            },
            adjust: { [self] track, field, direction in
                if let request = captureMixerStep(track, field: field, direction: direction, document: document) {
                    enqueueMixerValue(request)
                }
            })
        actions.capture = { [self] actions, track, field in
            captureFaderActions(&actions, track: track, field: field, document: document)
        }
        return actions
    }
}
