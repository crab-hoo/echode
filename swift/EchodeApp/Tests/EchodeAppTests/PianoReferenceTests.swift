import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoReferenceTests {
    @Test func contractReferenceNotesRenderBehindTheInputAndNeverOwnEdits() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-reference-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Chords")
        let chords = try #require(controller.selection)
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 2))
        await controller.addTrack(name: "Melody")
        let melody = try #require(controller.selection), document = try #require(controller.documentID)
        #expect(await controller.insertNote(pitch: 64, beat: 2, length: 0.5))
        #expect(controller.openPiano(PianoTrackTarget(document: document, track: melody)))
        let snapshot = try #require(controller.snapshot)
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        view.setReferences(controller.pianoReferences)
        view.setSnapshot(snapshot, track: melody, focus: nil, key: DisplayKey(), busy: false)
        func frame() -> PianoFrame {
            PianoFrame(input: CanvasInput(snapshot: snapshot, selected: melody, camera: ArrangeCamera(),
                width: 640, height: 400, scale: 1), piano: PianoRenderInput(track: melody, focus: nil,
                    camera: view.camera, key: DisplayKey(), preview: nil, references: controller.pianoReferences))
        }
        let visible = frame()
        #expect(visible.referenceNotes.count == 1 && visible.referenceNotes[0].id.track == chords)
        #expect(visible.notes.count == 1 && visible.velocityNotes.allSatisfy { $0.id.track == melody })
        let instances = visible.gridInstances
        let ghostIndex = try #require(instances.firstIndex { $0.color == PianoPalette.reference })
        let noteIndex = try #require(instances.firstIndex { $0.color == SIMD4<Float>(0.42, 0.76, 0.85, 1) })
        #expect(ghostIndex < noteIndex, "reference notes must paint behind editable notes")
        view.present(visible)
        let children = view.buildAccessibilityChildren(convert: { $0 }).compactMap { $0 as? PianoAccessibleNote }
        #expect(children.count == 1 && children[0].accessibilityIdentifier()?.hasPrefix(melody.uuidString) == true)
        let origin = view.pianoLayout.gridOrigin
        let point = CGPoint(x: origin.x + 1.5 * view.camera.pixelsPerBeat,
                            y: origin.y + (view.camera.topPitch - 60 + 0.5) * view.camera.pixelsPerPitch)
        var inserts: [Task<Bool, Never>] = [], edits = 0
        view.onInsert = { pitch, beat, length in
            inserts.append(Task { await controller.insertNote(pitch: pitch, beat: beat, length: length, on: melody, documentID: document) })
        }
        view.onEdit = { _, _ in edits += 1 }
        func send(_ type: NSEvent.EventType, _ point: CGPoint) throws {
            let event = try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: [],
                timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
            if type == .leftMouseDown { view.mouseDown(with: event) } else { view.mouseUp(with: event) }
        }
        let undo = controller.undoCount, original = try #require(snapshot.pianoNeighbor(track: chords, current: nil))
        try send(.leftMouseDown, point); try send(.leftMouseUp, point)
        #expect(inserts.count == 1 && edits == 0 && view.selectedNotes.isEmpty,
                "clicking a reference must insert on the existing input track, never select or edit the reference")
        for insert in inserts { #expect(await insert.value) }
        #expect(controller.undoCount == undo + 1 && controller.pianoTrack == melody)
        #expect(controller.snapshot?.pianoNote(original.id).map(NoteGeometry.init) == NoteGeometry(original))
        await controller.undo()
        // A same-pitch overlap keeps the editable note as the hit target.
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 2, on: melody))
        let overlap = try #require(controller.snapshot)
        view.setSnapshot(overlap, track: melody, focus: nil, key: DisplayKey(), busy: false)
        view.present(PianoFrame(input: CanvasInput(snapshot: overlap, selected: melody, camera: ArrangeCamera(),
            width: 640, height: 400, scale: 1), piano: PianoRenderInput(track: melody, focus: nil,
                camera: view.camera, key: DisplayKey(), preview: nil, references: controller.pianoReferences)))
        try send(.leftMouseDown, point); try send(.leftMouseUp, point)
        #expect(view.selectedNotes.count == 1 && view.selectedNotes.first?.track == melody && inserts.count == 1)
        await controller.close()
    }

    @Test func contractTrackSelectorSeparatesReferenceEditAndMixerState() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-selector-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Chords")
        let chords = try #require(controller.selection)
        await controller.addTrack(name: "Melody")
        let melody = try #require(controller.selection), document = try #require(controller.documentID)
        let undo = controller.undoCount
        let selector = PianoTrackListView()
        selector.show(controller)
        func cell(_ index: Int) throws -> PianoTrackCell {
            try #require(selector.tableView(selector.table, viewFor: selector.table.tableColumns[0], row: index) as? PianoTrackCell)
        }
        let chord = try cell(0), lead = try cell(1)
        #expect(selector.rows.count == 2 && selector.rows.allSatisfy(\.visible))
        chord.reference.performClick(nil)
        #expect(!controller.pianoReferences.contains(chords) && controller.hiddenPianoTracks == [chords])
        #expect(controller.pianoTrack == chords && controller.selection == melody && controller.undoCount == undo)
        lead.edit.performClick(nil)
        #expect(controller.pianoTrack == melody && controller.selection == melody)
        #expect(controller.pianoReferences.isEmpty)
        chord.reference.performClick(nil)
        #expect(controller.pianoReferences == [chords] && controller.undoCount == undo)
        // Display and edit buttons never change the sound; mix buttons use normal history.
        chord.solo.performClick(nil)
        #expect(await chord.pending?.value == true)
        lead.solo.performClick(nil)
        #expect(await lead.pending?.value == true)
        #expect(controller.mixer(chords)?.solo.value == 1 && controller.mixer(melody)?.solo.value == 1)
        chord.mute.performClick(nil)
        #expect(await chord.pending?.value == true)
        #expect(controller.mixer(chords)?.mute.value == 1 && controller.pianoTrack == melody)
        #expect(controller.undoCount == undo + 3)
        await controller.undo()
        #expect(controller.mixer(chords)?.mute.value == 0)
        await controller.redo()
        controller.setPianoReference(PianoTrackTarget(document: document, track: chords), visible: false)
        await controller.close(); await controller.open(url)
        #expect(controller.hiddenPianoTracks.isEmpty && controller.noteSelection.isEmpty)
        #expect(controller.mixer(chords)?.mute.value == 1 && controller.mixer(chords)?.solo.value == 1)
        let reopenedTarget = controller.pianoTrack
        #expect(!controller.openPiano(PianoTrackTarget(document: document, track: melody)))
        lead.edit.performClick(nil); chord.reference.performClick(nil); lead.solo.performClick(nil)
        #expect(controller.pianoTrack == reopenedTarget && controller.hiddenPianoTracks.isEmpty && controller.undoCount == 0,
                "controls retained from the previous document must not mutate identical reopened IDs")
        #expect(controller.mixer(melody)?.solo.value == 1)
        await controller.close()
    }
}
