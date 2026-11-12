import AppKit
import EchodeBridge
import Testing

@testable import EchodeAppCore

@MainActor
struct NoteVelocityBatchTests {
    @Test func contractSelectedVelocityDragsPreserveDifferencesAndHistory() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-velocity-batch-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        for (pitch, beat) in [(60, 1.0), (62, 2.0), (64, 3.0)] {
            #expect(await controller.insertNote(pitch: pitch, beat: beat, length: 1))
        }
        let track = try #require(controller.selection)
        let initialNotes = try #require(controller.snapshot).pianoNotes(
            track: track, originTick: 0, widthTicks: 8 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1
        ).sorted { $0.pitch < $1.pitch }
        for (note, value) in zip(initialNotes, [30, 100, 70]) where Int(note.velocity >> 9) != value {
            #expect(await controller.setNoteVelocity(note.id, velocity: value))
        }
        let notes = initialNotes.compactMap { controller.snapshot?.pianoNote($0.id) }
        let selected = Set(notes.prefix(2).map(\.id))
        controller.selectNotes(selected)
        let snapshot = try #require(controller.snapshot)
        let document = try #require(controller.documentID)
        let originals = selected.map { snapshot.pianoNote($0)! }
        let undo = controller.undoCount
        controller.noteInput.remember(12_345)
        let inputVelocity = controller.noteVelocity.value
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        var writes: [Task<Bool, Never>] = []
        var singles = 0
        var geometryEdits = 0
        view.onSelection = { controller.selectNotes($0) }
        view.onEdit = { _, _ in geometryEdits += 1 }
        view.onVelocity = { _, _ in singles += 1 }
        view.onVelocities = { change in
            writes.append(Task { await controller.setNoteVelocities(change, documentID: document) })
        }
        func frame(_ preview: PianoVelocityPreview? = nil) throws -> PianoFrame {
            let current = try #require(controller.snapshot)
            return PianoFrame(
                input: CanvasInput(
                    snapshot: current, selected: track, camera: ArrangeCamera(),
                    width: 640, height: 400, scale: 1),
                piano: PianoRenderInput(
                    track: track, focus: nil,
                    camera: view.camera, key: DisplayKey(), preview: nil, velocity: preview, selection: selected))
        }
        func present() throws {
            view.setSnapshot(
                try #require(controller.snapshot), track: track, focus: nil, key: DisplayKey(), busy: false,
                selection: selected)
            view.present(try frame())
        }
        try present()
        let point = CGPoint(
            x: view.pianoLayout.gridOrigin.x + 1.5 * view.camera.pixelsPerBeat,
            y: view.pianoLayout.gridOrigin.y + (view.camera.topPitch - 60 + 0.5) * view.camera.pixelsPerPitch)
        try send(.leftMouseDown, to: view, at: point, flags: .option)
        try send(.leftMouseDragged, to: view, at: CGPoint(x: point.x + 20, y: point.y - 10), flags: .option)
        #expect(
            view.selectedNotes == selected && controller.noteSelection == selected,
            "velocity drag on a selected note must preserve the entire selection")
        #expect(writes.isEmpty && controller.undoCount == undo && controller.noteVelocity.value == inputVelocity)
        let preview = try frame(view.velocityPreview)
        let before = try frame()
        for (note, value) in zip(notes, [40, 110, 70]) {
            #expect(preview.velocity(for: note) == value, "every selected note must preview the same velocity delta")
        }
        for note in originals {
            #expect(
                preview.velocityBars.first { $0.note.id == note.id }?.cap.midY
                    != before.velocityBars.first { $0.note.id == note.id }?.cap.midY)
        }
        view.present(preview)
        let lane = try #require(view.buildAccessibilityChildren(convert: { $0 }).last as? NSAccessibilityElement)
        let sliders = try #require(lane.accessibilityChildren() as? [PianoAccessibleNote])
        #expect(sliders.compactMap { $0.accessibilityValue() as? Int }.sorted() == [40, 70, 110])
        // Recompute from release coordinates; neither Option nor the last preview is required.
        let end = CGPoint(x: point.x + 50, y: point.y - 15)
        try send(.leftMouseUp, to: view, at: end)
        try send(.leftMouseUp, to: view, at: end)
        try #require(writes.count == 1 && singles == 0 && geometryEdits == 0)
        #expect(await writes[0].value)
        for (note, value) in zip(notes, [45, 115, 70]) {
            let changed = try #require(controller.snapshot?.pianoNote(note.id))
            #expect(changed.velocity >> 9 == value && NoteGeometry(changed) == NoteGeometry(note))
        }
        #expect(
            controller.undoCount == undo + 1 && controller.noteVelocity.value == 45
                && controller.noteInput.length == 12_345)
        #expect(controller.noteSelection == selected && controller.noteFocus == nil)
        await controller.undo()
        #expect(originals.allSatisfy { controller.snapshot?.pianoNote($0.id)?.velocity == $0.velocity })
        #expect(controller.noteVelocity.value == 45 && controller.noteInput.length == 12_345)
        await controller.redo()
        #expect(controller.snapshot?.pianoNote(notes[1].id).map { Int($0.velocity >> 9) } == 115)

        // The lane uses the same group, relative delta and shared range limit.
        try present()
        let laneFrame = try frame()
        let bar = try #require(laneFrame.velocityBars.first { $0.note.id == notes[0].id })
        let cap = CGPoint(x: bar.cap.midX, y: bar.cap.midY)
        let step = laneFrame.velocityLane.span / 127
        try send(.leftMouseDown, to: view, at: cap)
        try send(.leftMouseDragged, to: view, at: CGPoint(x: cap.x, y: cap.y - 40 * step))
        let limited = try frame(view.velocityPreview)
        #expect(
            limited.velocity(for: notes[0]) == 57 && limited.velocity(for: notes[1]) == 127,
            "the group must stop together at the loudest note's upper bound")
        try send(.leftMouseUp, to: view, at: CGPoint(x: cap.x + 30, y: cap.y - 8 * step))
        try #require(writes.count == 2)
        #expect(await writes[1].value)
        #expect(controller.undoCount == undo + 2 && controller.noteVelocity.value == 53)
        #expect(controller.snapshot?.pianoNote(notes[0].id).map { Int($0.velocity >> 9) } == 53)
        #expect(controller.snapshot?.pianoNote(notes[1].id).map { Int($0.velocity >> 9) } == 123)
        #expect(controller.snapshot?.pianoNote(notes[2].id).map { Int($0.velocity >> 9) } == 70)
        #expect(controller.noteSelection == selected && controller.noteInput.length == 12_345)
        await controller.undo()
        #expect(controller.snapshot?.pianoNote(notes[0].id).map { Int($0.velocity >> 9) } == 45)
        #expect(controller.snapshot?.pianoNote(notes[1].id).map { Int($0.velocity >> 9) } == 115)
        await controller.redo()

        // A click alone or returning to the origin never creates a transaction.
        try present()
        try send(.leftMouseDown, to: view, at: point, flags: .option)
        try send(.leftMouseUp, to: view, at: point)
        try send(.leftMouseDown, to: view, at: point, flags: .option)
        try send(.leftMouseDragged, to: view, at: end)
        try send(.leftMouseUp, to: view, at: point)
        #expect(writes.count == 2 && controller.undoCount == undo + 2)
        await controller.addTrack(name: "melody")
        #expect(await controller.insertNote(pitch: 67, beat: 0, length: 1))
        let otherTrack = try #require(controller.selection)
        #expect(
            controller.snapshot?.pianoNeighbor(track: otherTrack, current: nil).map { Int($0.velocity >> 9) } == 53,
            "new notes must inherit the grabbed note's final group velocity")
        await controller.close()
        await controller.open(url)
        #expect(controller.snapshot?.pianoNote(notes[0].id).map { Int($0.velocity >> 9) } == 53)
        #expect(controller.snapshot?.pianoNote(notes[1].id).map { Int($0.velocity >> 9) } == 123)
        #expect(controller.noteVelocity.value == 100 && controller.noteSelection.isEmpty)
        await controller.close()
    }

    private func send(
        _ type: NSEvent.EventType, to view: PianoGridView, at point: CGPoint,
        flags: NSEvent.ModifierFlags = []
    ) throws {
        let event = try #require(
            NSEvent.mouseEvent(
                with: type, location: view.convert(point, to: nil), modifierFlags: flags,
                timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        switch type {
        case .leftMouseDown: view.mouseDown(with: event)
        case .leftMouseDragged: view.mouseDragged(with: event)
        default: view.mouseUp(with: event)
        }
    }
}
