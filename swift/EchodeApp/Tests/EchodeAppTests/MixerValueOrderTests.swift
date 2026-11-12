import AppKit
import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

@MainActor
struct MixerValueOrderTests {
    @Test func queuedEntriesResolveCurrentValuesAndKeepTheSubmittedSelection() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-mixer-value-order-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "second")
        let second = try #require(controller.selection)
        let document = try #require(controller.documentID)
        #expect(await controller.setMixer(first, field: .gain, value: 0.5))
        #expect(await controller.setMixer(second, field: .gain, value: 0.25))
        controller.selectTrack(first, change: .toggle)
        let enter = try #require(controller.arrangeMixActions(document: document).enter)
        let before = controller.undoCount
        let gate = MixerValueGate()
        defer { gate.release() }
        let held = Task { await controller.edits.run { await gate.wait() } }
        await gate.entered()
        enter(first, .gain, 1)
        enter(first, .gain, 0.5)
        let last = try #require(controller.mixerSwitchTail)
        controller.selection = second
        gate.release()
        await held.value
        #expect(await last.value)
        #expect(controller.undoCount == before + 2)
        #expect(
            controller.mixer(first)?.gain.value == 0.5 && controller.mixer(second)?.gain.value == 0.25,
            "the second entry must resolve the current anchor and preserve the captured group")
        await controller.undo()
        #expect(controller.mixer(first)?.gain.value == 1 && controller.mixer(second)?.gain.value == 0.5)
        await controller.redo()
        controller.selectTrack(first, change: .toggle)
        let actions = controller.arrangeMixActions(document: document)
        #expect(actions.begin(first, .gain))
        actions.change(1)
        enter(first, .gain, 2)
        #expect(await (try #require(controller.mixerSwitchTail)).value)
        #expect(controller.faderPreview == nil && controller.undoCount == before + 4)
        #expect(
            controller.mixer(first)?.gain.value == 2 && controller.mixer(second)?.gain.value == 0.5,
            "numeric group edits must first finish the single-track live preview")
        await controller.undo()
        #expect(controller.mixer(first)?.gain.value == 1 && controller.mixer(second)?.gain.value == 0.25)
        await controller.close()
    }

    @Test func anOldNumericFieldCannotBorrowTheReopenedDocumentsCallback() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-mixer-value-field-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "saved")
        let track = try #require(controller.selection)
        #expect(await controller.setMixer(track, field: .gain, value: 0.5))
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 220, height: 64))
        header.setSnapshot(try #require(controller.snapshot), selected: track)
        header.mixActions = controller.arrangeMixActions(document: controller.documentID)
        let rect = TrackControlLayout(row: header.bounds).gain
        let event = try #require(
            NSEvent.mouseEvent(
                with: .leftMouseDown,
                location: header.convert(CGPoint(x: rect.midX, y: rect.midY), to: nil), modifierFlags: [], timestamp: 0,
                windowNumber: 0, context: nil, eventNumber: 1, clickCount: 3, pressure: 1))
        header.mouseDown(with: event)
        let field = try #require(header.entry)
        await controller.close()
        await controller.open(url)
        header.setSnapshot(try #require(controller.snapshot), selected: track)
        header.mixActions = controller.arrangeMixActions(document: controller.documentID)
        field.stringValue = "0 dB"
        field.submit()
        await field.pending?.value
        #expect(controller.mixerSwitchTail == nil && controller.undoCount == 0)
        #expect(controller.mixer(track)?.gain.value == 0.5)
        await controller.close()
    }
}

@MainActor
private final class MixerValueGate {
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
