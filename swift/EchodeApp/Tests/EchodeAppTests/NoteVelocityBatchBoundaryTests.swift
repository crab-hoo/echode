import AppKit
import Carbon.HIToolbox
@testable import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct NoteVelocityBatchBoundaryTests {
    @Test func contractLinkedVelocityBatchesValidateAllTargetsAndAccessibility() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-velocity-linked-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "linked keys")
        for pitch in [60, 62] { #expect(await controller.insertNote(pitch: pitch, beat: Double(pitch - 59), length: 1)) }
        let track = try #require(controller.selection), document = try #require(controller.documentID)
        let first = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        let second = try #require(controller.snapshot?.pianoNeighbor(track: track, current: first.id))
        #expect(await controller.setNoteVelocity(first.id, velocity: 20))
        #expect(await controller.setNoteVelocity(second.id, velocity: 80))
        #expect(await controller.edit(.linkClip(source: first.id.placement.uuidString, track: track.uuidString, start: 8 * 705_600)))
        let snapshot = try #require(controller.snapshot)
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 16 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        #expect(notes.count == 4 && Set(notes.map(\.id.note)).count == 2)
        let ids = Set(notes.map(\.id)), originals = Dictionary(uniqueKeysWithValues: notes.map { ($0.id, $0.velocity) })
        controller.selectNotes(ids)
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false, selection: ids)
        let frame = PianoFrame(input: CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1),
            piano: PianoRenderInput(track: track, focus: nil, camera: view.camera, key: DisplayKey(), preview: nil, selection: ids))
        view.present(frame)
        var writes: [Task<Bool, Never>] = []
        view.onSelection = { controller.selectNotes($0) }
        view.onVelocities = { change in writes.append(Task { await controller.setNoteVelocities(change, documentID: document) }) }
        let lane = try #require(view.buildAccessibilityChildren(convert: { $0 }).last as? NSAccessibilityElement)
        let slider = try #require((lane.accessibilityChildren() as? [PianoAccessibleNote])?.first {
            $0.accessibilityIdentifier()?.hasSuffix(first.id.note.uuidString) == true
        })
        let undo = controller.undoCount
        let once = PianoVelocityChange(anchor: first.id, originals: originals, delta: -1)
        #expect(once.actions(in: snapshot)?.count == 2, "linked content must produce one velocity action per note")
        #expect(slider.actions.decrement?() == true)
        try #require(writes.count == 1)
        #expect(await writes[0].value)
        #expect(controller.undoCount == undo + 1 && controller.noteSelection == ids && controller.noteVelocity.value == 19)
        for note in notes {
            let edited = try #require(controller.snapshot?.pianoNote(note.id))
            #expect(Int(edited.velocity >> 9) == Int(note.velocity >> 9) - 1 && NoteGeometry(edited) == NoteGeometry(note))
        }
        // Relative bounds include selected notes outside the current frame and apply once to linked content.
        let gesture = PianoVelocityGesture(id: first.id, original: 20, y: 0, originals: originals)
        let low = try #require(gesture.preview(y: 1000)), high = try #require(gesture.preview(y: -1000))
        for note in notes {
            #expect(low.value(for: note) == Int(note.velocity >> 9) - 20)
            #expect(high.value(for: note) == Int(note.velocity >> 9) + 47)
        }
        let extremes = originals.mapValues { $0 >> 9 == 20 ? UInt32(0) : UInt32(UInt16.max) }
        let pinned = PianoVelocityGesture(id: first.id, original: 0, y: 0, originals: extremes)
        #expect(pinned.preview(y: -1000)?.value == 0 && pinned.preview(y: 1000)?.value == 0,
                "a selection spanning both velocity endpoints cannot move without losing its differences")
        #expect(gesture.preview(y: .nan) == nil && gesture.preview(y: .infinity) == nil)
        let stale = PianoVelocityChange(anchor: first.id, originals: originals, delta: 1)
        #expect(!(await controller.setNoteVelocities(stale)), "one changed original must reject the entire velocity transaction")
        await controller.undo()
        #expect(notes.allSatisfy { controller.snapshot?.pianoNote($0.id)?.velocity == $0.velocity })
        await controller.redo()
        let currentSnapshot = try #require(controller.snapshot)
        let current = Dictionary(uniqueKeysWithValues: notes.compactMap { note in
            currentSnapshot.pianoNote(note.id).map { (note.id, $0.velocity) }
        })
        var oneStale = current
        oneStale[first.id] = originals[first.id]
        #expect(!(await controller.setNoteVelocities(PianoVelocityChange(anchor: first.id, originals: oneStale, delta: 1))))
        let missing = PianoNote.ID(track: track, placement: first.id.placement, note: UUID())
        var invalid = current
        invalid[missing] = first.velocity
        #expect(!(await controller.setNoteVelocities(PianoVelocityChange(anchor: first.id, originals: invalid, delta: 1))))
        #expect(!(await controller.setNoteVelocities(PianoVelocityChange(anchor: first.id, originals: current, delta: 0))))
        #expect(!(await controller.setNoteVelocities(PianoVelocityChange(anchor: first.id, originals: current, delta: 128))))
        #expect(!(await controller.setNoteVelocities(PianoVelocityChange(anchor: first.id, originals: current, delta: -20))))
        #expect(!(await controller.setNoteVelocities(PianoVelocityChange(anchor: first.id, originals: current, delta: 1), documentID: DocumentID())))
        #expect(controller.undoCount == undo + 1 && controller.noteVelocity.value == 19)
        // Changing to a multi-selection invalidates an earlier pending entry-value response without choosing a representative.
        controller.noteFocus = first.id
        let generation = controller.noteVelocity.generation
        controller.selectNotes(ids)
        controller.noteVelocity.finish(99, generation: generation)
        #expect(controller.noteVelocity.value == 19)
        await controller.close()
        await controller.open(url)
        #expect(!(await controller.setNoteVelocities(PianoVelocityChange(anchor: first.id, originals: current, delta: 1), documentID: document)))
        #expect(notes.allSatisfy { Int(controller.snapshot?.pianoNote($0.id)?.velocity ?? 0) >> 9 == Int($0.velocity >> 9) - 1 })
        await controller.close()
    }

    @Test func contractBatchVelocityGesturesCancelOnSelectionChanges() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-velocity-group-cancel-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        #expect(await controller.insertNote(pitch: 62, beat: 2, length: 1))
        let snapshot = try #require(controller.snapshot), track = try #require(controller.selection)
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 8 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        let ids = Set(notes.map(\.id)), first = try #require(notes.first)
        for lane in [false, true] {
            for mode in 0..<7 {
                let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
                defer { view.stop() }
                view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false, selection: ids)
                let frame = PianoFrame(input: CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1),
                    piano: PianoRenderInput(track: track, focus: nil, camera: view.camera, key: DisplayKey(), preview: nil, selection: ids))
                view.present(frame)
                var writes = 0
                view.onVelocity = { _, _ in writes += 1 }
                view.onVelocities = { _ in writes += 1 }
                let bar = try #require(frame.velocityBars.first { $0.note.id == first.id })
                let point = lane ? CGPoint(x: bar.cap.midX, y: bar.cap.midY) : CGPoint(
                    x: view.pianoLayout.gridOrigin.x + (Double(first.start) / 705_600 + 0.5) * view.camera.pixelsPerBeat,
                    y: view.pianoLayout.gridOrigin.y + (view.camera.topPitch - Double(first.pitch) + 0.5) * view.camera.pixelsPerPitch)
                let end = CGPoint(x: point.x, y: point.y - 5)
                try send(.leftMouseDown, to: view, at: point, flags: lane ? [] : .option)
                try send(.leftMouseDragged, to: view, at: end)
                #expect(view.velocityPreview != nil && view.selectedNotes == ids)
                switch mode {
                case 0: view.setSnapshot(snapshot, track: track, focus: first.id, key: DisplayKey(), busy: false, selection: [first.id])
                case 1: view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false, selection: [])
                case 2:
                    let event = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                        windowNumber: 0, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: UInt16(kVK_Escape)))
                    view.keyDown(with: event)
                case 3: _ = view.resignFirstResponder()
                case 4: view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: true, selection: ids)
                case 5: view.setFrameSize(NSSize(width: 650, height: 400))
                default: view.stop()
                }
                try send(.leftMouseUp, to: view, at: end)
                #expect(writes == 0 && view.velocityPreview == nil, "cancelled group velocity gestures must not commit")
            }
        }
        await controller.close()
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
