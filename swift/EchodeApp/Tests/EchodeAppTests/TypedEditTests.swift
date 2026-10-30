import EchodeBridge
import Foundation
import Testing

@Suite @MainActor
struct TypedEditTests {
    @Test func contractTypedEditsRetainAtomicHistoryAndPersistence() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-typed-edit-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let document = try Document(creating: url.path)
        defer { document.shutdown() }
        let added = try await document.perform(EditAction.addTrack(name: "typed"))
        let track = try #require(added.trackID(at: 0))
        let inserted = try await document.perform(EditAction.insertNote(
            track: track.uuidString, pitch: 60, velocity: 100, start: 1, length: 705_601))
        let original = try #require(inserted.pianoNeighbor(track: track, current: nil))
        let changed = try await document.perform([
            EditAction.setNoteVelocity(track: track.uuidString, clip: original.id.placement.uuidString,
                                       note: original.id.note.uuidString, velocity: 80),
            .editNote(track: track.uuidString, clip: original.id.placement.uuidString,
                      note: original.id.note.uuidString, pitch: 64, start: 2, length: 705_603)
        ])
        let note = try #require(changed.pianoNote(original.id))
        #expect(note.pitch == 64 && note.start == 2 && note.length == 705_603)
        #expect(note.velocity >> 9 == 80)
        let undone = try await document.undo()
        #expect(undone.pianoNote(original.id)?.pitch == 60)
        #expect(undone.pianoNote(original.id)?.velocity == original.velocity)
        #expect(undone.pianoNote(original.id)?.start == 1)
        // Both a model failure and a local encoding failure must preserve Redo.
        do {
            _ = try await document.perform([
                EditAction.addTrack(name: "must roll back"),
                .addDevice(track: UUID().uuidString, plugin: "org.example.fx")
            ])
            Issue.record("A failed typed transaction was accepted")
        } catch is BridgeError {}
        do {
            _ = try await document.perform([
                EditAction.addTrack(name: "must not be submitted"),
                .setClipGain(clip: original.id.placement.uuidString, gain: .nan)
            ])
            Issue.record("A non-finite typed transaction was encoded")
        } catch is EncodingError {}
        let empty = try await document.perform([EditAction]())
        #expect(try empty.historyReport().changed == false)
        #expect(empty.trackCount == 1)
        let cancelled = Task { () -> Bool in
            withUnsafeCurrentTask { $0?.cancel() }
            do {
                _ = try await document.perform(EditAction.addTrack(name: "cancelled"))
                return false
            } catch is CancellationError { return true }
            catch { return false }
        }
        #expect(await cancelled.value)
        let redone = try await document.redo()
        #expect(redone.trackCount == 1)
        #expect(redone.pianoNote(original.id)?.pitch == 64)
        #expect(redone.pianoNote(original.id)?.velocity == note.velocity)
        let noChange = try await document.perform(EditAction.setNoteVelocity(
            track: track.uuidString, clip: original.id.placement.uuidString, note: original.id.note.uuidString, velocity: 80))
        #expect(try noChange.historyReport().changed == false)
        let afterNoopUndo = try await document.undo()
        #expect(afterNoopUndo.pianoNote(original.id)?.pitch == 60)
        _ = try await document.redo()
        document.shutdown()
        let reopened = try Document(opening: url.path)
        defer { reopened.shutdown() }
        let saved = try await reopened.snapshot()
        #expect(saved.trackCount == 1 && saved.trackName(at: 0) == "typed")
        let savedNote = try #require(saved.pianoNote(original.id))
        #expect(savedNote.pitch == 64 && savedNote.start == 2 && savedNote.length == 705_603)
        #expect(savedNote.velocity == note.velocity)
    }
}
