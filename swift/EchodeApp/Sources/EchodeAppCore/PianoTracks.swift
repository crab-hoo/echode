import EchodeBridge
import Foundation

/// Captured by native controls before an asynchronous edit or document exchange.
struct PianoTrackTarget: Sendable, Equatable {
    let document: UUID
    let track: UUID
}

struct PianoLocation: Sendable, Equatable {
    let target: PianoTrackTarget
    let tick: Int64
    var id = UUID()
}

extension ProjectController {
    var pianoTrackRows: [PianoTrackRow] {
        guard let documentID else { return [] }
        return tracks.enumerated().map { index, track in
            let mix = snapshot?.mixer(onTrack: index)
            return PianoTrackRow(target: PianoTrackTarget(document: documentID, track: track.id), name: track.name,
                visible: !hiddenPianoTracks.contains(track.id), editing: pianoTrack == track.id,
                solo: mix?.solo.value == 1, mute: mix?.mute.value == 1, enabled: !busy)
        }
    }

    var pianoReferences: [UUID] {
        tracks.map(\.id).filter { $0 != pianoTrack && !hiddenPianoTracks.contains($0) }
    }

    @discardableResult
    func openPiano(_ target: PianoTrackTarget) -> Bool {
        guard !busy, documentID == target.document, tracks.contains(where: { $0.id == target.track }) else { return false }
        if pianoTrack != target.track {
            pianoTrack = target.track
            selectNotes([])
        }
        return true
    }

    @discardableResult
    func togglePianoMixer(_ target: PianoTrackTarget, field: MixerField) async -> Bool {
        guard field == .solo || field == .mute else { return false }
        return await performActions({ [self] in
            guard let mix = mixer(target.track) else { return nil }
            return [.setParameter(parameter: mix[field].id.uuidString, value: mix[field].value == 1 ? 0 : 1)]
        }, documentID: target.document)
    }

    func setPianoReference(_ target: PianoTrackTarget, visible: Bool) {
        guard !busy, documentID == target.document, tracks.contains(where: { $0.id == target.track }) else { return }
        if visible { hiddenPianoTracks.remove(target.track) } else { hiddenPianoTracks.insert(target.track) }
    }
}

extension WorkspaceSession {
    /// An explicit arrange double click reveals this location, not a new play start.
    func openPiano(_ target: PianoTrackTarget, tick: Int64) {
        guard tick >= 0, controller.openPiano(target) else { return }
        controller.pianoLocation = PianoLocation(target: target, tick: tick)
        state.layout.editor = true
        state.saveLayout()
    }
}
