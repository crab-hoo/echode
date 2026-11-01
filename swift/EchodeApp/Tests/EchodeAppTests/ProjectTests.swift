import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

extension ProjectController {
    /// Applies one fixture edit to the current document through the product's typed entry.
    func edit(_ action: EditAction) async -> Bool {
        guard let documentID else { return false }
        return await performActions({ [action] }, documentID: documentID)
    }
}

func wavMono(rate: UInt32, frames: Int) -> Data {
    var data = Data()
    data.append(contentsOf: [0x52, 0x49, 0x46, 0x46])
    let dataBytes = UInt32(frames * 2)
    data.append(contentsOf: withUnsafeBytes(of: (36 + dataBytes).littleEndian) { Array($0) })
    data.append(contentsOf: [0x57, 0x41, 0x56, 0x45, 0x66, 0x6d, 0x74, 0x20])
    data.append(contentsOf: withUnsafeBytes(of: UInt32(16).littleEndian) { Array($0) })
    data.append(contentsOf: withUnsafeBytes(of: UInt16(1).littleEndian) { Array($0) })
    data.append(contentsOf: withUnsafeBytes(of: UInt16(1).littleEndian) { Array($0) })
    data.append(contentsOf: withUnsafeBytes(of: rate.littleEndian) { Array($0) })
    data.append(contentsOf: withUnsafeBytes(of: (rate * 2).littleEndian) { Array($0) })
    data.append(contentsOf: withUnsafeBytes(of: UInt16(2).littleEndian) { Array($0) })
    data.append(contentsOf: withUnsafeBytes(of: UInt16(16).littleEndian) { Array($0) })
    data.append(contentsOf: [0x64, 0x61, 0x74, 0x61])
    data.append(contentsOf: withUnsafeBytes(of: dataBytes.littleEndian) { Array($0) })
    data.append(Data(count: Int(dataBytes)))
    return data
}

