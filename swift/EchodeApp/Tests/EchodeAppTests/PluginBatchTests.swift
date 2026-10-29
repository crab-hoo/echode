import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct PluginBatchTests {
    @Test func contractSelectedTrackFXBatchPersistsAsOneUndo() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-batch-fx-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        for name in ["chords", "melody", "bass"] { await controller.addTrack(name: name) }
        let ids = controller.tracks.map(\.id)
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 720, height: 260))
        pane.layoutSubtreeIfNeeded()
        pane.onSelectTrack = { controller.selectTrack($0, change: $1) }
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        pane.onOpenFX = { session.openPlugins(on: $0) }
        func refresh() throws {
            pane.setSnapshot(try #require(controller.snapshot), selected: controller.selection, selectedTracks: controller.selectedTracks)
        }
        func click(_ index: Int, flags: NSEvent.ModifierFlags = [], fx: Bool = false) throws {
            try refresh()
            let row = pane.headers.camera.rows(trackCount: ids.count, height: 260).rect(at: index, width: pane.headers.bounds.width)
            let point = fx ? CGPoint(x: TrackFXLayout(row: row).rect.midX, y: TrackFXLayout(row: row).rect.midY)
                : CGPoint(x: 4, y: row.minY + 4)
            pane.headers.mouseDown(with: try #require(NSEvent.mouseEvent(with: .leftMouseDown,
                location: pane.headers.convert(point, to: nil), modifierFlags: flags, timestamp: 0,
                windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1)))
        }
        try click(0)
        try click(2, flags: .shift)
        #expect(controller.selectedTracks == Set(ids))
        try click(1, flags: .command)
        #expect(controller.selectedTracks == Set([ids[0], ids[2]]))
        try click(0, fx: true)
        #expect(state.pluginBrowserDocument == document)
        #expect(controller.selection == ids[0])
        #expect(controller.selectedTracks == Set([ids[0], ids[2]]))
        try refresh()
        let frame = ArrangeFrame(input: pane.timeline.renderInput)
        let palette = try #require(frame.input.header).palette
        for row in frame.headers {
            let background = try #require(frame.headerInstances.first { $0.rect.x == 0 && $0.rect.y == row.y && $0.rect.z == Float(pane.headers.bounds.width) })
            #expect(background.color == (controller.selectedTracks.contains(row.id) ? palette.selected : palette.background))
        }
        let request = try #require(controller.capturePluginInsertion("org.example.batch", allSelected: true, document: document))
        #expect(request.tracks == [ids[0], ids[2]])
        // A later selection must not change the already submitted target IDs.
        controller.selection = ids[1]
        let before = controller.undoCount
        #expect(await controller.insertPlugin(request))
        #expect(controller.undoCount == before + 1)
        let first = try #require(controller.deviceRows(on: ids[0]).first)
        let last = try #require(controller.deviceRows(on: ids[2]).first)
        #expect(first.id != last.id && first.plugin == request.plugin && last.plugin == request.plugin)
        #expect(controller.deviceRows(on: ids[1]).isEmpty)
        await controller.undo()
        #expect(ids.allSatisfy { controller.deviceRows(on: $0).isEmpty })
        await controller.redo()
        #expect(controller.deviceRows(on: ids[0]) == [first])
        #expect(controller.deviceRows(on: ids[2]) == [last])
        await controller.close(); await controller.open(url)
        #expect(controller.deviceRows(on: ids[0]) == [first])
        #expect(controller.deviceRows(on: ids[2]) == [last])
        #expect(controller.deviceRows(on: ids[1]).isEmpty)
        #expect(!(await controller.insertPlugin(request)))
        #expect(controller.undoCount == 0)
        await controller.close()
    }

    @Test func invalidTargetsDoNotPartiallyInsertOrClearRedo() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-batch-invalid-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let plugin = FakeParameterPlugin()
        // A single candidate also opens its GUI; keep that request off real HAL.
        let controller = ProjectController(sendDevice: { try await plugin.send($0, $1) })
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "removed")
        let removed = try #require(controller.selection)
        let request = PluginInsertionRequest(document: document, plugin: "org.example.fx", tracks: [first, removed])
        await controller.undo()
        let before = controller.undoCount
        #expect(!(await controller.insertPlugin(request)))
        #expect(!(await controller.insertPlugin("", on: [first], documentID: document)))
        #expect(!(await controller.insertPlugin("org.example.fx", on: [], documentID: document)))
        #expect(!(await controller.insertPlugin("org.example.fx", on: [first, first], documentID: document)))
        #expect(controller.undoCount == before && controller.redoCount == 1)
        #expect(controller.deviceRows(on: first).isEmpty)
        await controller.redo()
        #expect(controller.tracks.map(\.id) == [first, removed])
        controller.selectTrack(first)
        controller.selectTrack(removed, change: .toggle)
        let single = try #require(controller.capturePluginInsertion("org.example.fx", allSelected: false, document: document))
        #expect(single.tracks == [removed])
        #expect(await controller.insertPlugin(single))
        #expect(controller.deviceRows(on: first).isEmpty)
        #expect(controller.deviceRows(on: removed).count == 1)
        await controller.pluginWindowTask?.value
        #expect(await plugin.windowRequests() == [try #require(controller.deviceRows(on: removed).first).id])
        await controller.close()
        #expect(controller.selectedTracks.isEmpty && controller.selection == nil)
    }

    @Test func trackSelectionKeepsRangeAnchorAndSingleInputFocus() {
        let ids = (0..<5).map { _ in UUID() }
        var selection = TrackSelection()
        selection.select(ids[1], change: .replace, order: ids)
        selection.select(ids[4], change: .extend, order: ids)
        #expect(selection.ids == Set(ids[1...4]) && selection.focus == ids[4])
        selection.select(ids[2], change: .extend, order: ids)
        #expect(selection.ids == Set(ids[1...2]))
        selection.select(ids[1], change: .toggle, order: ids)
        #expect(selection.ids == [ids[2]] && selection.focus == ids[2])
        selection.select(ids[2], change: .toggle, order: ids)
        #expect(selection.ids.isEmpty && selection.focus == nil)
        selection.select(UUID(), change: .replace, order: ids)
        #expect(selection.ids.isEmpty)
        selection.select(ids[4], change: .extend, order: ids)
        selection.select(ids[0], change: .toggle, order: ids)
        selection.reconcile(order: Array(ids.prefix(4)))
        #expect(selection.ids == [ids[0]] && selection.focus == ids[0])
        selection.reconcile(order: [])
        #expect(selection.ids.isEmpty && selection.focus == nil)
    }
}
