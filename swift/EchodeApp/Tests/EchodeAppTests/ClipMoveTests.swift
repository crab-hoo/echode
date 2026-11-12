import AppKit
import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

@MainActor
struct ClipMoveTests {
    private let beat: Int64 = 705_600

    private func mouse(
        _ type: NSEvent.EventType, _ point: CGPoint, in view: NSView,
        flags: NSEvent.ModifierFlags = []
    ) throws -> NSEvent {
        try #require(
            NSEvent.mouseEvent(
                with: type, location: view.convert(point, to: nil), modifierFlags: flags, timestamp: 0,
                windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
    }

    private func key(_ code: UInt16, in view: NSView) throws -> NSEvent {
        try #require(
            NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
                context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false,
                keyCode: code))
    }

    private func clips(_ snapshot: Snapshot) -> [UUID] {
        // The window bounds only these fixtures; it is not a product size cap.
        snapshot.visibleElements(
            originTick: 0, widthTicks: 100_000_000, originTrack: 0, heightTracks: 10,
            pixelsPerTick: 0.001, pixelsPerTrack: 40
        ).elements.compactMap { $0.role == .clip ? $0.id.placement : nil }
    }

    @Test func contractBodyDragMovesAndCommandDragCopiesClipsAsOneUndoAndReopen() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-clip-move-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("clip.wav")
        let url = root.appendingPathComponent("Project.echode")
        // One second at 120 bpm: two beats, 144 points at the pane's 72 points per beat.
        try wavMono(rate: 48_000, frames: 48_000).write(to: wav)
        let controller = ProjectController()
        let session = WorkspaceSession(controller: controller, state: WorkspaceState(defaults: .init()))
        await controller.open(url, creating: true)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "second")
        let second = try #require(controller.selection)
        #expect(await controller.importAudio(wav, on: first))
        let imported = try #require(controller.snapshot)
        let id = try #require(clips(imported).first)
        #expect(controller.snapshot?.clipMix(id)?.start == 0)
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 1200, height: 192))
        pane.layout()
        pane.timeline.audioDocument = controller.documentID
        var pending: Task<Bool, Never>?
        var requests: [ClipMove] = []
        pane.onClipMove = {
            requests.append($0)
            pending = session.moveClip($0)
        }
        pane.onSelect = { controller.selection = $0 }
        pane.onSelectClip = { controller.clipFocus = $0 }
        let origin = 161.0
        let perBeat = 72.0
        func present() throws {
            pane.setSnapshot(try #require(controller.snapshot), selected: controller.selection)
            pane.timeline.present(ArrangeFrame(input: pane.timeline.renderInput))
        }
        /// A press low in the clip body, clear of the gain and fade handles.
        func body(_ clip: UUID) throws -> CGPoint {
            let mix = try #require(controller.snapshot?.clipMix(clip))
            let row = try #require((0..<2).first { controller.snapshot?.trackID(at: $0) == mix.track })
            return CGPoint(x: origin + Double(mix.start) / Double(beat) * perBeat + 30, y: 24 + Double(row) * 64 + 52)
        }
        func drag(_ clip: UUID, by offset: CGSize, flags: NSEvent.ModifierFlags = []) async throws {
            try present()
            let down = try body(clip)
            let up = CGPoint(x: down.x + offset.width, y: down.y + offset.height)
            pane.timeline.mouseDown(with: try mouse(.leftMouseDown, down, in: pane.timeline))
            pane.timeline.mouseDragged(with: try mouse(.leftMouseDragged, up, in: pane.timeline, flags: flags))
            pane.timeline.mouseUp(with: try mouse(.leftMouseUp, up, in: pane.timeline, flags: flags))
            _ = await pending?.value
            pending = nil
        }

        // A drag of 1.3 beats and one row lands on the next quarter of the second track.
        try present()
        let down = try body(id)
        pane.timeline.mouseDown(with: try mouse(.leftMouseDown, down, in: pane.timeline))
        #expect(controller.clipFocus == id)
        let over = CGPoint(x: down.x + 1.3 * perBeat, y: down.y + 64)
        pane.timeline.mouseDragged(with: try mouse(.leftMouseDragged, over, in: pane.timeline))
        let preview = try #require(pane.timeline.renderInput.clipMove)
        #expect(preview == ClipMovePreview(row: 1, track: second, start: beat, length: 2 * beat, copy: false))
        let ghost = try #require(ArrangeFrame(input: pane.timeline.renderInput).clipMoveRect)
        #expect(ghost == CGRect(x: origin + perBeat, y: 24 + 64, width: 2 * perBeat, height: 63))
        #expect(controller.snapshot?.clipMix(id)?.track == first, "a drag only previews")
        let before = controller.undoCount
        pane.timeline.mouseUp(with: try mouse(.leftMouseUp, over, in: pane.timeline))
        #expect(await (try #require(pending)).value)
        #expect(controller.snapshot?.clipMix(id)?.track == second && controller.snapshot?.clipMix(id)?.start == beat)
        #expect(controller.undoCount == before + 1 && controller.selection == second && controller.clipFocus == id)
        #expect(pane.timeline.renderInput.clipMove == nil)

        // Shift keeps the pointer's tick instead of the quarter grid.
        try await drag(id, by: CGSize(width: -0.4 * perBeat, height: 0), flags: .shift)
        #expect(controller.snapshot?.clipMix(id)?.start == beat - beat * 2 / 5)
        #expect(controller.undoCount == before + 2)

        // Command at release copies to the first track and focuses the copy.
        try await drag(id, by: CGSize(width: 1.4 * perBeat, height: -64), flags: .command)
        #expect(requests.last?.copy == true)
        let copy = try #require(controller.clipFocus)
        #expect(copy != id && controller.selection == first)
        #expect(
            controller.snapshot?.clipMix(copy)?.track == first && controller.snapshot?.clipMix(copy)?.start == 2 * beat)
        #expect(controller.snapshot?.clipMix(id)?.track == second, "the original stays")
        #expect(controller.undoCount == before + 3)
        let moved = try #require(controller.snapshot?.clipMix(id))

        // A click, a press inside the threshold, Escape, and a cancelled
        // gesture leave the document alone.
        try await drag(id, by: CGSize(width: 2, height: 1))
        try present()
        let escape = try body(id)
        pane.timeline.mouseDown(with: try mouse(.leftMouseDown, escape, in: pane.timeline))
        pane.timeline.mouseDragged(
            with: try mouse(.leftMouseDragged, CGPoint(x: escape.x + 300, y: escape.y), in: pane.timeline))
        #expect(pane.timeline.renderInput.clipMove != nil)
        pane.timeline.keyDown(with: try key(53, in: pane.timeline))
        #expect(pane.timeline.renderInput.clipMove == nil)
        pane.timeline.mouseUp(with: try mouse(.leftMouseUp, CGPoint(x: escape.x + 300, y: escape.y), in: pane.timeline))
        pane.timeline.mouseDown(with: try mouse(.leftMouseDown, escape, in: pane.timeline))
        pane.timeline.mouseDragged(
            with: try mouse(.leftMouseDragged, CGPoint(x: escape.x + 300, y: escape.y), in: pane.timeline))
        pane.timeline.mouseCancelled(with: try mouse(.leftMouseUp, escape, in: pane.timeline))
        pane.timeline.mouseUp(with: try mouse(.leftMouseUp, CGPoint(x: escape.x + 300, y: escape.y), in: pane.timeline))
        #expect(pending == nil && requests.count == 3)
        #expect(controller.snapshot?.clipMix(id) == moved && controller.undoCount == before + 3)

        await controller.undo()
        #expect(controller.snapshot?.clipMix(copy) == nil)
        await controller.undo()
        await controller.undo()
        #expect(controller.snapshot?.clipMix(id)?.track == first && controller.snapshot?.clipMix(id)?.start == 0)
        await controller.redo()
        await controller.redo()
        await controller.redo()
        #expect(controller.snapshot?.clipMix(id) == moved && controller.snapshot?.clipMix(copy)?.start == 2 * beat)
        await controller.close()
        await controller.open(url)
        #expect(controller.snapshot?.clipMix(id) == moved && controller.snapshot?.clipMix(copy)?.track == first)
        await controller.close()
    }

    @Test func landingReplacesOverlapsAndStaleRequestsAreRefused() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-clip-replace-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("clip.wav")
        try wavMono(rate: 48_000, frames: 48_000).write(to: wav)
        let controller = ProjectController()
        await controller.open(root.appendingPathComponent("Project.echode"), creating: true)
        await controller.addTrack(name: "only")
        let track = try #require(controller.selection)
        let document = try #require(controller.documentID)
        #expect(await controller.importAudio(wav, on: track))
        let initial = try #require(controller.snapshot)
        let first = try #require(clips(initial).first)
        #expect(await controller.edit(.importAudio(track: track.uuidString, start: 4 * beat, path: wav.path)))
        let both = try #require(controller.snapshot)
        let second = try #require(clips(both).first { $0 != first })
        // Landing one beat into the second clip keeps its last beat as a trimmed piece.
        let count = controller.undoCount
        #expect(
            await controller.moveClip(
                ClipMove(document: document, clip: first, track: track, start: 3 * beat, copy: false)))
        #expect(
            controller.snapshot?.clipMix(second)?.start == 5 * beat
                && controller.snapshot?.clipMix(second)?.length == beat)
        #expect(controller.undoCount == count + 1)
        // No displacement, a removed track, and another document send nothing.
        #expect(
            !(await controller.moveClip(
                ClipMove(document: document, clip: first, track: track, start: 3 * beat, copy: true))))
        #expect(
            !(await controller.moveClip(ClipMove(document: document, clip: first, track: UUID(), start: 0, copy: false)))
        )
        #expect(
            !(await controller.moveClip(
                ClipMove(document: DocumentID(), clip: first, track: track, start: 0, copy: false))))
        #expect(
            !(await controller.moveClip(ClipMove(document: document, clip: first, track: track, start: -1, copy: false)))
        )
        #expect(controller.undoCount == count + 1)
        await controller.undo()
        #expect(
            controller.snapshot?.clipMix(second)?.start == 4 * beat && controller.snapshot?.clipMix(first)?.start == 0)
        await controller.close()
    }

    @Test func targetsSnapToQuartersAndClampToTheTrackList() {
        #expect(ClipMoveTarget.start(from: 0, beats: 1.49, free: false) == beat)
        #expect(ClipMoveTarget.start(from: beat / 3, beats: 0.2, free: false) == beat)
        #expect(ClipMoveTarget.start(from: 0, beats: 0.25, free: true) == beat / 4)
        #expect(ClipMoveTarget.start(from: beat, beats: -5, free: true) == 0)
        #expect(ClipMoveTarget.start(from: 0, beats: .nan, free: false) == nil)
        let rows = ArrangeRows(origin: 0.5, rowHeight: 64, height: 192, count: 3)
        #expect(rows.clampedIndex(at: -500) == 0 && rows.clampedIndex(at: 10) == 0)
        #expect(rows.clampedIndex(at: 40) == 1 && rows.clampedIndex(at: 5_000) == 2)
        #expect(ArrangeRows(origin: 0, rowHeight: 64, height: 192, count: 0).clampedIndex(at: 10) == nil)
    }
}
