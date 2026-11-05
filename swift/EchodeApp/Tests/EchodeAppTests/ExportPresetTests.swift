import AppKit
import SwiftUI
import Testing
@testable import EchodeAppCore

@MainActor
struct ExportPresetTests {
    @Test func presetControlsAreAvailableWithoutStartingExport() async throws {
        _ = NSApplication.shared
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-presets-\(UUID()).echode")
        defer { removeTestFile(root) }
        let controller = ProjectController()
        await controller.open(root, creating: true)
        let suite = "export-presets-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let sheet = PresetSheet(controller: controller, defaults: defaults)
        _ = try await sheet.loadedRate()
        #expect(sheet.fields.count == 3, "Collapsed presets must not add a hidden editable field")
        sheet.host.rootView.presetControls.state.expanded = true
        await sheet.settle()
        #expect(sheet.fields.count == 4, "Expanding export presets must expose the preset name field in the real sheet")
        #expect(!sheet.job.running && controller.undoCount == 0)
        await controller.close()
    }

    @Test func contractPresetsUseValidatedFieldsAndRemainIndependentOfDocumentDefaults() async throws {
        _ = NSApplication.shared
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-preset-controls-\(UUID())")
        defer { removeTestFile(root) }
        let suite = "export-preset-controls-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        var common = ExportSettings(); common.sampleRate = 96_000
        #expect(common.saveAsDefault(defaults: defaults))
        let controller = ProjectController(), source = root.appendingPathComponent("Song.echode")
        await controller.open(source, creating: true)
        let id = try #require(controller.documentID), document = try #require(controller.document)
        var previous = ExportSettings()
        previous.format = .pcm16; previous.tpdf = true; previous.automaticTail = false
        previous.tailSeconds = 0.125; previous.normalizePeakDb = -3.5
        _ = try await document.history(.exportSettings(json: previous.storedJSON))
        let sheet = PresetSheet(controller: controller, defaults: defaults)
        let rate = try await sheet.loadedRate(), controls = sheet.host.rootView.presetControls
        controls.state.expanded = true
        await sheet.settle()
        #expect(sheet.fields.count == 4)
        let name = try #require(sheet.fields.last)
        try await sheet.enter("  Delivery  ", into: name)
        try await sheet.enter("not a rate", into: rate)
        controls.save()
        #expect(try ExportPresets.load(defaults: defaults).isEmpty)
        try await sheet.enter("44100", into: rate)
        controls.save()
        var chosen = previous; chosen.sampleRate = 44_100
        #expect(try ExportPresets.load(defaults: defaults) == ["Delivery": chosen],
                "Named presets must persist the validated input from the real export sheet")
        #expect(controls.state.selectedName == "Delivery")
        try await sheet.enter("88200", into: rate)
        controls.save()
        #expect(try ExportPresets.load(defaults: defaults)["Delivery"] == chosen,
                "Saving an existing name must not silently replace a preset")
        #expect(controls.state.message == ExportPresets.Failure.duplicateName.message)
        try await sheet.enter("Archive", into: name)
        controls.save()
        var archive = previous; archive.sampleRate = 88_200
        #expect(try ExportPresets.load(defaults: defaults).count == 2)
        controls.state.selectedName = "Delivery"
        controls.apply()
        await sheet.settle()
        #expect(sheet.host.rootView.validatedSettings == chosen && rate.stringValue == "44100",
                "Applying a named preset must replace the displayed numeric draft")
        controls.delete()
        #expect(try ExportPresets.load(defaults: defaults) == ["Archive": archive])
        #expect(sheet.host.rootView.validatedSettings == chosen, "Deleting a preset must not change the current fields")
        #expect(ExportSettings.commonDefault(defaults: defaults) == common)
        #expect(await controller.loadExportSettings(documentID: id, defaults: defaults) == previous)
        #expect(!sheet.job.running && controller.undoCount == 0 && controller.errorMessage == nil)
        // A retained sheet cannot apply, delete or save after the document changes.
        try await sheet.enter("48000", into: rate)
        let beforeExchange = sheet.host.rootView.validatedSettings
        controls.state.selectedName = "Archive"; controls.state.name = "Stale"
        sheet.window.makeFirstResponder(nil)
        await controller.close()
        await controller.open(root.appendingPathComponent("Other.echode"), creating: true)
        controls.apply(); controls.delete(); controls.save()
        #expect(sheet.host.rootView.validatedSettings == beforeExchange)
        #expect(try ExportPresets.load(defaults: defaults) == ["Archive": archive])
        let other = try #require(controller.documentID)
        #expect(await controller.loadExportSettings(documentID: other, defaults: defaults) == common,
                "Named presets must never become automatic defaults")
        let otherSheet = PresetSheet(controller: controller, defaults: defaults)
        _ = try await otherSheet.loadedRate()
        let otherControls = otherSheet.host.rootView.presetControls
        #expect(otherControls.state.entries == ["Archive": archive])
        // Another sheet can delete a selection; applying must re-read, not resurrect it.
        otherControls.state.selectedName = "Archive"
        try ExportPresets.delete(name: "Archive", defaults: defaults)
        otherControls.apply()
        #expect(otherSheet.host.rootView.validatedSettings == common)
        #expect(otherControls.state.message == ExportPresets.Failure.missing.message)
        #expect(!otherSheet.job.running && controller.undoCount == 0)
        await controller.close()
        await controller.open(source)
        let reopened = try #require(controller.documentID)
        #expect(await controller.loadExportSettings(documentID: reopened, defaults: defaults) == previous)
        await controller.close()
    }

    @Test func loadingAndUnreadableStoresCannotReplacePreferencesOrBlockManualSettings() async throws {
        _ = NSApplication.shared
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-preset-rejection-\(UUID()).echode")
        defer { removeTestFile(root) }
        let suite = "export-preset-rejection-\(UUID())", defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let controller = ProjectController()
        await controller.open(root, creating: true)
        var saved = ExportSettings(); saved.sampleRate = 44_100
        try ExportPresets.save(name: "Mix", settings: saved, defaults: defaults)
        let sheet = PresetSheet(controller: controller, defaults: defaults)
        let controls = sheet.host.rootView.presetControls
        controls.state.selectedName = "Mix"; controls.state.name = "Before loading"
        controls.apply(); controls.delete(); controls.save()
        #expect(try ExportPresets.load(defaults: defaults) == ["Mix": saved])
        #expect(sheet.host.rootView.validatedSettings == ExportSettings.factoryDefault())
        _ = try await sheet.loadedRate()
        defaults.set("unreadable", forKey: ExportPresets.storageKey)
        controls.apply(); controls.delete(); controls.save()
        #expect(!controls.state.readable && controls.state.entries.isEmpty)
        #expect(controls.state.message == ExportPresets.Failure.unreadable.message)
        #expect(defaults.string(forKey: ExportPresets.storageKey) == "unreadable")
        #expect(sheet.host.rootView.validatedSettings == ExportSettings.factoryDefault(),
                "An unreadable preset store must not prevent manual export settings")
        #expect(!sheet.job.running && controller.undoCount == 0)
        await controller.close()
    }
}

@MainActor
private final class PresetSheet {
    let job = ExportJob()
    let host: NSHostingView<WorkspaceExportView>
    let window: NSWindow

    init(controller: ProjectController, defaults: UserDefaults) {
        host = NSHostingView(rootView: WorkspaceExportView(controller: controller, job: job, defaults: defaults, close: {}))
        host.frame = CGRect(x: 0, y: 0, width: 460, height: 780)
        window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: true)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
    }

    var fields: [NSTextField] {
        func find(_ view: NSView) -> [NSTextField] {
            (view as? NSTextField).map { [$0] } ?? view.subviews.flatMap(find)
        }
        return find(host)
    }

    func loadedRate() async throws -> NSTextField {
        let rate = try #require(fields.first)
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
}
