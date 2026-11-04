import AppKit
import SwiftUI
import Testing
@testable import EchodeAppCore

@MainActor
struct ExportDefaultsTests {
    @Test func commonDefaultsSeedOnlyProjectsWithoutPreviousSettings() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-defaults-\(UUID())")
        defer { removeTestFile(root) }
        let suite = "export-defaults-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        var common = ExportSettings()
        common.format = .pcm16; common.sampleRate = 44_100; common.tpdf = true
        common.automaticTail = false; common.tailSeconds = 0.125; common.normalizePeakDb = -3.5
        let json = try #require(common.storedJSON)
        defaults.set(json, forKey: "workspace.export.default.v1")
        let source = root.appendingPathComponent("Song.echode")
        let controller = ProjectController()
        await controller.open(source, creating: true)
        let id = try #require(controller.documentID), document = try #require(controller.document)
        #expect(await controller.loadExportSettings(documentID: id, defaults: defaults) == common,
                "A fresh project must use this Mac's common export defaults")
        #expect(!FileManager.default.fileExists(atPath: source.appendingPathComponent("export.json").path))
        var legacy = common; legacy.sampleRate = 96_000
        legacy.save(project: source, defaults: defaults)
        #expect(await controller.loadExportSettings(documentID: id, defaults: defaults) == legacy)
        var portable = common; portable.sampleRate = 88_200
        _ = try await document.history(.exportSettings(json: portable.storedJSON))
        #expect(await controller.loadExportSettings(documentID: id, defaults: defaults) == portable)
        #expect(controller.undoCount == 0 && controller.errorMessage == nil)
        await controller.close()
        await controller.open(root.appendingPathComponent("Other.echode"), creating: true)
        let other = try #require(controller.documentID)
        #expect(await controller.loadExportSettings(documentID: other, defaults: defaults) == common)
        #expect(await controller.loadExportSettings(documentID: id, defaults: defaults) == nil)
        await controller.close()
    }

    @Test func contractDefaultsUseValidatedInputWithoutChangingProjectHistory() async throws {
        _ = NSApplication.shared
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-default-controls-\(UUID())")
        defer { removeTestFile(root) }
        let suite = "export-default-controls-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let controller = ProjectController()
        let source = root.appendingPathComponent("Song.echode")
        await controller.open(source, creating: true)
        let id = try #require(controller.documentID), document = try #require(controller.document)
        var previous = ExportSettings()
        previous.format = .pcm16; previous.tpdf = true; previous.automaticTail = false
        previous.tailSeconds = 0.125; previous.normalizePeakDb = -3.5
        _ = try await document.history(.exportSettings(json: previous.storedJSON))
        let job = ExportJob()
        let sheet = DefaultsSheet(controller: controller, job: job, defaults: defaults)
        let rate = try await sheet.loadedRate()
        #expect(sheet.host.rootView.validatedSettings == previous)
        try await sheet.enter("not a rate", into: rate)
        #expect(sheet.host.rootView.validatedSettings == nil)
        sheet.host.rootView.saveCommonDefault()
        #expect(defaults.string(forKey: "workspace.export.default.v1") == nil)
        try await sheet.enter("44100", into: rate)
        var chosen = previous; chosen.sampleRate = 44_100
        sheet.host.rootView.saveCommonDefault()
        #expect(ExportSettings.commonDefault(defaults: defaults) == chosen,
                "The common default must store the validated numeric input from the real sheet")
        #expect(await controller.loadExportSettings(documentID: id, defaults: defaults) == previous,
                "Saving a common default must not replace a project's previous export settings")
        try await sheet.enter("96000", into: rate)
        sheet.host.rootView.useCommonDefault()
        await sheet.settle()
        #expect(sheet.host.rootView.validatedSettings == chosen)
        #expect(rate.stringValue == "44100")
        #expect(!job.running && controller.undoCount == 0)
        sheet.window.makeFirstResponder(nil)
        await controller.close()
        await controller.open(root.appendingPathComponent("Other.echode"), creating: true)
        let other = try #require(controller.documentID)
        #expect(await controller.loadExportSettings(documentID: other, defaults: defaults) == chosen,
                "A second project without previous settings must inherit the saved common default")
        // Retained commands cannot write or clear defaults after a document exchange,
        // even before SwiftUI has disabled the old native controls.
        sheet.host.rootView.resetCommonDefault()
        sheet.host.rootView.saveCommonDefault()
        #expect(ExportSettings.commonDefault(defaults: defaults) == chosen)
        let otherSheet = DefaultsSheet(controller: controller, job: job, defaults: defaults)
        _ = try await otherSheet.loadedRate()
        otherSheet.host.rootView.resetCommonDefault()
        await otherSheet.settle()
        #expect(defaults.string(forKey: "workspace.export.default.v1") == nil)
        #expect(otherSheet.host.rootView.validatedSettings == ExportSettings.factoryDefault())
        #expect(!FileManager.default.fileExists(atPath: root.appendingPathComponent("Other.echode/export.json").path))
        #expect(controller.undoCount == 0 && controller.errorMessage == nil)
        await controller.close()
        await controller.open(source)
        let reopened = try #require(controller.documentID)
        #expect(await controller.loadExportSettings(documentID: reopened, defaults: defaults) == previous)
        await controller.close()
    }

    @Test func invalidCommonDefaultsFallBackWithoutRepairOrOverwritingAValidDefault() throws {
        let suite = "export-default-invalid-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let key = "workspace.export.default.v1"
        var chosen = ExportSettings(); chosen.sampleRate = 44_100
        let json = try #require(chosen.storedJSON)
        for bad in ["not JSON", "[]", json.replacingOccurrences(of: "\"version\":1", with: "\"version\":2"),
                    json.replacingOccurrences(of: "44100", with: "0")] {
            defaults.set(bad, forKey: key)
            #expect(ExportSettings.commonDefault(defaults: defaults, initialRate: 96_000) == .factoryDefault(initialRate: 96_000))
            #expect(defaults.string(forKey: key) == bad)
        }
        #expect(chosen.saveAsDefault(defaults: defaults))
        for bad in [Double.nan, .infinity, -1] {
            var invalid = chosen; invalid.tailSeconds = bad
            #expect(!invalid.saveAsDefault(defaults: defaults))
            #expect(ExportSettings.commonDefault(defaults: defaults) == chosen)
        }
        ExportSettings.resetCommonDefault(defaults: defaults)
        #expect(ExportSettings.commonDefault(defaults: defaults, initialRate: 0) == ExportSettings())
        #expect(defaults.string(forKey: key) == nil)
    }
}

