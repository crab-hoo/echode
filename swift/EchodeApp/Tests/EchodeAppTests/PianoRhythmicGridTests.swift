import AppKit
import Carbon.HIToolbox
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoRhythmicGridTests {
    @Test func contractTripletAndDottedControlsDriveInputLinesAndHistory() async throws {
        try await exercise(ticks: 235_200, name: "Triplet")
        try await exercise(ticks: 529_200, name: "Dotted")
    }

    private func exercise(ticks: Int64, name: String) async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-rhythmic-grid-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Keys")
        let track = try #require(controller.pianoTrack), document = try #require(controller.documentID)
        let snapshot = try #require(controller.snapshot)
        let navigation = CanvasNavigationState()
        navigation.prepare(document: document, tracks: [track])
        let roll = PianoRollView(frame: NSRect(x: 0, y: 0, width: 640, height: 428))
        roll.layout()
        let view = roll.grid
        defer { view.stop() }
        view.setNavigation(navigation, documentID: document)
        view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false)
        view.inputLength = { controller.noteInput.length }
        var writes: [Task<Void, Never>] = [], starts: [Int64] = [], steps: [PianoStep] = []
        view.onInsert = { pitch, beat, length in
            writes.append(Task { await controller.insertNote(pitch: pitch, beat: beat, length: length, on: track, documentID: document) })
        }
        view.onSetStart = { starts.append($0) }; view.onSteps = { steps.append($0) }
        let undo = controller.undoCount
        let index = try #require(PianoGrid.choices.firstIndex(of: .fixed(ticks)), "Rhythmic grid must be available in native picker")
        roll.gridBar.picker.selectItem(at: index)
        #expect(roll.gridBar.picker.sendAction(roll.gridBar.picker.action, to: roll.gridBar.picker.target))
        let title = "1/8 \(text(name))"
        #expect(roll.gridBar.picker.titleOfSelectedItem == title, "Rhythmic note values must not be labeled as straight fractions")
        #expect(roll.gridBar.subviews.compactMap { $0 as? NSTextField }.first?.stringValue == title)
        #expect(view.camera.grid == ticks && navigation.pianoCamera?.gridMode == .fixed(ticks))
        #expect(controller.noteInput.length == nil && controller.undoCount == undo)
        let frame = gridFrame(view, snapshot: snapshot, track: track)
        #expect(Array(frame.gridTicks.prefix(3)) == [0, ticks, 2 * ticks])
        #expect(frame.gridInstances.count == frame.rows.count + frame.gridLineTicks.count)
        view.present(frame)
        // At 0.45 beats, eighth triplets snap to 1/3; dotted eighths snap to 3/4.
        let point = CGPoint(x: view.pianoLayout.gridOrigin.x + 0.45 * 72, y: view.pianoLayout.gridOrigin.y + 90)
        view.mouseDown(with: try gridMouse(view, .leftMouseDown, point))
        view.mouseUp(with: try gridMouse(view, .leftMouseUp, point))
        #expect(writes.count == 1)
        for write in writes { await write.value }
        let note = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        #expect(note.start == ticks && note.length == ticks)
        #expect(controller.undoCount == undo + 1 && controller.noteInput.length == ticks)
        let inserted = try #require(controller.snapshot)
        view.setSnapshot(inserted, track: track, focus: note.id, key: DisplayKey(), busy: false)
        let right = try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, characters: "", charactersIgnoringModifiers: "", isARepeat: false, keyCode: UInt16(kVK_RightArrow)))
        view.keyDown(with: right)
        #expect(steps.count == 1 && steps[0].time == ticks)
        view.present(gridFrame(view, snapshot: inserted, track: track))
        let ruler = CGPoint(x: point.x, y: 16)
        view.mouseDown(with: try gridMouse(view, .leftMouseDown, ruler))
        view.mouseUp(with: try gridMouse(view, .leftMouseUp, ruler))
        #expect(starts == [ticks])
        view.present(gridFrame(view, snapshot: inserted, track: track))
        view.mouseDown(with: try gridMouse(view, .leftMouseDown, ruler, flags: .shift))
        view.mouseUp(with: try gridMouse(view, .leftMouseUp, ruler, flags: .shift))
        #expect(starts.last == 317_520, "Shift releases rhythmic snapping without changing the grid")
        // Returning to straight time neither rounds the input memory nor commits an old press.
        view.present(gridFrame(view, snapshot: inserted, track: track))
        let blank = CGPoint(x: point.x + 200, y: point.y)
        view.mouseDown(with: try gridMouse(view, .leftMouseDown, blank))
        roll.gridBar.picker.selectItem(at: try #require(PianoGrid.choices.firstIndex(of: .fixed(176_400))))
        #expect(roll.gridBar.picker.sendAction(roll.gridBar.picker.action, to: roll.gridBar.picker.target))
        view.mouseUp(with: try gridMouse(view, .leftMouseUp, blank))
        #expect(writes.count == 1 && view.displayedInsertion == nil)
        #expect(controller.noteInput.length == ticks && controller.undoCount == undo + 1)
        await controller.undo()
        #expect(controller.snapshot?.pianoNote(note.id) == nil && controller.noteInput.length == nil)
        await controller.redo()
        #expect(controller.snapshot?.pianoNote(note.id).map(NoteGeometry.init) == NoteGeometry(note))
        await controller.close()
        await controller.open(url)
        #expect(controller.snapshot?.pianoNote(note.id).map(NoteGeometry.init) == NoteGeometry(note))
        #expect(controller.noteInput.length == nil)
        await controller.close()
    }
}
