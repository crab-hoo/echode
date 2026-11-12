import AppKit
import SwiftUI
import Testing

@testable import EchodeAppCore

@MainActor
struct ExportInputTests {
    @Test func contractInvalidNumericTextCannotExportThePreviousValue() async throws {
        _ = NSApplication.shared
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-export-input-\(UUID()).echode")
        defer { removeTestFile(root) }
        let controller = ProjectController()
        await controller.open(root, creating: true)
        var initial = ExportSettings()
        initial.automaticTail = false
        initial.tailSeconds = 0.5
        initial.normalizePeakDb = -1
        let document = try #require(controller.document)
        _ = try await document.history(.exportSettings(json: initial.storedJSON))
        let job = ExportJob()
        let host = NSHostingView(rootView: WorkspaceExportView(controller: controller, job: job, close: {}))
        host.frame = CGRect(x: 0, y: 0, width: 460, height: 520)
        let window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: true)
        window.contentView = host
        func fields(in view: NSView) -> [NSTextField] {
            (view as? NSTextField).map { [$0] } ?? view.subviews.flatMap { fields(in: $0) }
        }
        host.layoutSubtreeIfNeeded()
        let inputs = fields(in: host)
        #expect(inputs.count == 3)
        let rate = try #require(inputs.first)
        let deadline = ContinuousClock.now + .seconds(5)
        while !rate.isEnabled, ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(5))
            host.layoutSubtreeIfNeeded()
        }
        #expect(rate.isEnabled && host.rootView.validatedSettings == initial, "Fixture settings must finish loading")
        func enter(_ value: String, into field: NSTextField) async throws {
            #expect(window.makeFirstResponder(field))
            let editor = try #require(field.currentEditor() as? NSTextView)
            editor.selectAll(nil)
            editor.insertText(value, replacementRange: editor.selectedRange())
            await Task.yield()
            host.layoutSubtreeIfNeeded()
        }
        // The native field editor feeds the production sheet's String binding,
        // including an unfinished value while focus is still inside the field.
        try await enter("not a sample rate", into: rate)
        #expect(
            host.rootView.validatedSettings == nil,
            "Invalid numeric text must disable export instead of reusing the previous sample rate")
        try await enter("44100", into: rate)
        #expect(host.rootView.validatedSettings?.sampleRate == 44_100)
        let tail = try #require(inputs.dropFirst().first)
        try await enter("0.1 seconds", into: tail)
        #expect(host.rootView.validatedSettings == nil, "Invalid tail text must not export the previous duration")
        let separator = Locale.current.decimalSeparator ?? "."
        try await enter("0\(separator)125", into: tail)
        #expect(host.rootView.validatedSettings?.arguments?.contains("5513") == true)
        let peak = try #require(inputs.last)
        try await enter("1", into: peak)
        #expect(
            host.rootView.validatedSettings == nil, "An out-of-range peak target must not export the previous target")
        try await enter("-3\(separator)5", into: peak)
        #expect(host.rootView.validatedSettings?.normalizePeakDb == -3.5)
        // Draft changes never save settings or consume Undo before starting a job.
        let stored = try await document.history(.exportSettings()).historyReport()
        #expect(ExportSettings.decodeStored(stored.exportSettings) == initial)
        window.makeFirstResponder(nil)
        #expect(!job.running && controller.undoCount == 0)
        await controller.close()
    }
}
