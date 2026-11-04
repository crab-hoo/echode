import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct ExportBoundaryTests {
    @Test func staleDocumentsAndPackageOutputsAreRejectedWithoutEdits() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-boundary-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode")
        let controller = ProjectController()
        await controller.open(source, creating: true)
        let old = try #require(controller.documentID)
        await controller.addTrack(name: "keep")
        let job = ExportJob()
        let suite = "export-boundary-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let state = WorkspaceState(defaults: defaults)
        let session = WorkspaceSession(controller: controller, state: state)
        #expect(session.enabled(.exportAudio))
        session.perform(.exportAudio)
        #expect(state.presentation == .exportAudio)
        state.presentation = nil
        let settings = ExportSettings()
        #expect(job.start(controller: controller, document: old, project: source, output: source.appendingPathComponent("project.json"), settings: settings) == nil)
        #expect(!job.running && controller.undoCount == 1)
        await controller.close(); await controller.open(source)
        let destination = root.appendingPathComponent("stale")
        do {
            try await controller.freezeExport(to: destination, documentID: old)
            Issue.record("A stale export captured a new document")
        } catch { #expect(!FileManager.default.fileExists(atPath: destination.path)) }
        #expect(job.start(controller: controller, document: old, project: source, output: root.appendingPathComponent("out.wav"), settings: settings) == nil)
        #expect(controller.tracks.first?.name == "keep" && controller.undoCount == 0)
        await controller.close()
        #expect(!session.enabled(.exportAudio))
    }

    @Test func failedCLIExportPreservesAnExistingOutputAndReportsTheError() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-error-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode"), output = root.appendingPathComponent("original.wav")
        let controller = ProjectController()
        await controller.open(source, creating: true)
        await controller.addTrack(name: "Unavailable plugin")
        let track = try #require(controller.selection), document = try #require(controller.documentID)
        #expect(await controller.edit(.addDevice(track: track.uuidString, plugin: "org.example.missing-export-plugin")))
        let sentinel = Data("keep original output".utf8)
        try sentinel.write(to: output)
        let cli = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("target/debug/echode")
        let job = ExportJob(); job.executable = cli
        let suite = "export-error-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let undo = controller.undoCount
        let task = try #require(job.start(controller: controller, document: document, project: source, output: output, settings: ExportSettings(), defaults: defaults))
        await task.value
        #expect(!job.running && job.detail.contains("missing plugin"), "CLI failure was not reported: \(job.detail)")
        #expect(!job.detail.contains("exported "))
        #expect(try Data(contentsOf: output) == sentinel)
        #expect(controller.undoCount == undo && controller.errorMessage == nil)
        let contents = try FileManager.default.contentsOfDirectory(atPath: root.path)
        #expect(!contents.contains { $0.hasSuffix(".part") })
        await controller.close()
    }
}
