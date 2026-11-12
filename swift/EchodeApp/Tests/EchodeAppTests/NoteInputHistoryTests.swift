import Testing

@testable import EchodeAppCore

struct NoteInputHistoryTests {
    @Test func initialLengthUndoBranchesAndSelectionsDuringWrites() {
        var memory = NoteInputLength()
        memory.record(nil)  // A track edit must reserve an untouched history slot.
        memory.record(memory.change(to: 17))
        #expect(memory.length == 17)
        memory.undo()
        #expect(memory.length == nil)
        memory.redo()
        #expect(memory.length == 17)
        let pending = memory.change(to: 29)
        memory.remember(31)  // A newer selection wins over a delayed journal reply.
        memory.record(pending)
        #expect(memory.length == 31)
        memory.undo()
        #expect(memory.length == 17)
        memory.redo()
        #expect(memory.length == 29)
        memory.record(nil)
        memory.remember(37)
        memory.undo()
        #expect(memory.length == 37)
        memory.undo()
        #expect(memory.length == 17)
        memory.record(memory.change(to: 41))  // Branch discards both redo slots.
        memory.redo()
        #expect(memory.length == 41)
        memory.undo()
        memory.undo()
        memory.undo()
        #expect(memory.length == nil)
        memory.remember(0)
        memory.remember(-1)
        #expect(memory.length == nil && memory.change(to: 0) == nil)
    }

    @Test func clicksKeepRememberedLengthWhileDragsUseTheirGridMinimum() throws {
        for grid: Int64 in [1, 176_400, 705_600] {
            for pixels in [18.0, 72.0, 477.0] {
                let camera = PianoCamera(pixelsPerBeat: pixels, gridMode: .fixed(grid))
                for length: Int64 in [1, 70_560, 585_648] {
                    var press = try #require(
                        PianoInsertion(x: pixels, y: 90, camera: camera, snapping: true, length: length))
                    press.update(x: pixels + 1, y: 91, snapping: false)
                    #expect(
                        press.geometry?.length == length, "a click must not round its remembered length to the grid")
                    press.update(x: 0, y: 90, snapping: false)
                    #expect(press.geometry?.length == grid, "a new drag retains the grid-cell minimum")
                }
                let initial = try #require(PianoInsertion(x: 0, y: 90, camera: camera, snapping: true))
                #expect(initial.geometry?.length == grid)
            }
        }
        #expect(PianoInsertion(x: 0, y: 90, camera: PianoCamera(), snapping: true, length: 0) == nil)
        #expect(PianoInsertion(x: 0, y: 90, camera: PianoCamera(), snapping: true, length: -1) == nil)
        #expect(PianoInsertion(x: 72, y: 90, camera: PianoCamera(), snapping: true, length: .max) == nil)
    }
}
