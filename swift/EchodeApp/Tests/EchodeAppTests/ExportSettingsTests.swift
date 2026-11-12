import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

@MainActor
struct ExportSettingsTests {
    @Test func contractExportSettingsTravelWithThePackage() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-settings-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode")
        let moved = root.appendingPathComponent("Moved.echode")
        let suite = "export-portable-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let controller = ProjectController()
        await controller.open(source, creating: true)
        let document = try #require(controller.documentID)
        let cli = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent(
                "target/debug/echode")
        let job = ExportJob()
        job.executable = cli
        var settings = ExportSettings()
        settings.format = .pcm16
        settings.sampleRate = 44_100
        settings.tpdf = true
        settings.automaticTail = false
        settings.tailSeconds = 0.01
        let task = try #require(
            job.start(
                controller: controller, document: document, project: source,
                output: root.appendingPathComponent("out.wav"), settings: settings, defaults: defaults))
        await task.value
        #expect(job.detail.contains("exported "), "Export failed: \(job.detail)")
        #expect(controller.undoCount == 0 && controller.errorMessage == nil)
        await controller.close()
        try #require(
            FileManager.default.fileExists(atPath: source.appendingPathComponent("export.json").path),
            "Accepted export settings were not stored in the package")
        try FileManager.default.moveItem(at: source, to: moved)
        defaults.removePersistentDomain(forName: suite)
        await controller.open(moved)
        let reopened = try #require(controller.documentID)
        let loaded = await controller.loadExportSettings(documentID: reopened, defaults: defaults)
        #expect(loaded == settings, "Export settings did not travel with the package")
        #expect(controller.undoCount == 0 && controller.errorMessage == nil)
        #expect(await controller.loadExportSettings(documentID: document, defaults: defaults) == nil)
        let frozen = root.appendingPathComponent("frozen.echode")
        try await controller.freezeExport(to: frozen, documentID: reopened)
        #expect(!FileManager.default.fileExists(atPath: frozen.appendingPathComponent("export.json").path))
        await controller.close()
        await controller.open(root.appendingPathComponent("Other.echode"), creating: true)
        let other = try #require(controller.documentID)
        #expect(await controller.loadExportSettings(documentID: other, defaults: defaults) != settings)
        await controller.close()
    }

    @Test func legacySettingsMigrateAndBadOrNewerRecordsFallBackWithoutRepair() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-migrate-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode")
        let entry = source.appendingPathComponent("export.json")
        let suite = "export-migrate-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        var settings = ExportSettings()
        settings.sampleRate = 96_000
        settings.save(project: source, defaults: defaults)
        let controller = ProjectController()
        await controller.open(source, creating: true)
        let document = try #require(controller.documentID)
        #expect(await controller.loadExportSettings(documentID: document, defaults: defaults) == settings)
        #expect(!FileManager.default.fileExists(atPath: entry.path))
        let warning = try await controller.freezeExport(
            to: root.appendingPathComponent("frozen.echode"), documentID: document, settings: settings)
        #expect(warning == nil)
        let stored = try #require(settings.storedJSON)
        #expect(try String(contentsOf: entry, encoding: .utf8) == stored)
        for bad in [
            "not JSON", "[]", stored.replacingOccurrences(of: "\"version\":1", with: "\"version\":2"),
            stored.replacingOccurrences(of: "96000", with: "0"),
        ] {
            try Data(bad.utf8).write(to: entry)
            #expect(await controller.loadExportSettings(documentID: document, defaults: defaults) == settings)
            #expect(try String(contentsOf: entry, encoding: .utf8) == bad)
        }
        // Package preferences take precedence over this Mac's older path-based value.
        var portable = settings
        portable.sampleRate = 44_100
        let handle = try #require(controller.document)
        let result = try await handle.history(.exportSettings(json: portable.storedJSON))
        #expect(try !result.historyReport().changed)
        #expect(await controller.loadExportSettings(documentID: document, defaults: defaults) == portable)
        #expect(controller.undoCount == 0 && controller.errorMessage == nil)
        await controller.close()
    }

    @Test func preferenceWriteFailureDoesNotPreventSnapshotAndDoesNotFollowLinks() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-export-preference-error-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode")
        let outside = root.appendingPathComponent("outside.json")
        let controller = ProjectController()
        await controller.open(source, creating: true)
        let document = try #require(controller.documentID)
        let original = Data("keep external file".utf8)
        try original.write(to: outside)
        try FileManager.default.createSymbolicLink(
            at: source.appendingPathComponent("export.json"), withDestinationURL: outside)
        let frozen = root.appendingPathComponent("frozen.echode")
        let warning = try await controller.freezeExport(to: frozen, documentID: document, settings: ExportSettings())
        #expect(warning?.contains("Export settings were not saved in the package") == true)
        #expect(FileManager.default.fileExists(atPath: frozen.appendingPathComponent("project.json").path))
        #expect(try Data(contentsOf: outside) == original)
        let cli = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent(
                "target/debug/echode")
        let suite = "export-warning-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let job = ExportJob()
        job.executable = cli
        var settings = ExportSettings()
        settings.automaticTail = false
        settings.tailSeconds = 0
        let task = try #require(
            job.start(
                controller: controller, document: document, project: source,
                output: root.appendingPathComponent("out.wav"), settings: settings, defaults: defaults))
        await task.value
        #expect(
            job.detail.contains("exported ") && job.detail.contains("Export settings were not saved in the package"))
        #expect(job.fractionCompleted == 1)
        #expect(try Data(contentsOf: outside) == original)
        #expect(controller.undoCount == 0 && controller.errorMessage == nil)
        await controller.close()
    }

    @Test func cancellingBeforePreparationDoesNotCreatePortableSettings() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-no-settings-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode")
        let controller = ProjectController()
        await controller.open(source, creating: true)
        let document = try #require(controller.documentID)
        let job = ExportJob()
        let task = try #require(
            job.start(
                controller: controller, document: document, project: source,
                output: root.appendingPathComponent("out.wav"), settings: ExportSettings()))
        job.cancel()
        await task.value
        #expect(!FileManager.default.fileExists(atPath: source.appendingPathComponent("export.json").path))
        await controller.close()
    }
}
