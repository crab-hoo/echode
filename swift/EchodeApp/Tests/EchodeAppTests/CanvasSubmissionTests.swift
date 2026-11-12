import AppKit
import EchodeBridge
import Foundation
import Metal
import QuartzCore
import Testing

@testable import EchodeAppCore

@MainActor
struct CanvasSubmissionTests {
    @Test func contractStaleDrawableSubmissionDrawsAndRetries() async throws {
        let device = try #require(MTLCreateSystemDefaultDevice())
        let queue = try #require(device.makeCommandQueue())
        let painter = try ArrangePainter(device: device)
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-submission-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        for index in 0..<3 { await controller.addTrack(name: "Track \(index)") }
        let snapshot = try #require(controller.snapshot)
        let selected = try #require(snapshot.trackID(at: 0))
        // Dimensions and frame counts are scoped to this product regression.
        // The last 64pt arrange row is partially visible at 160pt height.
        for piano in [false, true] {
            var input = CanvasInput(
                snapshot: snapshot, selected: selected,
                camera: ArrangeCamera(originBeat: 2, pixelsPerBeat: 72, pixelsPerTrack: 64),
                width: 320, height: 160, scale: 2,
                header: ArrangeHeaderInput(width: 120, palette: ArrangeHeaderPalette()))
            if piano {
                input.piano = PianoRenderInput(
                    track: selected, focus: nil, camera: PianoCamera(),
                    key: DisplayKey(), preview: nil)
            }
            let reference: CanvasReadback
            if let piano = input.piano {
                reference = try renderOffscreen(
                    PianoFrame(input: input, piano: piano), painter: painter, device: device, queue: queue)
            } else {
                reference = try renderOffscreen(
                    ArrangeFrame(input: input), painter: painter, device: device, queue: queue)
            }
            let painted = await reference.scan { _, _, pixel in pixel != .canvasBackground }
            #expect(painted.count > 1_000)
            let renderer = try CanvasRenderer(layer: CAMetalLayer(), onElements: { _ in }, startsDisplayLink: false)
            defer { renderer.stop() }
            renderer.update(input)
            let firstRevision = renderer.measurement.revision
            let matching = try submit(renderer, size: input.drawableSize, device: device)
            #expect(matching.width == reference.width && matching.height == reference.height)
            await expectPixels(matching, reference: reference)
            #expect(renderer.measurement.drawnRevision == firstRevision)
            #expect(renderer.measurement.displayLinkPaused)

            // A mismatch can arrive after this revision was already consumed.
            // Retry must stay awake even though revision equality still holds.
            let sameRevision = try submit(renderer, size: CGSize(width: 700, height: 380), device: device)
            await expectPixels(sameRevision, reference: reference)
            #expect(renderer.measurement.drawnRevision == firstRevision)
            #expect(!renderer.measurement.displayLinkPaused)

            renderer.update(input)
            let nextRevision = renderer.measurement.revision
            #expect(nextRevision != firstRevision)
            for size in [
                CGSize(width: 700, height: 380), CGSize(width: 600, height: 280),
                CGSize(width: 700, height: 280), CGSize(width: 600, height: 380),
            ] {
                let stale = try submit(renderer, size: size, device: device)
                await expectPixels(stale, reference: reference)
                #expect(renderer.measurement.drawnRevision == firstRevision)
                #expect(!renderer.measurement.displayLinkPaused)
            }
            #expect(renderer.measurement.sizeMismatches == 5)
            let corrected = try submit(renderer, size: input.drawableSize, device: device)
            #expect(corrected.width == reference.width && corrected.height == reference.height)
            await expectPixels(corrected, reference: reference)
            #expect(renderer.measurement.drawnRevision == nextRevision)
            #expect(renderer.measurement.displayLinkPaused)

            let empty = CanvasInput(
                snapshot: snapshot, selected: selected, camera: input.camera, width: 0, height: 0, scale: 2)
            renderer.update(empty)
            let blank = try submit(renderer, size: input.drawableSize, device: device)
            let nonBackground = await blank.scan { _, _, pixel in pixel != .canvasBackground }
            #expect(nonBackground.count == 0)
            #expect(renderer.measurement.drawnRevision == nextRevision)
            #expect(renderer.measurement.displayLinkPaused)
        }
        await controller.close()
    }

    private func expectPixels(_ image: CanvasReadback, reference: CanvasReadback) async {
        // Report mismatch counts instead of expanding entire pixel arrays in
        // expected negative-control failures.
        let background = CanvasReadback.Pixel.canvasBackground
        let overlap = await image.scan { x, y, pixel in
            x < reference.width && y < reference.height && pixel != reference.pixel(x: x, y: y)
        }
        let margin = await image.scan { x, y, pixel in
            (x >= reference.width || y >= reference.height) && pixel != background
        }
        #expect(overlap.count == 0, "first overlap difference at \(overlap.first ?? "none")")
        #expect(margin.count == 0, "first margin difference at \(margin.first ?? "none")")
    }

    private func submit(_ renderer: CanvasRenderer, size: CGSize, device: any MTLDevice) throws -> CanvasReadback {
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm,
            width: Int(size.width), height: Int(size.height), mipmapped: false)
        descriptor.storageMode = .shared
        descriptor.usage = .renderTarget
        let texture = try #require(device.makeTexture(descriptor: descriptor))
        let command = try #require(renderer.submit(texture: texture))
        // Only the offscreen test waits for GPU completion to read its pixels.
        command.waitUntilCompleted()
        if let error = command.error { throw error }
        var pixels = [UInt8](repeating: 0, count: texture.width * texture.height * 4)
        pixels.withUnsafeMutableBytes {
            texture.getBytes(
                $0.baseAddress!, bytesPerRow: texture.width * 4,
                from: MTLRegionMake2D(0, 0, texture.width, texture.height), mipmapLevel: 0)
        }
        return CanvasReadback(width: texture.width, height: texture.height, pixels: pixels)
    }
}
