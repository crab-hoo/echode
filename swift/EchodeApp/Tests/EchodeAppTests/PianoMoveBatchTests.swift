import AppKit
import EchodeBridge
import Testing

@testable import EchodeAppCore

@MainActor
struct PianoMoveBatchTests {
    @Test func contractSelectedNotesMoveTogetherAndRoundTripHistory() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-move-batch-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        for (pitch, beat) in [(60, 1.1), (62, 2.2), (64, 3.0)] {
            #expect(await controller.insertNote(pitch: pitch, beat: beat, length: 0.7))
        }
        let track = try #require(controller.selection)
        let document = try #require(controller.documentID)
        let initial = try #require(controller.snapshot)
        let notes = initial.pianoNotes(
            track: track, originTick: 0, widthTicks: 8 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1
        ).sorted { $0.pitch < $1.pitch }
        let selected = Set(notes.prefix(2).map(\.id))
        let undo = controller.undoCount
        controller.selectNotes(selected)
        controller.noteInput.remember(12_345)
        controller.noteVelocity.remember(77)
        var lastTime: Int64 = 0
        for releaseShift in [true, false] {
            let harness = PianoMoveBatchHarness(
                snapshot: try #require(controller.snapshot), track: track, selected: selected)
            defer { harness.view.stop() }
            let view = harness.view
            let point = harness.point(notes[0])
            var writes: [Task<Bool, Never>] = []
            var singles = 0
            view.onSelection = { controller.selectNotes($0) }
            view.onEdit = { _, _ in singles += 1 }
            view.onMoves = { change in writes.append(Task { await controller.moveNotes(change, documentID: document) })
            }
            try harness.send(.leftMouseDown, at: point)
            try harness.send(
                .leftMouseDragged, at: CGPoint(x: point.x + 0.2 * view.camera.pixelsPerBeat, y: point.y - 20),
                flags: releaseShift ? [] : .shift)
            #expect(
                view.selectedNotes == selected && controller.noteSelection == selected,
                "moving a selected note must preserve the entire selection")
            #expect(writes.isEmpty && singles == 0 && controller.undoCount == undo)
            let preview = try #require(view.movePreview)
            let frame = harness.frame(preview)
            #expect(preview.change.time == (releaseShift ? 105_840 : 141_120))
            #expect(preview.change.pitch == 1 && preview.change.originals.count == 2)
            let ghosts = frame.gridInstances.filter { $0.color == SIMD4<Float>(0.96, 0.87, 0.54, 1) }
            #expect(ghosts.count == 2, "every selected note must have a movement preview")
            for (ghost, note) in zip(ghosts, selected.sorted { $0.descriptionKey < $1.descriptionKey }) {
                let geometry = try #require(preview.change.geometries?[note])
                #expect(
                    abs(
                        Double(ghost.rect.x)
                            - (frame.layout.gridOrigin.x + Double(geometry.start) / 705_600 * view.camera.pixelsPerBeat)
                    ) < 0.001)
            }
            // Release position and Shift differ from the last preview.
            let end = CGPoint(x: point.x + 0.1 * view.camera.pixelsPerBeat, y: point.y - 40)
            try harness.send(.leftMouseUp, at: end, flags: releaseShift ? .shift : [])
            try harness.send(.leftMouseUp, at: end)
            try #require(writes.count == 1 && singles == 0)
            #expect(await writes[0].value)
            lastTime = releaseShift ? 70_560 : 105_840
            for (index, note) in notes.enumerated() {
                let edited = try #require(controller.snapshot?.pianoNote(note.id))
                #expect(edited.start == note.start + (index < 2 ? lastTime : 0))
                #expect(edited.pitch == note.pitch + (index < 2 ? 2 : 0))
                #expect(edited.length == note.length && edited.velocity == note.velocity && edited.id == note.id)
            }
            #expect(
                controller.undoCount == undo + 1 && controller.noteSelection == selected && controller.noteFocus == nil)
            #expect(controller.noteInput.length == 12_345 && controller.noteVelocity.value == 77)
            await controller.undo()
            #expect(
                notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) })
            #expect(controller.undoCount == undo && controller.noteInput.length == 12_345)
        }
        await controller.redo()
        #expect(controller.snapshot?.pianoNote(notes[0].id)?.start == notes[0].start + lastTime)
        await controller.close()
        await controller.open(url)
        for note in notes.prefix(2) {
            #expect(controller.snapshot?.pianoNote(note.id)?.start == note.start + lastTime)
            #expect(controller.snapshot?.pianoNote(note.id)?.pitch == note.pitch + 2)
        }
        #expect(controller.noteSelection.isEmpty)
        await controller.close()
    }
}
