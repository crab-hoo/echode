import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct MixerSwitchBoundaryTests {
    @Test func contractQueuedSwitchesUseCurrentStateAndNoOpsKeepRedoAndPianoScope() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-switch-order-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "second")
        let second = try #require(controller.selection), document = try #require(controller.documentID)
        controller.selectTrack(first, change: .toggle)
        let undo = controller.undoCount
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 220, height: 128))
        header.onSelectTrack = { controller.selectTrack($0, change: $1) }
        header.setSnapshot(try #require(controller.snapshot), selected: first, selectedTracks: [first, second])
        let actions = controller.arrangeMixActions(document: document)
        header.mixActions = actions
        let gate = MixerSwitchGate()
        defer { gate.release() }
        let held = Task { await controller.edits.run { await gate.wait() } }
        await gate.entered()
        let layout = TrackControlLayout(row: CGRect(x: 0, y: 0, width: 220, height: 64))
        let event = try #require(NSEvent.mouseEvent(with: .leftMouseDown,
            location: header.convert(CGPoint(x: layout.mute.midX, y: layout.mute.midY), to: nil), modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        header.mouseDown(with: event)
        header.mouseDown(with: event)
        let last = try #require(controller.mixerSwitchTail)
        #expect(!controller.busy && controller.undoCount == undo)
        gate.release()
        await held.value
        #expect(await last.value)
        #expect(controller.mixer(first)?.mute.value == 0 && controller.mixer(second)?.mute.value == 0,
            "queued header toggles must resolve the latest anchor state")
        #expect(controller.undoCount == undo + 2, "each queued toggle must remain one independent undo step")
        await controller.undo()
        #expect(controller.mixer(first)?.mute.value == 1 && controller.mixer(second)?.mute.value == 1)
        let redo = controller.redoCount, currentUndo = controller.undoCount
        // The inspector uses an absolute value, not a toggle, through this same product action factory.
        actions.set(first, .mute, 1)
        let unchanged = try #require(controller.mixerSwitchTail)
        #expect(!(await unchanged.value))
        #expect(controller.undoCount == currentUndo && controller.redoCount == redo)
        actions.set(first, .solo, 1)
        #expect(await (try #require(controller.mixerSwitchTail)).value)
        #expect(controller.mixer(first)?.solo.value == 1 && controller.mixer(second)?.solo.value == 1)
        // Piano row buttons deliberately remain single-track even when arrange tracks are selected as a group.
        let row = try #require(controller.pianoTrackRows.first { $0.target.track == first })
        let cell = PianoTrackCell()
        cell.show(row, controller: controller)
        cell.solo.performClick(nil)
        #expect(await (try #require(cell.pending)).value)
        #expect(controller.mixer(first)?.solo.value == 0 && controller.mixer(second)?.solo.value == 1,
            "a piano track button must not apply the arrange's multi-selection")
        let previous = try #require(controller.captureMixerSwitch(first, field: .mute, document: document))
        await controller.close(); await controller.open(url)
        #expect(controller.enqueueMixerSwitch(previous) == nil)
        #expect(!(await controller.applyMixerSwitch(previous)))
        actions.set(first, .solo, 0)
        #expect(controller.mixerSwitchTail == nil && controller.undoCount == 0)
        let newActions = controller.arrangeMixActions(document: controller.documentID)
        #expect(newActions.begin(first, .gain))
        #expect(!actions.begin(first, .gain))
        actions.change(0.25); actions.finish(true); actions.set(first, .pan, 0.5)
        #expect(controller.faderPreview?.value == 1 && controller.undoCount == 0)
        await controller.finishFader(commit: false)
        await controller.close()
    }
}

@MainActor
private final class MixerSwitchGate {
    private var enteredReply: CheckedContinuation<Void, Never>?
    private var held: CheckedContinuation<Void, Never>?
    private var waiting = false
    private var released = false
    func wait() async {
        waiting = true
        enteredReply?.resume(); enteredReply = nil
        if !released { await withCheckedContinuation { held = $0 } }
    }
    func entered() async {
        if !waiting { await withCheckedContinuation { enteredReply = $0 } }
    }
    func release() {
        released = true
        held?.resume(); held = nil
    }
}
