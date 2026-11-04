import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct ExportNormalizationTests {
    @Test func contractPeakNormalizationReachesTheCLIAndStaysWithTheProject() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-normalize-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let source = root.appendingPathComponent("Song.echode"), output = root.appendingPathComponent("normalized.wav")
        let input = root.appendingPathComponent("input.wav")
        // A short, quiet PCM fixture exercises the real import, snapshot and CLI path.
        var pcm = wavMono(rate: 48_000, frames: 513)
        for index in stride(from: 44, to: pcm.count, by: 2) { pcm[index] = 0; pcm[index + 1] = 16 }
        try pcm.write(to: input)
        let suite = "export-normalize-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let controller = ProjectController()
        await controller.open(source, creating: true)
        await controller.addTrack(name: "Quiet")
        #expect(await controller.importAudio(input))
        let document = try #require(controller.documentID)
        let cli = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("target/debug/echode")
        let job = ExportJob(); job.executable = cli
        var settings = ExportSettings()
        settings.format = .float32; settings.automaticTail = false; settings.tailSeconds = 0
        settings.normalizePeakDb = -6
        let undo = controller.undoCount
        let task = try #require(job.start(controller: controller, document: document, project: source,
                                          output: output, settings: settings, defaults: defaults))
        await task.value
        #expect(job.detail.contains("exported "), "Normalized export failed: \(job.detail)")
        let bytes = try Data(contentsOf: output)
        // Small float WAVs have an 18-byte fmt and a fact chunk, then stereo samples.
        try #require(bytes.count >= 58 && bytes[50..<54] == Data("data".utf8))
        let values = stride(from: 58, to: bytes.count, by: 4).map { at in
            Float(bitPattern: UInt32(bytes[at]) | UInt32(bytes[at + 1]) << 8 |
                  UInt32(bytes[at + 2]) << 16 | UInt32(bytes[at + 3]) << 24)
        }
        let peak = values.map { abs(Double($0)) }.max() ?? 0
        #expect(abs(peak - pow(10, -6.0 / 20)) < 1e-6, "GUI peak target did not reach the CLI")
        #expect(controller.undoCount == undo && controller.errorMessage == nil)
        #expect(await controller.loadExportSettings(documentID: document, defaults: defaults) == settings)
        await controller.close()
    }

    @Test func oldSettingsDefaultToNoNormalizationAndTargetsAreValidated() throws {
        let legacy = "{\"version\":1,\"settings\":{\"format\":\"pcm24\",\"sampleRate\":48000,\"automaticTail\":true,\"tailSeconds\":2,\"tpdf\":false}}"
        let old = try #require(ExportSettings.decodeStored(legacy))
        #expect(old.normalizePeakDb == nil && old.isValid)
        #expect(old.arguments?.contains("--normalize-peak-db") == false)
        var settings = old
        for target in [-120.0, -6, 0] {
            settings.normalizePeakDb = target
            #expect(settings.arguments?.suffix(2) == ["--normalize-peak-db", String(target)])
            #expect(ExportSettings.decodeStored(settings.storedJSON) == settings)
        }
        for target in [Double.nan, .infinity, -.infinity, -120.1, 0.1] {
            settings.normalizePeakDb = target
            #expect(!settings.isValid && settings.arguments == nil && settings.storedJSON == nil)
        }
        let suite = "export-normalize-legacy-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let project = URL(fileURLWithPath: "/tmp/legacy-normalize.echode")
        let legacySettings = Data("{\"format\":\"pcm16\",\"sampleRate\":44100,\"automaticTail\":false,\"tailSeconds\":1,\"tpdf\":true}".utf8)
        defaults.set(legacySettings, forKey: "workspace.export.v1." + project.resolvingSymlinksInPath().path)
        let loaded = ExportSettings.load(project: project, defaults: defaults)
        #expect(loaded.format == .pcm16 && loaded.sampleRate == 44_100 && loaded.normalizePeakDb == nil)
    }
}