@MainActor
struct ProjectTests {
    @Test func transportCommandsDoNotConsumeUndoAndClosingClearsTheFeed() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-transport-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "pad")
        let feed = try #require(controller.playback)
        await controller.locate(beat: 3)
        #expect(controller.audio.tick == 705_600 * 3)
        #expect(feed.position.tick == 705_600 * 3)
        #expect(controller.undoCount == 1)
        await controller.locate(beat: .infinity)
        #expect(controller.audio.tick == 705_600 * 3)
        // Stop returns to the remembered start, which a locate does not move (ADR 0155).
        await controller.stopPlayback()
        #expect(controller.audio.tick == 0 && controller.playbackStart == 0)
        await controller.toggleMetronome()
        #expect(controller.audio.metronome)
        await controller.toggleLoop()
        #expect(controller.audio.loopStart == 0 && controller.audio.loopEnd == 705_600 * 4)
        #expect(LoopRange(start: 0, end: 0) == nil)
        await controller.setLoop(enabled: true)
        #expect(controller.audio.loopStart == 0 && controller.audio.loopEnd == 705_600 * 4)
        await controller.toggleLoop()
        #expect(controller.audio.loopStart == nil && controller.audio.loopEnd == nil)
        await controller.toggleMetronome()
        #expect(!controller.audio.metronome)
        #expect(controller.undoCount == 1)
        await controller.undo()
        #expect(controller.tracks.isEmpty)
        await controller.close()
        #expect(controller.playback == nil && !feed.position.playing)
    }

    @Test func importingAudioPlacesAClipAndUndoRemovesIt() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-import-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("tone.wav")
        try wavMono(rate: 48_000, frames: 4_800).write(to: wav)
        let url = root.appendingPathComponent("Song.echode")
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "audio")
        let rejected = await controller.importAudio(root.appendingPathComponent("notes.txt"))
        #expect(!rejected && controller.undoCount == 1)
        let imported = await controller.importAudio(wav)
        #expect(imported && controller.undoCount == 2)
        let objects = try FileManager.default.contentsOfDirectory(
            at: url.appendingPathComponent("media/objects"), includingPropertiesForKeys: nil)
        #expect(objects.count == 1)
        let index = try String(contentsOf: url.appendingPathComponent("media/index.json"), encoding: .utf8)
        #expect(index.contains("tone.wav"))
        let snapshot = try #require(controller.snapshot)
        let visible = snapshot.visibleElements(
            originTick: 0, widthTicks: 20_000_000, originTrack: 0, heightTracks: 4,
            pixelsPerTick: 0.001, pixelsPerTrack: 40)
        #expect(visible.elements.contains { $0.role == .clip })
        await controller.undo()
        #expect(controller.undoCount == 1)
        await controller.close()
    }
    @Test func insertingAPluginKeepsTheMixerAndSurvivesReopen() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-plugin-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "pad")
        await controller.insertPlugin("dev.echode.test-gain")
        let inserted = try #require(controller.snapshot)
        #expect(inserted.deviceCount(onTrack: 0) == 2)
        #expect(inserted.devicePlugin(onTrack: 0, at: 0) == "dev.echode.test-gain")
        #expect(inserted.devicePlugin(onTrack: 0, at: 1) == nil)
        let device = try #require(inserted.deviceID(onTrack: 0, at: 0))
        await controller.close()
        await controller.open(url)
        #expect(controller.snapshot?.devicePlugin(onTrack: 0, at: 0) == "dev.echode.test-gain")
        await controller.removeDevice(device)
        #expect(controller.snapshot?.deviceCount(onTrack: 0) == 1)
        await controller.undo()
        #expect(controller.snapshot?.devicePlugin(onTrack: 0, at: 0) == "dev.echode.test-gain")
        await controller.close()
    }

    @Test func editsUndoRedoAndReopenUseTheSameDurableProject() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-app-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        #expect(controller.errorMessage == nil)
        #expect(controller.snapshot?.trackCount == 0)
        await controller.addTrack(name: "鍵盤 🎹")
        let track = try #require(controller.selection)
        #expect(controller.tracks.first?.name == "鍵盤 🎹")
        await controller.insertNote(pitch: 64, beat: 2, length: 5)
        let edited = try #require(controller.snapshot)
        #expect(notes(edited) == 1)
        await controller.undo()
        #expect(notes(try #require(controller.snapshot)) == 0)
        await controller.redo()
        #expect(notes(try #require(controller.snapshot)) == 1)
        await controller.close()
        // Immutable views remain readable after the document and its threads stop.
        #expect(notes(edited) == 1)
        await controller.open(url)
        #expect(controller.errorMessage == nil)
        #expect(controller.selection == track)
        #expect(controller.tracks.first?.name == "鍵盤 🎹")
        #expect(notes(try #require(controller.snapshot)) == 1)
        #expect(controller.undoCount == 0)
        await controller.close()
    }

    @Test func failedOpenPreservesTheCurrentProjectAndCreationNeverOverwrites() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-app-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keep")
        let id = controller.selection
        await controller.open(url, creating: true)
        #expect(controller.errorMessage != nil)
        #expect(controller.selection == id)
        #expect(controller.tracks.first?.name == "keep")
        await controller.close()
        controller.errorMessage = nil
        await controller.open(url)
        #expect(controller.tracks.first?.name == "keep")
        await controller.close()
    }

    @Test func aFreshEditAfterUndoClearsRedoAndInvalidNotesDoNotEdit() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-app-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "first")
        await controller.addTrack(name: "second")
        await controller.undo()
        #expect(controller.redoCount == 1)
        await controller.insertNote(pitch: 60, beat: .infinity, length: 1)
        #expect(controller.redoCount == 1)
        await controller.addTrack(name: "replacement")
        #expect(controller.redoCount == 0)
        #expect(controller.tracks.map(\.name) == ["first", "replacement"])
        await controller.close()
    }

    @Test func restoringAnOlderAutosaveBecomesANewUndoableVersion() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-history-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "one")
        #expect(await controller.savedRevisions().count == 2)
        await controller.addTrack(name: "two")
        let revisions = await controller.savedRevisions()
        #expect(revisions.count == 3)
        // Newest first. The last commit is the empty base written at open.
        let afterFirst = try #require(revisions.dropLast().last)
        let restored = try #require(await controller.restoreRevision(afterFirst.id))
        #expect(restored.changed)
        #expect(controller.tracks.map(\.name) == ["one"])
        #expect(controller.undoCount == 3 && controller.redoCount == 0)
        let current = try #require(restored.revisions.first)
        let same = try #require(await controller.restoreRevision(current.id))
        #expect(!same.changed && controller.undoCount == 3)
        await controller.undo()
        #expect(controller.tracks.map(\.name) == ["one", "two"])
        await controller.close()
    }

    @Test func fuzzyCommandsAndNoteNamesHaveUsableResults() {
        #expect(fuzzyMatch("atr", in: "Add Track"))
        #expect(fuzzyMatch("", in: "Add Track"))
        #expect(!fuzzyMatch("txr", in: "Add Track"))
        #expect(fuzzyMatch("取す", in: "取り消す"))
        #expect(noteName(60) == "C4")
        #expect(noteName(0) == "C-1")
        #expect(noteName(-1) == "-1")
        #expect(noteName(128) == "128")
    }

    @Test func buffersStayReservedUntilTheirGpuFrameCompletes() {
        let slots = FrameSlots()
        let a = slots.acquire(), b = slots.acquire(), c = slots.acquire()
        #expect(a != nil && b != nil && c != nil)
        #expect(a != b && b != c && a != c)
        #expect(slots.acquire() == nil)
        slots.release(b!)
        #expect(slots.acquire() == b)
        #expect(slots.acquire() == nil)
    }

    private func notes(_ snapshot: Snapshot) -> Int {
        snapshot.visibleElements(originTick: 0, widthTicks: 705_600 * 16,
                                 originTrack: 0, heightTracks: 10,
                                 pixelsPerTick: 1.0 / 705_600, pixelsPerTrack: 64).elements.filter { $0.role == .note }.count
    }
}
