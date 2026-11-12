import EchodeBridge
import Foundation
import ImageIO
import Metal

private struct OffscreenArrangeReport: Codable {
    let frames: Int
    let comparedScanlines: Int
    let mismatchedScanlines: Int
    let laneMismatches: Int
    let clippedHeaderMismatches: Int
    let rulerMismatches: Int
    let staleOverlapPixels: Int
    let staleMarginPixels: Int
    let headerProbeXPoints: Double
    let labelPixels: Int
    let device: String
}

/// Explicit GPU probe, with no NSApplication, window, display link or screen
/// capture. GPU readback/waits belong to this probe, never the audio thread.
@MainActor
public func runOffscreenArrangeProbe(reportPath: String) async throws {
    guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
        throw CocoaError(.featureUnsupported)
    }
    let painter = try ArrangePainter(device: device)
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-offscreen-\(UUID()).echode")
    defer { try? FileManager.default.removeItem(at: url) }
    let controller = ProjectController()
    await controller.open(url, creating: true)
    // Project size and event count are scoped to this regression only.
    for index in 0..<20 { await controller.addTrack(name: index == 0 ? "ピアノ 🎹" : "Track \(index)") }
    guard let snapshot = controller.snapshot else { throw CocoaError(.coderValueNotFound) }
    var frames = 0
    var scanlines = 0
    var laneMismatches = 0
    var clippedHeaderMismatches = 0
    var staleOverlapPixels = 0
    var staleMarginPixels = 0
    var labelPixels = 0
    var rulerMismatches = 0
    // This probe samples reserved left padding, outside labels, icons and mixer
    // controls. Its coordinates and scene sizes are scoped to this regression.
    let headerProbeXPoints = 2.0
    for scale in [1.0, 2.0] {
        for event in 0..<80 {
            var palette = ArrangeHeaderPalette()
            palette.separator = SIMD4(0.09, 0.10, 0.12, 1)
            let camera = ArrangeCamera(
                originBeat: Double(event) / 7, originTrack: Double(event % 24) / 16,
                pixelsPerBeat: [24.0, 72.0, 144.0][(event / 3) % 3],
                pixelsPerTrack: [16.0, 64.0, 96.0][event % 3])
            let selected = snapshot.trackID(at: Int(camera.originTrack) + 1)
            let input = CanvasInput(
                snapshot: snapshot, selected: selected, camera: camera,
                width: event % 2 == 0 ? 640 : 720, height: 211, scale: scale,
                header: ArrangeHeaderInput(width: event % 2 == 0 ? 160 : 200, palette: palette),
                arrangeRuler: ArrangeRulerInput(start: nil, loop: nil))
            let frame = ArrangeFrame(input: input)
            let image = try renderOffscreen(frame, painter: painter, device: device, queue: queue)
            if scale == 2, event == 1 {
                try image.writePNG(
                    to: URL(fileURLWithPath: reportPath).deletingPathExtension().appendingPathExtension("png"))
            }
            // Pick a timeline column clear of beat lines. Compare every
            // physical row with the unadorned header background/separator.
            let timelineX = (3..<Int(frame.timelineWidth - 3)).first { x in
                let beatX =
                    (camera.originBeat + Double(x) / camera.pixelsPerBeat).truncatingRemainder(dividingBy: 1)
                    * camera.pixelsPerBeat
                return beatX > 3 && beatX < camera.pixelsPerBeat - 3
            }!
            for y in Int(input.arrangeTop * scale)..<image.height {
                let a = image.pixel(x: Int(headerProbeXPoints * scale), y: y)
                let b = image.pixel(x: Int((frame.timelineOrigin + Double(timelineX)) * scale), y: y)
                scanlines += 1
                if a != b { laneMismatches += 1 }
            }
            // Text must actually reach the GPU attachment, including Japanese
            // and fallback glyphs; checking only CPU geometry is insufficient.
            for y in 0..<image.height {
                for x in Int(30 * scale)..<Int((input.header!.width - 8) * scale) {
                    let pixel = image.pixel(x: x, y: y)
                    if pixel.r > 180 && pixel.g > 180 && pixel.b > 180 { labelPixels += 1 }
                }
            }
            frames += 1
        }
    }
    // A long clip starting before the viewport must never spill into headers.
    controller.selection = snapshot.trackID(at: 0)
    await controller.insertNote(pitch: 60, beat: 0, length: 16, documentID: controller.documentID!)
    guard let notes = controller.snapshot else { throw CocoaError(.coderValueNotFound) }
    let clipped = ArrangeFrame(
        input: CanvasInput(
            snapshot: notes, selected: nil, camera: ArrangeCamera(originBeat: 7),
            width: 640, height: 211, scale: 2, header: ArrangeHeaderInput(width: 160, palette: ArrangeHeaderPalette())))
    let image = try renderOffscreen(clipped, painter: painter, device: device, queue: queue)
    let headerX = Int(headerProbeXPoints * clipped.input.scale)
    let background = image.pixel(x: headerX, y: 1)
    for y in 2..<120 {
        scanlines += 1
        if image.pixel(x: headerX, y: y) != background { clippedHeaderMismatches += 1 }
    }
    frames += 1
    // A drawable that lags the input still receives the frame at the input's
    // pixel positions. Only the newly exposed margin stays background.
    let staleInput = CanvasInput(
        snapshot: notes, selected: nil, camera: ArrangeCamera(originBeat: 3, pixelsPerBeat: 72, pixelsPerTrack: 64),
        width: 640, height: 211, scale: 2, header: ArrangeHeaderInput(width: 160, palette: ArrangeHeaderPalette()),
        arrangeRuler: ArrangeRulerInput(
            start: 3 * Tick.perQuarter,
            loop: RulerLoop(range: LoopRange(start: Tick.perQuarter, end: 5 * Tick.perQuarter)!, enabled: true)))
    let staleFrame = ArrangeFrame(input: staleInput)
    let exact = try renderOffscreen(staleFrame, painter: painter, device: device, queue: queue)
    let stale = try renderOffscreen(
        staleFrame, painter: painter, device: device, queue: queue,
        size: CGSize(width: exact.width + 37, height: exact.height + 23))
    let staleBackground = CanvasReadback.Pixel.canvasBackground
    for y in 0..<stale.height {
        for x in 0..<stale.width {
            let pixel = stale.pixel(x: x, y: y)
            if x < exact.width, y < exact.height {
                if pixel != exact.pixel(x: x, y: y) { staleOverlapPixels += 1 }
            } else if pixel != staleBackground {
                staleMarginPixels += 1
            }
        }
    }
    frames += 2
    // Ruler colors, clipping and the start flag use the same attachment as the rows.
    for scale in [1.0, 2.0] {
        for enabled in [false, true] {
            let input = CanvasInput(
                snapshot: nil, selected: nil, camera: ArrangeCamera(originBeat: 2),
                width: 640, height: 211, scale: scale,
                header: ArrangeHeaderInput(width: 160, palette: ArrangeHeaderPalette()),
                arrangeRuler: ArrangeRulerInput(
                    start: 3 * Tick.perQuarter,
                    loop: RulerLoop(
                        range: LoopRange(start: Tick.perQuarter, end: 4 * Tick.perQuarter)!, enabled: enabled)))
            let frame = ArrangeFrame(input: input)
            let image = try renderOffscreen(frame, painter: painter, device: device, queue: queue)
            let stripe = enabled ? SIMD4<Float>(0.40, 0.84, 0.56, 1) : SIMD4<Float>(0.45, 0.48, 0.53, 1)
            let band = enabled ? SIMD4<Float>(0.22, 0.34, 0.30, 1) : SIMD4<Float>(0.20, 0.22, 0.25, 1)
            let grip = enabled ? SIMD4<Float>(0.55, 0.94, 0.67, 1) : SIMD4<Float>(0.65, 0.68, 0.73, 1)
            let background = SIMD4<Float>(0.12, 0.13, 0.16, 1)
            for (x, y, color) in [
                (191.0, 1.0, stripe), (191, 22, band), (2, 1, background),
                (310, 22, background), (233, 10, SIMD4<Float>(0.40, 0.84, 0.56, 1)),
                (161, 6, band), (304, 6, grip), (303, 1, grip),
            ] {
                let expected = CanvasReadback.Pixel(
                    b: UInt8((color.z * 255).rounded()), g: UInt8((color.y * 255).rounded()),
                    r: UInt8((color.x * 255).rounded()), a: 255)
                if image.pixel(x: Int(x * scale), y: Int(y * scale)) != expected { rulerMismatches += 1 }
            }
            frames += 1
        }
    }
    await controller.close()
    let mismatches = laneMismatches + clippedHeaderMismatches + rulerMismatches
    let report = OffscreenArrangeReport(
        frames: frames, comparedScanlines: scanlines, mismatchedScanlines: mismatches,
        laneMismatches: laneMismatches, clippedHeaderMismatches: clippedHeaderMismatches,
        rulerMismatches: rulerMismatches,
        staleOverlapPixels: staleOverlapPixels, staleMarginPixels: staleMarginPixels,
        headerProbeXPoints: headerProbeXPoints,
        labelPixels: labelPixels, device: device.name)
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    try encoder.encode(report).write(to: URL(fileURLWithPath: reportPath))
    print(
        "offscreen arrange: \(frames) frames, \(scanlines) compared scanlines, \(mismatches) mismatches, \(staleOverlapPixels) stale overlap pixels, \(staleMarginPixels) stale margin pixels, \(labelPixels) text pixels, \(device.name)"
    )
    guard mismatches == 0, staleOverlapPixels == 0, staleMarginPixels == 0, labelPixels > 0 else {
        throw CocoaError(.coderInvalidValue)
    }
}

