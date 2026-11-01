import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct NoopEditTests {
    @Test func contractNoopEditsKeepVisibleHistoryAndInputMemoryAligned() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-noop-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let track = try #require(controller.selection), document = try #require(controller.documentID)
        #expect(await controller.insertNote(pitch: 60, beat: 0, length: 0.7))
        let note = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        controller.noteFocus = note.id
        let longer = NoteGeometry(start: note.start, length: 585_648, pitch: note.pitch)
        #expect(await controller.editNote(note.id, geometry: longer))
        await controller.undo()
        let undo = controller.undoCount, redo = controller.redoCount
        let velocity = EditAction.setNoteVelocity(track: track.uuidString, clip: note.id.placement.uuidString,
                                                  note: note.id.note.uuidString, velocity: 100)
        #expect(await controller.edit(velocity))
        #expect(controller.undoCount == undo && controller.redoCount == redo,
                "a no-op must not consume visible history")
        #expect(controller.noteInput.length == note.length)
        #expect(await controller.performActions({ [velocity] }, documentID: document))
        let quieter = EditAction.setNoteVelocity(track: track.uuidString, clip: note.id.placement.uuidString,
                                                 note: note.id.note.uuidString, velocity: 60)
        #expect(await controller.performActions({ [quieter, velocity] }, documentID: document))
        #expect(controller.undoCount == undo && controller.redoCount == redo)
        #expect(try controller.snapshot?.historyReport().changed == false)
        await controller.redo()
        #expect(controller.snapshot?.pianoNeighbor(track: track, current: nil)?.length == longer.length)
        #expect(controller.noteInput.length == longer.length, "no-op edits must not insert input-memory slots")
        #expect(await controller.edit(quieter))
        #expect(try controller.snapshot?.historyReport().changed == true)
        await controller.undo()
        #expect(controller.noteInput.length == longer.length)
        #expect(controller.errorMessage == nil)
        await controller.close()
        await controller.open(url)
        #expect(controller.snapshot?.pianoNeighbor(track: track, current: nil)?.length == longer.length)
        await controller.close()
    }
}
