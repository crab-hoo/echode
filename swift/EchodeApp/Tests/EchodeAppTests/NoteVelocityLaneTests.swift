import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct NoteVelocityLaneTests {
    @Test func contractLaneDragSharesVelocityHistoryAndPreservesGeometry() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-velocity-lane-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        let track = try #require(controller.selection), document = try #require(controller.documentID)
        let note = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        let undo = controller.undoCount
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        let snapshot = try #require(controller.snapshot)
        view.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: false)
        let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
        view.present(PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: note.id,
            camera: view.camera, key: DisplayKey(), preview: nil)))
        view.onSelect = { controller.noteFocus = $0 }
        var writes: [Task<Bool, Never>] = []
        var insertions = 0, geometryEdits = 0
        view.onInsert = { _, _, _ in insertions += 1 }
        view.onEdit = { _, _ in geometryEdits += 1 }
        view.onVelocity = { id, value in writes.append(Task { await controller.setNoteVelocity(id, velocity: value, documentID: document) }) }
        // The lower 72pt lane has 8pt margins and a full 0...127 range.
        let point = CGPoint(x: view.pianoLayout.gridOrigin.x + view.camera.pixelsPerBeat + 2,
                            y: 400 - 8 - 56 * 100 / 127.0)
        try send(.leftMouseDown, to: view, at: point)
        try send(.leftMouseUp, to: view, at: point)
        #expect(writes.isEmpty && controller.undoCount == undo, "a lane click must select without changing the value")
        try send(.leftMouseDown, to: view, at: point)
        try send(.leftMouseDragged, to: view, at: CGPoint(x: point.x + 60, y: point.y - 10 * 56 / 127.0))
        #expect(view.velocityPreview?.value == 110, "lane drag must preview velocity rather than insert a note")
        #expect(writes.isEmpty && controller.undoCount == undo)
        let end = CGPoint(x: point.x + 70, y: point.y - 20 * 56 / 127.0)
        try send(.leftMouseUp, to: view, at: end)
        try send(.leftMouseUp, to: view, at: end)
        #expect(writes.count == 1, "lane drag must commit one velocity edit on release")
        #expect(insertions == 0 && geometryEdits == 0)
        for write in writes { #expect(await write.value) }
        let edited = try #require(controller.snapshot?.pianoNote(note.id))
        #expect(edited.velocity >> 9 == 120 && NoteGeometry(edited) == NoteGeometry(note))
        #expect(controller.undoCount == undo + 1 && controller.noteVelocity.value == 120)
        await controller.undo()
        #expect(controller.snapshot?.pianoNote(note.id)?.velocity == note.velocity)
        await controller.redo()
        #expect(controller.snapshot?.pianoNote(note.id)?.velocity == edited.velocity)
        await controller.close()
        await controller.open(url)
        #expect(controller.snapshot?.pianoNote(note.id)?.velocity == edited.velocity)
        await controller.close()
    }

    @Test func contractLaneShowsAllPitchesAndSynchronizesPreviewAndAccessibility() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-velocity-lane-frame-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let track = try #require(controller.selection)
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        let first = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        #expect(await controller.setNoteVelocity(first.id, velocity: 0))
        #expect(await controller.insertNote(pitch: 127, beat: 1, length: 1))
        let high = try #require(controller.snapshot?.pianoNeighbor(track: track, current: first.id))
        #expect(await controller.setNoteVelocity(high.id, velocity: 127))
        #expect(await controller.insertNote(pitch: 0, beat: 3, length: 1))
        let low = try #require(controller.snapshot?.pianoNeighbor(track: track, current: high.id))
        let snapshot = try #require(controller.snapshot)
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        view.setSnapshot(snapshot, track: track, focus: first.id, key: DisplayKey(), busy: false)
        let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 2)
        func frame(_ preview: PianoVelocityPreview? = nil, camera: PianoCamera? = nil) -> PianoFrame {
            PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: first.id,
                camera: camera ?? view.camera, key: DisplayKey(), preview: nil, velocity: preview))
        }
        let before = frame()
        #expect(before.notes.map(\.id) == [first.id])
        #expect(Set(before.velocityBars.map(\.note.id)) == Set([first.id, high.id, low.id]), "lane must include pitches outside the note grid")
        let zero = try #require(before.velocityBars.first { $0.note.id == first.id })
        let top = try #require(before.velocityBars.first { $0.note.id == high.id })
        #expect(before.velocityLane.hit(at: CGPoint(x: zero.stem.minX, y: zero.cap.midY), bars: before.velocityBars, focus: high.id)?.id == first.id)
        #expect(before.velocityLane.hit(at: CGPoint(x: top.stem.minX, y: top.cap.midY), bars: before.velocityBars, focus: first.id)?.id == high.id)
        let changed = frame(PianoVelocityPreview(id: first.id, value: 64))
        #expect(changed.velocityInstances.map(\.rect) != before.velocityInstances.map(\.rect), "lane must draw the same velocity preview as the note")
        #expect(changed.gridInstances.map(\.rect) != before.gridInstances.map(\.rect))
        let heading = try #require(changed.labels.first { $0.text == "64" })
        let headingY = Double(heading.clip.y), laneY = Double(changed.layout.velocity.minY)
        #expect(headingY == laneY, "heading difference \(headingY - laneY)")
        #expect(changed.layout.velocity.minY == changed.layout.grid.maxY && changed.layout.velocity.maxY == 400)
        let scrolled = frame(camera: PianoCamera(originBeat: 1.5))
        #expect(!scrolled.velocityBars.contains { $0.note.id == first.id || $0.note.id == high.id }, "sustained notes must not gain a false onset at the left edge")
        view.present(changed)
        let group = try #require(view.buildAccessibilityChildren(convert: { $0 }).last as? NSAccessibilityElement)
        let sliders = try #require(group.accessibilityChildren() as? [PianoAccessibleNote])
        #expect(sliders.count == 3 && sliders.allSatisfy { $0.accessibilityRole() == .slider })
        let slider = try #require(sliders.first { $0.accessibilityIdentifier()?.hasSuffix("/\(first.id.note)") == true })
        #expect(slider.accessibilityValue() as? Int == 64)
        let changedBar = try #require(changed.velocityBars.first { $0.note.id == first.id })
        #expect(slider.accessibilityFrame() == changedBar.stem.union(changedBar.cap).intersection(changed.layout.velocity))
        var writes: [(PianoNote.ID, Int)] = []
        view.onVelocity = { writes.append(($0, $1)) }
        #expect(slider.actions.increment?() == true)
        #expect(writes.count == 1 && writes.first?.0 == first.id && writes.first?.1 == 65)
        view.setSnapshot(snapshot, track: track, focus: first.id, key: DisplayKey(), busy: true)
        #expect(slider.actions.decrement?() == false)
        view.setSnapshot(snapshot, track: UUID(), focus: nil, key: DisplayKey(), busy: false)
        #expect(slider.actions.increment?() == false)
        #expect(writes.count == 1)
        await controller.close()
    }

    @Test func laneGesturesCancelAndBlankClicksNeverInsert() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-velocity-lane-cancel-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        let track = try #require(controller.selection), snapshot = try #require(controller.snapshot)
        let note = try #require(snapshot.pianoNeighbor(track: track, current: nil))
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        var writes = 0, insertions = 0
        view.onVelocity = { _, _ in writes += 1 }
        view.onInsert = { _, _, _ in insertions += 1 }
        for mode in 0..<8 {
            view.setFrameSize(NSSize(width: 640, height: 400))
            view.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: false)
            let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
            let frame = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: note.id, camera: view.camera, key: DisplayKey(), preview: nil))
            view.present(frame)
            let bar = try #require(frame.velocityBars.first)
            let point = CGPoint(x: bar.stem.minX, y: bar.cap.midY)
            let end = CGPoint(x: point.x, y: point.y + 8)
            try send(.leftMouseDown, to: view, at: point)
            try send(.leftMouseDragged, to: view, at: end)
            #expect(view.velocityPreview != nil)
            switch mode {
            case 0: _ = view.resignFirstResponder()
            case 1: view.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: true)
            case 2: view.setFrameSize(NSSize(width: 600, height: 300))
            case 3: view.setSnapshot(snapshot, track: UUID(), focus: nil, key: DisplayKey(), busy: false)
            case 4: view.setNavigation(CanvasNavigationState(), documentID: DocumentID())
            case 5: view.stop()
            case 6:
                let escape = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                    windowNumber: 0, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53))
                view.keyDown(with: escape)
            default:
                let cancel = try #require(NSEvent.mouseEvent(with: .leftMouseUp, location: .zero, modifierFlags: [], timestamp: 0,
                    windowNumber: 0, context: nil, eventNumber: 0, clickCount: 1, pressure: 0))
                view.mouseCancelled(with: cancel)
            }
            try send(.leftMouseUp, to: view, at: end)
            #expect(writes == 0 && view.velocityPreview == nil, "cancelled lane gesture must not commit")
            view.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: false)
            view.present(frame)
            let blank = CGPoint(x: 550, y: frame.layout.velocity.midY)
            try send(.leftMouseDown, to: view, at: blank)
            try send(.leftMouseUp, to: view, at: blank)
            #expect(insertions == 0 && writes == 0)
        }
        await controller.close()
    }

    private func send(_ type: NSEvent.EventType, to view: PianoGridView, at point: CGPoint) throws {
        let event = try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: [],
            timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        switch type {
        case .leftMouseDown: view.mouseDown(with: event)
        case .leftMouseDragged: view.mouseDragged(with: event)
        default: view.mouseUp(with: event)
        }
    }
}
