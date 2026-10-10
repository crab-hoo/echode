import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct ProjectTests {
    @Test func transportCommandsDoNotConsumeUndoAndClosingClearsTheFeed() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-transport-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
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
        await controller.stopPlayback()
        #expect(controller.audio.tick == 0)
        #expect(controller.undoCount == 1)
        await controller.undo()
        #expect(controller.tracks.isEmpty)
        await controller.close()
        #expect(controller.playback == nil && !feed.position.playing)
    }
    @Test func insertingAPluginKeepsTheMixerAndSurvivesReopen() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-plugin-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
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
        defer { try? FileManager.default.removeItem(at: url) }
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
        defer { try? FileManager.default.removeItem(at: url) }
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
        defer { try? FileManager.default.removeItem(at: url) }
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

    @Test func fuzzyCommandsAndNoteNamesHaveUsableResults() {
        #expect(fuzzyMatch("atr", in: "Add Track"))
        #expect(fuzzyMatch("", in: "Add Track"))
        #expect(!fuzzyMatch("txr", in: "Add Track"))
        #expect(fuzzyMatch("取す", in: "取り消す"))
        #expect(noteName(60) == "C4")
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