struct CanvasReadback {
    let width: Int
    let height: Int
    let pixels: [UInt8]
    struct Pixel: Equatable {
        let b: UInt8
        let g: UInt8
        let r: UInt8
        let a: UInt8
        /// The clear color rounded to the 8-bit attachment. Probes compare
        /// margins against the same value the renderer clears to.
        static var canvasBackground: Pixel {
            let color = CanvasBackground.clearColor
            return Pixel(
                b: UInt8((color.blue * 255).rounded()), g: UInt8((color.green * 255).rounded()),
                r: UInt8((color.red * 255).rounded()), a: UInt8((color.alpha * 255).rounded()))
        }
    }
    func pixel(x: Int, y: Int) -> Pixel {
        let index = (y * width + x) * 4
        return Pixel(b: pixels[index], g: pixels[index + 1], r: pixels[index + 2], a: pixels[index + 3])
    }
    func writePNG(to url: URL) throws {
        let info = CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue).union(.byteOrder32Little)
        guard let provider = CGDataProvider(data: Data(pixels) as CFData),
            let image = CGImage(
                width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: width * 4,
                space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: info, provider: provider,
                decode: nil, shouldInterpolate: false, intent: .defaultIntent),
            let destination = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil)
        else {
            throw CocoaError(.fileWriteUnknown)
        }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { throw CocoaError(.fileWriteUnknown) }
    }
}

