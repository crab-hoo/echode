import Darwin
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct ExportProgressExitTests {
    private final class Updates { var phases: [ExportProgress.Phase] = [] }

    @Test func bufferedProgressIsDrainedAfterTheChildExitsDuringAnUpdate() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-progress-exit-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let pidFile = root.appendingPathComponent("pid"), release = root.appendingPathComponent("release")
        let wrapper = root.appendingPathComponent("render-fixture")
        defer { try? Data().write(to: release) }
        let script = """
        #!/bin/sh
        echo $$ > '\(pidFile.path)'
        /usr/bin/printf '%s\\n' '{"version":1,"phase":"rendering","rendered_frames":50,"total_frames":100}'
        count=0
        while [ ! -f '\(release.path)' ]; do
          count=$((count+1))
          [ $count -lt 1000 ] || exit 9
          /bin/sleep 0.01
        done
        /usr/bin/printf '%s\\n' '{"version":1,"phase":"complete","rendered_frames":100,"total_frames":100}'
        # Keep inherited stdout busy after the CLI exits; post-exit draining must be bounded.
        (while [ -d '\(root.path)' ]; do /usr/bin/printf '%04096d\\n' 0 || break; done) 2>/dev/null &
        exit 2
        """
        try Data(script.utf8).write(to: wrapper)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: wrapper.path)
        let updates = Updates()
        let result = try await ExportJob.render(executable: wrapper, package: root, output: root.appendingPathComponent("out.wav"),
                                                arguments: [], work: root, onProgress: { update in
            updates.phases.append(update.phase)
            guard update.phase == .rendering else { return }
            do {
                let pidText = try String(contentsOf: pidFile, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
                let pid = try #require(Int32(pidText))
                try Data().write(to: release)
                // Keep the first update suspended until the child has written its
                // final record and exited. No timing assumption about the renderer.
                let deadline = ContinuousClock.now + .seconds(10)
                while Darwin.kill(pid, 0) == 0, ContinuousClock.now < deadline {
                    try await Task.sleep(for: .milliseconds(10))
                }
                #expect(Darwin.kill(pid, 0) == -1 && errno == ESRCH, "Progress fixture did not exit")
            } catch { Issue.record(error) }
        })
        #expect(updates.phases == [.rendering, .complete], "Buffered final progress was discarded after child exit")
        #expect(!result.success && !result.cancelled && result.detail.contains("Exit: 2"))
    }
}
