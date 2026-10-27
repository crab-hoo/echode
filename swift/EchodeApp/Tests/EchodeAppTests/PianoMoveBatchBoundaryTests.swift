import AppKit
@testable import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoMoveBatchBoundaryTests {
    @Test func contractLinkedMovesValidateEveryTargetAndShareBounds() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-move-linked-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "linked keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 0.7))
        #expect(await controller.insertNote(pitch: 127, beat: 2, length: 0.7))
        let track = try #require(controller.selection), document = try #require(controller.documentID)
        let first = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        #expect(await controller.edit(["type": "clip.link", "source": first.id.placement.uuidString,
                                      "track": track.uuidString, "start": 8 * 705_600]))
        let snapshot = try #require(controller.snapshot)
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 16 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        try #require(notes.count == 4)
        let ids = Set(notes.map(\.id)), originals = Dictionary(uniqueKeysWithValues: notes.map { ($0.id, NoteGeometry($0)) })
        controller.selectNotes(ids)
        let harness = PianoMoveBatchHarness(snapshot: snapshot, track: track, selected: ids)
        defer { harness.view.stop() }
        let point = harness.point(first), undo = controller.undoCount
        var writes: [Task<Bool, Never>] = []
        harness.view.onMoves = { change in writes.append(Task { await controller.moveNotes(change, documentID: document) }) }
        try harness.send(.leftMouseDown, at: point)
        try harness.send(.leftMouseDragged, at: CGPoint(x: point.x - 1000, y: point.y - 1000), flags: .shift)
        let low = try #require(harness.view.movePreview)
        #expect(low.change.time == -first.start && low.change.pitch == 0,
                "all selected notes including offscreen links must share the time and pitch bounds")
        #expect(low.change.geometries?.values.map(\.start).min() == 0)
        #expect(harness.frame(low).gridInstances.filter { $0.color == SIMD4<Float>(0.96, 0.87, 0.54, 1) }.count == 4)
        let end = CGPoint(x: point.x + 7.2, y: point.y + 20)
        try harness.send(.leftMouseUp, at: end, flags: .shift)
        try #require(writes.count == 1)
        #expect(await writes[0].value)
        let change = PianoMoveChange(anchor: first.id, originals: originals, time: 70_560, pitch: -1)
        #expect(change.actions(in: snapshot)?.count == 2, "linked content must produce one move action per note")
        #expect(controller.undoCount == undo + 1 && controller.noteSelection == ids)
        for note in notes {
            let edited = try #require(controller.snapshot?.pianoNote(note.id))
            #expect(edited.start == note.start + 70_560 && edited.pitch == note.pitch - 1)
            #expect(edited.length == note.length && edited.velocity == note.velocity)
        }
        #expect(!(await controller.moveNotes(change, documentID: document)), "a stale original must reject the whole move")
        await controller.undo()
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) })
        var stale = originals
        stale[first.id]?.start += 1
        var missing = originals
        missing[PianoNote.ID(track: track, placement: first.id.placement, note: UUID())] = NoteGeometry(first)
        for invalid in [stale, missing] {
            #expect(!(await controller.moveNotes(PianoMoveChange(anchor: first.id, originals: invalid, time: 1, pitch: 0))))
        }
        for (time, pitch) in [(Int64(0), 0), (-2 * 705_600, 0), (1, 1), (Int64.max, 0), (0, Int.max)] {
            #expect(!(await controller.moveNotes(PianoMoveChange(anchor: first.id, originals: originals, time: time, pitch: pitch))))
        }
        #expect(!(await controller.moveNotes(change, documentID: UUID())))
        #expect(controller.undoCount == undo)
        await controller.redo()
        await controller.close()
        await controller.open(url)
        #expect(!(await controller.moveNotes(change, documentID: document)))
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id)?.start == $0.start + 70_560 })
        await controller.close()

        let second = try #require(notes.first { $0.id.note != first.id.note })
        let edge = [first.id: NoteGeometry(start: 10, length: 1, pitch: 1),
                    second.id: NoteGeometry(start: Int64.max - 20, length: 10, pitch: 126)]
        let gesture = PianoMoveGesture(anchor: first.id, originals: edge, point: .zero, camera: PianoCamera())
        let high = try #require(gesture.preview(at: CGPoint(x: 100, y: -1000), snapping: false))
        #expect(high.change.time == 10 && high.change.pitch == 1, "the last end must constrain the entire move before overflow")
        let down = try #require(gesture.preview(at: CGPoint(x: -100, y: 1000), snapping: false))
        #expect(down.change.time == -10 && down.change.pitch == -1)
        let earlier = [first.id: NoteGeometry(start: 10, length: 1, pitch: 1), second.id: NoteGeometry(start: 20, length: 1, pitch: 126)]
        let laterAnchor = PianoMoveGesture(anchor: second.id, originals: earlier, point: .zero, camera: PianoCamera())
        #expect(laterAnchor.preview(at: CGPoint(x: -100, y: 0), snapping: false)?.change.time == -10)
        #expect(gesture.preview(at: CGPoint(x: CGFloat.infinity, y: 0), snapping: false) == nil)
        #expect(gesture.preview(at: CGPoint(x: 0, y: CGFloat.nan), snapping: false) == nil)
        let linkedPreview = PianoMovePreview(change: PianoMoveChange(anchor: first.id, originals: [first.id: NoteGeometry(first)], time: 1, pitch: 0))
        #expect(linkedPreview.geometries(in: notes).count == 2, "unselected aliases must preview their shared content move")
    }

    @Test func batchTimeMovesGrowFixedWindowsAndUndoTogether() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-move-fixed-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "fixed keys")
        for pitch in [60, 62] { #expect(await controller.insertNote(pitch: pitch, beat: Double(pitch - 59), length: 0.7)) }
        let track = try #require(controller.selection)
        let first = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        #expect(await controller.edit(["type": "clip.resize", "track": track.uuidString, "clip": first.id.placement.uuidString,
                                      "start": 0, "length": 4 * 705_600]))
        let snapshot = try #require(controller.snapshot), clip = try #require(snapshot.clipMix(first.id.placement))
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 8 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        let originals = Dictionary(uniqueKeysWithValues: notes.map { ($0.id, NoteGeometry($0)) })
        let undo = controller.undoCount
        #expect(await controller.moveNotes(PianoMoveChange(anchor: first.id, originals: originals, time: 4 * 705_600, pitch: 0)))
        let end = try #require(notes.map { $0.start + $0.length }.max()) + 4 * 705_600
        #expect(controller.snapshot?.clipMix(first.id.placement)?.length == end)
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id)?.start == $0.start + 4 * 705_600 })
        #expect(controller.undoCount == undo + 1)
        await controller.undo()
        #expect(controller.snapshot?.clipMix(first.id.placement) == clip)
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) })
        await controller.close()
    }

    @Test func anOverlapFailureRollsBackEarlierMovesInTheTransaction() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-move-rollback-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let track = try #require(controller.selection)
        for beat in [1.0, 6.0, 9.0] {
            #expect(await controller.insertNote(pitch: 60, beat: beat, length: 0.7))
            let current = try #require(controller.snapshot)
            let latest = try #require(current.pianoNotes(track: track, originTick: 0, widthTicks: 16 * 705_600,
                topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1).max { $0.start < $1.start })
            let clip = try #require(current.clipMix(latest.id.placement))
            #expect(await controller.edit(["type": "clip.resize", "track": track.uuidString, "clip": latest.id.placement.uuidString,
                                          "start": clip.start, "length": clip.length]))
        }
        let snapshot = try #require(controller.snapshot)
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 16 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1).sorted { $0.start < $1.start }
        try #require(notes.count == 3 && Set(notes.map(\.id.placement)).count == 3)
        let originals = Dictionary(uniqueKeysWithValues: [notes[0], notes[2]].map { ($0.id, NoteGeometry($0)) })
        let undo = controller.undoCount
        // Move the last clip first successfully, then collide with the middle clip.
        #expect(!(await controller.moveNotes(PianoMoveChange(anchor: notes[2].id, originals: originals, time: 4 * 705_600, pitch: 0))))
        #expect(controller.undoCount == undo)
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id).map(NoteGeometry.init) == NoteGeometry($0) })
        await controller.close()
    }
}
