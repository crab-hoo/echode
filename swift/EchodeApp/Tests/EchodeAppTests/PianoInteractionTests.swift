import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

/// AppKit can drop a mouse-up, for example when a menu or a drag session takes
/// the event stream. The next press must start over, not resume the old one.
@MainActor
struct PianoInteractionTests {
    private func fixture() async throws -> (Snapshot, UUID, PianoNote, Set<PianoNote.ID>, URL) {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-interaction-\(UUID()).echode")
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        #expect(await controller.insertNote(pitch: 64, beat: 3, length: 1))
        let snapshot = try #require(controller.snapshot), track = try #require(controller.selection)
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 8 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        let first = try #require(notes.first(where: { $0.pitch == 60 }))
        return (snapshot, track, first, Set(notes.map(\.id)), url)
    }

    private func everyInteraction(_ id: PianoNote.ID) throws -> [(name: String, value: PianoInteraction, keptBySelection: Bool, keptByResize: Bool)] {
        let camera = PianoCamera(), geometry = NoteGeometry(start: 0, length: 705_600, pitch: 60), point = CGPoint(x: 10, y: 10)
        return [
            ("ruler", .ruler(RulerPress(x: 10, originBeat: 0, pixelsPerBeat: camera.pixelsPerBeat, grid: camera.grid)), true, false),
            ("marquee", .marquee(PianoSelectionPress(at: point)), true, false),
            ("insertion", .insertion(try #require(PianoInsertion(x: 10, y: 10, camera: camera, snapping: true))), true, false),
            ("note", .note(PianoGesture(id: id, original: geometry, mode: .move, x: 10, y: 10), preview: nil), true, true),
            ("velocity", .velocity(PianoVelocityGesture(id: id, original: 100, y: 10), preview: nil), false, false),
            ("move", .move(PianoMoveGesture(anchor: id, originals: [id: geometry], point: point, camera: camera), preview: nil), false, false),
            ("resize", .resize(PianoResizeGesture(anchor: id, originals: [id: geometry], edge: .rightEdge, point: point, camera: camera), preview: nil), false, false),
        ]
    }

    @Test func selectionAndResizeEndOnlyTheInteractionsBoundToThem() async throws {
        let (_, _, first, _, url) = try await fixture()
        defer { removeTestFile(url) }
        for (name, value, keptBySelection, keptByResize) in try everyInteraction(first.id) {
            #expect(!value.isIdle, "\(name)")
            var selection = value, resize = value
            selection.endIfBoundToSelection()
            resize.endOnResize()
            #expect(selection.isIdle == !keptBySelection, "\(name)")
            #expect(resize.isIdle == !keptByResize, "\(name)")
        }
    }

    @Test func aNoteTransformDrawsNothingUntilItsDragThreshold() async throws {
        let (_, _, first, _, url) = try await fixture()
        defer { removeTestFile(url) }
        let camera = PianoCamera()
        let values = try everyInteraction(first.id)
        var move = try #require(values.first { $0.name == "move" }).value
        var resize = try #require(values.first { $0.name == "resize" }).value
        move.drag(to: CGPoint(x: 11, y: 11), snapping: true, camera: camera)
        resize.drag(to: CGPoint(x: 12, y: 40), snapping: true, camera: camera)
        #expect(move.movePreview == nil && resize.resizePreview == nil, "a press below the threshold is still a click")
        move.drag(to: CGPoint(x: 40, y: 10), snapping: true, camera: camera)
        resize.drag(to: CGPoint(x: 40, y: 10), snapping: true, camera: camera)
        #expect(move.movePreview != nil && resize.resizePreview != nil)
    }

    @Test func aLeftDragLeavesTheMarqueeAndIdleUntouched() async throws {
        var marquee = PianoInteraction.marquee(PianoSelectionPress(at: CGPoint(x: 10, y: 10)))
        var idle = PianoInteraction.idle
        #expect(!marquee.followsLeftDrag && !idle.followsLeftDrag)
        marquee.drag(to: CGPoint(x: 90, y: 90), snapping: true, camera: PianoCamera())
        idle.drag(to: CGPoint(x: 90, y: 90), snapping: true, camera: PianoCamera())
        #expect(marquee.selectionPress?.dragged == false && idle.isIdle)
    }

    @Test func anEdgePressAfterAnUnreleasedGroupMoveResizes() async throws {
        let (snapshot, track, first, ids, url) = try await fixture()
        defer { removeTestFile(url) }
        let harness = PianoMoveBatchHarness(snapshot: snapshot, track: track, selected: ids)
        defer { harness.view.stop() }
        var moves = 0, resizes = 0
        harness.view.onMoves = { _ in moves += 1 }
        harness.view.onResizes = { _ in resizes += 1 }

        try harness.send(.leftMouseDown, at: harness.point(first))
        let edge = harness.edgePoint(first, edge: .rightEdge)
        try harness.send(.leftMouseDown, at: edge)
        let target = CGPoint(x: edge.x + 60, y: edge.y)
        try harness.send(.leftMouseDragged, at: target)
        #expect(harness.view.resizePreview != nil, "the edge press must own the drag")
        #expect(harness.view.movePreview == nil, "the unreleased move must not survive the next press")

        try harness.send(.leftMouseUp, at: target)
        #expect(resizes == 1 && moves == 0)
    }

    /// The drawn preview makes the frame stale, so this press cannot start a new
    /// interaction. It still ends the old one, whose drag must not resume.
    @Test func aPressDuringAnUnreleasedPreviewEndsItWithoutCommitting() async throws {
        let (snapshot, track, first, ids, url) = try await fixture()
        defer { removeTestFile(url) }
        let harness = PianoMoveBatchHarness(snapshot: snapshot, track: track, selected: ids)
        defer { harness.view.stop() }
        var writes = 0
        harness.view.onMoves = { _ in writes += 1 }
        harness.view.onResizes = { _ in writes += 1 }

        let body = harness.point(first)
        let target = CGPoint(x: body.x + 60, y: body.y - 40)
        try harness.send(.leftMouseDown, at: body)
        try harness.send(.leftMouseDragged, at: target)
        let preview = try #require(harness.view.movePreview)
        harness.view.present(harness.frame(preview))

        try harness.send(.leftMouseDown, at: target)
        #expect(harness.view.movePreview == nil, "the next press must end the unreleased drag")
        try harness.send(.leftMouseDragged, at: CGPoint(x: target.x + 60, y: target.y))
        try harness.send(.leftMouseUp, at: CGPoint(x: target.x + 60, y: target.y))
        #expect(writes == 0, "the unreleased drag must not commit on a later release")
    }

    @Test func aNotePressAfterAnUnreleasedRulerPressEditsTheNote() async throws {
        let (snapshot, track, first, _, url) = try await fixture()
        defer { removeTestFile(url) }
        let harness = PianoMoveBatchHarness(snapshot: snapshot, track: track, selected: [first.id])
        defer { harness.view.stop() }
        var edits = 0, requests = 0
        harness.view.onEdit = { _, _ in edits += 1 }
        harness.view.onSetStart = { _ in requests += 1 }
        harness.view.onSetLoop = { _ in requests += 1 }

        let ruler = harness.view.pianoLayout.ruler
        try harness.send(.leftMouseDown, at: CGPoint(x: ruler.midX, y: ruler.midY))
        let body = harness.point(first)
        try harness.send(.leftMouseDown, at: body)
        let target = CGPoint(x: body.x + 60, y: body.y - 40)
        try harness.send(.leftMouseDragged, at: target)
        try harness.send(.leftMouseUp, at: target)
        #expect(requests == 0, "a note gesture must not set the start or the loop")
        #expect(edits == 1, "the note press must own the release")
    }
}
