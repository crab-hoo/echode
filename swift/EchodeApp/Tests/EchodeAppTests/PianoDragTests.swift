import AppKit
import Carbon.HIToolbox
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoDragTests {
    @Test func contractShiftNoteDragsReleaseTheGridAndCommitOnce() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-drag-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        await controller.insertNote(pitch: 60, beat: 1.1, length: 0.7)
        let track = try #require(controller.selection), snapshot = try #require(controller.snapshot)
        let note = try #require(snapshot.pianoNeighbor(track: track, current: nil))
        let document = controller.documentID, undo = controller.undoCount
        var last = NoteGeometry(note)

        for mode in [PianoGesture.Mode.move, .leftEdge, .rightEdge] {
            // A modifier change at release must not commit the previous preview.
            for (during, released) in [(false, true), (true, false), (true, true)] {
                let harness = PianoDragHarness(snapshot: snapshot, note: note)
                defer { harness.view.stop() }
                var sent: [NoteGeometry] = []
                var writes: [Task<Bool, Never>] = []
                harness.view.onEdit = { id, geometry in
                    #expect(id == note.id)
                    sent.append(geometry)
                    writes.append(Task { await controller.editNote(id, geometry: geometry, documentID: document) })
                }
                let delta = mode == .rightEdge ? -0.6 : 0.1
                let down = harness.point(for: mode)
                let target = down.applying(CGAffineTransform(translationX: delta * harness.view.camera.pixelsPerBeat,
                                                            y: mode == .move ? -20 : 0))
                try harness.send(.leftMouseDown, at: down)
                try harness.send(.leftMouseDragged, at: target, flags: during ? .shift : [])
                #expect(sent.isEmpty && controller.undoCount == undo)
                try harness.send(.leftMouseUp, at: target, flags: released ? .shift : [])
                try harness.send(.leftMouseUp, at: target, flags: released ? .shift : [])
                let expected: NoteGeometry
                switch mode {
                case .move:
                    expected = NoteGeometry(start: ticks(released ? 1.2 : 1.25), length: note.length, pitch: 61)
                case .leftEdge:
                    expected = NoteGeometry(start: ticks(released ? 1.2 : 1.25), length: ticks(released ? 0.6 : 0.55), pitch: 60)
                case .rightEdge:
                    expected = NoteGeometry(start: note.start, length: ticks(released ? 0.1 : 0.25), pitch: 60)
                }
                #expect(sent == [expected], "Shift note drag must use the release modifier and allow sub-grid lengths")
                #expect(writes.count == 1)
                for write in writes { #expect(await write.value) }
                let edited = try #require(controller.snapshot?.pianoNote(note.id))
                #expect(NoteGeometry(edited) == expected)
                #expect(edited.velocity == note.velocity && edited.id == note.id)
                #expect(controller.undoCount == undo + 1 && controller.errorMessage == nil)
                await controller.undo()
                #expect(controller.snapshot?.pianoNote(note.id).map(NoteGeometry.init) == NoteGeometry(note))
                #expect(controller.undoCount == undo)
                last = expected
            }
        }
        await controller.redo()
        #expect(controller.snapshot?.pianoNote(note.id).map(NoteGeometry.init) == last)
        await controller.close()
        await controller.open(url)
        #expect(controller.snapshot?.pianoNote(note.id).map(NoteGeometry.init) == last)
        #expect(controller.snapshot?.pianoNote(note.id)?.velocity == note.velocity)
        await controller.close()
    }

    @Test func cancelledOrUnmovedNotePressesNeverEdit() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-cancel-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        await controller.insertNote(pitch: 60, beat: 1.1, length: 0.7)
        let snapshot = try #require(controller.snapshot), track = try #require(controller.selection)
        let note = try #require(snapshot.pianoNeighbor(track: track, current: nil))
        for cancellation in 0..<6 {
            let harness = PianoDragHarness(snapshot: snapshot, note: note)
            var sent = 0
            harness.view.onEdit = { _, _ in sent += 1 }
            let down = harness.point(for: .move), target = down.applying(CGAffineTransform(translationX: 13, y: -20))
            try harness.send(.leftMouseDown, at: down)
            if cancellation != 0 { try harness.send(.leftMouseDragged, at: target, flags: .shift) }
            switch cancellation {
            case 1:
                let escape = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                    windowNumber: 0, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}",
                    isARepeat: false, keyCode: UInt16(kVK_Escape)))
                harness.view.keyDown(with: escape)
            case 2: _ = harness.view.resignFirstResponder()
            case 3: harness.view.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: true)
            case 4: harness.view.mouseCancelled(with: try harness.mouse(.leftMouseUp, at: target, flags: .shift))
            case 5: harness.view.stop()
            default: break
            }
            try harness.send(.leftMouseUp, at: target, flags: .shift)
            #expect(sent == 0)
            harness.view.stop()
        }
        await controller.close()
    }

    @Test func noteDragBoundariesKeepPositiveLengthsAndUseTheReleasePosition() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-limits-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        await controller.insertNote(pitch: 60, beat: 1.1, length: 0.7)
        let snapshot = try #require(controller.snapshot), track = try #require(controller.selection)
        let note = try #require(snapshot.pianoNeighbor(track: track, current: nil))
        let original = NoteGeometry(note)
        func gesture(_ mode: PianoGesture.Mode, original: NoteGeometry) -> PianoGesture {
            PianoGesture(id: note.id, original: original, mode: mode, x: 0, y: 0)
        }
        let camera = PianoCamera()
        let end = note.start + note.length
        let left = gesture(.leftEdge, original: original), right = gesture(.rightEdge, original: original)
        #expect(left.preview(x: 1_000, y: 0, camera: camera, snapping: false) == NoteGeometry(start: end - 1, length: 1, pitch: 60))
        #expect(right.preview(x: -1_000, y: 0, camera: camera, snapping: false) == NoteGeometry(start: note.start, length: 1, pitch: 60))
        #expect(left.preview(x: -1_000, y: 0, camera: camera, snapping: false) == NoteGeometry(start: 0, length: end, pitch: 60))
        let move = gesture(.move, original: original)
        #expect(move.preview(x: -1_000, y: -1_000_000, camera: camera, snapping: false) == NoteGeometry(start: 0, length: note.length, pitch: 127))
        #expect(move.preview(x: 0, y: 1_000_000, camera: camera, snapping: false)?.pitch == 0)
        for pixels in [18.0, 72.0, 477.0] {
            let zoomed = PianoCamera(originBeat: 0.5, pixelsPerBeat: pixels)
            #expect(move.preview(x: pixels * 0.1, y: 0, camera: zoomed)?.start == ticks(1.25))
            #expect(move.preview(x: pixels * 0.1, y: 0, camera: zoomed, snapping: false)?.start == ticks(1.2))
        }
        for mode in [PianoGesture.Mode.move, .leftEdge, .rightEdge] {
            let drag = gesture(mode, original: original)
            #expect(drag.preview(x: .infinity, y: 0, camera: camera, snapping: false) == nil)
            #expect(drag.preview(x: 0, y: .nan, camera: camera, snapping: false) == nil)
            #expect(gesture(mode, original: NoteGeometry(start: .max, length: 1, pitch: 60))
                .preview(x: 0, y: 0, camera: camera, snapping: false) == nil)
        }
        #expect(gesture(.rightEdge, original: NoteGeometry(start: .max - 4, length: 3, pitch: 60))
            .preview(x: 72, y: 0, camera: camera, snapping: false) == nil)
        let harness = PianoDragHarness(snapshot: snapshot, note: note)
        var sent: [NoteGeometry] = []
        harness.view.onEdit = { _, geometry in sent.append(geometry) }
        let down = harness.point(for: .move)
        try harness.send(.leftMouseDown, at: down)
        try harness.send(.leftMouseDragged, at: down.applying(CGAffineTransform(translationX: 7.2, y: 0)), flags: .shift)
        try harness.send(.leftMouseUp, at: down.applying(CGAffineTransform(translationX: 14.4, y: 0)), flags: .shift)
        #expect(sent == [NoteGeometry(start: ticks(1.3), length: note.length, pitch: 60)])
        harness.view.stop()
        await controller.close()
    }

    private func ticks(_ beat: Double) -> Int64 { PianoCamera.ticks(beat)! }
}

