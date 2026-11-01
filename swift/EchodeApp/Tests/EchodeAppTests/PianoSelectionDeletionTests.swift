import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoSelectionDeletionTests {
    @Test func contractSelectedNotesDeleteAtomicallyAndRoundTripHistory() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-selection-delete-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        #expect(await controller.insertNote(pitch: 62, beat: 2, length: 0.5))
        #expect(await controller.insertNote(pitch: 64, beat: 4, length: 1))
        let track = try #require(controller.selection), document = try #require(controller.documentID)
        let snapshot = try #require(controller.snapshot)
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 8 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        #expect(notes.count == 3)
        let selected = Set(notes.filter { $0.pitch < 64 }.map(\.id))
        let survivor = try #require(notes.first { $0.pitch == 64 })
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        controller.selectNotes(selected)
        view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false, selection: selected)
        view.present(PianoFrame(input: CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(),
            width: 640, height: 400, scale: 1), piano: PianoRenderInput(track: track, focus: nil,
            camera: view.camera, key: DisplayKey(), preview: nil, selection: selected)))
        view.onSelection = { controller.selectNotes($0) }
        var writes: [Task<Bool, Never>] = []
        view.onDeleteNotes = { ids in writes.append(Task { await controller.deleteNotes(ids, documentID: document) }) }
        let menu = try #require(view.noteContextMenu(at: CGPoint(x: 180, y: 114)))
        #expect(view.selectedNotes == selected && controller.noteSelection == selected, "right click on a selected note must keep the group")
        #expect(menu.items.count == 1 && menu.items[0].title == text("Delete Notes"))
        let count = controller.undoCount
        let event = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, characters: "\u{7f}", charactersIgnoringModifiers: "\u{7f}", isARepeat: false, keyCode: 51))
        view.keyDown(with: event)
        #expect(writes.count == 1, "Delete must submit the entire selection as one transaction")
        for write in writes { #expect(await write.value) }
        #expect(controller.undoCount == count + 1 && controller.noteSelection.isEmpty)
        #expect(selected.allSatisfy { controller.snapshot?.pianoNote($0) == nil })
        #expect(controller.snapshot?.pianoNote(survivor.id) != nil)
        await controller.undo()
        #expect(controller.undoCount == count && selected.allSatisfy { controller.snapshot?.pianoNote($0) != nil })
        await controller.redo()
        #expect(selected.allSatisfy { controller.snapshot?.pianoNote($0) == nil })
        let after = controller.undoCount
        #expect(!(await controller.deleteNotes(selected.union([survivor.id]), documentID: document)), "one stale target must reject the entire deletion")
        #expect(controller.undoCount == after && controller.snapshot?.pianoNote(survivor.id) != nil)
        view.setSnapshot(try #require(controller.snapshot), track: track, focus: nil, key: DisplayKey(), busy: false)
        view.deleteFromMenu(menu.items[0])
        #expect(writes.count == 1, "a menu from an older snapshot must not edit the replacement")
        await controller.close()
        await controller.open(url)
        #expect(selected.allSatisfy { controller.snapshot?.pianoNote($0) == nil })
        #expect(controller.snapshot?.pianoNote(survivor.id) != nil && controller.noteSelection.isEmpty)
        #expect(!(await controller.deleteNotes([survivor.id], documentID: document)))
        await controller.close()
    }

    @Test func linkedPlacementsDeleteEachContentNoteOnlyOnce() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-selection-linked-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "linked keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        #expect(await controller.insertNote(pitch: 62, beat: 2, length: 1))
        let track = try #require(controller.selection)
        let first = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        #expect(await controller.edit(.linkClip(source: first.id.placement.uuidString, track: track.uuidString, start: 8 * 705_600)))
        let snapshot = try #require(controller.snapshot)
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 16 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        #expect(notes.count == 4 && Set(notes.map(\.id.note)).count == 2)
        let count = controller.undoCount
        #expect(await controller.deleteNotes(Set(notes.map(\.id))))
        #expect(controller.undoCount == count + 1 && notes.allSatisfy { controller.snapshot?.pianoNote($0.id) == nil })
        await controller.undo()
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id) != nil })
        await controller.close()
    }

    @Test func menusSelectOnlyUnselectedNotesAndRejectInvalidContexts() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-selection-menu-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        #expect(await controller.insertNote(pitch: 62, beat: 2, length: 1))
        let track = try #require(controller.selection), snapshot = try #require(controller.snapshot)
        let first = try #require(snapshot.pianoNeighbor(track: track, current: nil))
        let second = try #require(snapshot.pianoNeighbor(track: track, current: first.id))
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        view.setSnapshot(snapshot, track: track, focus: first.id, key: DisplayKey(), busy: false)
        let frame = PianoFrame(input: CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1),
            piano: PianoRenderInput(track: track, focus: first.id, camera: view.camera, key: DisplayKey(), preview: nil))
        view.present(frame)
        var deleted: [Set<PianoNote.ID>] = []
        view.onDeleteNotes = { deleted.append($0) }
        let menu = try #require(view.noteContextMenu(at: CGPoint(x: 250, y: 74)))
        #expect(view.selectedNotes == [second.id] && menu.items[0].title == text("Delete Note"))
        view.deleteFromMenu(menu.items[0])
        #expect(deleted == [[second.id]])
        #expect(view.noteContextMenu(at: CGPoint(x: 550, y: 200)) == nil && view.selectedNotes.isEmpty)
        #expect(view.noteContextMenu(at: CGPoint(x: 100, y: 10)) == nil)
        #expect(view.noteContextMenu(at: CGPoint(x: 10, y: 100)) == nil)
        #expect(view.noteContextMenu(at: CGPoint(x: 250, y: 370)) == nil)
        view.setSnapshot(snapshot, track: track, focus: first.id, key: DisplayKey(), busy: true)
        #expect(view.noteContextMenu(at: CGPoint(x: 180, y: 114)) == nil)
        view.deleteFromMenu(menu.items[0])
        #expect(deleted.count == 1)
        view.setSnapshot(snapshot, track: UUID(), focus: nil, key: DisplayKey(), busy: false)
        view.present(frame) // A delayed frame from the old track must not restore hit testing.
        #expect(view.noteContextMenu(at: CGPoint(x: 180, y: 114)) == nil)
        view.deleteFromMenu(menu.items[0])
        #expect(deleted.count == 1)
        await controller.close()
    }
}
