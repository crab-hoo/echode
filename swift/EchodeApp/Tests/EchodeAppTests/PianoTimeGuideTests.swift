import Foundation
import Testing

@testable import EchodeAppCore

struct PianoTimeGuideTests {
    @Test func contractBeatAndBarGuidesSurviveCoarseAndThinnedRhythmicGrids() throws {
        // These viewport sizes and note values scope this rendering regression only.
        for scale in [1.0, 2.0] {
            for ticks: Int64 in [2_822_400, 1_881_600, 4_233_600, 14_700, 33_075, 706_100] {
                for pixels in [8.0, 24, 72] {
                    let camera = PianoCamera(originBeat: 3.3, pixelsPerBeat: pixels, gridMode: .fixed(ticks))
                    let frame = timeGuideFrame(camera, scale: scale)
                    let lines = frame.gridInstances.filter { $0.rect.z == Float(scale) }
                    #expect(lines.count <= Int(frame.layout.grid.width / 4 + frame.layout.grid.width / 8) + 2)
                    for beat in 4...10 {
                        let x = frame.layout.gridOrigin.x + (Double(beat) - camera.originBeat) * pixels
                        guard x < frame.layout.grid.maxX else { continue }
                        let color: SIMD4<Float> =
                            beat % 4 == 0 ? SIMD4(0.27, 0.29, 0.33, 1) : SIMD4(0.18, 0.20, 0.23, 1)
                        let line = try #require(
                            lines.last { abs(Double($0.rect.x) - x * scale) < 0.001 },
                            "Beat and bar guides must survive coarse or thinned rhythmic grids")
                        #expect(line.color == color)
                        // A nearby subdivision must never paint over a stronger guide.
                        let center = line.rect.x + Float(scale) / 2
                        let topmost = lines.last { $0.rect.x <= center && $0.rect.x + $0.rect.z > center }
                        #expect(topmost?.color == color, "Time guides must paint above nearby subdivisions")
                        #expect(
                            lines.filter { abs(Double($0.rect.x) - x * scale) < 0.001 }.count == 1,
                            "A coincident snap and time guide must be drawn only once")
                    }
                    #expect(frame.piano.camera == camera && camera.grid == ticks)
                    #expect(camera.snapped(ticks - 1) == ticks, "Display guides must not become snap targets")
                }
            }
        }
    }

    @Test func timeGuidesKeepProjectPhaseAndViewportBoundedWork() {
        for mode in PianoGrid.choices {
            for pixels in [8.0, 17, 72, 1200] {
                for origin in [0.0, 3.3, 1234.3, 1_000_000_000.3] {
                    let camera = PianoCamera(originBeat: origin, pixelsPerBeat: pixels, gridMode: mode)
                    let frame = timeGuideFrame(camera)
                    let lines = frame.gridLineTicks
                    #expect(Set(lines).count == lines.count)
                    #expect(lines.count <= Int(frame.layout.grid.width / 4 + frame.layout.grid.width / 8) + 2)
                    #expect(
                        lines.allSatisfy {
                            Double($0) / 705_600 >= origin
                                && Double($0) / 705_600 <= origin + frame.layout.grid.width / pixels
                        })
                    #expect(frame.gridTicks.allSatisfy { lines.contains($0) })
                    let first = Int64(ceil(origin))
                    let last = Int64(floor(origin + frame.layout.grid.width / pixels))
                    if first <= last {
                        for beat in first...last { #expect(lines.contains(beat * 705_600)) }
                    }
                    let levels = lines.map { $0 % 2_822_400 == 0 ? 2 : ($0 % 705_600 == 0 ? 1 : 0) }
                    #expect(zip(levels, levels.dropFirst()).allSatisfy { $0 <= $1 })
                }
            }
        }
        for pixels in [0.0, -.infinity, .infinity, .nan] {
            #expect(timeGuideFrame(PianoCamera(pixelsPerBeat: pixels)).gridLineTicks.isEmpty)
        }
        #expect(timeGuideFrame(PianoCamera(originBeat: -1)).gridLineTicks.isEmpty)
        #expect(timeGuideFrame(PianoCamera(originBeat: .infinity)).gridLineTicks.isEmpty)
    }
}

private func timeGuideFrame(_ camera: PianoCamera, scale: Double = 1, width: Double = 640) -> PianoFrame {
    let track = UUID()
    let input = CanvasInput(
        snapshot: nil, selected: track, camera: ArrangeCamera(), width: width, height: 400, scale: scale)
    return PianoFrame(
        input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera, key: DisplayKey(), preview: nil)
    )
}
