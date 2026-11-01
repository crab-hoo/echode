import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct RulerHandleTests {
    private let beat: Int64 = 705_600

    @Test func contractLoopHandlesResizeOneEdgeWithoutSeekingInBothEditors() async throws {
        let controller = ProjectController()
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-loop-handles-\(UUID()).echode")
        defer { removeTestFile(url) }
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let snapshot = try #require(controller.snapshot), track = try #require(controller.selection)
        let document = try #require(controller.documentID), undo = controller.undoCount
        await controller.setPlaybackStart(beat: 0.5)
        let original = try #require(LoopRange(start: beat, end: beat * 4))

        for isPiano in [false, true] {
            await controller.setLoop(enabled: false)
            await controller.setLoopRange(original, documentID: document)
            let harness = RulerHandleHarness(snapshot: snapshot, track: track, isPiano: isPiano)
            harness.setLoop(controller.rulerLoop)
            // An edge press is neither a seek nor a range creation, even with small jitter.
            try harness.send(.leftMouseDown, beat: 1)
            try harness.send(.leftMouseUp, beat: 1 + 1 / harness.pixels)
            #expect(harness.starts.isEmpty && harness.ranges.isEmpty)
            try harness.send(.leftMouseDown, beat: 1)
            try harness.send(.leftMouseDragged, beat: 2.3)
            let expected = try #require(LoopRange(start: isPiano ? beat * 9 / 4 : beat * 2, end: beat * 4))
            #expect(harness.displayedLoop?.range == expected)
            #expect(harness.starts.isEmpty && harness.ranges.isEmpty)
            try harness.send(.leftMouseUp, beat: 2.3)
            #expect(harness.ranges == [expected] && harness.starts.isEmpty)
            await controller.setLoopRange(expected, documentID: document)
            #expect(controller.audio.loopStart == nil && controller.rulerLoop?.range == expected)

            // Shift on release frees only the moving edge, retaining the original pointer offset.
            harness.setLoop(RulerLoop(range: original, enabled: false))
            try harness.send(.leftMouseDown, beat: 4 - 2 / harness.pixels)
            try harness.send(.leftMouseDragged, beat: 5.3 - 2 / harness.pixels)
            try harness.send(.leftMouseUp, beat: 5.3 - 2 / harness.pixels, flags: .shift)
            let shifted = try #require(harness.ranges.last)
            #expect(shifted == LoopRange(start: beat, end: Tick.ticks(5.3)!))
            await controller.toggleLoop()
            await controller.setLoopRange(shifted, documentID: document)
            #expect(controller.audio.loopStart == shifted.start && controller.audio.loopEnd == shifted.end)
            harness.setLoop(controller.rulerLoop)
            #expect(harness.displayedLoop == controller.rulerLoop)
            let grips = harness.rulerInstances.filter { $0.color == SIMD4<Float>(0.55, 0.94, 0.67, 1) }
            #expect(grips.count == 4)
            #expect(controller.playbackStart == beat / 2 && controller.audio.tick == beat / 2)
            #expect(controller.undoCount == undo && controller.selection == track)

            // Returning to the original edge sends nothing, and cannot turn into a click.
            let count = harness.ranges.count
            try harness.send(.leftMouseDown, beat: 1)
            try harness.send(.leftMouseDragged, beat: 2)
            try harness.send(.leftMouseUp, beat: 1)
            #expect(harness.ranges.count == count && harness.starts.isEmpty)
            // The middle/lower ruler remains available for a start click at the same tick.
            try harness.send(.leftMouseDown, beat: 1, y: 12)
            try harness.send(.leftMouseUp, beat: 1, y: 12)
            #expect(harness.starts == [beat])
            harness.stop()
        }
        await controller.close()
    }

    @Test func cancelledHandlesDoNotCommitOverExternalChanges() async throws {
        let controller = ProjectController()
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-loop-handle-cancel-\(UUID()).echode")
        defer { removeTestFile(url) }
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let snapshot = try #require(controller.snapshot), track = try #require(controller.selection)
        let original = try #require(LoopRange(start: beat, end: 4 * beat))
        let replacement = RulerLoop(range: try #require(LoopRange(start: 0, end: 6 * beat)), enabled: true)
        let escape = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53))
        let scrollEvent = try #require(CGEvent(scrollWheelEvent2Source: nil, units: .pixel,
            wheelCount: 1, wheel1: 1, wheel2: 0, wheel3: 0))
        let scroll = try #require(NSEvent(cgEvent: scrollEvent))
        for isPiano in [false, true] {
            for cancellation in 0..<9 {
                let harness = RulerHandleHarness(snapshot: snapshot, track: track, isPiano: isPiano)
                harness.setLoop(RulerLoop(range: original, enabled: false))
                try harness.send(.leftMouseDown, beat: 4)
                try harness.send(.leftMouseDragged, beat: 5)
                #expect(harness.displayedLoop?.range == LoopRange(start: beat, end: 5 * beat))
                switch cancellation {
                case 0: harness.view.keyDown(with: escape)
                case 1: _ = harness.view.resignFirstResponder()
                case 2: harness.view.setFrameSize(NSSize(width: 600, height: 200))
                case 3: harness.setLoop(replacement)
                case 4: harness.stop()
                case 5: harness.view.mouseCancelled(with: try harness.mouse(.leftMouseUp, beat: 5))
                case 6: harness.setBusy(snapshot: snapshot, track: track)
                case 7: harness.setLoop(RulerLoop(range: original, enabled: true))
                default: harness.view.scrollWheel(with: scroll)
                }
                try harness.send(.leftMouseUp, beat: 5)
                #expect(harness.ranges.isEmpty && harness.starts.isEmpty)
                if cancellation == 3 { #expect(harness.displayedLoop == replacement) }
                harness.stop()
            }
        }
        await controller.close()
    }

    @Test func handlesClampWithoutCrossingAndKeepTheStationaryEdgeExact() throws {
        let original = try #require(LoopRange(start: Tick.ticks(1.3)!, end: Tick.ticks(4.1)!))
        for grid in [beat, beat / 4] {
            var left = RulerPress(x: 100, originBeat: 0, pixelsPerBeat: 72, grid: grid, edge: .start, originalRange: original)
            left.update(x: 600, snapping: true)
            #expect(left.range == LoopRange(start: 4 * beat, end: original.end))
            left.update(x: 100, snapping: true)
            #expect(left.range == nil)
            left.update(x: 600, snapping: false)
            #expect(left.range == LoopRange(start: original.end - 1, end: original.end))
            left.update(x: -500, snapping: false)
            #expect(left.range == LoopRange(start: 0, end: original.end))
            var right = RulerPress(x: 100, originBeat: 0, pixelsPerBeat: 72, grid: grid, edge: .end, originalRange: original)
            right.update(x: -500, snapping: true)
            let limit = grid == beat ? 2 * beat : beat * 3 / 2
            #expect(right.range == LoopRange(start: original.start, end: limit))
            right.update(x: -500, snapping: false)
            #expect(right.range == LoopRange(start: original.start, end: original.start + 1))
        }
        // The checked next-grid calculation must also be safe near the tick boundary.
        let huge = try #require(LoopRange(start: Int64.max - 2, end: Int64.max))
        var press = RulerPress(x: 0, originBeat: 0, pixelsPerBeat: 72, grid: beat, edge: .end, originalRange: huge)
        press.update(x: -Double(huge.end) / 705_600 * 72, snapping: true)
        #expect(press.range == nil)
    }

    @Test func handleGeometryUsesRealEdgesClipsAndResolvesOverlappingTargets() throws {
        let range = try #require(LoopRange(start: beat, end: beat * 4))
        let rect = CGRect(x: 80, y: 0, width: 144, height: 24)
        let clipped = RulerHandles(range: range, ruler: rect, originBeat: 2, pixelsPerBeat: 72)
        #expect(clipped.handles.count == 1 && clipped.handles.first?.edge == .end)
        #expect(clipped.hit(at: CGPoint(x: 80, y: 4)) == nil)
        #expect(clipped.hit(at: CGPoint(x: 222, y: 4)) == .end)
        #expect(clipped.hit(at: CGPoint(x: 222, y: 8)) == nil)
        for scale in [1.0, 2.0] {
            let instances = clipped.instances(enabled: false, scale: scale)
            #expect(instances.count == 2)
            for instance in instances {
                let r = instance.rect / Float(scale)
                #expect(Double(r.x) >= rect.minX && Double(r.x + r.z) <= rect.maxX)
                #expect(r.y >= 0 && r.y + r.w <= 8)
            }
        }
        let tiny = try #require(LoopRange(start: beat, end: beat + 1))
        let overlap = RulerHandles(range: tiny, ruler: rect, originBeat: 0, pixelsPerBeat: 72)
        #expect(overlap.hit(at: CGPoint(x: 152, y: 4)) == .start)
        #expect(overlap.hit(at: CGPoint(x: 152 + 72.0 / Double(beat), y: 4)) == .end)
        #expect(overlap.hit(at: CGPoint(x: 152, y: 12)) == nil)
        #expect(RulerHandles(range: range, ruler: .zero, originBeat: 0, pixelsPerBeat: 72).handles.isEmpty)
    }
}

