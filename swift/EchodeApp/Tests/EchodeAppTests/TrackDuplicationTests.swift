import AppKit
import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

@MainActor
struct TrackDuplicationTests {
    @Test func contractHeaderMenuDuplicatesSelectedSubtreesAsOneUndoAndReopen() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-track-duplicate-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        await controller.open(url, creating: true)
        await controller.addTrack(name: "folder")
        let folder = try #require(controller.selection)
        #expect(await controller.edit(.addChild(parent: folder.uuidString, name: "child")))
        let child = try #require(controller.tracks.first { $0.name == "child" }?.id)
        await controller.addTrack(name: "lead")
        let lead = try #require(controller.selection)
        #expect(await controller.insertPlugin("org.example.duplicate", on: lead))
        #expect(await controller.insertNote(pitch: 64, beat: 1, length: 0.5, on: lead))
        await controller.addTrack(name: "tail")
        let tail = try #require(controller.selection)
        let ids = controller.tracks.map(\.id)
        #expect(ids == [folder, child, lead, tail])

        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 720, height: 320))
        pane.layoutSubtreeIfNeeded()
        pane.onSelectTrack = { controller.selectTrack($0, change: $1) }
        var scheduled: Task<Void, Never>?
        pane.onDuplicateTracks = { scheduled = session.duplicateTracks() }
        func event(_ type: NSEvent.EventType, row index: Int, flags: NSEvent.ModifierFlags = []) throws -> NSEvent {
            pane.setSnapshot(
                try #require(controller.snapshot), selected: controller.selection,
                selectedTracks: controller.selectedTracks)
            let row = pane.headers.camera.rows(trackCount: controller.tracks.count, height: 320).rect(
                at: index, width: pane.headers.bounds.width)
            return try #require(
                NSEvent.mouseEvent(
                    with: type, location: pane.headers.convert(CGPoint(x: 60, y: row.minY + 8), to: nil),
                    modifierFlags: flags, timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1,
                    pressure: 1))
        }
        pane.headers.mouseDown(with: try event(.leftMouseDown, row: 0))
        pane.headers.mouseDown(with: try event(.leftMouseDown, row: 2, flags: .shift))
        #expect(controller.selectedTracks == [folder, child, lead])
        // A right click inside the selection keeps it and only moves the focus.
        let menu = try #require(pane.headers.menu(for: try event(.rightMouseDown, row: 1)))
        #expect(controller.selectedTracks == [folder, child, lead] && controller.selection == child)
        #expect(menu.items.map(\.title) == [text("Duplicate Track")])
        let before = controller.undoCount
        // Tests have no NSApp event loop, so send the item's own target-action.
        let item = menu.items[0]
        _ = try #require(item.target as? NSObject).perform(item.action, with: item)
        // The captured sources survive a selection change before the edit runs.
        controller.selection = tail
        await (try #require(scheduled)).value
        #expect(controller.errorMessage == nil)
        let copied = controller.tracks.map(\.id)
        #expect(copied.count == 7)
        let (folderCopy, childCopy, leadCopy) = (copied[2], copied[3], copied[5])
        #expect(copied == [folder, child, folderCopy, childCopy, lead, leadCopy, tail])
        #expect(controller.tracks.map(\.name) == ["folder", "child", "folder", "child", "lead", "lead", "tail"])
        #expect(controller.selectedTracks == [folderCopy, leadCopy] && controller.selection == folderCopy)
        #expect(controller.undoCount == before + 1 && controller.redoCount == 0)
        let snapshot = try #require(controller.snapshot)
        #expect(snapshot.trackHierarchy(at: 3)?.parent == folderCopy)
        let device = try #require(controller.deviceRows(on: lead).first)
        let deviceCopy = try #require(controller.deviceRows(on: leadCopy).first)
        #expect(deviceCopy.plugin == device.plugin && deviceCopy.id != device.id)
        let note = try #require(snapshot.pianoNeighbor(track: lead, current: nil))
        let noteCopy = try #require(snapshot.pianoNeighbor(track: leadCopy, current: nil))
        #expect((noteCopy.pitch, noteCopy.start, noteCopy.length) == (note.pitch, note.start, note.length))
        #expect(noteCopy.id.note != note.id.note && noteCopy.id.placement != note.id.placement)
        #expect(state.pluginBrowserDocument == nil && state.presentation == nil)

        await controller.undo()
        #expect(controller.tracks.map(\.id) == ids)
        await controller.redo()
        #expect(controller.tracks.map(\.id) == copied)
        await controller.close()
        await controller.open(url)
        #expect(controller.tracks.map(\.id) == copied)
        #expect(controller.deviceRows(on: leadCopy).first?.id == deviceCopy.id)
        await controller.close()
    }

    @Test func outsideRightClickSelectsThatRowAndStaleRequestsAreRefused() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-stale-duplicate-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        let session = WorkspaceSession(controller: controller, state: WorkspaceState(defaults: .init()))
        #expect(session.duplicateTracks() == nil)
        await controller.open(url, creating: true)
        await controller.addTrack(name: "kept")
        let kept = try #require(controller.selection)
        await controller.addTrack(name: "removed")
        let removed = try #require(controller.selection)

        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 720, height: 200))
        pane.layoutSubtreeIfNeeded()
        pane.onSelectTrack = { controller.selectTrack($0, change: $1) }
        pane.onDuplicateTracks = {}
        pane.setSnapshot(
            try #require(controller.snapshot), selected: controller.selection, selectedTracks: controller.selectedTracks
        )
        let row = pane.headers.camera.rows(trackCount: 2, height: 200).rect(at: 0, width: pane.headers.bounds.width)
        let click = try #require(
            NSEvent.mouseEvent(
                with: .rightMouseDown, location: pane.headers.convert(CGPoint(x: 60, y: row.minY + 8), to: nil),
                modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1,
                pressure: 1))
        #expect(pane.headers.menu(for: click) != nil)
        #expect(controller.selectedTracks == [kept])

        controller.selection = removed
        let request = try #require(controller.captureTrackDuplication())
        #expect(request.sources == [removed])
        await controller.undo()
        let before = controller.undoCount
        #expect(!(await controller.duplicateTracks(request)))
        #expect(controller.tracks.map(\.id) == [kept])
        #expect(controller.undoCount == before && controller.redoCount == 1)
        controller.selection = kept
        let valid = try #require(controller.captureTrackDuplication())
        await controller.close()
        await controller.open(url)
        #expect(!(await controller.duplicateTracks(valid)))
        #expect(controller.tracks.map(\.id) == [kept] && controller.undoCount == 0)
        await controller.close()
    }
}
