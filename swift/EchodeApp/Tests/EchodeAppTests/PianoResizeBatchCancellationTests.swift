import AppKit
import Carbon.HIToolbox
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoResizeBatchCancellationTests {
    @Test func contractGroupResizesCancelWithoutCommittingOrLosingSelection() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-resize-cancel-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1.1, length: 0.7))
        #expect(await controller.insertNote(pitch: 62, beat: 2.2, length: 0.4))
        let snapshot = try #require(controller.snapshot), track = try #require(controller.selection)
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 8 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        let first = try #require(notes.first), ids = Set(notes.map(\.id))
        await controller.addTrack(name: "new snapshot")
        let replacement = try #require(controller.snapshot)
        for edge in [PianoGesture.Mode.leftEdge, .rightEdge] {
            for mode in 0..<15 {
                let harness = PianoMoveBatchHarness(snapshot: snapshot, track: track, selected: ids)
                defer { harness.view.stop() }
                let view = harness.view, point = harness.edgePoint(first, edge: edge)
                let end = CGPoint(x: point.x + 30, y: point.y - 20)
                var writes = 0
                view.onResizes = { _ in writes += 1 }
                view.onEdit = { _, _ in writes += 1 }
                try harness.send(.leftMouseDown, at: point)
                if mode == 1 { try harness.send(.leftMouseDragged, at: CGPoint(x: point.x + 1, y: point.y - 100)) }
                if mode > 1 {
                    try harness.send(.leftMouseDragged, at: end)
                    #expect(view.resizePreview != nil && view.selectedNotes == ids)
                    let camera = view.camera
                    view.follower.show?(100 * 705_600)
                    #expect(view.camera == camera, "a group resize must hold the viewport against playhead following")
                }
                switch mode {
                case 2:
                    let event = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                        windowNumber: 0, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: UInt16(kVK_Escape)))
                    view.keyDown(with: event)
                case 3: _ = view.resignFirstResponder()
                case 4: view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: true, selection: ids)
                case 5: view.setFrameSize(NSSize(width: 650, height: 400))
                case 6: view.setBoundsSize(NSSize(width: 640, height: 410))
                case 7: view.setSnapshot(replacement, track: track, focus: nil, key: DisplayKey(), busy: false, selection: ids)
                case 8: view.setSnapshot(snapshot, track: UUID(), focus: nil, key: DisplayKey(), busy: false, selection: ids)
                case 9: view.setNavigation(nil, documentID: DocumentID())
                case 10:
                    let event = try #require(CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: 1, wheel2: 0, wheel3: 0))
                    view.scrollWheel(with: try #require(NSEvent(cgEvent: event)))
                case 11: view.mouseCancelled(with: try #require(NSEvent.mouseEvent(with: .leftMouseUp, location: end,
                    modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 0)))
                case 12: view.stop()
                case 13: view.setSnapshot(snapshot, track: track, focus: first.id, key: DisplayKey(), busy: false, selection: [first.id])
                case 14:
                    // Return to the exact original geometry before releasing.
                    try harness.send(.leftMouseUp, at: point, flags: .shift)
                default: break
                }
                try harness.send(.leftMouseUp, at: end)
                #expect(writes == 0 && view.resizePreview == nil, "cancelled or click-only group resizes must never commit")
            }
            let harness = PianoMoveBatchHarness(snapshot: snapshot, track: track, selected: ids)
            defer { harness.view.stop() }
            let point = harness.edgePoint(first, edge: edge)
            try harness.send(.leftMouseDown, at: point)
            try harness.send(.leftMouseDragged, at: CGPoint(x: point.x + 30, y: point.y))
            let preview = try #require(harness.view.resizePreview)
            harness.view.present(harness.frame(resize: preview))
            _ = harness.view.resignFirstResponder()
            var writes = 0
            harness.view.onResizes = { _ in writes += 1 }
            harness.view.onEdit = { _, _ in writes += 1 }
            try harness.send(.leftMouseDown, at: point)
            try harness.send(.leftMouseDragged, at: CGPoint(x: point.x + 30, y: point.y))
            try harness.send(.leftMouseUp, at: CGPoint(x: point.x + 30, y: point.y))
            #expect(writes == 0 && harness.view.resizePreview == nil, "a stale resize frame cannot become a new press target")
        }
        await controller.close()
    }
}
