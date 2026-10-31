import AppKit
import Carbon.HIToolbox
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoInsertionTests {
    @Test func contractBlankDragsInsertOnReleaseAndKeepUndoAndSave() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-insert-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let track = try #require(controller.selection), snapshot = try #require(controller.snapshot)
        let document = try #require(controller.documentID), undo = controller.undoCount
        var last: NoteGeometry?
        for (downShift, releaseShift) in [(false, false), (true, true), (true, false), (false, true)] {
            let harness = PianoInsertionHarness(snapshot: snapshot, track: track)
            defer { harness.view.stop() }
            var sent: [NoteGeometry] = []
            var writes: [Task<Bool, Never>] = []
            harness.view.onInsert = { pitch, beat, length in
                sent.append(NoteGeometry(start: self.ticks(beat), length: self.ticks(length), pitch: pitch))
                writes.append(Task { await controller.insertNote(pitch: pitch, beat: beat, length: length,
                                                                 on: track, documentID: document) })
            }
            let down = harness.point(beat: 1.1, pitch: 60)
            try harness.send(.leftMouseDown, at: down, flags: downShift ? .shift : [])
            try harness.send(.leftMouseDragged, at: harness.point(beat: 1.7, pitch: 61), flags: [])
            #expect(sent.isEmpty && controller.undoCount == undo)
            let up = harness.point(beat: 2.1, pitch: 62)
            try harness.send(.leftMouseUp, at: up, flags: releaseShift ? .shift : [])
            try harness.send(.leftMouseUp, at: up, flags: releaseShift ? .shift : [])
            let start = ticks(downShift ? 1.1 : 1)
            let end = ticks(releaseShift ? 2.1 : 2)
            let expected = NoteGeometry(start: start, length: end - start, pitch: 62)
            #expect(sent == [expected], "blank note drag must insert once using the fixed start and release geometry")
            #expect(writes.count == 1)
            for write in writes { #expect(await write.value) }
            let inserted = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
            #expect(NoteGeometry(inserted) == expected)
            #expect(inserted.velocity >> 9 == 100)
            #expect(controller.undoCount == undo + 1 && controller.errorMessage == nil)
            await controller.undo()
            #expect(controller.snapshot?.pianoNeighbor(track: track, current: nil) == nil)
            #expect(controller.undoCount == undo)
            last = expected
        }
        await controller.redo()
        #expect(controller.snapshot?.pianoNeighbor(track: track, current: nil).map(NoteGeometry.init) == last)
        await controller.close()
        await controller.open(url)
        #expect(controller.snapshot?.pianoNeighbor(track: track, current: nil).map(NoteGeometry.init) == last)
        await controller.close()
    }

    @Test func blankPressCancellationNeverInsertsOrLeavesAPreview() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-insert-cancel-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let track = try #require(controller.selection), snapshot = try #require(controller.snapshot)
        for cancellation in 0..<10 {
            let harness = PianoInsertionHarness(snapshot: snapshot, track: track)
            var inserts = 0
            harness.view.onInsert = { _, _, _ in inserts += 1 }
            let down = harness.point(beat: 1, pitch: 60), up = harness.point(beat: 2, pitch: 61)
            try harness.send(.leftMouseDown, at: down)
            try harness.send(.leftMouseDragged, at: up)
            #expect(harness.view.displayedInsertion != nil)
            switch cancellation {
            case 0:
                let escape = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                    windowNumber: 0, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}",
                    isARepeat: false, keyCode: UInt16(kVK_Escape)))
                harness.view.keyDown(with: escape)
            case 1: _ = harness.view.resignFirstResponder()
            case 2: harness.view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: true)
            case 3: harness.view.stop()
            case 4: harness.view.setFrameSize(NSSize(width: 650, height: 410))
            case 5: harness.view.setBoundsSize(NSSize(width: 630, height: 390))
            case 6:
                let scroll = try #require(CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: 1, wheel2: 0, wheel3: 0))
                harness.view.scrollWheel(with: try #require(NSEvent(cgEvent: scroll)))
            case 7: harness.view.setSnapshot(snapshot, track: UUID(), focus: nil, key: DisplayKey(), busy: false)
            case 8:
                await controller.addTrack(name: "new snapshot")
                harness.view.setSnapshot(try #require(controller.snapshot), track: track, focus: nil, key: DisplayKey(), busy: false)
            default:
                harness.view.mouseCancelled(with: try #require(NSEvent.mouseEvent(with: .leftMouseUp, location: up,
                    modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 0)))
            }
            #expect(harness.view.displayedInsertion == nil)
            try harness.send(.leftMouseUp, at: up)
            #expect(inserts == 0)
            harness.view.stop()
        }
        await controller.close()
    }

    @Test func blankClicksInsertOnceAndDrawTheInsertion() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-insert-click-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let track = try #require(controller.selection), snapshot = try #require(controller.snapshot)
        let harness = PianoInsertionHarness(snapshot: snapshot, track: track)
        defer { harness.view.stop() }
        var sent: [NoteGeometry] = []
        harness.view.onInsert = { pitch, beat, length in
            sent.append(NoteGeometry(start: self.ticks(beat), length: self.ticks(length), pitch: pitch))
        }
        let down = harness.point(beat: 1, pitch: 60)
        try harness.send(.leftMouseDown, at: down)
        try harness.send(.leftMouseUp, at: CGPoint(x: down.x + 1, y: down.y + 1))
        #expect(sent == [NoteGeometry(start: ticks(1), length: ticks(0.25), pitch: 60)] && harness.view.displayedInsertion == nil)
        try harness.send(.leftMouseDown, at: down, clicks: 2)
        #expect(sent.count == 1)
        try harness.send(.leftMouseUp, at: down, clicks: 2)
        #expect(sent == [NoteGeometry(start: ticks(1), length: ticks(0.25), pitch: 60)])
        try harness.send(.leftMouseDown, at: down)
        try harness.send(.leftMouseDragged, at: harness.point(beat: 3, pitch: 62))
        let geometry = try #require(harness.view.displayedInsertion)
        #expect(geometry == NoteGeometry(start: ticks(1), length: ticks(2), pitch: 62))
        let camera = harness.view.camera
        harness.view.follower.show?(ticks(20))
        #expect(harness.view.camera == camera, "a blank note press must hold the viewport against playhead following")
        let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 2)
        let plain = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil,
            camera: harness.view.camera, key: DisplayKey(), preview: nil))
        let frame = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil,
            camera: harness.view.camera, key: DisplayKey(), preview: nil, insertion: geometry))
        #expect(frame.notes.isEmpty && frame.gridInstances.count == plain.gridInstances.count + 1)
        let rect = try #require(frame.gridInstances.last).rect
        let point = harness.point(beat: 1, pitch: 62)
        #expect(rect == SIMD4(Float(point.x * 2), Float((point.y - 10 + 1) * 2), 288, 36))
        // The previously drawn insertion cannot be treated as an existing model note.
        harness.view.present(frame)
        try harness.send(.leftMouseUp, at: harness.point(beat: 0, pitch: 60), flags: .shift)
        #expect(sent.last == NoteGeometry(start: ticks(1), length: ticks(0.25), pitch: 60))
        let count = sent.count
        try harness.send(.leftMouseDown, at: down, clicks: 2)
        try harness.send(.leftMouseUp, at: down, clicks: 2)
        #expect(sent.count == count)
        await controller.close()
    }

    @Test func insertionGeometryKeepsItsStartAndGridMinimumAndRejectsInvalidInputs() throws {
        for pixels in [18.0, 72.0, 477.0] {
            let camera = PianoCamera(originBeat: 0.5, pixelsPerBeat: pixels)
            var drag = try #require(PianoInsertion(x: pixels * 0.6, y: 90, camera: camera, snapping: false))
            #expect(drag.original == NoteGeometry(start: ticks(1.1), length: camera.grid, pitch: 60))
            drag.update(x: pixels * 2, y: 50, snapping: false)
            #expect(drag.geometry == NoteGeometry(start: ticks(1.1), length: ticks(1.4), pitch: 62))
            drag.update(x: -1_000, y: 1_000_000, snapping: false)
            #expect(drag.geometry == NoteGeometry(start: ticks(1.1), length: camera.grid, pitch: 0))
            drag.update(x: pixels * 2, y: -1_000_000, snapping: true)
            #expect(drag.geometry?.pitch == 127)
            drag.update(x: .infinity, y: 0, snapping: false)
            #expect(drag.geometry == nil)
            drag.update(x: 0, y: .nan, snapping: false)
            #expect(drag.geometry == nil)
            drag.update(x: .greatestFiniteMagnitude, y: 0, snapping: false)
            #expect(drag.geometry == nil)
        }
        for camera in [PianoCamera(gridMode: .fixed(0)), PianoCamera(pixelsPerBeat: 0), PianoCamera(pixelsPerPitch: .nan),
                       PianoCamera(originBeat: .infinity), PianoCamera(originBeat: Double(Int64.max) / 705_600)] {
            #expect(PianoInsertion(x: 0, y: 0, camera: camera, snapping: false) == nil)
        }
        #expect(PianoInsertion(x: .nan, y: 0, camera: PianoCamera(), snapping: true) == nil)
    }

    private func ticks(_ beat: Double) -> Int64 { Tick.ticks(beat)! }
}

/// Uses the accepted product frame and AppKit input, without a window or audio device.
@MainActor
private final class PianoInsertionHarness {
    let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))

    init(snapshot: Snapshot, track: UUID) {
        view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false)
        let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
        view.present(PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil,
            camera: view.camera, key: DisplayKey(), preview: nil)))
    }

    func point(beat: Double, pitch: Int) -> CGPoint {
        let camera = view.camera, origin = view.pianoLayout.gridOrigin
        return CGPoint(x: origin.x + (beat - camera.originBeat) * camera.pixelsPerBeat,
                       y: origin.y + (camera.topPitch - Double(pitch) + 0.5) * camera.pixelsPerPitch)
    }

    func send(_ type: NSEvent.EventType, at point: CGPoint, flags: NSEvent.ModifierFlags = [], clicks: Int = 1) throws {
        let event = try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: flags,
            timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: clicks, pressure: 1))
        switch type {
        case .leftMouseDown: view.mouseDown(with: event)
        case .leftMouseDragged: view.mouseDragged(with: event)
        default: view.mouseUp(with: event)
        }
    }
}
