import AppKit
import EchodeBridge
import Testing

@testable import EchodeAppCore

@MainActor
struct PianoResizeBatchTests {
    @Test func contractSelectedEdgesResizeTogetherAndRoundTripHistory() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-resize-batch-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        for (pitch, beat, length) in [(60, 1.1, 0.7), (62, 2.2, 0.4), (64, 3.0, 1.0)] {
            #expect(await controller.insertNote(pitch: pitch, beat: beat, length: length))
        }
        let track = try #require(controller.selection)
        let document = try #require(controller.documentID)
        let snapshot = try #require(controller.snapshot)
        let notes = snapshot.pianoNotes(
            track: track, originTick: 0, widthTicks: 8 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1
        ).sorted { $0.pitch < $1.pitch }
        let selected = Set(notes.prefix(2).map(\.id))
        let undo = controller.undoCount
        controller.selectNotes(selected)
        controller.noteInput.remember(12_345)
        controller.noteVelocity.remember(77)
        var final: [PianoNote.ID: NoteGeometry] = [:]
        for edge in [PianoGesture.Mode.leftEdge, .rightEdge] {
            for shift in [true, false] {
                let harness = PianoMoveBatchHarness(
                    snapshot: try #require(controller.snapshot), track: track, selected: selected)
                defer { harness.view.stop() }
                let view = harness.view
                let point = harness.edgePoint(notes[0], edge: edge)
                var writes: [Task<Bool, Never>] = []
                var singles = 0
                view.onSelection = { controller.selectNotes($0) }
                view.onEdit = { _, _ in singles += 1 }
                view.onResizes = { change in
                    writes.append(Task { await controller.resizeNotes(change, documentID: document) })
                }
                try harness.send(.leftMouseDown, at: point)
                try harness.send(
                    .leftMouseDragged, at: CGPoint(x: point.x + 0.2 * view.camera.pixelsPerBeat, y: point.y - 20),
                    flags: shift ? [] : .shift)
                #expect(
                    view.selectedNotes == selected && controller.noteSelection == selected,
                    "resizing a selected edge must preserve the entire selection")
                #expect(writes.isEmpty && singles == 0 && controller.undoCount == undo)
                let preview = try #require(view.resizePreview)
                let frame = harness.frame(resize: preview)
                #expect(preview.change.edge == edge && preview.change.originals.count == 2)
                let ghosts = frame.gridInstances.filter { $0.color == SIMD4<Float>(0.96, 0.87, 0.54, 1) }
                #expect(ghosts.count == 2, "every selected note must have an edge resize preview")
                for (ghost, id) in zip(ghosts, selected.sorted { $0.descriptionKey < $1.descriptionKey }) {
                    let geometry = try #require(preview.change.geometries?[id])
                    #expect(
                        abs(Double(ghost.rect.z) - Double(geometry.length) / 705_600 * view.camera.pixelsPerBeat)
                            < 0.001)
                    #expect(
                        abs(
                            Double(ghost.rect.x)
                                - (frame.layout.gridOrigin.x + Double(geometry.start) / 705_600
                                    * view.camera.pixelsPerBeat)) < 0.001)
                }
                // Release coordinates and Shift, not the last rendered preview, determine the edit.
                let end = CGPoint(x: point.x + 0.1 * view.camera.pixelsPerBeat, y: point.y - 40)
                try harness.send(.leftMouseUp, at: end, flags: shift ? .shift : [])
                try harness.send(.leftMouseUp, at: end)
                try #require(writes.count == 1 && singles == 0)
                #expect(await writes[0].value)
                let delta: Int64 = shift ? 70_560 : (edge == .leftEdge ? 105_840 : 141_120)
                final = [:]
                for (index, note) in notes.enumerated() {
                    let edited = try #require(controller.snapshot?.pianoNote(note.id))
                    let expected = NoteGeometry(
                        start: note.start + (index < 2 && edge == .leftEdge ? delta : 0),
                        length: note.length + (index < 2 ? (edge == .leftEdge ? -delta : delta) : 0), pitch: note.pitch)
                    #expect(
                        NoteGeometry(edited) == expected, "all selected edges must receive the release displacement")
                    #expect(edited.velocity == note.velocity && edited.id == note.id)
                    final[note.id] = expected
                }
                #expect(
                    controller.undoCount == undo + 1 && controller.noteSelection == selected
                        && controller.noteFocus == nil)
                #expect(controller.noteInput.length == 12_345 && controller.noteVelocity.value == 77)
                await controller.undo()
                #expect(
                    notes.allSatisfy {
                        controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0)
                    })
                #expect(controller.undoCount == undo && controller.noteInput.length == 12_345)
            }
        }
        await controller.redo()
        #expect(final.allSatisfy { controller.snapshot?.pianoNote($0.key).map(NoteGeometry.init) == $0.value })
        #expect(controller.noteInput.length == 12_345)
        await controller.close()
        await controller.open(url)
        #expect(final.allSatisfy { controller.snapshot?.pianoNote($0.key).map(NoteGeometry.init) == $0.value })
        #expect(controller.noteSelection.isEmpty && controller.noteInput.length == nil)
        await controller.close()
    }
}
