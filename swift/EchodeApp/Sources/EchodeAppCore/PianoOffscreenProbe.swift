import EchodeBridge
import Foundation
import Metal

private struct OffscreenPianoReport: Codable {
    let frames: Int
    let comparedPixels: Int
    let checkedPitchRows: Int
    let checkedDegreeRows: Int
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
    for scale in [1.0, 2.0] {
        for event in 0..<24 {
            let camera = PianoCamera(originBeat: Double(event % 5) / 7, topPitch: 74 + Double(event % 8) / 8,
                                     pixelsPerBeat: [24.0, 72.0, 144.0][event % 3], pixelsPerPitch: [12.0, 20.0, 48.0][(event / 3) % 3])
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
                    guard y >= frame.layout.rulerHeight, y < input.height else { continue }
                    checkedDegrees += 1
                    let outside = key.degree(row.pitch) == nil
                    let expected = outside ? CanvasReadback.Pixel(b: 31, g: 26, r: 23, a: 255) : CanvasReadback.Pixel(b: 43, g: 38, r: 36, a: 255)
                    if image.pixel(x: Int(2 * scale), y: Int(y * scale)) != expected { mismatches += 1 }
                    let separatorY = Int((Double(row.y + row.height - 1) * scale - 0.5).rounded(.up))
                    if separatorY >= Int(frame.layout.rulerHeight * scale), separatorY < image.height,
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
            // Choose a background column past the notes, clear of beat lines.
            let gridX = (Int(referenceFrame.layout.grid.width - 24)..<Int(referenceFrame.layout.grid.width - 3)).first { x in
                let beatX = (camera.originBeat + Double(x) / camera.pixelsPerBeat).truncatingRemainder(dividingBy: 1) * camera.pixelsPerBeat
                return beatX > 3 && beatX < camera.pixelsPerBeat - 3
            }!
            for row in referenceFrame.rows {
                let y = Double(row.y) + Double(row.height) / 2
                guard y >= referenceFrame.layout.rulerHeight, y < input.height else { continue }
                let grid = reference.pixel(x: Int((referenceFrame.layout.gridOrigin.x + Double(gridX)) * scale), y: Int(y * scale))
                let keyboard = reference.pixel(x: Int((referenceFrame.layout.gridOrigin.x - 4) * scale), y: Int(y * scale))
                let expectedGrid = row.black ? CanvasReadback.Pixel(b: 31, g: 26, r: 23, a: 255) : CanvasReadback.Pixel(b: 43, g: 38, r: 36, a: 255)
                let expectedKey = row.black ? CanvasReadback.Pixel(b: 33, g: 28, r: 26, a: 255) : CanvasReadback.Pixel(b: 232, g: 227, r: 224, a: 255)
                checkedRows += 1
                if grid != expectedGrid || keyboard != expectedKey { mismatches += 1 }
                let backY = Double(row.y) + Double(row.height) / 4
                if row.black, backY >= referenceFrame.layout.rulerHeight {
                    let back = reference.pixel(x: Int((referenceFrame.layout.degreeWidth + 4) * scale), y: Int(backY * scale))
                    if back != CanvasReadback.Pixel(b: 232, g: 227, r: 224, a: 255) { mismatches += 1 }
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
                                      checkedDegreeRows: checkedDegrees, mismatches: mismatches,
                                      staleOverlapPixels: staleOverlapPixels, staleMarginPixels: staleMarginPixels,
                                      degreePixels: degreePixels, device: device.name)
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    try encoder.encode(report).write(to: URL(fileURLWithPath: reportPath))
    print("offscreen piano: \(frames) frames, \(compared) compared pixels, \(checkedRows) pitch rows, \(mismatches) mismatches, \(staleOverlapPixels) stale overlap pixels, \(staleMarginPixels) stale margin pixels, \(degreePixels) degree pixels, \(device.name)")
    guard mismatches == 0, staleOverlapPixels == 0, staleMarginPixels == 0, degreePixels > 0 else { throw CocoaError(.coderInvalidValue) }
}
