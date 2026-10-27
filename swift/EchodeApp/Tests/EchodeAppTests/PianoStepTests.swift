import AppKit
import Carbon.HIToolbox
@testable import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoStepTests {
    @Test func contractArrowRepeatsEditTheWholeSelectionOnTheirTurn() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-key-batch-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1.1, length: 0.2))
        #expect(await controller.insertNote(pitch: 64, beat: 2.2, length: 0.7))
        let track = try #require(controller.pianoTrack), document = try #require(controller.documentID)
        let snapshot = try #require(controller.snapshot)
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 8 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        let ids = Set(notes.map(\.id)), undo = controller.undoCount
        controller.selectNotes(ids)
        controller.noteInput.remember(12_345); controller.noteVelocity.remember(77)
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false, selection: ids)
        var writes: [Task<Bool, Never>] = []
        view.onSteps = { step in writes.append(controller.enqueuePianoStep(step, documentID: document)) }
        // All four events precede any new snapshot, including a held-key repeat.
        view.keyDown(with: try PianoWorkflowTests.key(kVK_UpArrow))
        view.keyDown(with: try PianoWorkflowTests.key(kVK_UpArrow, flags: .option, repeating: true))
        view.keyDown(with: try PianoWorkflowTests.key(kVK_RightArrow))
        view.keyDown(with: try PianoWorkflowTests.key(kVK_LeftArrow, flags: .shift))
        #expect(writes.count == 4, "every arrow event must address the full selection")
        for write in writes { #expect(await write.value) }
        let shortest = try #require(notes.map(\.length).min())
        for note in notes {
            let changed = try #require(controller.snapshot?.pianoNote(note.id))
            #expect(changed.start == note.start + view.camera.grid && changed.pitch == note.pitch + 13)
            #expect(changed.length == note.length - shortest + 1 && changed.velocity == note.velocity)
        }
        #expect(controller.undoCount == undo + 4 && controller.noteSelection == ids)
        #expect(controller.noteInput.length == 12_345 && controller.noteVelocity.value == 77)
        await controller.undo()
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id)?.length == $0.length })
        await controller.redo()
        for _ in 0..<4 { await controller.undo() }
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) })
        for _ in 0..<4 { await controller.redo() }
        await controller.close(); await controller.open(url)
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id)?.pitch == $0.pitch + 13 })
        #expect(!(await controller.stepNotes(PianoStep(ids: ids, pitch: 1), documentID: document)))
        await controller.close()
    }

    @Test func contractKeyboardBoundsAndLinkedNotesStayAtomic() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-key-linked-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Links")
        #expect(await controller.insertNote(pitch: 1, beat: 1, length: 0.2))
        #expect(await controller.insertNote(pitch: 126, beat: 2, length: 0.7))
        let track = try #require(controller.pianoTrack), document = try #require(controller.documentID)
        let first = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        #expect(await controller.edit(["type": "clip.link", "source": first.id.placement.uuidString,
                                      "track": track.uuidString, "start": 8 * 705_600]))
        let snapshot = try #require(controller.snapshot)
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 16 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        try #require(notes.count == 4)
        let ids = Set(notes.map(\.id)), undo = controller.undoCount
        let up = PianoStep(ids: ids, pitch: Int.max)
        #expect(up.actions(in: snapshot)?.count == 2, "linked notes must be edited only once")
        #expect(await controller.stepNotes(up, documentID: document))
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id)?.pitch == $0.pitch + 1 })
        #expect(!(await controller.stepNotes(up, documentID: document)), "at the common upper pitch bound, no empty Undo is recorded")
        #expect(controller.undoCount == undo + 1)
        await controller.undo()
        let earlier = PianoStep(ids: ids, time: Int64.min)
        #expect(await controller.stepNotes(earlier, documentID: document))
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id)?.start == $0.start - 705_600 })
        await controller.undo()
        let shorter = PianoStep(ids: ids, time: Int64.min, resize: true)
        #expect(await controller.stepNotes(shorter, documentID: document))
        let shortest = try #require(notes.map(\.length).min())
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id)?.length == $0.length - shortest + 1 })
        #expect(!(await controller.stepNotes(shorter, documentID: document)))
        await controller.undo()
        let missing = PianoNote.ID(track: track, placement: first.id.placement, note: UUID())
        #expect(!(await controller.stepNotes(PianoStep(ids: ids.union([missing]), pitch: 1), documentID: document)))
        #expect(controller.undoCount == undo)
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) })
        let raise = controller.enqueuePianoStep(PianoStep(ids: ids, pitch: 12), documentID: document)
        let lower = controller.enqueuePianoStep(PianoStep(ids: ids, pitch: -1), documentID: document)
        #expect(await raise.value)
        #expect(await lower.value)
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) },
                "queued events retain their native order even when the first reaches a shared pitch bound")
        await controller.undo(); await controller.undo()
        // Single-note length edits remember their length; batch edits above did not.
        controller.selectNotes([first.id])
        #expect(await controller.stepNotes(PianoStep(ids: [first.id], time: 1, resize: true), documentID: document))
        #expect(controller.noteInput.length == first.length + 1)
        await controller.undo()
        #expect(controller.noteInput.length == first.length)
        await controller.close()
    }
}
