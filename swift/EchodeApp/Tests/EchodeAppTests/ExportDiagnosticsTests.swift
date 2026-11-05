import Foundation
import Testing
@testable import EchodeAppCore

struct ExportDiagnosticsTests {
    @Test(arguments: [0, 2])
    func contractVerboseRendererKeepsItsFinalDiagnosticWithinTheDisplayBudget(status: Int) async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-diagnostics-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wrapper = root.appendingPathComponent("verbose-renderer")
        // This size exceeds only the diagnostic display budget, not an audio or project limit.
        let script = """
        #!/bin/sh
        echo 'initial renderer diagnostic' >&2
        /usr/bin/printf '%0131072d' 0 >&2
        echo 'final renderer diagnostic' >&2
        exit \(status)
        """
        try Data(script.utf8).write(to: wrapper)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: wrapper.path)
        let result = try await ExportJob.render(executable: wrapper, package: root,
                                               output: root.appendingPathComponent("out.wav"), arguments: [], work: root)
        #expect(result.success == (status == 0) && !result.cancelled)
        #expect(result.detail.contains("Exit: \(status)"))
        #expect(result.detail.hasSuffix("final renderer diagnostic\n"), "Final renderer diagnostic was discarded")
        #expect(result.detail.contains("diagnostics omitted"))
        #expect(result.detail.utf8.count < 66_000, "The display must not read the entire diagnostic file")
    }

    @Test(arguments: [0, 1, 65_536, 65_537, 131_072])
    func diagnosticBudgetPreservesSmallLogsAndBothEndsOfLargeLogs(size: Int) throws {
        let log = FileManager.default.temporaryDirectory.appendingPathComponent("echode-diagnostic-budget-\(UUID())")
        defer { removeTestFile(log) }
        // Sizes cover only this diagnostic display boundary, not a product capacity limit.
        let input = Data((0..<size).map { UInt8(65 + $0 % 26) })
        try input.write(to: log)
        let actual = try ExportJob.readDiagnostics(from: log)
        if size <= 65_536 {
            #expect(actual == String(decoding: input, as: UTF8.self))
        } else {
            let expected = String(decoding: input.prefix(32_768), as: UTF8.self)
                + "\n[... diagnostics omitted ...]\n" + String(decoding: input.suffix(32_768), as: UTF8.self)
            #expect(actual == expected)
            #expect(actual.utf8.count < 66_000)
        }
    }

    @Test func smallUnicodeDiagnosticsRemainUnchanged() throws {
        let log = FileManager.default.temporaryDirectory.appendingPathComponent("echode-diagnostic-unicode-\(UUID())")
        defer { removeTestFile(log) }
        let input = "音声を書き出しました\n"
        try Data(input.utf8).write(to: log)
        #expect(try ExportJob.readDiagnostics(from: log) == input)
    }
}
