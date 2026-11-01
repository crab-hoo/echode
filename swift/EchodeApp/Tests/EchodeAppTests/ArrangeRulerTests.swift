import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct ArrangeRulerTests {
    private let beat: Int64 = 705_600

    private func mouse(_ type: NSEvent.EventType, _ point: CGPoint, in view: NSView,
                       flags: NSEvent.ModifierFlags = []) throws -> NSEvent {
        try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: flags,
            timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
    }

    @Test func contractArrangeRulerSharesStartAndLoopWithoutSelectingTracks() async throws {
        let controller = ProjectController()
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-arrange-ruler-\(UUID()).echode")
        defer { removeTestFile(url) }
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let snapshot = try #require(controller.snapshot), document = try #require(controller.documentID)
        let undo = controller.undoCount, selection = controller.selection
        let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 900, height: 200))
        pane.layout(); pane.setSnapshot(snapshot, selected: selection)
        let view = pane.timeline
        view.audioDocument = document
        var selections: [UUID] = [], starts: [Int64] = [], ranges: [LoopRange] = []
        pane.onSelect = { selections.append($0) }
        view.onSetStart = { starts.append($0) }
        view.onSetLoop = { ranges.append($0) }
        view.setRuler(start: controller.playbackStart, loop: controller.rulerLoop, busy: false)
        func event(_ type: NSEvent.EventType, _ position: Double, flags: NSEvent.ModifierFlags = []) throws -> NSEvent {
            let origin = view.renderInput.header!.width + 1
            let x = origin + (position - view.camera.originBeat) * view.camera.pixelsPerBeat
            return try mouse(type, CGPoint(x: x, y: 10), in: view, flags: flags)
        }
        func down(_ position: Double) throws { view.mouseDown(with: try event(.leftMouseDown, position)) }
        func drag(_ position: Double, flags: NSEvent.ModifierFlags = []) throws { view.mouseDragged(with: try event(.leftMouseDragged, position, flags: flags)) }
        func up(_ position: Double, flags: NSEvent.ModifierFlags = []) throws { view.mouseUp(with: try event(.leftMouseUp, position, flags: flags)) }

        try down(1.3)
        #expect(starts.isEmpty && ranges.isEmpty && selections.isEmpty)
        try up(1.3 + 1 / 72.0)
        #expect(starts == [beat] && ranges.isEmpty)
        await controller.setPlaybackStart(tick: starts[0])
        try down(2.3); try up(2.3, flags: .shift)
        #expect(starts.last == Tick.ticks(2.3))
        try down(1.3); try drag(4.2)
        let expected = try #require(LoopRange(start: beat, end: beat * 4))
        #expect(view.displayedLoop?.range == expected && view.displayedLoop?.enabled == false)
        #expect(ranges.isEmpty && starts.count == 2)
        try up(4.2)
        #expect(ranges == [expected] && starts.count == 2 && selections.isEmpty)
        await controller.setLoopRange(expected, documentID: document)
        #expect(controller.audio.loopStart == nil && controller.rulerLoop?.range == expected)
        #expect(controller.playbackStart == beat && controller.audio.tick == beat)
        await controller.toggleLoop()
        view.setRuler(start: controller.playbackStart, loop: controller.rulerLoop, busy: false)
        try down(4.3); try drag(2.2, flags: .shift); try up(2.2, flags: .shift)
        let reverse = try #require(ranges.last)
        #expect(reverse == LoopRange(start: Tick.ticks(2.2)!, end: Tick.ticks(4.3)!))
        await controller.setLoopRange(reverse, documentID: document)
        #expect(controller.audio.loopStart == reverse.start && controller.audio.loopEnd == reverse.end)
        #expect(controller.selection == selection && controller.undoCount == undo && controller.playbackStart == beat)

        // Both editors display the same state without converting the arrange's quarter-note grid to a note grid.
        let piano = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 200))
        piano.setLoop(controller.rulerLoop)
        #expect(piano.displayedLoop == controller.rulerLoop)
        view.setRuler(start: controller.playbackStart, loop: controller.rulerLoop, busy: false)
        pane.resizeHeader(to: 200)
        pane.timeline.onScroll?(-72, 0)
        pane.timeline.onZoom?(2, [], .zero)
        try down(2.7); try up(2.7)
        #expect(starts.last == beat * 3)
        // A real drag that snaps to one point must not become a click.
        let count = ranges.count, startCount = starts.count
        try down(2); try drag(2.05); try up(2.05)
        #expect(ranges.count == count && starts.count == startCount)
        // Header headings and the half-open bottom of the ruler are not ruler targets.
        view.mouseDown(with: try mouse(.leftMouseDown, CGPoint(x: 100, y: 10), in: view))
        view.mouseUp(with: try mouse(.leftMouseUp, CGPoint(x: 100, y: 10), in: view))
        view.mouseDown(with: try mouse(.leftMouseDown, CGPoint(x: 250, y: 24), in: view))
        #expect(starts.count == startCount && selections.count == 1)
        #expect(pane.headers.frame.minY == 24 && pane.headers.bounds.height == 176)

        let frame = ArrangeFrame(input: view.renderInput)
        let lane = try #require(frame.elements.first { $0.role == .track })
        let header = try #require(frame.headers.first)
        #expect(header.y == lane.y + 24)
        #expect(frame.timelineInstances[0].rect.y == (lane.y + 24) * Float(frame.input.scale))
        #expect(frame.startX == 201) // The remembered beat 1 is at the scrolled left edge.
        #expect(frame.loopRect?.minX ?? 0 >= frame.timelineOrigin)
        #expect(frame.loopRect?.height == 24 && !frame.rulerLabels.isEmpty)
        await controller.close(); await controller.open(url)
        #expect(controller.playbackStart == beat && controller.audio.loopStart == nil)
        await controller.setLoopRange(expected, documentID: document)
        #expect(controller.rulerLoop?.range == LoopRange(start: 0, end: beat * 4))
        await controller.close()
    }

    @Test func cancelledRulerGesturesNeverSendAStartOrRange() async throws {
        let controller = ProjectController()
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-arrange-ruler-cancel-\(UUID()).echode")
        defer { removeTestFile(url) }
        await controller.open(url, creating: true)
        let snapshot = try #require(controller.snapshot)
        let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 640, height: 200))
        pane.layout(); pane.setSnapshot(snapshot, selected: nil)
        let view = pane.timeline
        var requests = 0
        view.onSetStart = { _ in requests += 1 }; view.onSetLoop = { _ in requests += 1 }
        let down = try mouse(.leftMouseDown, CGPoint(x: 250, y: 10), in: view)
        let drag = try mouse(.leftMouseDragged, CGPoint(x: 450, y: 10), in: view)
        let up = try mouse(.leftMouseUp, CGPoint(x: 450, y: 10), in: view)
        let escape = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53))
        let cancellations: [() -> Void] = [
            { view.keyDown(with: escape) },
            { view.mouseCancelled(with: up) },
            { _ = view.resignFirstResponder() },
            { pane.resizeHeader(to: 200) },
            { view.setFrameSize(NSSize(width: 600, height: 200)) },
            { pane.timeline.onScroll?(-10, 0) },
            { pane.timeline.onZoom?(1.2, [], .zero) },
            { view.setRuler(start: nil, loop: nil, busy: true) },
            { view.audioDocument = DocumentID() },
            { view.stop() },
        ]
        for cancel in cancellations {
            view.setRuler(start: nil, loop: nil, busy: false)
            view.mouseDown(with: down); view.mouseDragged(with: drag)
            #expect(view.rulerPress?.range != nil)
            cancel(); view.mouseUp(with: up)
            #expect(view.rulerPress == nil && requests == 0)
        }
        view.mouseDown(with: down); view.mouseDragged(with: drag)
        await controller.addTrack(name: "new snapshot")
        pane.setSnapshot(try #require(controller.snapshot), selected: nil)
        view.mouseUp(with: up)
        #expect(requests == 0)
        await controller.close()
    }

    @Test func initialSelectionRevealsBeforeTheFirstChildLayout() async throws {
        let controller = ProjectController()
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-arrange-ruler-layout-\(UUID()).echode")
        defer { removeTestFile(url) }
        await controller.open(url, creating: true)
        await controller.addTrack(name: "first"); await controller.addTrack(name: "last")
        let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 640, height: 128))
        pane.setSnapshot(try #require(controller.snapshot), selected: controller.selection)
        pane.layout()
        #expect(pane.timeline.camera.originTrack == 0.375) // Two rows minus the 104-point row area.
        let frame = ArrangeFrame(input: pane.timeline.renderInput)
        #expect(frame.headers.last?.y == 64 && frame.headers.last?.height == 64)
        await controller.close()
    }

    @Test func rulerBandAndMarkerClipAtBothEdgesAndSmallHeights() throws {
        let range = try #require(LoopRange(start: beat, end: 4 * beat))
        for scale in [1.0, 2.0] {
            var input = CanvasInput(snapshot: nil, selected: nil, camera: ArrangeCamera(originBeat: 2),
                width: 300, height: 200, scale: scale, header: ArrangeHeaderInput(width: 160, palette: ArrangeHeaderPalette()),
                arrangeRuler: ArrangeRulerInput(start: beat * 2, loop: RulerLoop(range: range, enabled: false)))
            let inactive = ArrangeFrame(input: input)
            #expect(inactive.loopRect == CGRect(x: 161, y: 0, width: 139, height: 24))
            #expect(inactive.startX == 161)
            input.arrangeRuler?.loop = RulerLoop(range: range, enabled: true)
            let active = ArrangeFrame(input: input)
            #expect(active.loopRect == inactive.loopRect)
            #expect(active.rulerInstances[2].color != inactive.rulerInstances[2].color)
            input.arrangeRuler?.start = beat
            #expect(ArrangeFrame(input: input).startX == nil)
            input.arrangeRuler?.start = beat * 4
            #expect(ArrangeFrame(input: input).startX == nil)
            for height in [0.0, 10.0, 24.0] {
                let tiny = CanvasInput(snapshot: nil, selected: nil, camera: input.camera, width: 300, height: height, scale: scale,
                    header: input.header, arrangeRuler: input.arrangeRuler)
                #expect(tiny.arrangeHeight == 0 && tiny.arrangeTop == height)
                #expect(ArrangeFrame(input: tiny).loopRect?.height ?? 0 <= height)
            }
        }
    }
}
