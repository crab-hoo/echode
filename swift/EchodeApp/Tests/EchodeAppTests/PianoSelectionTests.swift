import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoSelectionTests {
    @Test func contractRightDragSelectsIntersectingNotes() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-note-selection-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        #expect(await controller.insertNote(pitch: 62, beat: 2, length: 1))
        #expect(await controller.insertNote(pitch: 64, beat: 4, length: 1))
        let track = try #require(controller.selection), snapshot = try #require(controller.snapshot)
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false)
        let frame = PianoFrame(input: CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(),
            width: 640, height: 400, scale: 1), piano: PianoRenderInput(track: track, focus: nil,
            camera: view.camera, key: DisplayKey(), preview: nil))
        view.present(frame)
        view.onSelection = { controller.selectNotes($0) }
        controller.noteInput.remember(12_345)
        controller.noteVelocity.remember(37)
        var inserts = 0, edits = 0
        view.onInsert = { _, _, _ in inserts += 1 }
        view.onEdit = { _, _ in edits += 1 }
        let undo = controller.undoCount
        // Touch just the ends of two notes, not their centers; reverse both axes.
        try send(.rightMouseDown, to: view, at: CGPoint(x: 230, y: 123))
        try send(.rightMouseDragged, to: view, at: CGPoint(x: 220, y: 70))
        #expect(view.selectedNotes.isEmpty && view.displayedSelection.count == 2, "marquee must preview without committing selection")
        let previewFrame = PianoFrame(input: frame.input, piano: PianoRenderInput(track: track, focus: nil,
            camera: view.camera, key: DisplayKey(), preview: nil, selection: view.displayedSelection, marquee: view.selectionPress?.rect))
        #expect(previewFrame.gridInstances.filter { $0.color == SIMD4<Float>(0.70, 0.84, 1, 1) }.count == 4)
        try send(.rightMouseUp, to: view, at: CGPoint(x: 220, y: 70))
        let selected = view.buildAccessibilityChildren(convert: { $0 }).compactMap { $0 as? PianoAccessibleNote }
            .filter { $0.accessibilityValue() as? String == text("Selected") }
        #expect(selected.count == 2, "right drag must select every partially intersecting note")
        #expect(controller.undoCount == undo)
        #expect(controller.noteSelection.count == 2 && controller.noteFocus == nil)
        #expect(controller.noteInput.length == 12_345 && controller.noteVelocity.value == 37, "multi-selection must not remember an arbitrary note")
        let painted = PianoFrame(input: frame.input, piano: PianoRenderInput(track: track, focus: nil,
            camera: view.camera, key: DisplayKey(), preview: nil, selection: view.selectedNotes))
        #expect(painted.gridInstances.filter { $0.color == SIMD4<Float>(0.99, 0.73, 0.31, 1) }.count == 2)
        #expect(painted.velocityInstances.filter { $0.color == SIMD4<Float>(0.99, 0.73, 0.31, 1) }.count == 4)
        // Command toggles without inserting, moving, resizing, or changing velocity.
        let third = CGPoint(x: 400, y: 34)
        try send(.leftMouseDown, to: view, at: third, flags: .command)
        try send(.leftMouseDragged, to: view, at: CGPoint(x: 401, y: 34), flags: .command)
        try send(.leftMouseUp, to: view, at: third, flags: .command)
        #expect(view.selectedNotes.count == 3 && controller.noteSelection.count == 3)
        try send(.leftMouseDown, to: view, at: third, flags: .command)
        try send(.leftMouseUp, to: view, at: third, flags: .command)
        #expect(view.selectedNotes.count == 2 && inserts == 0 && edits == 0)
        #expect(view.displayedInsertion == nil && view.velocityPreview == nil)
        try send(.leftMouseDown, to: view, at: CGPoint(x: 550, y: 180), flags: .command)
        try send(.leftMouseUp, to: view, at: CGPoint(x: 550, y: 180), flags: .command)
        #expect(view.selectedNotes.count == 2 && inserts == 0)
        // The selection belongs to the document, not this NSView instance.
        let recreated = PianoGridView(frame: view.frame)
        recreated.setSnapshot(snapshot, track: track, focus: controller.noteFocus, key: DisplayKey(), busy: false,
                              selection: controller.noteSelection)
        #expect(recreated.selectedNotes == view.selectedNotes)
        recreated.stop()
        let children = recreated.buildAccessibilityChildren(convert: { $0 })
        #expect(children.isEmpty) // No accepted frame yet; stale accessibility cannot reselect a note.
        let noteElement = try #require(view.buildAccessibilityChildren(convert: { $0 }).first as? PianoAccessibleNote)
        let action = try #require(noteElement.accessibilityCustomActions()?.first { $0.name == text("Toggle Note Selection") })
        #expect(action.handler?() == true && view.selectedNotes.count == 1)
        #expect(action.handler?() == true && view.selectedNotes.count == 2)
        controller.selection = nil
        #expect(controller.noteSelection == view.selectedNotes, "clearing arrange selection must preserve piano selection")
        await controller.close()
    }

    @Test func contractSelectionPressesCancelWithoutChangingSelection() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-selection-cancel-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 1))
        let track = try #require(controller.selection), snapshot = try #require(controller.snapshot)
        let note = try #require(snapshot.pianoNeighbor(track: track, current: nil))
        for mode in 0..<9 {
            let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
            view.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: false)
            let frame = PianoFrame(input: CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(),
                width: 640, height: 400, scale: 1), piano: PianoRenderInput(track: track, focus: note.id,
                camera: view.camera, key: DisplayKey(), preview: nil))
            view.present(frame)
            var changes = 0
            view.onSelection = { _ in changes += 1 }
            try send(.rightMouseDown, to: view, at: CGPoint(x: 450, y: 200))
            try send(.rightMouseDragged, to: view, at: CGPoint(x: 500, y: 250))
            #expect(view.selectionPress?.rect != nil)
            switch mode {
            case 0:
                let event = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                    windowNumber: 0, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53))
                view.keyDown(with: event)
            case 1: _ = view.resignFirstResponder()
            case 2: view.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: true)
            case 3: view.setFrameSize(NSSize(width: 620, height: 380))
            case 4: view.setSnapshot(snapshot, track: UUID(), focus: nil, key: DisplayKey(), busy: false)
            case 5: view.setNavigation(CanvasNavigationState(), documentID: DocumentID())
            case 6: view.stop()
            case 7:
                let event = try #require(NSEvent.mouseEvent(with: .rightMouseUp, location: .zero, modifierFlags: [], timestamp: 0,
                    windowNumber: 0, context: nil, eventNumber: 0, clickCount: 1, pressure: 0))
                view.mouseCancelled(with: event)
            default:
                let scroll = try #require(CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: 1, wheel2: 0, wheel3: 0))
                view.scrollWheel(with: try #require(NSEvent(cgEvent: scroll)))
            }
            try send(.rightMouseUp, to: view, at: CGPoint(x: 500, y: 250))
            #expect(changes == 0 && view.selectionPress == nil, "cancelled selection press must not replace the selection on late release")
            view.stop()
        }
        await controller.close()
    }

    private func send(_ type: NSEvent.EventType, to view: PianoGridView, at point: CGPoint, flags: NSEvent.ModifierFlags = []) throws {
        let event = try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: flags,
            timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        switch type {
        case .rightMouseDown: view.rightMouseDown(with: event)
        case .rightMouseDragged: view.rightMouseDragged(with: event)
        case .rightMouseUp: view.rightMouseUp(with: event)
        case .leftMouseDown: view.mouseDown(with: event)
        case .leftMouseDragged: view.mouseDragged(with: event)
        default: view.mouseUp(with: event)
        }
    }
}
