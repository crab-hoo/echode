import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@Suite @MainActor
struct TypedClipEditTests {
    @Test func contractClipEditsPreserveIntegerTargetsHistoryAndPersistence() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-typed-clips-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("音声 \"clip\".wav"), url = root.appendingPathComponent("Project.echode")
        // This one-second fixture supplies clip windows, not a product buffer limit.
        try wavMono(rate: 48_000, frames: 48_000).write(to: wav)
        let controller = ProjectController(sendAudio: { _, _ in AudioState() })
        await controller.open(url, creating: true)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "second")
        let second = try #require(controller.selection), document = try #require(controller.documentID)
        let tick: Int64 = 9_007_199_254_740_993, beat: Int64 = 705_600
        let request = AudioImportRequest(url: wav, document: document, track: first, tick: tick)
        let before = controller.undoCount
        #expect(await controller.importAudio(request))
        let source = try #require(clip(in: controller.snapshot, track: first, at: tick))
        let original = try #require(controller.snapshot?.clipMix(source))
        #expect(original.start == tick && original.length == 2 * beat)
        #expect(original.fadeIn == 0 && original.fadeOut == 0)
        #expect(controller.selection == second && controller.undoCount == before + 1)
        await controller.undo()
        #expect(controller.snapshot?.clipMix(source) == nil)
        await controller.redo()
        #expect(controller.snapshot?.clipMix(source) == original)

        let move = ClipMove(document: document, clip: source, track: second, start: tick + 2, copy: false)
        #expect(await controller.moveClip(move))
        let moved = try #require(controller.snapshot?.clipMix(source))
        #expect(moved.track == second && moved.start == tick + 2 && moved.length == original.length)
        #expect(controller.selection == second && controller.clipFocus == source && controller.undoCount == before + 2)
        await controller.undo()
        #expect(controller.snapshot?.clipMix(source) == original)
        await controller.redo()
        #expect(controller.snapshot?.clipMix(source) == moved)

        let duplicate = ClipMove(document: document, clip: source, track: first, start: tick + 4 * beat + 6, copy: true)
        #expect(await controller.moveClip(duplicate))
        let copy = try #require(controller.clipFocus)
        #expect(copy != source && controller.selection == first && controller.undoCount == before + 3)
        let copied = try #require(controller.snapshot?.clipMix(copy))
        #expect(copied.track == first && copied.start == duplicate.start && copied.length == original.length)
        #expect(controller.snapshot?.clipMix(source) == moved)
        await controller.undo()
        #expect(controller.snapshot?.clipMix(copy) == nil && controller.snapshot?.clipMix(source) == moved)
        await controller.redo()
        #expect(controller.snapshot?.clipMix(copy) == copied)

        // A drop onto an existing track retains the captured tick and target, not the selection.
        let drop = AudioFileDrop(document: document, url: wav, track: second, tick: tick + 8 * beat + 10)
        #expect(await controller.importDroppedAudio(drop))
        let dropped = try #require(clip(in: controller.snapshot, track: second, at: drop.tick))
        let droppedMix = try #require(controller.snapshot?.clipMix(dropped))
        #expect(droppedMix.start == drop.tick && controller.selection == first && controller.undoCount == before + 4)
        await controller.undo()
        let undo = controller.undoCount, redo = controller.redoCount
        #expect(redo == 1)
        let selection = controller.selection, focus = controller.clipFocus
        #expect(!(await controller.moveClip(ClipMove(document: document, clip: source, track: second, start: moved.start, copy: true))))
        #expect(!(await controller.moveClip(ClipMove(document: document, clip: source, track: UUID(), start: 0, copy: false))))
        #expect(!(await controller.moveClip(ClipMove(document: document, clip: UUID(), track: first, start: 0, copy: false))))
        #expect(!(await controller.moveClip(ClipMove(document: document, clip: source, track: first, start: -1, copy: false))))
        #expect(!(await controller.moveClip(ClipMove(document: document, clip: source, track: first, start: Int64.max, copy: false))))
        #expect(!(await controller.importAudio(AudioImportRequest(url: wav, document: document, track: second, tick: moved.start))))
        #expect(!(await controller.importAudio(AudioImportRequest(url: wav, document: document, track: first, tick: -1))))
        #expect(!(await controller.importAudio(AudioImportRequest(url: wav, document: document, track: UUID(), tick: 0))))
        let missing = root.appendingPathComponent("missing.wav")
        #expect(!(await controller.importAudio(AudioImportRequest(url: missing, document: document, track: first, tick: 0))))
        #expect(controller.undoCount == undo && controller.redoCount == redo)
        #expect(controller.selection == selection && controller.clipFocus == focus)
        #expect(controller.snapshot?.clipMix(source) == moved && controller.snapshot?.clipMix(copy) == copied)
        #expect(controller.snapshot?.clipMix(dropped) == nil)
        controller.errorMessage = nil
        try FileManager.default.removeItem(at: wav)
        await controller.redo() // Redo uses stored PCM; it must not decode the original file again.
        #expect(controller.errorMessage == nil && controller.snapshot?.clipMix(dropped) == droppedMix)

        // A drop below the rows places its clip on the track committed by the same transaction.
        let added = root.appendingPathComponent("新しい \"track\".wav")
        try wavMono(rate: 48_000, frames: 48_000).write(to: added)
        let existing = controller.tracks.map(\.id), undoBeforeAppend = controller.undoCount
        let append = AudioFileDrop(document: document, url: added, track: nil, tick: tick + 12 * beat + 14)
        #expect(await controller.importDroppedAudio(append))
        let appended = try #require(controller.tracks.last)
        #expect(!existing.contains(appended.id) && appended.name == "新しい \"track\"")
        #expect(controller.selection == appended.id && controller.undoCount == undoBeforeAppend + 1)
        let appendedClip = try #require(clip(in: controller.snapshot, track: appended.id, at: append.tick))
        let appendedMix = try #require(controller.snapshot?.clipMix(appendedClip))
        #expect(appendedMix.track == appended.id && appendedMix.start == append.tick)
        await controller.undo()
        #expect(controller.tracks.map(\.id) == existing && controller.snapshot?.clipMix(appendedClip) == nil)
        await controller.redo()
        #expect(controller.tracks.last?.id == appended.id && controller.snapshot?.clipMix(appendedClip) == appendedMix)
        // The failed import rolls back the track added in front of it.
        let lost = AudioFileDrop(document: document, url: root.appendingPathComponent("missing.wav"), track: nil, tick: 0)
        #expect(!(await controller.importDroppedAudio(lost)))
        #expect(controller.tracks.last?.id == appended.id && controller.undoCount == undoBeforeAppend + 1 && controller.redoCount == 0)
        controller.errorMessage = nil

        await controller.close(); await controller.open(url)
        #expect(controller.errorMessage == nil && controller.snapshot?.clipMix(source) == moved)
        #expect(controller.snapshot?.clipMix(copy) == copied && controller.snapshot?.clipMix(dropped) == droppedMix)
        #expect(!(await controller.moveClip(move)))
        #expect(!(await controller.moveClip(duplicate)))
        #expect(!(await controller.importAudio(request)))
        #expect(!(await controller.importDroppedAudio(drop)))
        #expect(!(await controller.importDroppedAudio(append)))
        #expect(controller.undoCount == 0 && controller.redoCount == 0)
        #expect(controller.tracks.last?.id == appended.id && controller.snapshot?.clipMix(appendedClip) == appendedMix)
        #expect(controller.snapshot?.clipMix(source) == moved && controller.snapshot?.clipMix(copy) == copied)
        #expect(controller.snapshot?.clipMix(dropped) == droppedMix)
        await controller.close()
    }

    private func clip(in snapshot: Snapshot?, track: UUID, at tick: Int64) -> UUID? {
        guard let snapshot, let row = (0..<snapshot.trackCount).first(where: { snapshot.trackID(at: $0) == track }) else { return nil }
        // Only query these fixture windows; no product track or timeline limit is imposed.
        return snapshot.visibleElements(originTick: tick, widthTicks: 2 * 705_600, originTrack: Double(row), heightTracks: 1,
            pixelsPerTick: 1, pixelsPerTrack: 64).elements.first { $0.role == .clip }?.id.placement
    }
}
