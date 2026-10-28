import EchodeBridge
import Foundation
import Metal

private struct OffscreenPianoReport: Codable {
    let frames: Int
    let comparedPixels: Int
    let checkedPitchRows: Int
    let checkedDegreeRows: Int
    let checkedVelocityBars: Int
    let checkedReferenceNotes: Int
    let checkedGridLines: Int
    let checkedIndependentGuides: Int
    let mismatches: Int
    let staleOverlapPixels: Int
    let staleMarginPixels: Int
    let degreePixels: Int
    let device: String
}

/// Uses the product encoder, without windows, display links or screen capture.
@MainActor
public func runOffscreenPianoProbe(reportPath: String) async throws {
    guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else { throw CocoaError(.featureUnsupported) }
    let painter = try ArrangePainter(device: device)
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-frame-\(UUID()).echode")
    defer { try? FileManager.default.removeItem(at: url) }
    let controller = ProjectController()
    await controller.open(url, creating: true)
    await controller.addTrack(name: "Piano")
    // This project's note count and event count are scoped to this probe.
    for pitch in 60...73 { await controller.insertNote(pitch: pitch, beat: Double(pitch - 60) / 8, length: 0.75) }
    guard let snapshot = controller.snapshot, let track = controller.selection else { throw CocoaError(.coderValueNotFound) }
    var frames = 0, compared = 0, checkedRows = 0, checkedDegrees = 0, mismatches = 0, degreePixels = 0
    var checkedGridLines = 0, checkedIndependentGuides = 0
    for scale in [1.0, 2.0] {
        for event in 0..<40 {
            let camera = PianoCamera(originBeat: Double(event % 5) / 7, topPitch: 74 + Double(event % 8) / 8,
                                     pixelsPerBeat: (event < 32 ? [24.0, 72.0, 144.0] : [8.0, 24.0, 72.0])[event % 3], pixelsPerPitch: [12.0, 20.0, 48.0][(event / 3) % 3],
                                     gridMode: event < 24 ? .automatic : .fixed(event < 32 ? (event % 2 == 0 ? 235_200 : 529_200) :
                                        [2_822_400, 1_881_600, 4_233_600, 33_075][event % 4]))
            let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(),
                                     width: event % 2 == 0 ? 640 : 720, height: 331, scale: scale)
            let referenceFrame = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera, key: DisplayKey(), preview: nil))
            let reference = try renderOffscreen(referenceFrame, painter: painter, device: device, queue: queue)
            frames += 1
            for key in [DisplayKey(tonic: 2), DisplayKey(tonic: 6, minor: true)] {
                let frame = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera, key: key, preview: nil))
                let image = try renderOffscreen(frame, painter: painter, device: device, queue: queue)
                // Changing scale may affect only the degree column. Compare
                // every GPU pixel in the keyboard, ruler and note grid.
                for y in 0..<image.height {
                    for x in Int(frame.layout.degreeWidth * scale)..<image.width {
                        compared += 1
                        if image.pixel(x: x, y: y) != reference.pixel(x: x, y: y) { mismatches += 1 }
                    }
                }
                // Degree rows use the grid's white/black shades according to
                // scale membership, with a separator at every pitch boundary.
                for row in frame.rows {
                    let y = Double(row.y) + Double(row.height) / 2
                    guard y >= frame.layout.rulerHeight, y < frame.layout.grid.maxY else { continue }
                    checkedDegrees += 1
                    let outside = key.degree(row.pitch) == nil
                    let expected = outside ? CanvasReadback.Pixel(b: 31, g: 26, r: 23, a: 255) : CanvasReadback.Pixel(b: 43, g: 38, r: 36, a: 255)
                    if image.pixel(x: Int(2 * scale), y: Int(y * scale)) != expected { mismatches += 1 }
                    let separatorY = Int((Double(row.y + row.height - 1) * scale - 0.5).rounded(.up))
                    if separatorY >= Int(frame.layout.rulerHeight * scale), separatorY < Int(frame.layout.grid.maxY * scale),
                       image.pixel(x: Int(2 * scale), y: separatorY) != CanvasReadback.Pixel(b: 84, g: 74, r: 69, a: 255) { mismatches += 1 }
                    // Out-of-scale rows stay blank, including beside notes
                    // beginning before the viewport's left edge.
                    if outside {
                        for x in Int(4 * scale)..<Int((frame.layout.degreeWidth - 4) * scale) {
                            if image.pixel(x: x, y: Int(y * scale)) != expected { mismatches += 1 }
                        }
                    }
                }
                if scale == 2, event == 3, key.tonic == 2 {
                    try image.writePNG(to: URL(fileURLWithPath: reportPath).deletingPathExtension().appendingPathExtension("png"))
                }
                frames += 1
            }
            // Choose a background column past the notes, clear of grid lines.
            let gridX = (Int(referenceFrame.layout.grid.width - 24)..<Int(referenceFrame.layout.grid.width - 3)).first { x in
                referenceFrame.gridLineTicks.allSatisfy { tick in
                    abs((Double(tick) / 705_600 - camera.originBeat) * camera.pixelsPerBeat - Double(x)) > 1.5
                }
            }!
            for row in referenceFrame.rows {
                let y = Double(row.y) + Double(row.height) / 2
                guard y >= referenceFrame.layout.rulerHeight, y < referenceFrame.layout.grid.maxY else { continue }
                let grid = reference.pixel(x: Int((referenceFrame.layout.gridOrigin.x + Double(gridX)) * scale), y: Int(y * scale))
                let keyboard = reference.pixel(x: Int((referenceFrame.layout.gridOrigin.x - 4) * scale), y: Int(y * scale))
                let expectedGrid = row.black ? CanvasReadback.Pixel(b: 31, g: 26, r: 23, a: 255) : CanvasReadback.Pixel(b: 43, g: 38, r: 36, a: 255)
                let expectedKey = row.black ? CanvasReadback.Pixel(b: 33, g: 28, r: 26, a: 255) : CanvasReadback.Pixel(b: 232, g: 227, r: 224, a: 255)
                checkedRows += 1
                if grid != expectedGrid || keyboard != expectedKey { mismatches += 1 }
                let backY = Double(row.y) + Double(row.height) / 4
                if row.black, backY >= referenceFrame.layout.rulerHeight, backY < referenceFrame.layout.grid.maxY {
                    let back = reference.pixel(x: Int((referenceFrame.layout.degreeWidth + 4) * scale), y: Int(backY * scale))
                    if back != CanvasReadback.Pixel(b: 232, g: 227, r: 224, a: 255) { mismatches += 1 }
                }
            }
            // Check subdivision pixels beyond the notes using the product encoder.
            for tick in referenceFrame.gridLineTicks {
                let x = (Double(tick) / 705_600 - camera.originBeat) * camera.pixelsPerBeat
                guard x > referenceFrame.layout.grid.width - 150, x < referenceFrame.layout.grid.width - 3,
                      let row = referenceFrame.rows.first else { continue }
                let y = Double(row.y) + Double(row.height) / 2
                guard y >= referenceFrame.layout.rulerHeight, y < referenceFrame.layout.grid.maxY else { continue }
                let color: SIMD4<Float> = tick % 2_822_400 == 0 ? SIMD4(0.27, 0.29, 0.33, 1) :
                    (tick % 705_600 == 0 ? SIMD4(0.18, 0.20, 0.23, 1) : SIMD4(0.13, 0.15, 0.18, 1))
                let expected = CanvasReadback.Pixel(b: UInt8((color.z * 255).rounded()), g: UInt8((color.y * 255).rounded()),
                                                    r: UInt8((color.x * 255).rounded()), a: 255)
                checkedGridLines += 1
                if tick % 705_600 == 0, !referenceFrame.gridTicks.contains(tick) { checkedIndependentGuides += 1 }
                // Sample a covered pixel center, using the encoder's Float coordinates.
                let pixelX = Int(ceil(Double(Float(referenceFrame.layout.gridOrigin.x + x) * Float(scale)) - 0.5))
                let actual = reference.pixel(x: pixelX, y: Int(y * scale))
                if actual != expected {
                    mismatches += 1
                    print("grid pixel mismatch: scale=\(scale), grid=\(camera.gridMode), tick=\(tick), x=\(pixelX), actual=\(actual), expected=\(expected)")
                }
            }
            for y in Int(referenceFrame.layout.rulerHeight * scale)..<reference.height {
                for x in Int(4 * scale)..<Int((referenceFrame.layout.degreeWidth - 4) * scale) {
                    let pixel = reference.pixel(x: x, y: y)
                    if pixel.r > 180 && pixel.g > 160 { degreePixels += 1 }
                }
            }
        }
    }
    // Lane caps, including zero, use the same product encoder and never spill into the grid.
    var checkedVelocityBars = 0
    if let note = snapshot.pianoNeighbor(track: track, current: nil) {
        for scale in [1.0, 2.0] {
            for value in [0, 64, 127] {
                let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 331, scale: scale)
                let frame = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: note.id, camera: PianoCamera(),
                    key: DisplayKey(), preview: nil, velocity: PianoVelocityPreview(id: note.id, value: value)))
                let image = try renderOffscreen(frame, painter: painter, device: device, queue: queue)
                guard let bar = frame.velocityBars.first(where: { $0.note.id == note.id }) else { throw CocoaError(.coderValueNotFound) }
                let color = SIMD4<Float>(0.99, 0.73, 0.31, 1)
                let expected = CanvasReadback.Pixel(b: UInt8((color.z * 255).rounded()), g: UInt8((color.y * 255).rounded()),
                                                    r: UInt8((color.x * 255).rounded()), a: 255)
                if image.pixel(x: Int((bar.stem.minX + 1) * scale), y: Int(bar.cap.midY * scale)) != expected { mismatches += 1 }
                checkedVelocityBars += 1; frames += 1
            }
        }
    }
    // The real endpoint has a grip; a clipped band edge must not acquire one.
    for scale in [1.0, 2.0] {
        for enabled in [false, true] {
            let input = CanvasInput(snapshot: nil, selected: nil, camera: ArrangeCamera(), width: 640, height: 200, scale: scale)
            let loop = RulerLoop(range: LoopRange(start: 705_600, end: 4 * 705_600)!, enabled: enabled)
            let frame = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil,
                camera: PianoCamera(originBeat: 2), key: DisplayKey(), preview: nil, loop: loop))
            let image = try renderOffscreen(frame, painter: painter, device: device, queue: queue)
            let grip = enabled ? SIMD4<Float>(0.55, 0.94, 0.67, 1) : SIMD4<Float>(0.65, 0.68, 0.73, 1)
            let band = enabled ? SIMD4<Float>(0.22, 0.34, 0.30, 1) : SIMD4<Float>(0.20, 0.22, 0.25, 1)
            for (x, y, color) in [(223.0, 6.0, grip), (222, 1, grip), (80, 6, band)] {
                let expected = CanvasReadback.Pixel(b: UInt8((color.z * 255).rounded()), g: UInt8((color.y * 255).rounded()),
                                                    r: UInt8((color.x * 255).rounded()), a: 255)
                compared += 1
                if image.pixel(x: Int(x * scale), y: Int(y * scale)) != expected { mismatches += 1 }
            }
            frames += 1
        }
    }
    // Reference notes share the product encoder, but never the velocity or keyboard planes.
    await controller.addTrack(name: "Reference")
    guard let referenceTrack = controller.selection else { throw CocoaError(.coderValueNotFound) }
    await controller.insertNote(pitch: 62, beat: 4, length: 1, on: referenceTrack)
    await controller.insertNote(pitch: 62, beat: 0.25, length: 0.75, on: referenceTrack)
    guard let references = controller.snapshot else { throw CocoaError(.coderValueNotFound) }
    var checkedReferenceNotes = 0
    for scale in [1.0, 2.0] {
        let input = CanvasInput(snapshot: references, selected: track, camera: ArrangeCamera(), width: 640, height: 331, scale: scale)
        let camera = PianoCamera()
        let frame = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera,
            key: DisplayKey(), preview: nil, references: [referenceTrack]))
        let plainFrame = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera, key: DisplayKey(), preview: nil))
        let image = try renderOffscreen(frame, painter: painter, device: device, queue: queue)
        let plain = try renderOffscreen(plainFrame, painter: painter, device: device, queue: queue)
        for (beat, color) in [(4.5, PianoPalette.reference), (0.6, SIMD4<Float>(0.42, 0.76, 0.85, 1))] {
            let point = CGPoint(x: frame.layout.gridOrigin.x + beat * camera.pixelsPerBeat,
                y: frame.layout.gridOrigin.y + (camera.topPitch - 62 + 0.5) * camera.pixelsPerPitch)
            let expected = CanvasReadback.Pixel(b: UInt8((color.z * 255).rounded()), g: UInt8((color.y * 255).rounded()),
                r: UInt8((color.x * 255).rounded()), a: 255)
            if image.pixel(x: Int(point.x * scale), y: Int(point.y * scale)) != expected { mismatches += 1 }
            checkedReferenceNotes += 1
        }
        for y in 0..<image.height {
            for x in 0..<image.width where y >= Int(frame.layout.velocity.minY * scale) || x < Int(frame.layout.grid.minX * scale) {
                compared += 1
                if image.pixel(x: x, y: y) != plain.pixel(x: x, y: y) { mismatches += 1 }
            }
        }
        frames += 2
    }
    // The shared encoder draws a stale extent at the input's pixel positions.
    let staleInput = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(),
                                 width: 640, height: 331, scale: 2)
    let staleFrame = PianoFrame(input: staleInput, piano: PianoRenderInput(track: track, focus: nil, camera: PianoCamera(),
                                                                           key: DisplayKey(), preview: nil))
    let exact = try renderOffscreen(staleFrame, painter: painter, device: device, queue: queue)
    let stale = try renderOffscreen(staleFrame, painter: painter, device: device, queue: queue,
                                    size: CGSize(width: exact.width + 29, height: exact.height + 17))
    let staleBackground = CanvasReadback.Pixel.canvasBackground
    var staleOverlapPixels = 0, staleMarginPixels = 0
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
    await controller.close()
    let report = OffscreenPianoReport(frames: frames, comparedPixels: compared, checkedPitchRows: checkedRows,
                                      checkedDegreeRows: checkedDegrees, checkedVelocityBars: checkedVelocityBars, checkedReferenceNotes: checkedReferenceNotes, checkedGridLines: checkedGridLines, checkedIndependentGuides: checkedIndependentGuides, mismatches: mismatches,
                                      staleOverlapPixels: staleOverlapPixels, staleMarginPixels: staleMarginPixels,
                                      degreePixels: degreePixels, device: device.name)
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    try encoder.encode(report).write(to: URL(fileURLWithPath: reportPath))
    print("offscreen piano: \(frames) frames, \(compared) compared pixels, \(checkedRows) pitch rows, \(checkedVelocityBars) velocity bars, \(checkedReferenceNotes) reference notes, \(checkedGridLines) grid lines, \(checkedIndependentGuides) independent guides, \(mismatches) mismatches, \(staleOverlapPixels) stale overlap pixels, \(staleMarginPixels) stale margin pixels, \(degreePixels) degree pixels, \(device.name)")
    guard mismatches == 0, staleOverlapPixels == 0, staleMarginPixels == 0, degreePixels > 0, checkedVelocityBars == 6, checkedReferenceNotes == 4, checkedGridLines > 0, checkedIndependentGuides > 0 else { throw CocoaError(.coderInvalidValue) }
}
