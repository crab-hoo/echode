import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct ExportTests {
    @Test func snapshotIsIndependentOfLaterEditsAndTheOpenPackageLock() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode")
        let frozen = root.appendingPathComponent("frozen.echode")
        let controller = ProjectController()
        await controller.open(source, creating: true)
        await controller.addTrack(name: "Captured")
        let document = try #require(controller.document)
        do { _ = try await document.history(.exportSnapshot(path: frozen.path)) }
        catch { Issue.record("Export snapshot failed: \(error)"); await controller.close(); return }
        await controller.addTrack(name: "Later")
        let copy = try await Task.detached { try Document(opening: frozen.path) }.value
        let snapshot = try await copy.snapshot()
        #expect(snapshot.trackCount == 1 && snapshot.trackName(at: 0) == "Captured")
        #expect(controller.tracks.count == 2 && controller.undoCount == 2)
        await Task.detached { copy.shutdown() }.value
        await controller.close()
    }

    @Test func contractExportKeepsItsSnapshotWhileTheDocumentChanges() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-job-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let suite = "export-test-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let source = root.appendingPathComponent("Song.echode"), output = root.appendingPathComponent("音声 mix.wav")
        let wav = root.appendingPathComponent("input.wav")
        // This short fixture exercises copied PCM and a partial render block only.
        var input = wavMono(rate: 48_000, frames: 480)
        for index in stride(from: 44, to: input.count, by: 2) { input[index] = 0; input[index + 1] = 32 }
        try input.write(to: wav)
        let controller = ProjectController()
        await controller.open(source, creating: true)
        await controller.addTrack(name: "Captured")
        #expect(await controller.importAudio(wav))
        let document = try #require(controller.documentID)
        let cli = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("target/debug/echode")
        let marker = root.appendingPathComponent("started"), release = root.appendingPathComponent("release")
        defer { try? Data().write(to: release) }
        let wrapper = root.appendingPathComponent("render-fixture")
        let script = "#!/bin/sh\n: > '\(marker.path)'\ncount=0\nwhile [ ! -f '\(release.path)' ]; do\n count=$((count+1))\n [ $count -lt 3000 ] || exit 9\n /bin/sleep 0.01\ndone\nexec '\(cli.path)' \"$@\"\n"
        try Data(script.utf8).write(to: wrapper)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: wrapper.path)
        let job = ExportJob(); job.executable = wrapper
        var settings = ExportSettings(); settings.automaticTail = false; settings.tailSeconds = 0
        let undo = controller.undoCount
        let task = try #require(job.start(controller: controller, document: document, project: source, output: output, settings: settings, defaults: defaults))
        #expect(job.start(controller: controller, document: document, project: source, output: output, settings: settings) == nil)
        let deadline = ContinuousClock.now + .seconds(30)
        while !FileManager.default.fileExists(atPath: marker.path), job.running, ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(FileManager.default.fileExists(atPath: marker.path), "Export process did not start: \(job.detail)")
        #expect(!controller.busy && job.running && controller.undoCount == undo)
        #expect(ExportSettings.load(project: source, defaults: defaults) == settings)
        // Edits, transport and closing the source are independent of the child process.
        await controller.addTrack(name: "Later")
        await controller.locate(beat: 3)
        #expect(controller.undoCount == undo + 1 && controller.audio.tick == 3 * 705_600)
        await controller.close()
        try FileManager.default.removeItem(at: source)
        await controller.open(root.appendingPathComponent("Other.echode"), creating: true)
        try Data().write(to: release)
        await task.value
        #expect(!job.running && job.detail.contains("exported "), "Export did not finish: \(job.detail)")
        let rendered = try Data(contentsOf: output)
        // The initial GUI format is PCM24; nonzero PCM proves referenced media survived source removal.
        #expect(rendered[34] == 24 && rendered.count > 480 * 6)
        #expect(rendered.suffix(480 * 6).contains { $0 != 0 })
        #expect(controller.tracks.isEmpty && controller.undoCount == 0 && controller.errorMessage == nil)
        await controller.close()
    }

    @Test func settingsValidateAndStayWithEachProject() throws {
        let suite = "export-settings-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let first = URL(fileURLWithPath: "/tmp/first.echode"), second = URL(fileURLWithPath: "/tmp/second.echode")
        #expect(ExportSettings.load(project: first, defaults: defaults).format == .pcm24)
        var settings = ExportSettings()
        settings.format = .pcm16; settings.tpdf = true; settings.sampleRate = 44_100
        settings.automaticTail = false; settings.tailSeconds = 0.5
        #expect(settings.arguments?.suffix(2) == ["--tail-frames", "22050"])
        settings.save(project: first, defaults: defaults)
        #expect(ExportSettings.load(project: first, defaults: defaults) == settings)
        #expect(ExportSettings.load(project: second, defaults: defaults).sampleRate == 48_000)
        settings.format = .float32
        #expect(settings.arguments == nil)
        settings.tpdf = false
        for value in [Double.nan, .infinity, -1, Double(UInt32.max)] {
            settings.tailSeconds = value
            #expect(settings.arguments == nil)
        }
        settings.tailSeconds = 0; settings.sampleRate = 0
        #expect(settings.arguments == nil)
    }
}
