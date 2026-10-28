import AppKit
import Carbon.HIToolbox
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoGridControlTests {
    @Test func contractFixedGridControlSharesStateCancelsPressesAndKeepsInputMemory() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-grid-control-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Keys")
        let track = try #require(controller.pianoTrack)
        await controller.insertNote(pitch: 60, beat: 1, length: 0.3)
        await controller.addTrack(name: "Other")
        let other = try #require(controller.selection), snapshot = try #require(controller.snapshot)
        let note = try #require(snapshot.pianoNeighbor(track: track, current: nil))
        let undo = controller.undoCount, inputLength = controller.noteInput.length
        let navigation = CanvasNavigationState()
        navigation.prepare(document: controller.documentID, tracks: [track, other])
        let roll = PianoRollView(frame: NSRect(x: 0, y: 0, width: 640, height: 428))
        roll.layout()
        let view = roll.grid
        defer { view.stop() }
        view.setNavigation(navigation, documentID: controller.documentID)
        view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false)
        view.inputLength = { controller.noteInput.length }
        #expect(roll.gridBar.picker.accessibilityLabel() == text("Note Grid"))
        #expect(roll.gridBar.frame.maxY == view.frame.minY && view.bounds.height == 400)
        func choose(_ mode: PianoGrid) throws {
            roll.gridBar.picker.selectItem(at: try #require(PianoGrid.choices.firstIndex(of: mode)))
            #expect(roll.gridBar.picker.sendAction(roll.gridBar.picker.action, to: roll.gridBar.picker.target))
        }
        try choose(.fixed(352_800))
        #expect(view.camera.grid == 352_800 && view.camera.gridMode == .fixed(352_800))
        let event = GridMagnifyEvent()
        event.point = view.convert(CGPoint(x: 200, y: 100), to: nil)
        view.magnify(with: event)
        #expect(view.camera.grid == 352_800 && view.camera.pixelsPerBeat == 144)
        #expect(controller.noteInput.length == inputLength && controller.undoCount == undo)
        // Both insertion and ruler drags must discard the old-grid release.
        var inserts: [NoteGeometry] = [], starts: [Int64] = [], loops: [LoopRange] = []
        view.onInsert = { inserts.append(NoteGeometry(start: Int64(($1 * 705_600).rounded()), length: Int64(($2 * 705_600).rounded()), pitch: $0)) }
        view.onSetStart = { starts.append($0) }; view.onSetLoop = { loops.append($0) }
        let point = CGPoint(x: 300, y: 114)
        view.present(gridFrame(view, snapshot: snapshot, track: track))
        view.mouseDown(with: try gridMouse(view, .leftMouseDown, point))
        #expect(view.displayedInsertion?.length == inputLength)
        try choose(.fixed(705_600))
        #expect(view.displayedInsertion == nil, "Changing the note grid must cancel an old-grid press")
        view.mouseUp(with: try gridMouse(view, .leftMouseUp, point))
        #expect(inserts.isEmpty)
        view.present(gridFrame(view, snapshot: snapshot, track: track))
        let ruler = CGPoint(x: 250, y: 16)
        view.mouseDown(with: try gridMouse(view, .leftMouseDown, ruler))
        view.mouseDragged(with: try gridMouse(view, .leftMouseDragged, CGPoint(x: 400, y: 16)))
        try choose(.fixed(176_400))
        view.mouseUp(with: try gridMouse(view, .leftMouseUp, CGPoint(x: 400, y: 16)))
        #expect(starts.isEmpty && loops.isEmpty)
        view.present(gridFrame(view, snapshot: snapshot, track: track))
        view.mouseDown(with: try gridMouse(view, .leftMouseDown, point))
        view.mouseUp(with: try gridMouse(view, .leftMouseUp, point))
        #expect(inserts.count == 1 && inserts[0].length == inputLength)
        let raw = try #require(PianoCamera.ticks(view.camera.originBeat + (point.x - view.pianoLayout.gridOrigin.x) / 144))
        #expect(inserts[0].start == view.camera.snapped(raw))
        // Arrows and the ruler read the same fixed width, not a hard-coded sixteenth.
        view.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: false)
        try choose(.fixed(352_800))
        var steps: [PianoStep] = []
        view.onSteps = { steps.append($0) }
        let right = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, characters: "", charactersIgnoringModifiers: "", isARepeat: false, keyCode: UInt16(kVK_RightArrow)))
        view.keyDown(with: right)
        #expect(steps.count == 1 && steps[0].time == 352_800)
        view.present(gridFrame(view, snapshot: snapshot, track: track))
        view.mouseDown(with: try gridMouse(view, .leftMouseDown, ruler))
        view.mouseUp(with: try gridMouse(view, .leftMouseUp, ruler))
        #expect(starts == [view.camera.rulerTick(at: ruler.x - view.pianoLayout.gridOrigin.x, snapping: true)])
        var edits: [NoteGeometry] = []
        view.onEdit = { _, geometry in edits.append(geometry) }
        for flags: NSEvent.ModifierFlags in [[], .shift] {
            view.present(gridFrame(view, snapshot: snapshot, track: track))
            let edge = CGPoint(x: view.pianoLayout.gridOrigin.x + (1.3 - view.camera.originBeat) * 144 - 1,
                               y: view.pianoLayout.gridOrigin.y + (view.camera.topPitch - 60 + 0.5) * view.camera.pixelsPerPitch)
            let release = CGPoint(x: edge.x + 0.1 * 144, y: edge.y)
            view.mouseDown(with: try gridMouse(view, .leftMouseDown, edge))
            view.mouseDragged(with: try gridMouse(view, .leftMouseDragged, release, flags: flags))
            view.mouseUp(with: try gridMouse(view, .leftMouseUp, release, flags: flags))
            #expect(edits.last?.length == (flags.isEmpty ? 352_800 : 282_240))
        }
        #expect(edits.count == 2)
        let horizontal = view.camera.pixelsPerBeat
        event.flags = .option
        view.magnify(with: event)
        #expect(view.camera.pixelsPerBeat == horizontal && view.camera.grid == 352_800)
        #expect(view.camera.pixelsPerPitch == 40)
        event.flags = .shift; event.amount = -0.5
        view.magnify(with: event)
        #expect(view.camera.pixelsPerBeat == horizontal / 2 && view.camera.pixelsPerPitch == 20)
        #expect(view.camera.grid == 352_800)
        let shared = view.camera
        view.setSnapshot(snapshot, track: other, focus: nil, key: DisplayKey(), busy: false)
        #expect(view.camera == shared)
        let recreated = PianoRollView(frame: roll.frame)
        recreated.layout()
        defer { recreated.grid.stop() }
        recreated.grid.setNavigation(navigation, documentID: controller.documentID)
        recreated.grid.setSnapshot(snapshot, track: other, focus: nil, key: DisplayKey(), busy: false)
        #expect(recreated.grid.camera == shared)
        #expect(recreated.gridBar.picker.indexOfSelectedItem == PianoGrid.choices.firstIndex(of: shared.gridMode))
        // A stale control cannot write into a replacement document's shared state.
        navigation.prepare(document: UUID(), tracks: [track, other])
        try choose(.fixed(705_600))
        #expect(navigation.pianoCamera == nil && view.camera == shared)
        recreated.grid.setNavigation(navigation)
        recreated.grid.setSnapshot(snapshot, track: other, focus: nil, key: DisplayKey(), busy: false)
        #expect(recreated.grid.camera == PianoCamera() && recreated.gridBar.picker.indexOfSelectedItem == 0)
        #expect(controller.noteInput.length == inputLength && controller.undoCount == undo)
        await controller.close()
    }
}
