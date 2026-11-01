import AppKit
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct SampleBrowserContentTests {
    @Test func contractBrowserShowsProductionFilesAndMarksMIDIUnavailable() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-browser-content-\(UUID())")
        defer { removeTestFile(root) }
        for name in ["folder", "Song.echode", "Other.app"] {
            try FileManager.default.createDirectory(at: root.appendingPathComponent(name), withIntermediateDirectories: true)
        }
        for name in ["kick.WAV", "melody.MID", "drums.midi", "notes.txt", "preset.clap-preset", ".hidden.wav"] {
            try Data().write(to: root.appendingPathComponent(name))
        }
        let browser = FileBrowserState()
        await browser.register(root)
        #expect(Set(browser.files.map(\.url.lastPathComponent)) == ["folder", "Song.echode", "kick.WAV", "melody.MID", "drums.midi"])
        browser.query = "melody"
        #expect(browser.filtered.map(\.url.lastPathComponent) == ["melody.MID"])
        let midi = try #require(browser.files.first { $0.midi })
        let cell = FileRowView()
        cell.configure(midi, showingFolder: false)
        #expect(cell.textField?.stringValue.contains(text("MIDI import not available yet")) == true)
        let controller = ProjectController()
        let session = WorkspaceSession(controller: controller, state: WorkspaceState(defaults: .init()))
        #expect(session.openInBrowser(midi) == nil && session.importSample(midi) == nil)
        let list = FileListView()
        list.show(browser.files, searching: false, selection: nil)
        for (row, file) in browser.files.enumerated() {
            #expect((list.table.pasteboardWriter(for: row) != nil) == file.audio)
        }
        #expect(list.table.pasteboardWriter(for: -1) == nil)
        #expect(list.table.pasteboardWriter(for: browser.files.count) == nil)
    }
}
