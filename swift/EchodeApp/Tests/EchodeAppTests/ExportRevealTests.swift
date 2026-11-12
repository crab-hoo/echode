import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

@MainActor
struct ExportRevealTests {
    private var cli: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent(
                "target/debug/echode")
    }

    @Test func contractRevealUsesOnlyTheCompletedJobsPinnedDestination() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-reveal-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode")
        let outside = root.appendingPathComponent("exports")
        let alias = root.appendingPathComponent("chosen-directory")
        let controller = ProjectController()
        await controller.open(source, creating: true)
        try FileManager.default.createDirectory(at: outside, withIntermediateDirectories: false)
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: outside)
        let target = outside.appendingPathComponent("keep.wav")
        let output = outside.appendingPathComponent("音声 mix.wav")
        let original = Data("keep linked file".utf8)
        try original.write(to: target)
        try FileManager.default.createSymbolicLink(at: output, withDestinationURL: target)
        let destination = outside.resolvingSymlinksInPath().appendingPathComponent(output.lastPathComponent)
        let suite = "export-reveal-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        var settings = ExportSettings()
        settings.automaticTail = false
        settings.tailSeconds = 0
        let document = try #require(controller.documentID)
        let job = ExportJob()
        job.executable = cli
        var revealed: [URL] = []
        job.revealOutput { revealed.append($0) }
        #expect(job.completedOutput == nil && revealed.isEmpty)
        let task = try #require(
            job.start(
                controller: controller, document: document, project: source,
                output: alias.appendingPathComponent(output.lastPathComponent), settings: settings, defaults: defaults))
        job.revealOutput { revealed.append($0) }
        #expect(job.running && job.completedOutput == nil && revealed.isEmpty)
        // Retarget without yielding: both the renderer and reveal must retain the
        // original parent and leaf, not resolve the selected links a second time.
        try FileManager.default.removeItem(at: alias)
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: source)
        await task.value
        #expect(!job.running && job.detail.contains("exported "), "Export did not complete: \(job.detail)")
        #expect(job.completedOutput == destination, "Reveal must retain the successfully published destination")
        #expect(try Data(contentsOf: output).starts(with: Data("RIFF".utf8)))
        #expect(try Data(contentsOf: target) == original)
        await controller.close()
        await controller.open(root.appendingPathComponent("Other.echode"), creating: true)
        job.revealOutput { revealed.append($0) }
        #expect(
            revealed == [destination], "Reveal must use the completed job, not the new document or the selected alias")
        #expect(controller.tracks.isEmpty && controller.undoCount == 0 && controller.errorMessage == nil)
        await controller.close()
    }

    @Test(arguments: ["missing-cli", "cli-failure", "cancelled", "rejected"])
    func anUnsuccessfulAttemptCannotRevealThePreviousExport(outcome: String) async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-reveal-reset-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode")
        let output = root.appendingPathComponent("mix.wav")
        let controller = ProjectController()
        await controller.open(source, creating: true)
        let suite = "export-reveal-reset-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        var settings = ExportSettings()
        settings.automaticTail = false
        settings.tailSeconds = 0
        let document = try #require(controller.documentID)
        let job = ExportJob()
        job.executable = cli
        let first = try #require(
            job.start(
                controller: controller, document: document, project: source,
                output: output, settings: settings, defaults: defaults))
        await first.value
        #expect(job.completedOutput != nil, "Fixture export did not complete: \(job.detail)")
        let original = try Data(contentsOf: output)
        if outcome == "missing-cli" { job.executable = nil }
        if outcome == "cli-failure" {
            await controller.addTrack(name: "Unavailable plugin")
            let track = try #require(controller.selection)
            #expect(
                await controller.edit(
                    .addDevice(track: track.uuidString, plugin: "org.example.missing-export-reveal-plugin")))
        }
        let next = job.start(
            controller: controller, document: document, project: source,
            output: outcome == "rejected" ? source.appendingPathComponent("project.json") : output,
            settings: settings, defaults: defaults)
        var revealed: [URL] = []
        job.revealOutput { revealed.append($0) }
        #expect(job.completedOutput == nil && revealed.isEmpty, "A new attempt must clear the previous reveal action")
        if outcome == "rejected" {
            #expect(next == nil)
        } else {
            let task = try #require(next)
            if outcome == "cancelled" { job.cancel() }
            await task.value
        }
        job.revealOutput { revealed.append($0) }
        #expect(
            !job.running && job.completedOutput == nil && revealed.isEmpty,
            "Failure, cancellation, and rejection must never expose a previous output")
        #expect(try Data(contentsOf: output) == original)
        await controller.close()
    }
}
