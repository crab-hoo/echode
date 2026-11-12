import AppKit
import Testing

@testable import EchodeAppCore
@testable import EchodeBridge

@MainActor
struct PianoResizeBatchBoundaryTests {
    @Test func contractLinkedResizesValidateTargetsAndShareEdgeBounds() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-resize-linked-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "linked keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 0.7))
        #expect(await controller.insertNote(pitch: 127, beat: 2, length: 0.2))
        let track = try #require(controller.selection)
        let document = try #require(controller.documentID)
        let first = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        #expect(
            await controller.edit(
                .linkClip(source: first.id.placement.uuidString, track: track.uuidString, start: 8 * 705_600)))
        let snapshot = try #require(controller.snapshot)
        let notes = snapshot.pianoNotes(
            track: track, originTick: 0, widthTicks: 16 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        try #require(notes.count == 4)
        let ids = Set(notes.map(\.id))
        let originals = Dictionary(uniqueKeysWithValues: notes.map { ($0.id, NoteGeometry($0)) })
        controller.selectNotes(ids)
        let undo = controller.undoCount
        for edge in [PianoGesture.Mode.leftEdge, .rightEdge] {
            let harness = PianoMoveBatchHarness(
                snapshot: try #require(controller.snapshot), track: track, selected: ids)
            defer { harness.view.stop() }
            let view = harness.view
            let point = harness.edgePoint(first, edge: edge)
            var writes: [Task<Bool, Never>] = []
            view.onResizes = { change in
                writes.append(Task { await controller.resizeNotes(change, documentID: document) })
            }
            try harness.send(.leftMouseDown, at: point)
            let direction = edge == .leftEdge ? 1.0 : -1.0
            try harness.send(
                .leftMouseDragged, at: CGPoint(x: point.x + direction * 1000, y: point.y - 1000), flags: .shift)
            let preview = try #require(view.resizePreview)
            #expect(
                preview.change.delta == (edge == .leftEdge ? 141_119 : -141_119),
                "the shortest offscreen note must constrain every selected edge to one tick")
            #expect(preview.change.geometries?.values.map(\.length).min() == 1)
            try harness.send(.leftMouseDragged, at: CGPoint(x: point.x + direction * 1000, y: point.y))
            #expect(
                view.resizePreview?.change.delta == preview.change.delta,
                "snapping must not change the common one-tick minimum")
            #expect(
                harness.frame(resize: preview).gridInstances.filter { $0.color == SIMD4<Float>(0.96, 0.87, 0.54, 1) }
                    .count == 4)
            let end = CGPoint(x: point.x + 7.2, y: point.y + 20)
            try harness.send(.leftMouseUp, at: end, flags: .shift)
            try #require(writes.count == 1)
            #expect(await writes[0].value)
            let change = PianoResizeChange(anchor: first.id, originals: originals, edge: edge, delta: 70_560)
            #expect(change.actions(in: snapshot)?.count == 2, "linked content must produce one resize action per note")
            #expect(controller.undoCount == undo + 1 && controller.noteSelection == ids)
            for note in notes {
                #expect(
                    controller.snapshot?.pianoNote(note.id).map(NoteGeometry.init) == change.resized(NoteGeometry(note))
                )
                #expect(controller.snapshot?.pianoNote(note.id)?.velocity == note.velocity)
            }
            #expect(
                !(await controller.resizeNotes(change, documentID: document)),
                "a stale geometry must reject the entire resize")
            await controller.undo()
            #expect(
                notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) })
            #expect(!(await controller.resizeNotes(change, documentID: DocumentID())))
        }
        var stale = originals
        stale[first.id]?.length += 1
        var missing = originals
        missing[PianoNote.ID(track: track, placement: first.id.placement, note: UUID())] = NoteGeometry(first)
        for invalid in [stale, missing] {
            #expect(
                !(await controller.resizeNotes(
                    PianoResizeChange(anchor: first.id, originals: invalid, edge: .rightEdge, delta: 1))))
        }
        for (edge, delta) in [
            (PianoGesture.Mode.move, Int64(1)), (.rightEdge, 0), (.rightEdge, Int64.max),
            (.rightEdge, Int64.min), (.leftEdge, Int64.min), (.leftEdge, 141_120),
        ] {
            #expect(
                !(await controller.resizeNotes(
                    PianoResizeChange(anchor: first.id, originals: originals, edge: edge, delta: delta))))
        }
        #expect(controller.undoCount == undo)
        await controller.close()
        await controller.open(url)
        #expect(
            !(await controller.resizeNotes(
                PianoResizeChange(anchor: first.id, originals: originals, edge: .rightEdge, delta: 1),
                documentID: document)))
        await controller.close()

        let second = try #require(notes.first { $0.id.note != first.id.note })
        let extremes = [
            first.id: NoteGeometry(start: 10, length: 20, pitch: 60),
            second.id: NoteGeometry(start: Int64.max - 20, length: 10, pitch: 127),
        ]
        for edge in [PianoGesture.Mode.leftEdge, .rightEdge] {
            let gesture = PianoResizeGesture(
                anchor: first.id, originals: extremes, edge: edge, point: .zero, camera: PianoCamera())
            #expect(
                gesture.preview(at: CGPoint(x: -100, y: 0), snapping: false)?.change.delta
                    == (edge == .leftEdge ? -10 : -9))
            #expect(
                gesture.preview(at: CGPoint(x: 100, y: 0), snapping: false)?.change.delta
                    == (edge == .leftEdge ? 9 : 10),
                "the common edge limit must protect integer bounds and the shortest note")
            #expect(gesture.preview(at: CGPoint(x: CGFloat.infinity, y: 0), snapping: false) == nil)
            #expect(gesture.preview(at: CGPoint(x: 0, y: CGFloat.nan), snapping: false) == nil)
            #expect(gesture.preview(at: CGPoint(x: CGFloat.greatestFiniteMagnitude, y: 0), snapping: false) == nil)
        }
        let linked = PianoResizePreview(
            change: PianoResizeChange(
                anchor: first.id, originals: [first.id: NoteGeometry(first)], edge: .rightEdge, delta: 1))
        #expect(linked.geometries(in: notes).count == 2, "unselected aliases must preview their shared content resize")
    }

    @Test func fixedWindowsGrowAndLaterOverlapFailureRollsBackEarlierResizes() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-resize-fixed-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let track = try #require(controller.selection)
        for beat in [1.0, 6.0, 9.0] {
            #expect(await controller.insertNote(pitch: 60, beat: beat, length: 0.7))
            let snapshot = try #require(controller.snapshot)
            let note = try #require(
                snapshot.pianoNotes(
                    track: track, originTick: 0, widthTicks: 16 * 705_600,
                    topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1
                ).max { $0.start < $1.start })
            let clip = try #require(snapshot.clipMix(note.id.placement))
            #expect(
                await controller.edit(
                    .resizeClip(
                        track: track.uuidString, clip: clip.id.uuidString, start: clip.start,
                        length: note.start + note.length - clip.start)))
        }
        let snapshot = try #require(controller.snapshot)
        let notes = snapshot.pianoNotes(
            track: track, originTick: 0, widthTicks: 16 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1
        ).sorted { $0.start < $1.start }
        try #require(notes.count == 3 && Set(notes.map(\.id.placement)).count == 3)
        let originals = Dictionary(uniqueKeysWithValues: [notes[0], notes[2]].map { ($0.id, NoteGeometry($0)) })
        let undo = controller.undoCount
        // Grow the last fixed window, then fail to grow the first through the middle window.
        #expect(
            !(await controller.resizeNotes(
                PianoResizeChange(anchor: notes[2].id, originals: originals, edge: .rightEdge, delta: 5 * 705_600))))
        #expect(controller.undoCount == undo)
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) })
        #expect(
            await controller.resizeNotes(
                PianoResizeChange(anchor: notes[2].id, originals: originals, edge: .rightEdge, delta: 141_120)))
        #expect(controller.undoCount == undo + 1)
        for note in [notes[0], notes[2]] {
            let clip = try #require(controller.snapshot?.clipMix(note.id.placement))
            #expect(clip.start + clip.length == note.start + note.length + 141_120)
        }
        await controller.undo()
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) })
        #expect(notes.allSatisfy { controller.snapshot?.clipMix($0.id.placement) == snapshot.clipMix($0.id.placement) })
        await controller.close()
    }
}
