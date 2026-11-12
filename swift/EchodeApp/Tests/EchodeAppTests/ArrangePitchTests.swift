import AppKit
import EchodeBridge
import Foundation
import Metal
import Testing

@testable import EchodeAppCore

@MainActor
struct ArrangePitchTests {
    @Test func contractPitchPreviewMatchesRenderedAndAccessibleRects() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-pitches-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Pitches")
        // Scene sizes and zooms are regression inputs, not product limits.
        for (index, pitch) in [48, 60, 72].enumerated() {
            await controller.insertNote(pitch: pitch, beat: Double(index), length: 1)
        }
        let snapshot = try #require(controller.snapshot)
        let device = try #require(MTLCreateSystemDefaultDevice())
        let painter = try ArrangePainter(device: device)
        let queue = try #require(device.makeCommandQueue())
        for scale in [1.0, 2.0] {
            for rowHeight in [16.0, 64.0, 240.0] {
                let input = CanvasInput(
                    snapshot: snapshot, selected: nil,
                    camera: ArrangeCamera(pixelsPerBeat: 72, pixelsPerTrack: rowHeight),
                    width: 480, height: 280, scale: scale,
                    header: ArrangeHeaderInput(width: 160, palette: ArrangeHeaderPalette()),
                    arrangeRuler: ArrangeRulerInput(start: nil, loop: nil))
                let frame = ArrangeFrame(input: input)
                let notes = frame.elements.filter { $0.role == .note }
                #expect(notes.count == 3)
                #expect(notes[0].y > notes[1].y && notes[1].y > notes[2].y)
                let instances = Array(
                    frame.timelineInstances[frame.foregroundStart..<frame.foregroundStart + notes.count])
                let view = ArrangeView(frame: NSRect(x: 0, y: 0, width: input.width, height: input.height))
                view.setSnapshot(snapshot, selected: nil)
                view.present(frame)
                let image = try renderOffscreen(frame, painter: painter, device: device, queue: queue)
                for (note, instance) in zip(notes, instances) {
                    let expected = CGRect(
                        x: frame.timelineOrigin + Double(note.x), y: input.arrangeTop + Double(note.y),
                        width: Double(note.width), height: Double(note.height))
                    #expect(view.accessibilityRect(for: note) == expected)
                    #expect(abs(Double(instance.rect.y / Float(scale)) - expected.minY) < 0.0001)
                    #expect(abs(Double(instance.rect.w / Float(scale)) - expected.height) < 0.0001)
                    let x = Int(expected.midX * scale)
                    // The note's rasterized rows must agree with the shared rect,
                    // including fractional edges and the one-point minimum.
                    var painted = 0
                    for y in Int(input.arrangeTop * scale)..<Int((input.arrangeTop + rowHeight - 1) * scale) {
                        let center = (Double(y) + 0.5) / scale
                        let inside = center >= expected.minY && center < expected.maxY
                        let pixel = image.pixel(x: x, y: y)
                        let isNote = pixel == CanvasReadback.Pixel(b: 224, g: 212, r: 133, a: 255)
                        #expect(isNote == inside, "pitch raster differs at \(x),\(y)")
                        if isNote { painted += 1 }
                    }
                    #expect(painted > 0)
                }
                #expect(view.window == nil && !view.rendererAvailable)
                view.stop()
            }
        }
        await controller.close()
    }
}
