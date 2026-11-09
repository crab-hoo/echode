import EchodeBridge
import Foundation
import Metal
import Testing
@testable import EchodeAppCore

@MainActor
struct ArrangeLabelCacheTests {
    /// The ruler and the track headers each replaced the label cache, so every
    /// arrange frame rasterized all of their labels again: 47 a frame and
    /// 2.1 ms of render-thread time in `make spike-main-thread` (ADR 0289).
    @Test func contractRedrawingTheSameArrangeRasterizesNoLabel() async throws {
        guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
            throw CocoaError(.featureUnsupported)
        }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-label-cache-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        for index in 0..<4 { await controller.addTrack(name: "Track \(index)") }
        let snapshot = try #require(controller.snapshot)
        // The sizes are scoped to this regression: four header rows and a
        // ruler with a label for every beat.
        let input = CanvasInput(snapshot: snapshot, selected: snapshot.trackID(at: 0),
                                camera: ArrangeCamera(originBeat: 0, pixelsPerBeat: 24, pixelsPerTrack: 40),
                                width: 480, height: 200, scale: 2,
                                header: ArrangeHeaderInput(width: 160, palette: ArrangeHeaderPalette()),
                                arrangeRuler: ArrangeRulerInput(start: nil, loop: nil))
        let frame = ArrangeFrame(input: input)
        #expect(!frame.rulerLabels.isEmpty)
        let painter = try ArrangePainter(device: device)
        _ = try renderOffscreen(frame, painter: painter, device: device, queue: queue)
        let first = painter.rasterizedLabels
        #expect(first > frame.rulerLabels.count, "The headers' labels were not drawn")
        for _ in 0..<2 { _ = try renderOffscreen(frame, painter: painter, device: device, queue: queue) }
        #expect(painter.rasterizedLabels == first)
        await controller.close()
    }
}
