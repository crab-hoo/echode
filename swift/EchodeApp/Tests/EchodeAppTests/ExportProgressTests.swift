import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

struct ExportProgressTests {
    private func record(_ phase: String = "rendering", _ frames: String = "50", _ total: Int = 100, version: Int = 1) -> Data {
        Data("{\"version\":\(version),\"phase\":\"\(phase)\",\"rendered_frames\":\(frames),\"total_frames\":\(total)}\n".utf8)
    }

    @Test func decoderBoundsFramingAndRejectsRegressionsUnknownVersionsAndInvalidCounts() {
        var decoder = ExportProgressDecoder()
        let valid = record()
        #expect(decoder.append(valid.prefix(17)).isEmpty)
        #expect(decoder.append(valid.dropFirst(17)).first?.fraction == 0.5)
        for invalid in [record(), record("unknown"), record(version: 2), record("rendering", "false"),
                        record("rendering", "-1"), record("rendering", "101"), record("rendering", "4294967296"),
                        record("rendering", "49"), record("rendering", "70", 200), record("complete", "50")] {
            #expect(decoder.append(invalid).isEmpty)
        }
        #expect(decoder.append(Data(repeating: 120, count: 20_000)).isEmpty)
        #expect(decoder.append(Data("\nnot json\n".utf8)).isEmpty)
        #expect(decoder.append(record("finalizing", "100")).first?.fraction == 0.99)
        #expect(decoder.append(record("rendering", "100")).isEmpty)
        #expect(decoder.append(record("complete", "100")).first?.fraction == 0.99)
        #expect(decoder.append(record("finalizing", "100")).isEmpty)
        var empty = ExportProgressDecoder()
        #expect(empty.append(record("rendering", "0", 0)).first?.fraction == 0)
        #expect(empty.append(record("complete", "0", 0)).first?.fraction == 0.99)
        var partial = ExportProgressDecoder()
        #expect(partial.append(record().dropLast()).isEmpty)
    }

    @MainActor @Test func contractExportProgressArrivesBeforeExitAndFailureCannotReachOneHundred() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-progress-ui-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let source = root.appendingPathComponent("Song.echode"), output = root.appendingPathComponent("out.wav")
        let release = root.appendingPathComponent("release"), wrapper = root.appendingPathComponent("render-fixture")
        let finishDescendant = root.appendingPathComponent("finish-descendant"), descendantStopped = root.appendingPathComponent("descendant-stopped")
        defer { try? Data().write(to: finishDescendant) }
        defer { try? Data().write(to: release) }
        // The child waits for a UI acknowledgement, proving progress is drained before exit.
        // stderr exceeds a pipe buffer; it must remain independent of progress framing.
        let script = """
        #!/bin/sh
        (
          count=0
          while [ -d '\(root.path)' ] && [ ! -f '\(finishDescendant.path)' ]; do
            count=$((count+1))
            [ $count -lt 1000 ] || break
            /bin/sleep 0.01
          done
          : > '\(descendantStopped.path)'
        ) &
        /usr/bin/printf '%080000d' 0 >&2
        /usr/bin/printf '%s\\n' '{"version":1,"phase":"rendering","rendered_frames":50,"total_frames":100}'
        count=0
        while [ ! -f '\(release.path)' ]; do
          count=$((count+1))
          [ $count -lt 1000 ] || exit 9
          /bin/sleep 0.01
        done
        /usr/bin/printf '%s\\n' '{"version":1,"phase":"complete","rendered_frames":100,"total_frames":100}'
        exit 2
        """
        try Data(script.utf8).write(to: wrapper)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: wrapper.path)
        let controller = ProjectController()
        await controller.open(source, creating: true)
        let document = try #require(controller.documentID)
        let job = ExportJob(); job.executable = wrapper
        let suite = "export-progress-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let task = try #require(job.start(controller: controller, document: document, project: source,
                                          output: output, settings: ExportSettings(), defaults: defaults))
        #expect(job.fractionCompleted == nil)
        let deadline = ContinuousClock.now + .seconds(10)
        while job.fractionCompleted == nil, job.running, ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(job.running && job.fractionCompleted == 0.5, "Live export progress was not delivered before process exit; running=\(job.running), fraction=\(String(describing: job.fractionCompleted)), message=\(job.message ?? "nil"), detail=\(job.detail)")
        try Data().write(to: release)
        await task.value
        #expect(!job.running && job.fractionCompleted == 0.99)
        #expect(job.detail.contains("Exit: 2") && job.detail.utf8.count < 66_000)
        #expect(!FileManager.default.fileExists(atPath: descendantStopped.path), "Inherited stdout held the export after the CLI exited")
        #expect(controller.errorMessage == nil && controller.undoCount == 0)
        await controller.close()
    }
}