/// Routes synthetic input through the real views without opening a window or output device.
@MainActor
private final class RulerHandleHarness {
    let pane: ArrangePaneView
    let piano: PianoGridView
    let isPiano: Bool
    var starts: [Int64] = []
    var ranges: [LoopRange] = []
    var view: NSView { isPiano ? piano : pane.timeline }
    var pixels: Double { isPiano ? piano.camera.pixelsPerBeat : pane.timeline.camera.pixelsPerBeat }
    var displayedLoop: RulerLoop? { isPiano ? piano.displayedLoop : pane.timeline.displayedLoop }
    var rulerInstances: [Instance] {
        if !isPiano { return ArrangeFrame(input: pane.timeline.renderInput).rulerInstances }
        let input = CanvasInput(snapshot: nil, selected: nil, camera: ArrangeCamera(), width: 900, height: 200, scale: 1)
        return PianoFrame(input: input, piano: PianoRenderInput(track: UUID(), focus: nil, camera: piano.camera,
            key: DisplayKey(), preview: nil, loop: displayedLoop)).rulerInstances
    }

    init(snapshot: Snapshot, track: UUID, isPiano: Bool) {
        self.isPiano = isPiano
        pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 900, height: 200))
        piano = PianoGridView(frame: NSRect(x: 0, y: 0, width: 900, height: 200))
        pane.layout(); pane.setSnapshot(snapshot, selected: track)
        piano.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false)
        let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 900, height: 200, scale: 1)
        piano.present(PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: piano.camera,
            key: DisplayKey(), preview: nil)))
        pane.timeline.onSetStart = { [weak self] in self?.starts.append($0) }
        piano.onSetStart = { [weak self] in self?.starts.append($0) }
        pane.timeline.onSetLoop = { [weak self] in self?.ranges.append($0) }
        piano.onSetLoop = { [weak self] in self?.ranges.append($0) }
    }

    func setLoop(_ loop: RulerLoop?) {
        pane.timeline.setRuler(start: nil, loop: loop, busy: false)
        piano.setLoop(loop)
    }
    func setBusy(snapshot: Snapshot, track: UUID) {
        pane.timeline.setRuler(start: nil, loop: displayedLoop, busy: true)
        piano.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: true)
    }
    func stop() { pane.stop(); piano.stop() }
    func mouse(_ type: NSEvent.EventType, beat: Double, y: Double = 4, flags: NSEvent.ModifierFlags = []) throws -> NSEvent {
        let origin = isPiano ? piano.pianoLayout.gridOrigin.x : pane.timeline.renderInput.header!.width + 1
        return try #require(NSEvent.mouseEvent(with: type, location: view.convert(CGPoint(x: origin + beat * pixels, y: y), to: nil),
            modifierFlags: flags, timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
    }
    func send(_ type: NSEvent.EventType, beat: Double, y: Double = 4, flags: NSEvent.ModifierFlags = []) throws {
        let event = try mouse(type, beat: beat, y: y, flags: flags)
        switch type {
        case .leftMouseDown: view.mouseDown(with: event)
        case .leftMouseDragged: view.mouseDragged(with: event)
        default: view.mouseUp(with: event)
        }
    }
}
