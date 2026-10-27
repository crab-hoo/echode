import AppKit
import Carbon.HIToolbox
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct NoteVelocityTests {
    @Test func contractOptionDragChangesVelocityWithoutMovingTheNote() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-note-velocity-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        let track = try #require(controller.selection), document = controller.documentID
        let note = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        let undo = controller.undoCount
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        func present() throws {
            let snapshot = try #require(controller.snapshot)
            view.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: false)
            let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
            view.present(PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: note.id,
                camera: view.camera, key: DisplayKey(), preview: nil)))
        }
        try present()
        view.onSelect = { controller.noteFocus = $0 }
        var geometryEdits = 0
        var writes: [Task<Bool, Never>] = []
        view.onEdit = { _, _ in geometryEdits += 1 }
        view.onVelocity = { id, value in writes.append(Task { await controller.setNoteVelocity(id, velocity: value, documentID: document) }) }
        let point = CGPoint(x: view.pianoLayout.gridOrigin.x + 1.5 * view.camera.pixelsPerBeat,
                            y: view.pianoLayout.gridOrigin.y + (view.camera.topPitch - 60 + 0.5) * view.camera.pixelsPerPitch)
        // Option edits and lane edits share the same preview value in one frame.
        try send(.leftMouseDown, to: view, at: point, flags: .option)
        try send(.leftMouseDragged, to: view, at: CGPoint(x: point.x + 40, y: point.y - 20), flags: .option)
        #expect(writes.isEmpty && controller.undoCount == undo)
        #expect(view.velocityPreview?.value == 120)
        let snapshot = try #require(controller.snapshot)
        let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
        let preview = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: note.id,
            camera: view.camera, key: DisplayKey(), preview: nil, velocity: view.velocityPreview))
        #expect(preview.velocityBars.first { $0.note.id == note.id }?.cap.midY != PianoFrame(input: input,
            piano: PianoRenderInput(track: track, focus: note.id, camera: view.camera, key: DisplayKey(), preview: nil))
            .velocityBars.first { $0.note.id == note.id }?.cap.midY)
        #expect(preview.velocity(for: note) == 120 && preview.labels.contains { $0.text.hasSuffix("120") })
        #expect(preview.gridInstances.map(\.rect) != PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: note.id,
            camera: view.camera, key: DisplayKey(), preview: nil)).gridInstances.map(\.rect))
        // Release with different coordinates and without Option: the pressed mode stays locked.
        let end = CGPoint(x: point.x + 50, y: point.y - 15)
        try send(.leftMouseUp, to: view, at: end)
        try send(.leftMouseUp, to: view, at: end)
        #expect(geometryEdits == 0, "Option drag must never send geometry edits")
        #expect(writes.count == 1, "one velocity drag must commit exactly once on release")
        for write in writes { #expect(await write.value) }
        let edited = try #require(controller.snapshot?.pianoNote(note.id))
        #expect(edited.velocity >> 9 == 115, "velocity must use the release position, not the last preview")
        #expect(NoteGeometry(edited) == NoteGeometry(note) && edited.id == note.id)
        #expect(controller.undoCount == undo + 1 && controller.noteVelocity.value == 115)
        #expect(controller.noteInput.length == note.length)
        await controller.undo()
        #expect(controller.snapshot?.pianoNote(note.id)?.velocity == note.velocity)
        await controller.redo()
        #expect(controller.snapshot?.pianoNote(note.id)?.velocity == edited.velocity)
        #expect(!(await controller.setNoteVelocity(note.id, velocity: 115)))
        #expect(!(await controller.setNoteVelocity(note.id, velocity: 128)))
        #expect(!(await controller.setNoteVelocity(note.id, velocity: -1)))
        #expect(!(await controller.setNoteVelocity(note.id, velocity: 80, documentID: UUID())))
        #expect(controller.undoCount == undo + 1 && controller.noteVelocity.value == 115)

        // A click alone and a drag returning to its origin do not add history.
        try present()
        try send(.leftMouseDown, to: view, at: point, flags: .option)
        try send(.leftMouseUp, to: view, at: point, flags: .option)
        try send(.leftMouseDown, to: view, at: point, flags: .option)
        try send(.leftMouseDragged, to: view, at: end, flags: .option)
        try send(.leftMouseUp, to: view, at: point)
        #expect(writes.count == 1 && view.velocityPreview == nil)

        await controller.addTrack(name: "melody")
        let second = try #require(controller.selection)
        #expect(await controller.insertNote(pitch: 67, beat: 2, length: 0.5))
        let inserted = try #require(controller.snapshot?.pianoNeighbor(track: second, current: nil))
        #expect(inserted.velocity == edited.velocity, "new notes must inherit velocity across tracks")
        controller.selection = track
        #expect(await controller.insertNote(pitch: 64, beat: 3, length: 0.5))
        let other = try #require(controller.snapshot?.pianoNeighbor(track: track, current: note.id))
        #expect(await controller.setNoteVelocity(other.id, velocity: 42))
        controller.noteFocus = note.id
        #expect(controller.noteVelocity.value == 115, "single selection must remember velocity without editing")
        let beforeSelection = controller.undoCount
        controller.noteFocus = other.id
        #expect(controller.noteVelocity.value == 42 && controller.undoCount == beforeSelection)
        await controller.close()
        await controller.open(url)
        #expect(controller.noteVelocity.value == 100, "reopening resets ephemeral input velocity")
        #expect(controller.snapshot?.pianoNote(note.id)?.velocity == edited.velocity)
        #expect(controller.snapshot?.pianoNote(inserted.id)?.velocity == inserted.velocity)
        await controller.close()
    }

    @Test func cancelledVelocityDragsNeverCommit() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-velocity-cancel-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        let snapshot = try #require(controller.snapshot), track = try #require(controller.selection)
        let note = try #require(snapshot.pianoNeighbor(track: track, current: nil))
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        var commits = 0
        view.onVelocity = { _, _ in commits += 1 }
        for mode in 0..<9 {
            view.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: false)
            let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: view.bounds.width, height: view.bounds.height, scale: 1)
            view.present(PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: note.id,
                camera: view.camera, key: DisplayKey(), preview: nil)))
            let point = CGPoint(x: view.pianoLayout.gridOrigin.x + 1.5 * view.camera.pixelsPerBeat,
                                y: view.pianoLayout.gridOrigin.y + (view.camera.topPitch - 60 + 0.5) * view.camera.pixelsPerPitch)
            try send(.leftMouseDown, to: view, at: point, flags: .option)
            let end = CGPoint(x: point.x, y: point.y - 10)
            try send(.leftMouseDragged, to: view, at: end, flags: .option)
            #expect(view.velocityPreview != nil, "velocity press must start before cancellation: \(mode)")
            switch mode {
            case 0:
                let escape = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                    windowNumber: 0, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}",
                    isARepeat: false, keyCode: UInt16(kVK_Escape)))
                view.keyDown(with: escape)
            case 1: _ = view.resignFirstResponder()
            case 2: view.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: true)
            case 3: view.setFrameSize(NSSize(width: 640 + Double(mode), height: 400))
            case 4: view.setSnapshot(snapshot, track: UUID(), focus: nil, key: DisplayKey(), busy: false)
            case 5:
                await controller.addTrack(name: "new snapshot")
                view.setSnapshot(try #require(controller.snapshot), track: track, focus: note.id, key: DisplayKey(), busy: false)
            case 6:
                let scroll = try #require(CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: 1, wheel2: 0, wheel3: 0))
                view.scrollWheel(with: try #require(NSEvent(cgEvent: scroll)))
            case 7: view.mouseCancelled(with: try #require(NSEvent.mouseEvent(with: .leftMouseUp, location: .zero,
                modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 0)))
            default: view.stop()
            }
            try send(.leftMouseUp, to: view, at: end)
            #expect(commits == 0 && view.velocityPreview == nil, "cancelled velocity gestures must not commit: \(mode)")
        }
        await controller.close()
    }

    @Test func relativeVelocityClampsAndLateResponsesRespectNewerSelections() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-velocity-bounds-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 0, length: 1))
        let track = try #require(controller.selection)
        let id = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil)?.id)
        await controller.close()
        let gesture = PianoVelocityGesture(id: id, original: 100, y: 10)
        #expect(gesture.preview(y: -1000)?.value == 127)
        #expect(gesture.preview(y: 1000)?.value == 0)
        #expect(gesture.preview(y: 10)?.value == 100)
        #expect(gesture.preview(y: .nan) == nil && gesture.preview(y: .infinity) == nil)
        var input = NoteInputVelocity()
        let generation = input.generation
        input.remember(40)
        input.finish(80, generation: generation)
        #expect(input.value == 40)
        input.finish(81, generation: input.generation)
        #expect(input.value == 81)
        input.remember(-1); input.remember(128)
        #expect(input.value == 81)
    }

    private func send(_ type: NSEvent.EventType, to view: PianoGridView, at point: CGPoint,
                      flags: NSEvent.ModifierFlags = []) throws {
        let event = try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: flags,
            timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        switch type {
        case .leftMouseDown: view.mouseDown(with: event)
        case .leftMouseDragged: view.mouseDragged(with: event)
        default: view.mouseUp(with: event)
        }
    }
}
