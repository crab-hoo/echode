import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct ExportDestinationTests {
    private var cli: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("target/debug/echode")
    }

    @Test func contractExportPinsTheDestinationBeforePreparingItsSnapshot() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-destination-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode"), outside = root.appendingPathComponent("exports")
        let alias = root.appendingPathComponent("chosen-directory")
        let controller = ProjectController()
        await controller.open(source, creating: true)
        try FileManager.default.createDirectory(at: outside, withIntermediateDirectories: false)
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: outside)
        let original = try Data(contentsOf: source.appendingPathComponent("project.json"))
        let output = outside.appendingPathComponent("project.json")
        try Data("previous export".utf8).write(to: output)
        let suite = "export-destination-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let document = try #require(controller.documentID)
        var settings = ExportSettings(); settings.automaticTail = false; settings.tailSeconds = 0
        let job = ExportJob(); job.executable = cli
        let task = try #require(job.start(controller: controller, document: document, project: source,
                                          output: alias.appendingPathComponent("project.json"), settings: settings, defaults: defaults))
        // No suspension on this actor until the alias has changed: preparation and
        // CLI launch must use the destination captured by start(), not resolve again.
        try FileManager.default.removeItem(at: alias)
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: source)
        await task.value
        #expect(!job.running && job.detail.contains("exported "), "Export did not complete: \(job.detail)")
        #expect(try Data(contentsOf: source.appendingPathComponent("project.json")) == original,
                "Export must not follow a retargeted output directory into the source package")
        let rendered = try Data(contentsOf: output)
        #expect(rendered.starts(with: Data("RIFF".utf8)), "Export must use the originally selected output directory")
        #expect(job.detail.contains("Output: \(output.resolvingSymlinksInPath().path)"))
        #expect(controller.undoCount == 0 && controller.errorMessage == nil)
        await controller.close()
    }

    @Test(arguments: ["package", "outward-link", "inward-link", "directory-link"])
    func packageEntriesAndAliasesAreRejectedBeforePreparation(kind: String) async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-reject-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode"), external = root.appendingPathComponent("external.wav")
        let controller = ProjectController()
        await controller.open(source, creating: true)
        let sentinel = Data("keep external file".utf8)
        try sentinel.write(to: external)
        let output: URL
        switch kind {
        case "package": output = source
        case "outward-link":
            output = source.appendingPathComponent("outward.wav")
            try FileManager.default.createSymbolicLink(at: output, withDestinationURL: external)
        case "inward-link":
            output = root.appendingPathComponent("inward.wav")
            try FileManager.default.createSymbolicLink(at: output, withDestinationURL: source.appendingPathComponent("project.json"))
        default:
            let alias = root.appendingPathComponent("alias")
            try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: source)
            output = alias.appendingPathComponent("new.wav")
        }
        let document = try #require(controller.documentID)
        let job = ExportJob(); job.executable = nil
        let task = job.start(controller: controller, document: document, project: source, output: output, settings: ExportSettings())
        #expect(task == nil, "Export must reject both package entries and links targeting the package")
        if let task { job.cancel(); await task.value }
        #expect(job.message == text("Choose an output outside the project package"))
        #expect(try Data(contentsOf: external) == sentinel)
        #expect(!FileManager.default.fileExists(atPath: source.appendingPathComponent("export.json").path))
        #expect(controller.undoCount == 0 && controller.errorMessage == nil)
        await controller.close()
    }

    @Test func externalLeafLinksAreReplacedWithoutWritingTheirTargets() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-leaf-\(UUID())")
        defer { removeTestFile(root) }
        let source = root.appendingPathComponent("Song.echode"), outside = root.appendingPathComponent("Song.echode-exports")
        let controller = ProjectController()
        await controller.open(source, creating: true)
        try FileManager.default.createDirectory(at: outside, withIntermediateDirectories: false)
        let target = outside.appendingPathComponent("keep.wav"), output = outside.appendingPathComponent("mix.wav")
        let sentinel = Data("keep linked file".utf8)
        try sentinel.write(to: target)
        try FileManager.default.createSymbolicLink(at: output, withDestinationURL: target)
        let suite = "export-leaf-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        var settings = ExportSettings(); settings.automaticTail = false; settings.tailSeconds = 0
        let document = try #require(controller.documentID)
        let job = ExportJob(); job.executable = cli
        let task = try #require(job.start(controller: controller, document: document, project: source,
                                          output: output, settings: settings, defaults: defaults))
        await task.value
        #expect(job.detail.contains("exported "), "Export did not complete: \(job.detail)")
        #expect(try Data(contentsOf: target) == sentinel)
        #expect(try Data(contentsOf: output).starts(with: Data("RIFF".utf8)))
        #expect(try output.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink == false)
        await controller.close()
    }
}
