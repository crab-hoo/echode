import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct MixerStepBoundaryTests {
    @Test func contractAccessibleStepsClampTogetherAndRejectOldControls() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-step-bounds-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "anchor")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "near limits")
        let second = try #require(controller.selection)
        #expect(await controller.setMixer(first, field: .gain, value: 0.5))
        #expect(await controller.setMixer(second, field: .gain, value: 3.8))
        #expect(await controller.setMixer(first, field: .pan, value: -0.5))
        #expect(await controller.setMixer(second, field: .pan, value: 0.98))
        controller.selectTrack(first, change: .toggle)
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 220, height: 128), styleMask: [.borderless], backing: .buffered, defer: true)
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 220, height: 128))
        window.contentView = header
        func control(_ field: MixerField) throws -> NSAccessibilityElement {
            header.documentID = controller.documentID
            header.mixActions = controller.arrangeMixActions(document: controller.documentID)
            header.setSnapshot(try #require(controller.snapshot), selected: controller.selection, selectedTracks: controller.selectedTracks)
            let row = try #require((header.accessibilityChildren() as? [NSAccessibilityElement])?.first)
            return try #require((row.accessibilityChildren() as? [NSAccessibilityElement])?.first {
                $0.accessibilityIdentifier() == "\(first)/\(field.rawValue)"
            })
        }
        let old = try control(.gain), oldActions = controller.arrangeMixActions(document: controller.documentID)
        let stale = try #require(controller.captureMixerStep(first, field: .gain, direction: -1, document: controller.documentID))
        #expect(old.accessibilityPerformIncrement())
        #expect(await (try #require(controller.mixerSwitchTail)).value)
        #expect(abs(try #require(controller.mixer(first)).gain.value - 0.5 * 4 / 3.8) < 1e-6)
        #expect(controller.mixer(second)?.gain.value == 4, "accessible steps must stop the common gain factor at the loudest member")
        let readback = try control(.gain)
        #expect((readback.accessibilityValue() as? NSNumber)?.floatValue == controller.mixer(first)?.gain.value,
            "accessibility readback must use the clamped committed value, not a speculative step")
        #expect(try control(.pan).accessibilityPerformIncrement())
        #expect(await (try #require(controller.mixerSwitchTail)).value)
        #expect(abs(try #require(controller.mixer(first)).pan.value + 0.48) < 1e-6)
        #expect(controller.mixer(second)?.pan.value == 1, "accessible steps must stop the common pan difference at the outer member")
        await controller.undo()
        let undo = controller.undoCount, redo = controller.redoCount
        #expect(try control(.gain).accessibilityPerformIncrement())
        #expect(!(await (try #require(controller.mixerSwitchTail)).value))
        #expect(controller.undoCount == undo && controller.redoCount == redo)
        await controller.close(); await controller.open(url)
        let current = try control(.gain)
        #expect(current !== old, "a reopened document must not reuse old accessibility controls")
        let saved = [controller.mixer(first), controller.mixer(second)]
        #expect(old.accessibilityPerformDecrement())
        oldActions.adjust?(first, .gain, -1)
        #expect(controller.enqueueMixerValue(stale) == nil)
        #expect(!(await controller.applyMixerValue(stale)))
        #expect(controller.mixerSwitchTail == nil && controller.undoCount == 0)
        #expect([controller.mixer(first), controller.mixer(second)] == saved)
        await controller.close()
    }

    @Test func relativeStepEndpointsAndMalformedTargetsAreExplicit() {
        let first = UUID(), second = UUID(), document = UUID()
        func values(_ field: MixerField, _ direction: Double, _ originals: [Float], targets: [UUID]? = nil) -> [Float]? {
            MixerValueBatch(document: document, tracks: targets ?? [first, second], anchor: first,
                field: field, value: 0, direction: direction).values(from: originals)
        }
        #expect(values(.gain, 1, [0, 1]) == nil)
        #expect(values(.gain, -1, [0, 1]) == [0, 1])
        let raised = values(.gain, 1, [0.5, 0])
        #expect(raised?[1] == 0 && (raised?[0] ?? 0) > 0.5)
        let quiet = values(.gain, -1, [0.001, 0.0005])
        #expect((quiet?[0] ?? 0) > 0 && (quiet?[1] ?? 0) > 0, "finite group steps must not inherit the single-slider silence floor")
        #expect(values(.gain, -1, [1, .leastNonzeroMagnitude]) == [1, .leastNonzeroMagnitude])
        #expect(values(.gain, -1, [.leastNonzeroMagnitude, 1]) == [.leastNonzeroMagnitude, 1])
        #expect(values(.gain, 1, [4, 1]) == [4, 1])
        #expect(values(.pan, -1, [-0.5, -0.98]) == [-0.52, -1])
        for invalid in [0, 2, -2, Double.nan, .infinity] {
            #expect(values(.gain, invalid, [1, 1]) == nil)
        }
        #expect(values(.mute, 1, [0, 0]) == nil)
        #expect(values(.gain, 1, [1, .nan]) == nil)
        #expect(values(.pan, 1, [0, 2]) == nil)
        for targets in [[], [first, first], [second]] {
            #expect(values(.gain, 1, targets.map { _ in 1 }, targets: targets) == nil)
        }
        #expect(values(.gain, 1, [0], targets: [first]) == [MixScale.adjusted(.gain, value: 0, direction: 1)])
        #expect(values(.gain, -1, [0.001], targets: [first]) == [0])
    }
}
