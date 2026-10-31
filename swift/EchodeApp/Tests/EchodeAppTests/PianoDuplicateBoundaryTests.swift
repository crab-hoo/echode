import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoDuplicateBoundaryTests {
    @Test func contractCopiesShareBoundsDeduplicateLinksAndRejectStaleTargets() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-copy-links-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "linked keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 0.7))
        #expect(await controller.insertNote(pitch: 127, beat: 2, length: 0.7))
        let track = try #require(controller.pianoTrack), document = try #require(controller.documentID)
        let first = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        #expect(await controller.edit(.linkClip(source: first.id.placement.uuidString, track: track.uuidString, start: 8 * 705_600)))
        let snapshot = try #require(controller.snapshot)
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 16 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        try #require(notes.count == 4)
        let selected = Set(notes.map(\.id)), undo = controller.undoCount
        controller.selectNotes(selected)
        let harness = PianoMoveBatchHarness(snapshot: snapshot, track: track, selected: selected)
        defer { harness.view.stop() }
        let view = harness.view, point = harness.point(first)
        var changes: [PianoMoveChange] = []
        view.onMoves = { changes.append($0) }
        try harness.send(.leftMouseDown, at: point, flags: .command)
        try harness.send(.leftMouseDragged, at: CGPoint(x: point.x - 1000, y: point.y - 1000), flags: .shift)
        let bounded = try #require(view.movePreview)
        #expect(bounded.change.time == -first.start && bounded.change.pitch == 0)
        #expect(bounded.change.geometries?.values.map(\.start).min() == 0)
        try harness.send(.leftMouseUp, at: CGPoint(x: point.x + 7.2, y: point.y + 20), flags: .shift)
        let change = try #require(changes.first)
        #expect(change.actions(in: snapshot)?.count == 2, "linked aliases must be copied once per content note")
        #expect(await controller.moveNotes(change, documentID: document))
        #expect(controller.undoCount == undo + 1 && controller.noteSelection.count == 4)
        #expect(controller.noteSelection.isDisjoint(with: selected))
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) })
        #expect(!(await controller.moveNotes(change, documentID: document)), "reusing copied ids must be rejected atomically")
        await controller.undo()
        var stale = change.originals
        stale[first.id]?.start += 1
        #expect(!(await controller.moveNotes(PianoMoveChange(anchor: first.id, originals: stale,
            time: 70_560, pitch: -1, copies: change.copies), documentID: document)), "every copy source must be validated")
        #expect(!(await controller.moveNotes(change, documentID: DocumentID())))
        #expect(controller.undoCount == undo)
        await controller.close()

        // Command on an unselected edge copies only that note, never resizes the selection.
        for edge in [PianoGesture.Mode.leftEdge, .rightEdge] {
            let single = PianoMoveBatchHarness(snapshot: snapshot, track: track, selected: selected.subtracting([first.id]))
            defer { single.view.stop() }
            var sent: [PianoMoveChange] = [], edits = 0
            single.view.onMoves = { sent.append($0) }
            single.view.onEdit = { _, _ in edits += 1 }
            let start = single.edgePoint(first, edge: edge), end = CGPoint(x: start.x + 8, y: start.y)
            try single.send(.leftMouseDown, at: start, flags: [.command, .option])
            try single.send(.leftMouseDragged, at: end)
            try single.send(.leftMouseUp, at: end)
            #expect(sent.count == 1 && sent.first?.originals.count == 1 && sent.first?.copies.count == 1 && edits == 0)
            #expect(sent.first?.originals[first.id]?.length == first.length)
        }
    }

    @Test func fixedWindowCopyGrowsWithoutMovingSourceAndFailureRollsBack() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-copy-fixed-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        let track = try #require(controller.pianoTrack)
        let note = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        #expect(await controller.edit(.resizeClip(track: track.uuidString, clip: note.id.placement.uuidString, start: 0, length: 4 * 705_600)))
        let original = try #require(controller.snapshot?.pianoNote(note.id)), undo = controller.undoCount
        controller.noteInput.remember(12_345); controller.noteVelocity.remember(77)
        let change = PianoMoveChange(anchor: note.id, originals: [note.id: NoteGeometry(original)],
            time: 4 * 705_600, pitch: 0, copies: [note.id.note: UUID()])
        #expect(await controller.moveNotes(change))
        let grown = try #require(controller.snapshot?.clipMix(note.id.placement)?.length)
        #expect(grown == 6 * 705_600)
        #expect(controller.snapshot?.pianoNote(note.id).map(NoteGeometry.init) == NoteGeometry(original))
        #expect(controller.noteInput.length == 12_345 && controller.noteVelocity.value == 77)
        await controller.undo()
        #expect(controller.undoCount == undo && controller.snapshot?.clipMix(note.id.placement)?.length == 4 * 705_600)
        // A later invalid copy id must roll back an earlier successful copy.
        #expect(await controller.insertNote(pitch: 62, beat: 2, length: 1))
        let second = try #require(controller.snapshot?.pianoNeighbor(track: track, current: note.id))
        let before = controller.undoCount
        let invalid = PianoMoveChange(anchor: note.id, originals: [note.id: NoteGeometry(original), second.id: NoteGeometry(second)],
            time: 1, pitch: 0, copies: [note.id.note: UUID(), second.id.note: track])
        #expect(!(await controller.moveNotes(invalid)))
        #expect(controller.undoCount == before && controller.snapshot?.pianoNeighbor(track: track, current: second.id) == nil)
        await controller.close()
    }
}