func renderOffscreen(
    _ frame: ArrangeFrame, painter: ArrangePainter, device: any MTLDevice,
    queue: any MTLCommandQueue, size: CGSize? = nil
) throws -> CanvasReadback {
    try renderOffscreen(
        input: frame.input, timeline: frame.timelineInstances, headers: frame.headerInstances,
        frame: frame, piano: nil, painter: painter, device: device, queue: queue, size: size)
}

func renderOffscreen(
    _ frame: PianoFrame, painter: ArrangePainter, device: any MTLDevice,
    queue: any MTLCommandQueue, size: CGSize? = nil
) throws -> CanvasReadback {
    try renderOffscreen(
        input: frame.input, timeline: frame.gridInstances,
        headers: frame.degreeInstances + frame.keyboardInstances + frame.rulerInstances + frame.velocityInstances,
        frame: nil, piano: frame, painter: painter, device: device, queue: queue, size: size)
}

private func renderOffscreen(
    input: CanvasInput, timeline: [Instance], headers: [Instance], frame: ArrangeFrame?, piano: PianoFrame?,
    painter: ArrangePainter, device: any MTLDevice, queue: any MTLCommandQueue,
    size: CGSize? = nil
) throws -> CanvasReadback {
    let extent =
        size ?? CGSize(width: max(1, Int(input.width * input.scale)), height: max(1, Int(input.height * input.scale)))
    let descriptor = MTLTextureDescriptor.texture2DDescriptor(
        pixelFormat: .bgra8Unorm,
        width: max(1, Int(extent.width)), height: max(1, Int(extent.height)), mipmapped: false)
    descriptor.storageMode = .shared
    descriptor.usage = .renderTarget
    let instances = timeline + headers
    guard let texture = device.makeTexture(descriptor: descriptor), let command = queue.makeCommandBuffer(),
        let buffer = device.makeBuffer(
            length: max(1, instances.count) * MemoryLayout<Instance>.stride, options: .storageModeShared)
    else {
        throw CocoaError(.featureUnsupported)
    }
    instances.withUnsafeBytes { data in
        if let source = data.baseAddress { buffer.contents().copyMemory(from: source, byteCount: data.count) }
    }
    let pass = MTLRenderPassDescriptor()
    pass.colorAttachments[0].texture = texture
    pass.colorAttachments[0].loadAction = .clear
    pass.colorAttachments[0].storeAction = .store
    pass.colorAttachments[0].clearColor = CanvasBackground.clearColor
    guard let encoder = command.makeRenderCommandEncoder(descriptor: pass) else {
        throw CocoaError(.featureUnsupported)
    }
    painter.encode(
        encoder: encoder, texture: texture, buffer: buffer, timelineCount: timeline.count, headerCount: headers.count,
        frame: frame, piano: piano)
    encoder.endEncoding()
    command.commit()
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
