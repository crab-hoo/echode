import Foundation
import Testing
@testable import EchodeAppCore

struct PianoRhythmicGridBoundaryTests {
    @Test func everyRhythmicValueHasAnExactUniqueWidthAndReadableTitle() {
        let triplets: [Int64] = [1_881_600, 940_800, 470_400, 235_200, 117_600, 58_800, 29_400, 14_700]
        let dotted: [Int64] = [4_233_600, 2_116_800, 1_058_400, 529_200, 264_600, 132_300, 66_150, 33_075]
        #expect(PianoGrid.choices.count == 25 && Set(PianoGrid.choices.map(\.title)).count == 25)
        for (values, name) in [(triplets, "Triplet"), (dotted, "Dotted")] {
            for (index, ticks) in values.enumerated() {
                let mode = PianoGrid.fixed(ticks)
                #expect(PianoGrid.choices.contains(mode))
                #expect(mode.title == "1/\(1 << index) \(text(name))")
                for pixels in [8.0, 72, 1200] {
                    let camera = PianoCamera(pixelsPerBeat: pixels, gridMode: mode)
                    #expect(camera.grid == ticks)
                    #expect(camera.snapped(ticks - 1) == ticks)
                    #expect(camera.snapped(ticks + ticks / 2 + 1) == ticks * 2)
                }
            }
        }
        #expect(PianoGrid.noteValue(0) == "0 ticks")
        #expect(PianoGrid.noteValue(123) == "123 ticks")
    }

    @Test func rhythmicLinesStayAlignedAfterScrollingAndBoundedWhenDense() {
        let track = UUID()
        let input = CanvasInput(snapshot: nil, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
        for ticks: Int64 in [235_200, 529_200, 14_700, 33_075] {
            for pixels in [8.0, 72, 1200] {
                let camera = PianoCamera(originBeat: 1234.3, pixelsPerBeat: pixels, gridMode: .fixed(ticks))
                let frame = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera, key: DisplayKey(), preview: nil))
                #expect(!frame.gridTicks.isEmpty)
                #expect(frame.gridTicks.count <= Int(frame.layout.grid.width / 4) + 1)
                #expect(frame.gridTicks.allSatisfy { $0 % ticks == 0 && Double($0) / 705_600 >= camera.originBeat })
                #expect(zip(frame.gridTicks, frame.gridTicks.dropFirst()).allSatisfy { Double($1 - $0) / 705_600 * pixels >= 4 })
                #expect(camera.grid == ticks)
            }
        }
    }
}
