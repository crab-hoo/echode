import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct LoopDraftTests {
    @Test func contractLoopDraftKeepsIntegerBoundsThroughTransportAndDocumentChanges() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-loop-draft-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "loop")
        let document = try #require(controller.documentID)
        let selection = controller.selection, undo = controller.undoCount
        // Distinct integer endpoints collapse if the draft round-trips through Double beats.
        let range = try #require(LoopRange(start: 9_007_199_254_740_993, end: 9_007_199_254_740_995))
        await controller.setLoopRange(range, documentID: document)
        #expect(controller.rulerLoop?.range == range, "loop draft lost integer endpoints")
        #expect(controller.audio.loopStart == nil)
        let entry = LoopEntry(range: range)
        #expect(entry.range == range, "untouched beat text lost integer endpoints")
        await controller.setLoopRange(try #require(entry.range), documentID: document)
        var changed = entry
        changed.start = NoteDraft.format(1.25)
        #expect(changed.range == LoopRange(start: 882_000, end: range.end))
        await controller.toggleLoop()
        #expect(controller.audio.loopStart == range.start && controller.audio.loopEnd == range.end)
        let edited = try #require(changed.range)
        await controller.setLoopRange(edited, documentID: document)
        #expect(controller.audio.loopStart == edited.start && controller.audio.loopEnd == range.end)
        await controller.setLoopRange(range, documentID: document)
        await controller.toggleLoop()
        #expect(controller.rulerLoop?.range == range && controller.audio.loopStart == nil)
        #expect(controller.playbackStart == 0 && controller.audio.tick == 0)
        #expect(controller.selection == selection && controller.undoCount == undo)
        await controller.close()
        await controller.open(url)
        await controller.setLoopRange(range, documentID: document)
        #expect(controller.rulerLoop?.range == LoopRange(start: 0, end: 2_822_400))
        #expect(controller.audio.loopStart == nil)
        await controller.close()
    }

    @Test func beatTextPreservesUntouchedEndpointsAndRejectsInvalidDrafts() throws {
        let huge = try #require(LoopRange(start: 9_007_199_254_740_993, end: Int64.max))
        var entry = LoopEntry(range: huge)
        #expect(entry.range == huge)
        entry.start = NoteDraft.format(1.25)
        #expect(entry.range == LoopRange(start: 882_000, end: Int64.max))
        entry = LoopEntry(range: huge)
        entry.end = NoteDraft.format(1)
        #expect(entry.range == nil) // An edited endpoint must not silently clamp or swap.
        for invalid in ["", "-1", "nan", "inf", "1e100", "1 beat"] {
            entry = LoopEntry(range: .initial)
            entry.start = invalid
            #expect(entry.range == nil)
        }
        entry = LoopEntry(range: .initial)
        entry.end = "0"
        #expect(entry.range == nil)
        entry.end = NoteDraft.format(1.0 / 3)
        #expect(entry.range == LoopRange(start: 0, end: 235_200))
        let tiny = try #require(LoopRange(start: Int64.max - 1, end: Int64.max))
        #expect(LoopEntry(range: tiny).range == tiny)
    }
}
