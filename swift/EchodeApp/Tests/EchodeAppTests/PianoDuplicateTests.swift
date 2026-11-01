import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoDuplicateTests {
    @Test func contractCommandDragCopiesSelectionAndRoundTripsHistory() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-note-copy-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        for (pitch, beat) in [(60, 1.1), (62, 2.2), (64, 3.0)] {
            #expect(await controller.insertNote(pitch: pitch, beat: beat, length: 0.7))
        }
        let track = try #require(controller.pianoTrack), document = try #require(controller.documentID)
        let initial = try #require(controller.snapshot)
        let notes = initial.pianoNotes(track: track, originTick: 0, widthTicks: 16 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1).sorted { $0.pitch < $1.pitch }
        let selected = Set(notes.prefix(2).map(\.id)), undo = controller.undoCount
        controller.selectNotes(selected)
        controller.noteInput.remember(12_345); controller.noteVelocity.remember(77)
        let harness = PianoMoveBatchHarness(snapshot: initial, track: track, selected: selected)
        defer { harness.view.stop() }
        let view = harness.view, point = harness.point(notes[0])
        var writes: [Task<Bool, Never>] = [], singles = 0
        view.onSelection = { controller.selectNotes($0) }
        view.onEdit = { _, _ in singles += 1 }
        view.onMoves = { change in writes.append(Task { await controller.moveNotes(change, documentID: document) }) }
        try harness.send(.leftMouseDown, at: point, flags: .command)
        #expect(controller.noteSelection == selected, "Command press must defer selection toggling until a click release")
        try harness.send(.leftMouseDragged, at: CGPoint(x: point.x + 20, y: point.y - 20))
        let preview = try #require(view.movePreview, "Command drag must preview the selected copies")
        #expect(harness.frame(preview).gridInstances.filter { $0.color == SIMD4<Float>(0.96, 0.87, 0.54, 1) }.count == 2)
        #expect(writes.isEmpty && controller.undoCount == undo)
        // Command intent is captured at press; Shift and position are read at release.
        let end = CGPoint(x: point.x + 7.2, y: point.y - 40)
        try harness.send(.leftMouseUp, at: end, flags: .shift)
        try harness.send(.leftMouseUp, at: end)
        try #require(writes.count == 1 && singles == 0, "Command drag must commit exactly one copy transaction")
        #expect(await writes[0].value)
        let copies = controller.noteSelection
        #expect(copies.count == 2 && copies.isDisjoint(with: selected), "only copied notes must be selected")
        for original in notes {
            #expect(controller.snapshot?.pianoNote(original.id).map(NoteGeometry.init) == NoteGeometry(original), "copying must leave every source unchanged")
        }
        for original in notes.prefix(2) {
            let copy = try #require(copies.compactMap { controller.snapshot?.pianoNote($0) }.first { $0.pitch == original.pitch + 2 })
            #expect(copy.start == original.start + 70_560 && copy.length == original.length && copy.velocity == original.velocity)
        }
        #expect(controller.undoCount == undo + 1 && controller.noteInput.length == 12_345 && controller.noteVelocity.value == 77)
        await controller.undo()
        #expect(copies.allSatisfy { controller.snapshot?.pianoNote($0) == nil })
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) })
        await controller.redo()
        #expect(copies.allSatisfy { controller.snapshot?.pianoNote($0) != nil })
        await controller.close()
        await controller.open(url)
        #expect(copies.allSatisfy { controller.snapshot?.pianoNote($0) != nil })
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) })
        await controller.close()
    }
}
