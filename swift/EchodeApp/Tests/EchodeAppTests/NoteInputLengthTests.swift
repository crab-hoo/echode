import AppKit
import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

@MainActor
struct NoteInputLengthTests {
    @Test func contractSingleClickInsertsOnceAndKeepsThePressedPosition() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-note-click-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let track = try #require(controller.selection)
        let snapshot = try #require(controller.snapshot)
        let document = try #require(controller.documentID)
        let undo = controller.undoCount
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false)
        let input = CanvasInput(
            snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
        view.present(
            PianoFrame(
                input: input,
                piano: PianoRenderInput(
                    track: track, focus: nil,
                    camera: view.camera, key: DisplayKey(), preview: nil)))
        var writes: [Task<Bool, Never>] = []
        view.onInsert = { pitch, beat, length in
            writes.append(
                Task {
                    await controller.insertNote(
                        pitch: pitch, beat: beat, length: length, on: track, documentID: document)
                })
        }
        let origin = view.pianoLayout.gridOrigin
        let down = CGPoint(
            x: origin.x + 1.1 * view.camera.pixelsPerBeat,
            y: origin.y + (view.camera.topPitch - 60 + 0.5) * view.camera.pixelsPerPitch)
        try send(.leftMouseDown, to: view, at: down, flags: .shift)
        #expect(writes.isEmpty && controller.undoCount == undo)
        let up = CGPoint(x: down.x + 1, y: down.y + 1)
        try send(.leftMouseUp, to: view, at: up)
        try send(.leftMouseUp, to: view, at: up)
        #expect(writes.count == 1, "a single blank-grid click must insert exactly one note on release")
        for write in writes { #expect(await write.value) }
        let note = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        #expect(note.start == Tick.ticks(1.1) && note.length == view.camera.grid && note.pitch == 60)
        #expect(controller.undoCount == undo + 1)
        await controller.undo()
        #expect(controller.snapshot?.pianoNeighbor(track: track, current: nil) == nil)
        #expect(controller.noteInput.length == nil, "Undo of the first insertion restores the grid fallback")
        await controller.redo()
        await controller.close()
        await controller.open(url)
        #expect(
            controller.snapshot?.pianoNeighbor(track: track, current: nil).map(NoteGeometry.init) == NoteGeometry(note))
        await controller.close()
    }

    @Test func contractInputLengthFollowsSelectionResizeAndHistoryAcrossTracks() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-note-length-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        #expect(controller.noteInput.length == nil)
        await controller.addTrack(name: "chords")
        let first = try #require(controller.selection)
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 0.7))
        let long = try #require(controller.snapshot?.pianoNeighbor(track: first, current: nil))
        #expect(await controller.insertNote(pitch: 64, beat: 2, length: 0.1))
        await controller.addTrack(name: "melody")
        let second = try #require(controller.selection)
        let document = try #require(controller.documentID)
        controller.selection = first
        let snapshot = try #require(controller.snapshot)
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        view.inputLength = { controller.noteInput.length }
        view.onSelect = { controller.noteFocus = $0 }
        func present(on track: UUID) throws {
            let snapshot = try #require(controller.snapshot)
            view.setSnapshot(snapshot, track: track, focus: controller.noteFocus, key: DisplayKey(), busy: false)
            let input = CanvasInput(
                snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
            view.present(
                PianoFrame(
                    input: input,
                    piano: PianoRenderInput(
                        track: track, focus: controller.noteFocus,
                        camera: view.camera, key: DisplayKey(), preview: nil)))
        }
        try present(on: first)
        let origin = view.pianoLayout.gridOrigin
        let point = CGPoint(
            x: origin.x + 1.35 * view.camera.pixelsPerBeat,
            y: origin.y + (view.camera.topPitch - 60 + 0.5) * view.camera.pixelsPerPitch)
        let beforeSelection = controller.undoCount
        try send(.leftMouseDown, to: view, at: point)
        try send(.leftMouseUp, to: view, at: point)
        #expect(controller.noteFocus == long.id && controller.noteInput.length == long.length)
        #expect(controller.undoCount == beforeSelection, "selection remembers length without a model edit")
        let resized = NoteGeometry(start: long.start, length: Tick.ticks(0.83)!, pitch: long.pitch)
        #expect(await controller.editNote(long.id, geometry: resized))
        #expect(controller.noteInput.length == resized.length, "a successful resize must remember its exact length")
        #expect(!(await controller.editNote(long.id, geometry: resized)))
        #expect(!(await controller.insertNote(pitch: 60, beat: 3, length: 1, documentID: DocumentID())))
        #expect(controller.noteInput.length == resized.length)
        await controller.undo()
        #expect(controller.noteInput.length == long.length, "Undo must restore input length with the note edit")
        await controller.redo()
        #expect(controller.noteInput.length == resized.length)
        #expect(await controller.setMixer(first, field: .gain, value: 0.5, documentID: document))
        let short = try #require(snapshot.pianoNeighbor(track: first, current: long.id))
        controller.noteFocus = short.id
        #expect(controller.noteInput.length == short.length)
        await controller.undo()
        #expect(
            controller.noteInput.length == short.length, "Undo of an unrelated edit must preserve a newer selection")
        await controller.undo()
        #expect(controller.noteInput.length == long.length)
        await controller.redo()
        #expect(controller.noteInput.length == resized.length)

        controller.selection = second
        try present(on: second)
        var writes: [Task<Bool, Never>] = []
        view.onInsert = { pitch, beat, length in
            writes.append(
                Task {
                    await controller.insertNote(
                        pitch: pitch, beat: beat, length: length, on: second, documentID: document)
                })
        }
        try send(.leftMouseDown, to: view, at: point)
        try send(.leftMouseUp, to: view, at: point)
        // Repeating the second half of a double click before a new frame arrives
        // must not add another note from the stale blank frame.
        try send(.leftMouseDown, to: view, at: point, clicks: 2)
        try send(.leftMouseUp, to: view, at: point, clicks: 2)
        #expect(writes.count == 1)
        for write in writes { #expect(await write.value) }
        let inserted = try #require(controller.snapshot?.pianoNeighbor(track: second, current: nil))
        #expect(inserted.length == resized.length, "click input must share exact unquantized lengths across tracks")
        #expect(controller.redoCount == 0)
        await controller.undo()
        #expect(controller.snapshot?.pianoNeighbor(track: second, current: nil) == nil)
        #expect(controller.noteInput.length == resized.length)
        await controller.redo()
        await controller.close()
        await controller.open(url)
        #expect(controller.noteInput.length == nil, "reopening resets input length to the current grid fallback")
        #expect(controller.snapshot?.pianoNeighbor(track: second, current: nil)?.length == resized.length)
        await controller.close()
    }

    private func send(
        _ type: NSEvent.EventType, to view: PianoGridView, at point: CGPoint,
        flags: NSEvent.ModifierFlags = [], clicks: Int = 1
    ) throws {
        let event = try #require(
            NSEvent.mouseEvent(
                with: type, location: view.convert(point, to: nil), modifierFlags: flags,
                timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: clicks, pressure: 1))
        if type == .leftMouseDown { view.mouseDown(with: event) } else { view.mouseUp(with: event) }
    }
}
