import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct SampleAudioDropTests {
    @Test func contractFileURLDropUsesPointerAndAppendsInOneUndoAcrossReopen() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-audio-drop-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("tone.wav"), project = root.appendingPathComponent("Song.echode")
        try wavMono(rate: 48_000, frames: 480).write(to: wav)
        let controller = ProjectController()
        let session = WorkspaceSession(controller: controller, state: WorkspaceState(defaults: .init()))
        await controller.open(project, creating: true)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "second")
        let second = try #require(controller.selection)
        let document = try #require(controller.documentID)
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 640, height: 320))
        pane.layout()
        pane.setSnapshot(try #require(controller.snapshot), selected: first)
        let view = pane.timeline
        view.audioDocument = document
        #expect(view.registeredDraggedTypes.contains(.fileURL))
        var pending: Task<Bool, Never>?
        view.onDropAudio = { pending = session.importDroppedAudio($0); return pending != nil }
        // The browser writes the same public file-URL type as Finder.
        let list = FileListView()
        list.show([BrowserFile(url: wav, directory: false)], searching: false, selection: nil)
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        #expect(board.writeObjects([try #require(list.table.pasteboardWriter(for: 0))]))
        #expect(board.types?.contains(.fileURL) == true)
        pane.headers.onScroll?(-72, 0)
        // Header gutter + scroll + camera scale, not the live playhead or selection.
        let point = CGPoint(x: 161 + 72 * 1.3, y: 70)
        let drop = try #require(view.audioDrop(board, at: point, flags: [], document: document))
        #expect(drop.track == second && drop.tick == 2 * 705_600)
        let unsnapped = try #require(view.audioDrop(board, at: point, flags: .shift, document: document))
        #expect(unsnapped.tick == 1_622_880)
        let before = controller.undoCount
        #expect(view.acceptAudio(board, at: point, flags: [], document: document))
        controller.selection = first
        #expect(await (try #require(pending)).value)
        #expect(controller.undoCount == before + 1 && controller.selection == first)
        let snapshot = try #require(controller.snapshot)
        let clip = try #require(clips(snapshot).first?.id.placement)
        #expect(snapshot.clipMix(clip)?.track == second && snapshot.clipMix(clip)?.start == drop.tick)
        #expect(!(await controller.importDroppedAudio(drop))) // Existing same-track overlap is still rejected.
        #expect(controller.undoCount == before + 1)
        await controller.undo()
        #expect(controller.snapshot?.clipMix(clip) == nil)
        await controller.redo()
        #expect(controller.snapshot?.clipMix(clip)?.start == drop.tick)
        // Below all visible rows appends at top level, regardless of focus.
        #expect(view.acceptAudio(board, at: CGPoint(x: point.x, y: 150), flags: .shift, document: document))
        #expect(await (try #require(pending)).value)
        let appended = try #require(controller.tracks.last?.id)
        #expect(controller.tracks.map(\.id) == [first, second, appended])
        #expect(controller.selection == appended && controller.undoCount == before + 2)
        let appendedSnapshot = try #require(controller.snapshot)
        let addedClip = try #require(clips(appendedSnapshot).first { $0.id.track == appended }?.id.placement)
        #expect(controller.snapshot?.clipMix(addedClip)?.start == unsnapped.tick)
        await controller.undo()
        #expect(controller.tracks.map(\.id) == [first, second] && controller.snapshot?.clipMix(addedClip) == nil)
        await controller.redo()
        #expect(controller.tracks.last?.id == appended && controller.snapshot?.clipMix(addedClip)?.start == unsnapped.tick)
        await controller.close(); await controller.open(project)
        #expect(controller.tracks.last?.id == appended && controller.snapshot?.clipMix(addedClip)?.start == unsnapped.tick)
        #expect(!(await controller.importDroppedAudio(drop)))
        await controller.close()
    }

    @Test func rejectsWrongTypesBoundariesRemovedTargetsAndFailedImportsWithoutPartialTracks() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-audio-drop-reject-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        let controller = ProjectController()
        await controller.open(root, creating: true)
        let document = try #require(controller.documentID)
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 640, height: 300))
        pane.layout(); pane.setSnapshot(try #require(controller.snapshot), selected: nil)
        let view = pane.timeline
        view.audioDocument = document
        view.onDropAudio = { _ in true }
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        func write(_ urls: [URL]) { board.clearContents(); board.writeObjects(urls.map { $0 as NSURL }) }
        let wav = root.appendingPathComponent("missing.wav")
        for name in ["notes.mid", "notes.txt", "Song.echode", "plugin.clap"] {
            write([root.appendingPathComponent(name)])
            #expect(view.audioDrop(board, at: CGPoint(x: 220, y: 10), flags: [], document: document) == nil)
        }
        write([wav, wav])
        #expect(AudioFileDrop.audioURL(from: board) == nil)
        write([try #require(URL(string: "https://example.invalid/tone.wav"))])
        #expect(AudioFileDrop.audioURL(from: board) == nil)
        write([wav])
        for point in [CGPoint(x: 160, y: 10), CGPoint(x: 640, y: 10), CGPoint(x: 220, y: -1), CGPoint(x: 220, y: 300), CGPoint(x: CGFloat.infinity, y: 0)] {
            #expect(view.audioDrop(board, at: point, flags: [], document: document) == nil)
        }
        #expect(view.audioDrop(board, at: CGPoint(x: 220, y: 10), flags: [], document: UUID()) == nil)
        // Empty projects are drop targets too. A decode error must not leave a track.
        let empty = try #require(view.audioDrop(board, at: CGPoint(x: 220, y: 10), flags: [], document: document))
        #expect(empty.track == nil)
        #expect(!(await controller.importDroppedAudio(empty)))
        #expect(controller.tracks.isEmpty && controller.undoCount == 0)
        await controller.addTrack(name: "removed")
        let removed = try #require(controller.selection)
        let stale = AudioFileDrop(document: document, url: wav, track: removed, tick: 0)
        await controller.undo()
        #expect(!(await controller.importDroppedAudio(stale)))
        #expect(controller.redoCount == 1 && controller.undoCount == 0)
        #expect(!(await controller.importDroppedAudio(empty)))
        #expect(controller.tracks.isEmpty && controller.redoCount == 1)
        await controller.redo()
        #expect(controller.tracks.map(\.id) == [removed])
        await controller.undo()
        try wavMono(rate: 48_000, frames: 480).write(to: wav)
        #expect(await controller.importDroppedAudio(empty))
        #expect(controller.tracks.count == 1 && controller.undoCount == 1 && controller.redoCount == 0)
        let initialSnapshot = try #require(controller.snapshot)
        let initialClip = try #require(clips(initialSnapshot).first?.id.placement)
        #expect(initialSnapshot.clipMix(initialClip)?.start == empty.tick)
        await controller.close()
    }

    @Test func projectedRowsAndFractionalScrollUseCurrentCameraNotHiddenChildren() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-audio-drop-rows-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        let controller = ProjectController()
        await controller.open(root, creating: true)
        await controller.addTrack(name: "folder")
        let folder = try #require(controller.selection)
        #expect(await controller.edit(["type": "track.add_child", "parent": folder.uuidString, "name": "hidden"]))
        await controller.addTrack(name: "tail")
        let tail = try #require(controller.selection), document = try #require(controller.documentID)
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 640, height: 100))
        pane.layout()
        pane.setSnapshot(try #require(controller.snapshot), selected: folder, collapsedTracks: [folder])
        let view = pane.timeline
        view.audioDocument = document; view.onDropAudio = { _ in true }
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        board.writeObjects([root.appendingPathComponent("tone.wav") as NSURL])
        pane.headers.onScroll?(0, -16)
        #expect(view.camera.originTrack == 0.25)
        #expect(view.audioDrop(board, at: CGPoint(x: 220, y: 47), flags: [], document: document)?.track == folder)
        #expect(view.audioDrop(board, at: CGPoint(x: 220, y: 48), flags: [], document: document)?.track == tail)
        pane.timeline.onZoom?(1.5, .option, CGPoint(x: 0, y: 0))
        #expect(view.audioDrop(board, at: CGPoint(x: 220, y: 71), flags: [], document: document)?.track == folder)
        #expect(view.audioDrop(board, at: CGPoint(x: 220, y: 72), flags: [], document: document)?.track == tail)
        pane.resizeHeader(to: 200)
        let origin = 201.0
        #expect(view.audioDrop(board, at: CGPoint(x: origin + 72 * 1.2, y: 10), flags: [], document: document)?.tick == 705_600)
        pane.timeline.onZoom?(2, [], .zero)
        #expect(view.audioDrop(board, at: CGPoint(x: origin + 144 * 1.2, y: 10), flags: [], document: document)?.tick == 705_600)
        #expect(view.audioDrop(board, at: CGPoint(x: 200, y: 10), flags: [], document: document) == nil)
        await controller.close()
    }

    @Test func nativeDropEntryRejectsDocumentChangesAndCancelledDrags() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-native-drop-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        let controller = ProjectController()
        await controller.open(root, creating: true)
        let view = ArrangeView(frame: CGRect(x: 0, y: 0, width: 400, height: 200))
        view.setSnapshot(try #require(controller.snapshot), selected: nil)
        let document = try #require(controller.documentID)
        view.audioDocument = document
        var dropped: [AudioFileDrop] = []
        view.onDropAudio = { dropped.append($0); return true }
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        board.writeObjects([root.appendingPathComponent("tone.wav") as NSURL])
        let info = FileDragInfo(board: board)
        #expect(view.draggingEntered(info) == .copy)
        #expect(view.prepareForDragOperation(info))
        #expect(view.performDragOperation(info) && dropped.count == 1)
        #expect(!view.performDragOperation(info))
        #expect(view.draggingEntered(info) == .copy)
        view.draggingExited(info)
        #expect(!view.performDragOperation(info))
        #expect(view.draggingEntered(info) == .copy)
        view.audioDocument = UUID()
        #expect(view.draggingUpdated(info).isEmpty && !view.performDragOperation(info))
        view.audioDocument = document
        info.draggingSourceOperationMask = .move
        #expect(view.draggingEntered(info).isEmpty && !view.prepareForDragOperation(info))
        #expect(!view.performDragOperation(info) && dropped.count == 1)
        await controller.close()
    }

    private func clips(_ snapshot: Snapshot) -> [ArrangeElement] {
        // The window bounds only these fixtures; it is not a product size cap.
        snapshot.visibleElements(originTick: 0, widthTicks: 10_000_000, originTrack: 0, heightTracks: 10,
                                 pixelsPerTick: 0.001, pixelsPerTrack: 40).elements.filter { $0.role == .clip }
    }
}

@MainActor
private final class FileDragInfo: NSObject, NSDraggingInfo {
    let draggingPasteboard: NSPasteboard
    var draggingSourceOperationMask: NSDragOperation = .copy
    let draggingDestinationWindow: NSWindow? = nil
    let draggingLocation = CGPoint(x: 100, y: 50)
    let draggedImageLocation = CGPoint.zero
    let draggedImage: NSImage? = nil
    let draggingSource: Any? = nil
    let draggingSequenceNumber = 1
    var draggingFormation: NSDraggingFormation = .none
    var animatesToDestination = false
    var numberOfValidItemsForDrop = 1
    let springLoadingHighlight: NSSpringLoadingHighlight = .none
    init(board: NSPasteboard) { draggingPasteboard = board }
    func slideDraggedImage(to screenPoint: NSPoint) {}
    func resetSpringLoading() {}
    func enumerateDraggingItems(options: NSDraggingItemEnumerationOptions, for view: NSView?, classes: [AnyClass],
                                searchOptions: [NSPasteboard.ReadingOptionKey: Any],
                                using block: (NSDraggingItem, Int, UnsafeMutablePointer<ObjCBool>) -> Void) {}
}
