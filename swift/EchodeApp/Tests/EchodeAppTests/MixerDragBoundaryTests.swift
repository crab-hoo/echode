import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct MixerDragBoundaryTests {
    @Test func pressCapturesSelectionBeforeMovementAndPanUsesACommonLimit() async throws {
        let fixture = try await MixerDragFixture()
        defer { removeTestFile(fixture.url) }
        let controller = fixture.controller, tracks = fixture.tracks
        let pan = fixture.point(.pan)
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, pan))
        controller.selection = tracks[2]
        fixture.header.mouseDragged(with: try fixture.mouse(.leftMouseDragged, pan, dx: 200))
        let preview = try #require(controller.faderPreview)
        #expect(preview.pan == 0.25 && preview.values(on: tracks[1])?.pan == 1)
        #expect(preview.values(on: tracks[2]) == nil)
        // The product drawing frame must put both knobs at their preview positions.
        var input = CanvasInput(snapshot: controller.snapshot, selected: tracks[2], camera: ArrangeCamera(), width: 500, height: 192, scale: 1)
        input.header = ArrangeHeaderInput(width: 220, palette: ArrangeHeaderPalette())
        input.faderPreview = preview
        let instances = ArrangeFrame(input: input).headerInstances
        for (index, value) in [(0, Float(0.25)), (1, Float(1))] {
            let rect = TrackControlLayout(row: CGRect(x: 0, y: index * 64, width: 220, height: 64)).pan
            let expected = SIMD4<Float>(Float(rect.minX + Double(value + 1) / 2 * rect.width - 2), Float(rect.minY + 3), 4, Float(rect.height - 6))
            #expect(instances.contains { $0.rect == expected })
        }
        fixture.header.mouseUp(with: try fixture.mouse(.leftMouseUp, pan, dx: 200))
        await fixture.completion?.value
        #expect(controller.mixer(tracks[0])?.pan.value == 0.25 && controller.mixer(tracks[1])?.pan.value == 1)
        #expect(controller.mixer(tracks[2])?.pan.value == 0)
        await controller.close()
    }

    @Test func quietGroupsUseFiniteOffsetsAndSilentAnchorsExplainRefusal() async throws {
        let fixture = try await MixerDragFixture()
        defer { removeTestFile(fixture.url) }
        let controller = fixture.controller, tracks = fixture.tracks
        #expect(await controller.setMixer(tracks[0], field: .gain, value: 0.00000001))
        #expect(await controller.setMixer(tracks[1], field: .gain, value: 0.00000002))
        try fixture.refresh()
        let gain = fixture.point(.gain)
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain))
        fixture.header.mouseDragged(with: try fixture.mouse(.leftMouseDragged, gain, dx: 1))
        let preview = try #require(controller.faderPreview)
        #expect(preview.gain > 0.00000001 && preview.gain < 0.000001)
        #expect(preview.values(on: tracks[1])?.gain == preview.gain * 2)
        fixture.header.mouseUp(with: try fixture.mouse(.leftMouseUp, gain))
        await fixture.completion?.value
        #expect(controller.mixer(tracks[0])?.gain.value == 0.00000001)
        #expect(await controller.setMixer(tracks[0], field: .gain, value: 0))
        try fixture.refresh()
        let undo = controller.undoCount
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain, clicks: 2))
        #expect(controller.errorMessage != nil && controller.faderPreview?.gain == 0)
        fixture.header.mouseUp(with: try fixture.mouse(.leftMouseUp, gain, clicks: 2))
        fixture.header.clicks.settle()
        await fixture.completion?.value
        #expect(controller.undoCount == undo && controller.mixer(tracks[1])?.gain.value == 0.00000002)
        controller.selection = tracks[0]
        try fixture.refresh()
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain, clicks: 2))
        fixture.header.mouseUp(with: try fixture.mouse(.leftMouseUp, gain, clicks: 2))
        fixture.header.clicks.settle()
        await fixture.completion?.value
        #expect(controller.mixer(tracks[0])?.gain.value == 1)
        await controller.close()
    }

    @Test func aDifferentPressCapturesThePendingGroupsShownResult() async throws {
        let fixture = try await MixerDragFixture()
        defer { removeTestFile(fixture.url) }
        let controller = fixture.controller, tracks = fixture.tracks, undo = controller.undoCount
        let gain = fixture.point(.gain), pan = fixture.point(.pan)
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain, clicks: 2))
        fixture.header.mouseUp(with: try fixture.mouse(.leftMouseUp, gain, clicks: 2))
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, pan))
        await fixture.completion?.value
        #expect(controller.mixer(tracks[0])?.gain.value == 1 && controller.mixer(tracks[1])?.gain.value == 0.5)
        fixture.header.mouseDragged(with: try fixture.mouse(.leftMouseDragged, pan, dx: 10))
        let preview = try #require(controller.faderPreview)
        #expect(preview.gain == 1 && preview.values(on: tracks[1])?.gain == 0.5)
        #expect(preview.pan > -0.5 && preview.values(on: tracks[1])?.pan == 0.25 + (preview.pan + 0.5))
        fixture.header.mouseUp(with: try fixture.mouse(.leftMouseUp, pan, dx: 10))
        await fixture.completion?.value
        #expect(controller.undoCount == undo + 2)
        await controller.undo()
        #expect(controller.mixer(tracks[0])?.pan.value == -0.5 && controller.mixer(tracks[1])?.gain.value == 0.5)
        await controller.undo()
        #expect(controller.mixer(tracks[0])?.gain.value == 0.5 && controller.mixer(tracks[1])?.gain.value == 0.25)
        await controller.close()
    }

    @Test func thirdClickAndDocumentReplacementCancelTheCompleteGroup() async throws {
        let fixture = try await MixerDragFixture()
        defer { removeTestFile(fixture.url) }
        let controller = fixture.controller, tracks = fixture.tracks
        let gain = fixture.point(.gain), undo = controller.undoCount
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain, clicks: 2))
        fixture.header.mouseUp(with: try fixture.mouse(.leftMouseUp, gain, clicks: 2))
        #expect(controller.faderPreview?.values(on: tracks[1])?.gain == 0.5)
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain, clicks: 3))
        await fixture.completion?.value
        #expect(fixture.header.entry != nil && controller.faderPreview == nil && controller.undoCount == undo)
        fixture.header.entry?.cancel()
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain))
        fixture.header.mouseDragged(with: try fixture.mouse(.leftMouseDragged, gain, dx: 10))
        let old = try #require(controller.faderPreview)
        await controller.close(); await controller.open(fixture.url)
        fixture.header.mixActions = controller.arrangeMixActions(document: controller.documentID)
        fixture.header.documentID = controller.documentID
        fixture.header.mouseUp(with: try fixture.mouse(.leftMouseUp, gain, dx: 20))
        await fixture.completion?.value
        #expect(controller.documentID != old.document && controller.faderPreview == nil)
        #expect(controller.undoCount == 0 && controller.mixer(tracks[0])?.gain.value == 0.5 && controller.mixer(tracks[1])?.gain.value == 0.25)
        await controller.close()
    }
}
