import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

@Suite @MainActor
struct TypedPianoEditTests {
    @Test func contractTypedPianoBatchesPreserveTargetsHistoryAndPersistence() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-typed-piano-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "typed piano")
        let track = try #require(controller.selection)
        let document = try #require(controller.documentID)
        #expect(await controller.insertNote(pitch: 0, beat: 1, length: 1, documentID: document))
        #expect(await controller.insertNote(pitch: 126, beat: 3, length: 1, documentID: document))
        let initial = try #require(controller.snapshot)
        let notes = initial.pianoNotes(
            track: track, originTick: 0, widthTicks: 8 * Tick.perQuarter,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        try #require(notes.count == 2)
        let anchor = notes[0].id
        let ids = Set(notes.map(\.id))
        let originals = Dictionary(uniqueKeysWithValues: notes.map { ($0.id, NoteGeometry($0)) })
        let change = PianoMoveChange(anchor: anchor, originals: originals, time: (Int64(1) << 53) + 1, pitch: 1)
        let actions: [EditAction] = try #require(change.actions(in: initial))
        #expect(actions.count == 2)
        for action in actions {
            guard case .editNote(let owner, let placement, let note, let pitch, let start, let length) = action else {
                Issue.record("A typed move must produce editNote requests")
                continue
            }
            let original = try #require(notes.first { $0.id.note.uuidString == note })
            #expect(owner == track.uuidString && placement == original.id.placement.uuidString)
            #expect(
                start == original.start + change.time && length == original.length && Int(pitch) == original.pitch + 1)
        }
        controller.selectNotes(ids)
        let undo = controller.undoCount
        #expect(await controller.moveNotes(change, documentID: document))
        #expect(controller.undoCount == undo + 1 && controller.noteSelection == ids)
        for note in notes {
            #expect(
                controller.snapshot?.pianoNote(note.id).map(NoteGeometry.init) == change.translated(NoteGeometry(note)))
        }
        #expect(
            !(await controller.moveNotes(change, documentID: document)),
            "stale originals must reject the entire typed batch")
        await controller.undo()
        #expect(controller.redoCount == 1)
        let inputLength = controller.noteInput.length
        let inputVelocity = controller.noteVelocity.value
        for invalid in [Int.min, -1, 128, Int.max] {
            #expect(!(await controller.setNoteVelocity(anchor, velocity: invalid, documentID: document)))
            #expect(!(await controller.insertNote(pitch: invalid, beat: 1, length: 1, documentID: document)))
        }
        #expect(
            !(await controller.editNote(
                anchor, geometry: NoteGeometry(start: 0, length: 1, pitch: 128), documentID: document)))
        #expect(!(await controller.setNoteVelocity(anchor, velocity: 0, documentID: DocumentID())))
        #expect(controller.undoCount == undo && controller.redoCount == 1)
        #expect(controller.noteInput.length == inputLength && controller.noteVelocity.value == inputVelocity)
        await controller.redo()
        let first = controller.enqueuePianoStep(PianoStep(ids: ids, time: 1), documentID: document)
        let second = controller.enqueuePianoStep(PianoStep(ids: ids, time: 1), documentID: document)
        #expect(await first.value)
        #expect(await second.value)
        #expect(controller.undoCount == undo + 3)
        for note in notes { #expect(controller.snapshot?.pianoNote(note.id)?.start == note.start + change.time + 2) }
        await controller.close()
        await controller.open(url)
        for note in notes { #expect(controller.snapshot?.pianoNote(note.id)?.start == note.start + change.time + 2) }
        #expect(controller.errorMessage == nil)
        #expect(
            !(await controller.deleteNotes(ids, documentID: document)),
            "a reopened document must reject the old typed request")
        await controller.close()
    }
}
