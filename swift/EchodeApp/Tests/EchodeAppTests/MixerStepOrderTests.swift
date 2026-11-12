import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

@MainActor
struct MixerStepOrderTests {
    @Test func stepsResolveAfterNumericEntriesAndLivePreviewsWithoutRetargeting() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-mixer-step-order-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "anchor")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "member")
        let second = try #require(controller.selection)
        #expect(await controller.setMixer(first, field: .gain, value: 0.5))
        #expect(await controller.setMixer(second, field: .gain, value: 0.25))
        controller.selectTrack(first, change: .toggle)
        let actions = controller.arrangeMixActions(document: controller.documentID)
        let step = try #require(actions.adjust)
        let enter = try #require(actions.enter)
        let before = controller.undoCount
        let gate = MixerStepGate()
        defer { gate.release() }
        let held = Task { await controller.edits.run { await gate.wait() } }
        await gate.entered()
        enter(first, .gain, 1)
        step(first, .gain, 1)
        step(first, .gain, -1)
        let last = try #require(controller.mixerSwitchTail)
        controller.selection = second
        gate.release()
        await held.value
        #expect(await last.value)
        #expect(controller.undoCount == before + 3)
        #expect(abs(try #require(controller.mixer(first)).gain.value - 1) < 1e-6)
        #expect(abs(try #require(controller.mixer(second)).gain.value - 0.5) < 1e-6)
        controller.selectTrack(first, change: .toggle)
        #expect(actions.begin(first, .gain))
        actions.change(2)
        step(first, .gain, 1)
        #expect(await (try #require(controller.mixerSwitchTail)).value)
        #expect(controller.faderPreview == nil && controller.undoCount == before + 5)
        let factor = Float(pow(10.0, 1.0 / 20))
        #expect(abs(try #require(controller.mixer(first)).gain.value - 2 * factor) < 1e-6)
        #expect(abs(try #require(controller.mixer(second)).gain.value - 0.5 * factor) < 1e-6)
        let invalid = MixerValueBatch(
            document: try #require(controller.documentID), tracks: [first, UUID()],
            anchor: first, field: .gain, value: 0, direction: -1)
        let recorded = controller.mixer(first)
        let undo = controller.undoCount
        #expect(!(await controller.applyMixerValue(invalid)))
        #expect(controller.mixer(first) == recorded && controller.undoCount == undo)
        // Unselected anchors remain single-track, including the existing silence endpoint.
        controller.selection = second
        #expect(await controller.setMixer(first, field: .gain, value: 0))
        step(first, .gain, 1)
        #expect(await (try #require(controller.mixerSwitchTail)).value)
        #expect(controller.mixer(first)?.gain.value == MixScale.adjusted(.gain, value: 0, direction: 1))
        #expect(abs(try #require(controller.mixer(second)).gain.value - 0.5 * factor) < 1e-6)
        controller.selectTrack(first, change: .toggle)
        #expect(await controller.setMixer(first, field: .gain, value: 0))
        let silentUndo = controller.undoCount
        step(first, .gain, 1)
        #expect(!(await (try #require(controller.mixerSwitchTail)).value))
        #expect(controller.errorMessage != nil && controller.undoCount == silentUndo)
        step(first, .gain, -1)
        #expect(!(await (try #require(controller.mixerSwitchTail)).value))
        #expect(controller.mixer(first)?.gain.value == 0 && controller.undoCount == silentUndo)
        await controller.close()
    }
}

@MainActor
private final class MixerStepGate {
    private var enteredReply: CheckedContinuation<Void, Never>?
    private var held: CheckedContinuation<Void, Never>?
    private var waiting = false
    private var released = false
    func wait() async {
        waiting = true
        enteredReply?.resume()
        enteredReply = nil
        if !released { await withCheckedContinuation { held = $0 } }
    }
    func entered() async {
        if !waiting { await withCheckedContinuation { enteredReply = $0 } }
    }
    func release() {
        released = true
        held?.resume()
        held = nil
    }
}
