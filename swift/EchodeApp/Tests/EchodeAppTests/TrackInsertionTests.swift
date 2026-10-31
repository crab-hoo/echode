import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct TrackInsertionTests {
    @Test func contractCommandAndFooterInsertTracksAsOneUndoAndReopen() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-track-insert-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        #expect(session.addTrack() == nil)
        await controller.open(url, creating: true)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "tail")
        let tail = try #require(controller.selection)
        controller.selection = first
        let before = controller.undoCount
        let command = try #require(session.addTrack())
        // Scheduling captures the original focus before a later UI selection.
        controller.selection = tail
        await command.value
        #expect(controller.errorMessage == nil)
        let inserted = try #require(controller.selection)
        #expect(controller.tracks.map(\.id) == [first, inserted, tail])
        #expect(controller.undoCount == before + 1 && controller.redoCount == 0)
        #expect(controller.selectedTracks == [inserted])
        #expect(controller.deviceRows(on: inserted).isEmpty)
        #expect(state.pluginBrowserDocument == nil && state.presentation == nil)
        await controller.undo()
        #expect(controller.tracks.map(\.id) == [first, tail])
        await controller.redo()
        #expect(controller.tracks.map(\.id) == [first, inserted, tail])

        controller.selection = first
        let footer = try #require(session.addTrack(atEnd: true))
        await footer.value
        let appended = try #require(controller.selection)
        #expect(controller.tracks.map(\.id) == [first, inserted, tail, appended])
        #expect(controller.undoCount == before + 2)
        await controller.undo()
        #expect(controller.tracks.map(\.id) == [first, inserted, tail])
        await controller.redo()
        await controller.close(); await controller.open(url)
        #expect(controller.tracks.map(\.id) == [first, inserted, tail, appended])
        #expect(controller.deviceRows(on: inserted).isEmpty && controller.deviceRows(on: appended).isEmpty)
        #expect(state.pluginBrowserDocument == nil)
        await controller.close()
    }

    @Test func expandedFoldersAppendChildrenAndLeafTracksKeepTheirParent() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-folder-insert-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "folder")
        let folder = try #require(controller.selection)
        await controller.addTrack(name: "tail")
        let tail = try #require(controller.selection)
        #expect(await controller.edit(.addChild(parent: folder.uuidString, name: "child")))
        let child = try #require(controller.tracks.first { $0.name == "child" }?.id)
        #expect(await controller.edit(.addChild(parent: child.uuidString, name: "nested")))
        let nested = try #require(controller.tracks.first { $0.name == "nested" }?.id)
        let session = WorkspaceSession(controller: controller, state: WorkspaceState(defaults: .init()))
        controller.selection = folder
        await (try #require(session.addTrack())).value
        #expect(controller.errorMessage == nil)
        let inside = try #require(controller.selection)
        #expect(controller.tracks.map(\.id) == [folder, child, nested, inside, tail])
        controller.selection = nested
        await (try #require(session.addTrack())).value
        let sibling = try #require(controller.selection)
        #expect(controller.tracks.map(\.id) == [folder, child, nested, sibling, inside, tail])
        controller.selection = nil
        await (try #require(session.addTrack())).value
        let appended = try #require(controller.selection)
        #expect(controller.tracks.map(\.id) == [folder, child, nested, sibling, inside, tail, appended])
        await controller.close(); await controller.open(url)
        #expect(controller.tracks.map(\.id) == [folder, child, nested, sibling, inside, tail, appended])
        await controller.close()
    }

    @Test func removedAnchorsAndReopenedDocumentsRejectCapturedRequestsWithoutClearingRedo() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-stale-insert-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "kept")
        let kept = try #require(controller.selection)
        await controller.addTrack(name: "removed")
        let request = try #require(controller.captureTrackInsertion(atEnd: false))
        await controller.undo()
        let before = controller.undoCount
        #expect(!(await controller.insertTrack(request)))
        #expect(controller.tracks.map(\.id) == [kept])
        #expect(controller.undoCount == before && controller.redoCount == 1)
        let valid = try #require(controller.captureTrackInsertion(atEnd: true))
        await controller.close(); await controller.open(url)
        #expect(!(await controller.insertTrack(valid)))
        #expect(controller.tracks.map(\.id) == [kept] && controller.undoCount == 0)
        await controller.close()
    }
}