/// Presents the product frame and delivers AppKit input without a window or HAL output.
@MainActor
private final class PianoDragHarness {
    let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
    let note: PianoNote

    init(snapshot: Snapshot, note: PianoNote) {
        self.note = note
        view.setSnapshot(snapshot, track: note.id.track, focus: note.id, key: DisplayKey(), busy: false)
        let input = CanvasInput(snapshot: snapshot, selected: note.id.track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
        view.present(PianoFrame(input: input, piano: PianoRenderInput(track: note.id.track, focus: note.id,
            camera: view.camera, key: DisplayKey(), preview: nil)))
    }

    func point(for mode: PianoGesture.Mode) -> CGPoint {
        let camera = view.camera, origin = view.pianoLayout.gridOrigin
        let start = (Double(note.start) / 705_600 - camera.originBeat) * camera.pixelsPerBeat
        let width = Double(note.length) / 705_600 * camera.pixelsPerBeat
        let offset: Double = switch mode { case .move: width / 2; case .leftEdge: 1; case .rightEdge: width - 1 }
        return CGPoint(x: origin.x + start + offset, y: origin.y + (camera.topPitch - Double(note.pitch) + 0.5) * camera.pixelsPerPitch)
    }

    func mouse(_ type: NSEvent.EventType, at point: CGPoint, flags: NSEvent.ModifierFlags) throws -> NSEvent {
        try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: flags,
            timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
    }

    func send(_ type: NSEvent.EventType, at point: CGPoint, flags: NSEvent.ModifierFlags = []) throws {
        let event = try mouse(type, at: point, flags: flags)
        switch type {
        case .leftMouseDown: view.mouseDown(with: event)
        case .leftMouseDragged: view.mouseDragged(with: event)
        default: view.mouseUp(with: event)
        }
    }
}
