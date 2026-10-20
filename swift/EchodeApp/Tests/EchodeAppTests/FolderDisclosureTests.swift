import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct FolderDisclosureTests {
    @Test func contractFolderDisclosureKeepsHeadersAndLanesOnTheSameVisibleTracks() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-folder-view-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "folder")
        let folder = try #require(controller.selection)
        #expect(await controller.edit(["type": "track.add_child", "parent": folder.uuidString, "name": "child"]))
        let child = try #require(controller.tracks.first { $0.name == "child" }?.id)
        #expect(await controller.edit(["type": "track.add_child", "parent": child.uuidString, "name": "nested"]))
        let nested = try #require(controller.tracks.first { $0.name == "nested" }?.id)
        await controller.addTrack(name: "tail")
        let tail = try #require(controller.selection)
        #expect(await controller.insertNote(pitch: 60, beat: 0, length: 1, on: child))
        #expect(await controller.insertNote(pitch: 64, beat: 0, length: 1, on: tail))
        #expect(await controller.setMixer(tail, field: .gain, value: 0.25))
        let document = try #require(controller.documentID)
        let state = WorkspaceState(defaults: .init())
        state.synchronize(controller)
        let session = WorkspaceSession(controller: controller, state: state)
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 720, height: 260))
        pane.layoutSubtreeIfNeeded()
        pane.onSelect = { controller.selection = $0 }
        pane.onSelectTrack = { controller.selectTrack($0, change: $1, visibleOrder: state.visibleTrackOrder(controller)) }
        pane.onFolderDisclosure = { id, collapsed in
            if collapsed { state.collapsedTracks.insert(id) } else { state.collapsedTracks.remove(id) }
        }
        func refresh() throws {
            pane.setSnapshot(try #require(controller.snapshot), selected: controller.selection,
                             selectedTracks: controller.selectedTracks, collapsedTracks: state.collapsedTracks)
            pane.headers.documentID = controller.documentID
        }
        controller.selection = nested
        try refresh()
        #expect(ArrangeFrame(input: pane.timeline.renderInput).headers.count == 4)
        let count = controller.undoCount
        pane.headers.mouseDown(with: try #require(NSEvent.mouseEvent(with: .leftMouseDown,
            location: pane.headers.convert(CGPoint(x: 12, y: 12), to: nil), modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1)))
        #expect(ArrangeFrame(input: pane.timeline.renderInput).headers.count == 2)
        #expect(state.collapsedTracks == [folder] && controller.selection == folder)
        #expect(controller.tracks.map(\.id) == [folder, child, nested, tail] && controller.undoCount == count)
        let frame = ArrangeFrame(input: pane.timeline.renderInput)
        #expect(frame.headers.map(\.id) == [folder, tail])
        #expect(frame.elements.filter { $0.role == .track }.map { $0.id.track } == [folder, tail])
        #expect(frame.elements.allSatisfy { $0.id.track == folder || $0.id.track == tail })
        #expect(frame.elements.contains { $0.role == .note && $0.id.track == tail && $0.y >= 64 && $0.y < 128 })
        #expect(frame.headers[1].mix?.gain.value == 0.25)
        pane.headers.mouseDown(with: try #require(NSEvent.mouseEvent(with: .leftMouseDown,
            location: pane.headers.convert(CGPoint(x: 4, y: 70), to: nil), modifierFlags: .shift, timestamp: 0,
            windowNumber: 0, context: nil, eventNumber: 2, clickCount: 1, pressure: 1)))
        #expect(controller.selectedTracks == [folder, tail])
        controller.selection = folder
        try refresh()
        var pending: Task<Bool, Never>?
        pane.onDropFX = { track, drop in pending = Task { await controller.dropFX(drop, on: track, documentID: document) } }
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        let drag = FXDrag(document: document, track: nil, device: nil, plugin: "org.example.folder-effect")
        board.setData(try JSONEncoder().encode(drag), forType: FXDrag.pluginType)
        #expect(pane.headers.acceptFX(board, at: CGPoint(x: 8, y: 70), copying: false))
        #expect(await pending?.value == true)
        #expect(controller.deviceRows(on: tail).count == 1 && controller.deviceRows(on: child).isEmpty)
        await controller.undo()
        #expect(controller.deviceRows(on: tail).isEmpty)
        controller.selection = folder
        let insertion = try #require(session.addTrack())
        // A later expansion cannot change the captured outside insertion.
        state.collapsedTracks.remove(folder)
        await insertion.value
        let outside = try #require(controller.selection)
        #expect(controller.tracks.map(\.id) == [folder, child, nested, outside, tail])
        #expect(controller.undoCount == count + 1)
        let source = try #require(controller.snapshot)
        #expect(source.trackHierarchy(at: 3)?.parent == nil)
        await controller.undo(); await controller.redo()
        #expect(controller.tracks.map(\.id) == [folder, child, nested, outside, tail])
        await controller.close(); await controller.open(url)
        state.synchronize(controller)
        #expect(state.collapsedTracks.isEmpty)
        #expect(controller.tracks.map(\.id) == [folder, child, nested, outside, tail])
        await controller.close()
    }

    @Test func nestedDisclosureAndKeyboardKeepTransientStateWithoutEditingTheProject() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-folder-keys-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "folder")
        let folder = try #require(controller.selection)
        #expect(await controller.edit(["type": "track.add_child", "parent": folder.uuidString, "name": "child"]))
        let child = try #require(controller.tracks.first { $0.name == "child" }?.id)
        #expect(await controller.edit(["type": "track.add_child", "parent": child.uuidString, "name": "nested"]))
        let source = try #require(controller.snapshot)
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 720, height: 130))
        pane.layoutSubtreeIfNeeded()
        pane.setSnapshot(source, selected: folder, collapsedTracks: [folder, child])
        let before = controller.undoCount
        func key(_ code: UInt16) throws {
            pane.headers.keyDown(with: try #require(NSEvent.keyEvent(with: .keyDown, location: .zero,
                modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, characters: "", charactersIgnoringModifiers: "", isARepeat: false, keyCode: code)))
        }
        try key(124)
        #expect(ArrangeFrame(input: pane.timeline.renderInput).headers.map(\.id) == [folder, child])
        try key(123)
        #expect(ArrangeFrame(input: pane.timeline.renderInput).headers.map(\.id) == [folder])
        #expect(controller.undoCount == before && source.trackCount == 3)
        let expanded = try #require(source.arrangeProjection(collapsedTracks: [folder])?.arrangeProjection(collapsedTracks: []))
        #expect(expanded.trackCount == 3 && expanded.trackHierarchy(at: 2)?.depth == 2)
        let deep = TrackFolderLayout(row: CGRect(x: 0, y: 0, width: 120, height: 16), depth: 10_000, folder: true)
        #expect(deep.disclosure.minX >= 0 && deep.disclosure.maxX <= 120)
        await controller.close()
    }
}
