import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct PluginCandidateListTests {
    private func listing(_ id: String, _ name: String) throws -> PluginListing {
        try JSONDecoder().decode(PluginListing.self, from: Data("""
            {"id":"\(id)","name":"\(name)","vendor":"Example","version":"1","features":["audio-effect"],"fresh":false}
            """.utf8))
    }

    /// A press anywhere on a row selects it and takes the keyboard at once;
    /// only the second click of a double click inserts and closes, and Return
    /// inserts and stays open. The rows are not rebuilt when only the
    /// selection changes. The menu inserts on every selected track as one
    /// undo step (ADR 0162, ADR 0124).
    @Test func contractPluginListSelectsOnPressAndInsertsFromAnywhereInTheRow() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-plugin-list-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        for name in ["chords", "bass"] { await controller.addTrack(name: name) }
        let ids = controller.tracks.map(\.id)
        controller.selectTrack(ids[0], change: .focus)
        controller.selectTrack(ids[1], change: .toggle)

        let plugins = try [listing("org.example.eq", "EQ"), listing("org.example.delay", "Delay"), listing("org.example.verb", "Reverb")]
        let list = PluginListView()
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 420, height: 300), styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        window.contentView = list
        var selected: String?, inserted: [(String, Bool)] = [], pending: Task<Bool, Never>?
        // The panel wires the list the same way.
        list.onSelect = { selected = $0 }
        list.table.document = document
        list.table.onInsert = { inserted.append(($0, $1)) }
        list.table.onInsertOnSelected = { plugin in
            guard let request = controller.capturePluginInsertion(plugin, allSelected: true, document: document) else { return }
            pending = Task { await controller.insertPlugin(request) }
        }
        list.table.canInsertOnSelected = true
        list.show(plugins, selection: nil)
        window.layoutIfNeeded()
        let table = list.table
        let firstRow = try #require(table.view(atColumn: 0, row: 0, makeIfNecessary: true))
        func point(_ row: Int) -> CGPoint {
            // The far right of the row, away from its labels.
            let rect = table.rect(ofRow: row)
            return table.convert(CGPoint(x: rect.maxX - 4, y: rect.midY), to: nil)
        }
        func mouse(_ type: NSEvent.EventType, _ row: Int, count: Int) throws -> NSEvent {
            try #require(NSEvent.mouseEvent(with: type, location: point(row), modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber, context: nil,
                eventNumber: count, clickCount: count, pressure: 1))
        }

        table.mouseDown(with: try mouse(.leftMouseDown, 1, count: 1))
        #expect(selected == "org.example.delay" && table.selectedRow == 1)
        #expect(window.firstResponder === table && inserted.isEmpty)
        table.mouseDown(with: try mouse(.leftMouseDown, 1, count: 2))
        #expect(inserted.map(\.0) == ["org.example.delay"] && inserted.map(\.1) == [true])

        table.keyDown(with: try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil,
            characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36)))
        #expect(inserted.map(\.0) == ["org.example.delay", "org.example.delay"] && inserted.last?.1 == false)

        // The panel echoes the selection back; the rows stay as they are.
        list.show(plugins, selection: selected)
        #expect(table.view(atColumn: 0, row: 0, makeIfNecessary: false) === firstRow)

        let menu = try #require(table.menu(for: try mouse(.rightMouseDown, 2, count: 1)))
        #expect(selected == "org.example.verb" && table.selectedRow == 2)
        let item = try #require(menu.items.first)
        #expect(item.isEnabled)
        let before = controller.undoCount
        menu.performActionForItem(at: 0)
        #expect(await pending?.value == true)
        #expect(controller.undoCount == before + 1)
        #expect(ids.allSatisfy { controller.deviceRows(on: $0).map(\.plugin) == ["org.example.verb"] })
        await controller.close()
    }
}
