import AppKit
import Carbon.HIToolbox
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoDuplicateCancellationTests {
    @Test func contractCopyClicksAndCancelledDragsNeverWrite() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-copy-cancel-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        #expect(await controller.insertNote(pitch: 60, beat: 1.1, length: 0.7))
        #expect(await controller.insertNote(pitch: 62, beat: 2.2, length: 0.7))
        let snapshot = try #require(controller.snapshot), track = try #require(controller.pianoTrack)
        let notes = snapshot.pianoNotes(track: track, originTick: 0, widthTicks: 8 * 705_600,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1)
        let first = try #require(notes.first), ids = Set(notes.map(\.id))
        for mode in 0..<9 {
            let harness = PianoMoveBatchHarness(snapshot: snapshot, track: track, selected: ids)
            defer { harness.view.stop() }
            let view = harness.view, point = harness.point(first), end = CGPoint(x: point.x + 30, y: point.y)
            var writes = 0
            view.onMoves = { _ in writes += 1 }; view.onEdit = { _, _ in writes += 1 }
            try harness.send(.leftMouseDown, at: point, flags: .command)
            #expect(view.selectedNotes == ids, "Command press must not toggle selection before release")
            if mode > 1 { try harness.send(.leftMouseDragged, at: end) }
            if mode == 1 { try harness.send(.leftMouseDragged, at: CGPoint(x: point.x + 1, y: point.y)) }
            switch mode {
            case 2: view.keyDown(with: try #require(NSEvent.keyEvent(with: .keyDown, location: .zero,
                modifierFlags: .command, timestamp: 0, windowNumber: 0, context: nil, characters: "\u{1b}",
                charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: UInt16(kVK_Escape))))
            case 3: _ = view.resignFirstResponder()
            case 4: view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: true, selection: ids)
            case 5: view.setFrameSize(NSSize(width: 650, height: 400))
            case 6: view.setNavigation(nil, documentID: DocumentID())
            case 7: view.setSnapshot(snapshot, track: track, focus: first.id, key: DisplayKey(), busy: false, selection: [first.id])
            case 8: view.stop()
            default: break
            }
            try harness.send(.leftMouseUp, at: mode < 2 ? point : end)
            #expect(writes == 0 && view.movePreview == nil, "click-only and cancelled copy drags must never commit")
            if mode < 2 { #expect(view.selectedNotes == ids.subtracting([first.id]), "a Command click must toggle on release") }
        }
        // Crossing the drag threshold then returning to the original position is still a copy.
        let harness = PianoMoveBatchHarness(snapshot: snapshot, track: track, selected: ids)
        defer { harness.view.stop() }
        let point = harness.point(first)
        var changes: [PianoMoveChange] = []
        harness.view.onMoves = { changes.append($0) }
        try harness.send(.leftMouseDown, at: point, flags: .command)
        try harness.send(.leftMouseDragged, at: CGPoint(x: point.x + 10, y: point.y))
        try harness.send(.leftMouseUp, at: point, flags: .shift)
        #expect(changes.count == 1 && changes.first?.time == 0 && changes.first?.pitch == 0,
                "a zero-displacement copy is not a selection toggle or no-op")
        await controller.close()
    }
}
