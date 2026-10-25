import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct RulerLoopTests {
    private let beat: Int64 = 705_600

    private func mouse(_ type: NSEvent.EventType, x: Double, in grid: PianoGridView,
                       flags: NSEvent.ModifierFlags = []) throws -> NSEvent {
        try #require(NSEvent.mouseEvent(with: type,
            location: grid.convert(CGPoint(x: grid.pianoLayout.gridOrigin.x + x, y: 10), to: nil),
            modifierFlags: flags, timestamp: 0, windowNumber: 0, context: nil,
            eventNumber: 1, clickCount: 1, pressure: 1))
    }

    @Test func contractPianoRulerDragSetsOnlyTheLoopRange() async throws {
        let controller = ProjectController()
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-ruler-loop-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let document = try #require(controller.documentID)
        let track = try #require(controller.selection)
        let snapshot = try #require(controller.snapshot)
        let undo = controller.undoCount
        let grid = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        grid.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false)
        let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
        func present() {
            grid.present(PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: grid.camera, key: DisplayKey(), preview: nil)))
        }
        present()
        grid.setLoop(controller.rulerLoop)
        var starts: [Int64] = []
        var ranges: [LoopRange] = []
        grid.onSetStart = { starts.append($0) }
        grid.onSetLoop = { ranges.append($0) }
        let pixels = grid.camera.pixelsPerBeat
        func down(_ beat: Double, flags: NSEvent.ModifierFlags = []) throws {
            grid.mouseDown(with: try mouse(.leftMouseDown, x: beat * pixels, in: grid, flags: flags))
        }
        func drag(_ beat: Double, flags: NSEvent.ModifierFlags = []) throws {
            grid.mouseDragged(with: try mouse(.leftMouseDragged, x: beat * pixels, in: grid, flags: flags))
        }
        func up(_ beat: Double, flags: NSEvent.ModifierFlags = []) throws {
            grid.mouseUp(with: try mouse(.leftMouseUp, x: beat * pixels, in: grid, flags: flags))
        }
        try down(1.3)
        #expect(starts.isEmpty && ranges.isEmpty)
        try drag(3.1)
        let expected = try #require(LoopRange(start: beat * 5 / 4, end: beat * 3))
        #expect(grid.displayedLoop?.range == expected && grid.displayedLoop?.enabled == false)
        #expect(starts.isEmpty && ranges.isEmpty)
        try up(3.1)
        #expect(ranges == [expected] && starts.isEmpty)
        await controller.setLoopRange(expected, documentID: document)
        #expect(controller.rulerLoop?.range == expected && controller.audio.loopStart == nil)
        #expect(controller.playbackStart == 0 && controller.audio.tick == 0 && !controller.audio.running)

        // Reversed drags sort their endpoints. Shift frees both endpoints.
        try down(4.1)
        try drag(2.3)
        try up(2.3)
        #expect(ranges.last == LoopRange(start: beat * 9 / 4, end: beat * 4))
        try down(1.3, flags: .shift)
        try drag(2.7, flags: .shift)
        try up(2.7, flags: .shift)
        let unsnapped = try #require(ranges.last)
        #expect(unsnapped == LoopRange(start: PianoCamera.ticks(1.3)!, end: PianoCamera.ticks(2.7)!))
        await controller.toggleLoop()
        #expect(controller.audio.loopStart == expected.start && controller.audio.loopEnd == expected.end)
        await controller.setLoopRange(unsnapped, documentID: document)
        #expect(controller.audio.loopStart == unsnapped.start && controller.audio.loopEnd == unsnapped.end)
        #expect(controller.playbackStart == 0 && controller.undoCount == undo)

        // A click with sub-threshold jitter changes only the remembered start.
        let count = ranges.count
        try down(1.3); try up(1.3 + 1 / pixels)
        #expect(starts == [beat * 5 / 4] && ranges.count == count)
        // A real drag whose snapped endpoints coincide is not a click.
        try down(1); try drag(1.05); try up(1.05)
        #expect(starts.count == 1 && ranges.count == count)
        // Escape, resizing, a busy document, and stop discard the preview.
        try down(1); try drag(3)
        let escape = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53))
        grid.keyDown(with: escape); try up(3)
        try down(1); try drag(3); grid.setFrameSize(NSSize(width: 600, height: 400)); try up(3)
        grid.setFrameSize(NSSize(width: 640, height: 400)); present()
        try down(1); try drag(3)
        grid.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: true); try up(3)
        grid.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false); present()
        try down(1); try drag(3); grid.stop(); try up(3)
        #expect(starts.count == 1 && ranges.count == count)

        await controller.close()
        await controller.open(url)
        // The range is session-only, and an old canvas cannot change a new document.
        await controller.setLoopRange(expected, documentID: document)
        #expect(controller.rulerLoop?.range == LoopRange(start: 0, end: beat * 4))
        #expect(controller.audio.loopStart == nil)
        await controller.close()
    }

    @Test func loopBandClipsToTheRulerAndDistinguishesDisabledLoops() throws {
        let track = UUID()
        let range = try #require(LoopRange(start: beat, end: beat * 4))
        let input = CanvasInput(snapshot: nil, selected: nil, camera: ArrangeCamera(), width: 300, height: 200, scale: 2)
        var camera = PianoCamera()
        camera.originBeat = 2
        func frame(_ enabled: Bool) -> PianoFrame {
            PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera,
                key: DisplayKey(), preview: nil, loop: RulerLoop(range: range, enabled: enabled)))
        }
        let active = frame(true)
        let inactive = frame(false)
        #expect(active.loopRect == CGRect(x: 80, y: 0, width: 144, height: 24))
        #expect(active.loopRect == inactive.loopRect)
        #expect(active.rulerInstances.count == 4 && inactive.rulerInstances.count == 4)
        #expect(active.rulerInstances[2].color != inactive.rulerInstances[2].color)
        camera.originBeat = 4
        #expect(frame(true).loopRect == nil)
        #expect(LoopRange(start: -1, end: 1) == nil && LoopRange(start: 1, end: 1) == nil)
    }
}
