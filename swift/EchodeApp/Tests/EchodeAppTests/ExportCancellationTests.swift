import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct ExportCancellationTests {
    private var cli: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("target/debug/echode")
    }

    @Test func contractCancellationSurvivesClosedProgressAndDocumentExchange() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-cancel-ui-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let source = root.appendingPathComponent("Song.echode"), output = root.appendingPathComponent("original.wav")
        let snapshotPath = root.appendingPathComponent("snapshot-path"), wrapper = root.appendingPathComponent("render-fixture")
        // Close stdout before waiting on cancellation. A missing control EOF times
        // out with exit 9, rather than hanging the test or impersonating cancellation.
        let script = """
        #!/bin/bash
        /usr/bin/printf '%s' "$2" > '\(snapshotPath.path)'
        /usr/bin/printf '%s\\n' '{"version":1,"phase":"rendering","rendered_frames":50,"total_frames":100}'
        exec 1>&-
        IFS= read -r -t 10 ignored
        [ $? -eq 1 ] || exit 9
        exec '\(cli.path)' "$@"
        """
        try Data(script.utf8).write(to: wrapper)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: wrapper.path)
        let original = Data("keep original output".utf8)
        try original.write(to: output)
        let controller = ProjectController()
        await controller.open(source, creating: true)
        let document = try #require(controller.documentID)
        let suite = "export-cancel-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        var settings = ExportSettings(); settings.automaticTail = false; settings.tailSeconds = 600
        let job = ExportJob(); job.executable = wrapper
        let task = try #require(job.start(controller: controller, document: document, project: source,
                                          output: output, settings: settings, defaults: defaults))
        let started = ContinuousClock.now, deadline = started + .seconds(10)
        while job.fractionCompleted == nil, job.running, ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        // The state tells an early failure (not running, with its detail) from progress that never arrived.
        #expect(job.running && job.fractionCompleted == 0.5, """
            Cancellable export did not deliver live progress after \(ContinuousClock.now - started): \
            running \(job.running), fraction \(String(describing: job.fractionCompleted)), \
            message \(job.message), detail \(job.detail)
            """)
        await controller.close()
        await controller.open(root.appendingPathComponent("Other.echode"), creating: true)
        job.cancel(); job.cancel()
        #expect(job.running && job.cancelRequested)
        #expect(job.message == text("Cancelling export"))
        await task.value
        #expect(!job.running && job.detail.contains("Exit: 130"), "Export cancellation was not acknowledged: \(job.detail)")
        #expect(job.message == text("Export cancelled") + ": " + output.lastPathComponent)
        #expect(job.fractionCompleted != 1 && !job.detail.contains("exported "))
        #expect(try Data(contentsOf: output) == original)
        let frozen = try String(contentsOf: snapshotPath, encoding: .utf8)
        #expect(!FileManager.default.fileExists(atPath: URL(fileURLWithPath: frozen).deletingLastPathComponent().path))
        #expect(try !FileManager.default.contentsOfDirectory(atPath: root.path).contains { $0.hasSuffix(".part") })
        #expect(controller.undoCount == 0 && controller.errorMessage == nil && controller.tracks.isEmpty)
        // Cancellation cannot poison the next job; a completed result cannot be cancelled retroactively.
        job.executable = cli; settings.tailSeconds = 0
        let nextDocument = try #require(controller.documentID), nextProject = try #require(controller.projectURL)
        let next = try #require(job.start(controller: controller, document: nextDocument,
                                          project: nextProject, output: output, settings: settings, defaults: defaults))
        #expect(!job.cancelRequested)
        await next.value
        #expect(job.fractionCompleted == 1 && job.detail.contains("exported "))
        let message = job.message
        job.cancel()
        #expect(job.message == message && !job.cancelRequested)
        await controller.close()
    }

    @Test func cancellationBeforePreparationDoesNotLaunchOrSaveSettings() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-cancel-prepare-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode"), output = root.appendingPathComponent("out.wav")
        let controller = ProjectController()
        await controller.open(source, creating: true)
        let suite = "export-cancel-prepare-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let job = ExportJob(); job.executable = nil
        var settings = ExportSettings(); settings.sampleRate = 44_100
        let document = try #require(controller.documentID)
        let task = try #require(job.start(controller: controller, document: document, project: source,
                                          output: output, settings: settings, defaults: defaults))
        job.cancel()
        await task.value
        #expect(!job.running && job.message == text("Export cancelled") + ": out.wav")
        #expect(!FileManager.default.fileExists(atPath: output.path))
        #expect(ExportSettings.load(project: source, defaults: defaults) == ExportSettings())
        #expect(controller.undoCount == 0 && controller.errorMessage == nil)
        await controller.close()
    }

    @Test func cancellationRequestsDoNotRelabelSuccessfulOrFailedExitStatuses() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-cancel-result-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wrapper = root.appendingPathComponent("render-fixture")
        for code in [0, 2] {
            try Data("#!/bin/sh\nexit \(code)\n".utf8).write(to: wrapper)
            try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: wrapper.path)
            let result = try await ExportJob.render(executable: wrapper, package: root, output: root.appendingPathComponent("out.wav"),
                                                     arguments: [], work: root, isCancelled: { true })
            #expect(result.success == (code == 0) && !result.cancelled)
        }
    }
}