@MainActor
private final class DefaultsSheet {
    let host: NSHostingView<WorkspaceExportView>
    let window: NSWindow

    init(controller: ProjectController, job: ExportJob, defaults: UserDefaults) {
        host = NSHostingView(rootView: WorkspaceExportView(controller: controller, job: job, defaults: defaults, close: {}))
        host.frame = CGRect(x: 0, y: 0, width: 460, height: 640)
        window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: true)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
    }

    func loadedRate() async throws -> NSTextField {
        let rate = try #require(views(in: host, of: NSTextField.self).first)
        let deadline = ContinuousClock.now + .seconds(5)
        while !rate.isEnabled, ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(5))
            host.layoutSubtreeIfNeeded()
        }
        try #require(rate.isEnabled, "Fixture export settings must finish loading")
        return rate
    }

    func enter(_ value: String, into field: NSTextField) async throws {
        #expect(window.makeFirstResponder(field))
        let editor = try #require(field.currentEditor() as? NSTextView)
        editor.selectAll(nil)
        editor.insertText(value, replacementRange: editor.selectedRange())
        await settle()
    }

    func settle() async {
        await Task.yield()
        host.layoutSubtreeIfNeeded()
    }

    private func views<T: NSView>(in view: NSView, of type: T.Type) -> [T] {
        (view as? T).map { [$0] } ?? view.subviews.flatMap { views(in: $0, of: type) }
    }
}
