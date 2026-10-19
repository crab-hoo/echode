import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct SampleBrowserTests {
    @Test func contractRegisteredSampleSearchCoversAllRootsAndSkipsPackagesAndLinks() async throws {
        let root = FileManager.default.temporaryDirectory.resolvingSymlinksInPath().appendingPathComponent("echode-library-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        let first = root.appendingPathComponent("first"), second = root.appendingPathComponent("second")
        let nested = first.appendingPathComponent("nested"), package = first.appendingPathComponent("Song.echode")
        for folder in [nested, package, second] { try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true) }
        for path in [nested.appendingPathComponent("Kick 2.WAV"), second.appendingPathComponent("kick 10.flac"),
                     first.appendingPathComponent("notes.txt"), first.appendingPathComponent(".hidden-kick.wav"), package.appendingPathComponent("kick.wav")] {
            try Data().write(to: path)
        }
        try FileManager.default.createSymbolicLink(at: nested.appendingPathComponent("cycle"), withDestinationURL: first)
        try FileManager.default.createSymbolicLink(at: first.appendingPathComponent("outside"), withDestinationURL: second)
        let name = "echode-library-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        let browser = FileBrowserState(defaults: defaults)
        await browser.register(first)
        browser.query = "kick"
        let kick = try #require(browser.roots.first).appendingPathComponent("nested/Kick 2.WAV")
        #expect(browser.filtered.map(\.url) == [kick])
        await browser.register(second)
        await browser.register(nested) // Overlapping roots cannot duplicate results.
        await browser.register(first)
        #expect(browser.roots.count == 3)
        browser.query = "KICK"
        let other = try #require(browser.roots.first { $0.lastPathComponent == "second" })
        let expected = [kick, other.appendingPathComponent("kick 10.flac")]
        #expect(browser.filtered.map(\.url) == expected)
        #expect(browser.libraryError == nil && !browser.indexing)
        let restored = FileBrowserState(defaults: defaults)
        await restored.prepare()
        restored.query = "kick"
        #expect(restored.filtered.map(\.url) == expected)
        await restored.load(second)
        #expect(restored.query.isEmpty && restored.filtered.count == 1)
        restored.query = "kick"
        await restored.refresh()
        #expect(restored.query == "kick" && restored.filtered.map(\.url) == expected)
        await restored.removeRoot(second)
        #expect(restored.filtered.map(\.url) == [expected[0]])
        try FileManager.default.removeItem(at: nested)
        await restored.refreshLibrary()
        #expect(restored.libraryError != nil && restored.libraryFiles.isEmpty && !restored.indexing)
        await restored.register(root.appendingPathComponent("missing"))
        #expect(restored.roots.count == 2 && restored.error != nil)
    }

    @Test func contractSampleInsertionCapturesItsTargetAndSurvivesUndoAndReopen() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-sample-target-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("tone.wav"), project = root.appendingPathComponent("Song.echode")
        try wavMono(rate: 48_000, frames: 4_800).write(to: wav)
        let controller = ProjectController()
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        await controller.open(project, creating: true)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "second")
        let second = try #require(controller.selection)
        controller.selection = first
        await controller.locate(beat: 1.2345)
        let tick = try #require(controller.playback?.position.tick)
        let undo = controller.undoCount
        let pending = try #require(session.importSample(BrowserFile(url: wav, directory: false)))
        controller.selection = second // Change before the queued import can begin.
        #expect(await pending.value)
        #expect(controller.selection == second && controller.undoCount == undo + 1)
        let clip = try #require(controller.snapshot?.visibleElements(originTick: 0, widthTicks: 20_000_000,
            originTrack: 0, heightTracks: 4, pixelsPerTick: 0.001, pixelsPerTrack: 40).elements.first { $0.role == .clip })
        let placement = try #require(clip.id.placement)
        #expect(clip.id.track == first && controller.snapshot?.clipMix(placement)?.start == tick)
        await controller.undo()
        #expect(controller.snapshot?.clipMix(placement) == nil)
        await controller.redo()
        #expect(controller.snapshot?.clipMix(placement)?.start == tick)
        let stale = try #require(controller.captureAudioImport(wav, on: second))
        await controller.close(); await controller.open(project)
        #expect(controller.snapshot?.clipMix(placement)?.track == first)
        #expect(controller.snapshot?.clipMix(placement)?.start == tick)
        #expect(!(await controller.importAudio(stale)))
        // Reopening has no undo history; a newly added target can be removed by undo.
        await controller.addTrack(name: "temporary")
        let absent = try #require(controller.captureAudioImport(wav))
        await controller.undo()
        #expect(!(await controller.importAudio(absent)))
        #expect(controller.captureAudioImport(wav, on: UUID()) == nil)
        #expect(session.importSample(BrowserFile(url: root, directory: true)) == nil)
        #expect(controller.undoCount == 0)
        await controller.close()
    }

    @Test func supersededLibraryScanCannotRestoreARemovedRoot() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-sample-race-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try Data().write(to: root.appendingPathComponent("sample.wav"))
        let browser = FileBrowserState()
        await browser.register(root)
        var started = false
        let older = Task { started = true; await browser.refreshLibrary() }
        while !started { await Task.yield() }
        await browser.removeRoot(try #require(browser.roots.first))
        await older.value
        #expect(browser.roots.isEmpty && browser.libraryFiles.isEmpty && !browser.indexing)
    }

    @Test func importUsesTheLivePlayheadInsteadOfTheLastStatusPoll() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-sample-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("tone.wav")
        try wavMono(rate: 48_000, frames: 4_800).write(to: wav)
        // Keep the UI's last status stale while the product feed advances.
        // Locate and import do not open an audio device.
        let controller = ProjectController(sendAudio: { document, command in
            _ = try await document.audio(command)
            return AudioState()
        })
        await controller.open(root.appendingPathComponent("Song.echode"), creating: true)
        await controller.addTrack(name: "audio")
        await controller.locate(beat: 1.2345)
        let tick = try #require(controller.playback?.position.tick)
        #expect(tick > 0 && controller.audio.tick == 0)
        #expect(await controller.importAudio(wav))
        let snapshot = try #require(controller.snapshot)
        let elements = snapshot.visibleElements(originTick: 0, widthTicks: 20_000_000,
            originTrack: 0, heightTracks: 4, pixelsPerTick: 0.001, pixelsPerTrack: 40)
        let clip = try #require(elements.elements.first { $0.role == .clip })
        #expect(snapshot.clipMix(try #require(clip.id.placement))?.start == tick)
        await controller.close()
    }
}
