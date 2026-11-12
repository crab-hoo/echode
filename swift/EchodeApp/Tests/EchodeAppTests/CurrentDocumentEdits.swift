import EchodeBridge
import Foundation

@testable import EchodeAppCore

/// Fixture shorthand: each call captures the document open when it starts. The
/// product has no such entries, so a stale view callback cannot retarget itself.
extension ProjectController {
    @discardableResult
    func insertPlugin(_ plugin: String, on track: UUID? = nil) async -> Bool {
        guard let documentID else { return false }
        return await insertPlugin(plugin, on: track, documentID: documentID)
    }

    func removeDevice(_ id: UUID) async {
        guard let documentID else { return }
        await removeDevice(id, documentID: documentID)
    }

    @discardableResult
    func insertNote(pitch: Int, beat: Double, length: Double, on track: UUID? = nil) async -> Bool {
        guard let documentID else { return false }
        return await insertNote(pitch: pitch, beat: beat, length: length, on: track, documentID: documentID)
    }

    @discardableResult
    func setNoteVelocity(_ id: PianoNote.ID, velocity: Int) async -> Bool {
        guard let documentID else { return false }
        return await setNoteVelocity(id, velocity: velocity, documentID: documentID)
    }

    @discardableResult
    func setNoteVelocities(_ change: PianoVelocityChange) async -> Bool {
        guard let documentID else { return false }
        return await setNoteVelocities(change, documentID: documentID)
    }

    @discardableResult
    func resizeNotes(_ change: PianoResizeChange) async -> Bool {
        guard let documentID else { return false }
        return await resizeNotes(change, documentID: documentID)
    }

    @discardableResult
    func moveNotes(_ change: PianoMoveChange) async -> Bool {
        guard let documentID else { return false }
        return await moveNotes(change, documentID: documentID)
    }

    @discardableResult
    func editNote(_ id: PianoNote.ID, geometry: NoteGeometry) async -> Bool {
        guard let documentID else { return false }
        return await editNote(id, geometry: geometry, documentID: documentID)
    }

    @discardableResult
    func deleteNote(_ id: PianoNote.ID) async -> Bool {
        guard let documentID else { return false }
        return await deleteNote(id, documentID: documentID)
    }

    @discardableResult
    func deleteNotes(_ ids: Set<PianoNote.ID>) async -> Bool {
        guard let documentID else { return false }
        return await deleteNotes(ids, documentID: documentID)
    }

    @discardableResult
    func setMixer(_ track: UUID, field: MixerField, value: Float) async -> Bool {
        guard let documentID else { return false }
        return await setMixer(track, field: field, value: value, documentID: documentID)
    }

    @discardableResult
    func setClipGain(_ id: UUID, gain: Float) async -> Bool {
        guard let documentID else { return false }
        return await setClipGain(id, gain: gain, documentID: documentID)
    }

    @discardableResult
    func setClipFades(_ id: UUID, fadeIn: Int64, fadeOut: Int64) async -> Bool {
        guard let documentID else { return false }
        return await setClipFades(id, fadeIn: fadeIn, fadeOut: fadeOut, documentID: documentID)
    }

    @discardableResult
    func beginFader(_ track: UUID, field: MixerField) -> Bool {
        guard let documentID else { return false }
        return beginFader(track, field: field, documentID: documentID)
    }

    func changeFader(_ value: Float) {
        if let documentID { changeFader(value, documentID: documentID) }
    }

    func finishFader(commit: Bool) async {
        if let documentID { await finishFader(commit: commit, documentID: documentID) }
    }

    @discardableResult
    func beginParameter(_ id: UInt32, device: UUID) -> Bool {
        guard let documentID else { return false }
        return beginParameter(id, device: device, documentID: documentID)
    }

    func changeParameter(_ value: Double) {
        if let documentID { changeParameter(value, documentID: documentID) }
    }

    func finishParameter(commit: Bool) async {
        if let documentID { await finishParameter(commit: commit, documentID: documentID) }
    }

    @discardableResult
    func setParameter(_ id: UInt32, device: UUID, value: Double) async -> Bool {
        guard let documentID else { return false }
        return await setParameter(id, device: device, value: value, documentID: documentID)
    }

    func enterParameter(_ text: String, id: UInt32, device: UUID) async -> Bool {
        guard let documentID else { return false }
        return await enterParameter(text, id: id, device: device, documentID: documentID)
    }

    func restoreRevision(_ id: String) async -> HistoryReport? {
        guard let documentID else { return nil }
        return await restoreRevision(id, documentID: documentID)
    }
}
