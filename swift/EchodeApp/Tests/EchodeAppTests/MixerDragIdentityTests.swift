import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct MixerDragIdentityTests {
    private func mouse(_ type: NSEvent.EventType, _ point: CGPoint, in header: ArrangeHeaderView) throws -> NSEvent {
        try #require(NSEvent.mouseEvent(with: type, location: header.convert(point, to: nil), modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
    }

    @Test func contractZeroHorizontalHeaderDragsPreserveQuietGainAndRedo() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-drag-identity-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController(sendAudio: { _, _ in AudioState() })
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Quiet")
        let track = try #require(controller.selection)
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 220, height: 64))
        let rect = TrackControlLayout(row: header.bounds).gain
        let start = CGPoint(x: rect.midX, y: rect.midY)
        let vertical = CGPoint(x: start.x, y: start.y + 8)
        let displaced = CGPoint(x: start.x + rect.width / 10, y: start.y)

        // Include loaded values below the numeric entry range and display floor,
        // an arbitrary in-range Float, and the explicit endpoint values.
        for original: Float in [0.00000001, 0.000001, 0.0001, 0.001, 0.12345679, 4, 0] {
            #expect(await controller.setMixer(track, field: .gain, value: original))
            #expect(await controller.setMixer(track, field: .pan, value: 0.25))
            await controller.undo()
            let before = controller.undoCount
            #expect(controller.redoCount == 1)
            header.documentID = controller.documentID
            header.setSnapshot(try #require(controller.snapshot), selected: track)
            let actions = controller.arrangeMixActions(document: controller.documentID)
            var completion: Task<Void, Never>?
            var observed = actions
            observed.finish = { commit in completion = Task { await controller.finishFader(commit: commit) } }
            header.mixActions = observed

            header.mouseDown(with: try mouse(.leftMouseDown, start, in: header))
            header.mouseDragged(with: try mouse(.leftMouseDragged, vertical, in: header))
            #expect(controller.faderPreview?.gain == original,
                "a vertical header drag must preserve the exact gain, including below the display floor")
            header.mouseUp(with: try mouse(.leftMouseUp, vertical, in: header))
            await completion?.value
            #expect(controller.mixer(track)?.gain.value == original)
            #expect(controller.undoCount == before && controller.redoCount == 1,
                "zero horizontal travel must not record an edit or discard Redo")

            header.setSnapshot(try #require(controller.snapshot), selected: track)
            header.mouseDown(with: try mouse(.leftMouseDown, start, in: header))
            header.mouseDragged(with: try mouse(.leftMouseDragged, displaced, in: header))
            #expect(controller.faderPreview?.gain == MixScale.dragged(.gain, from: original, by: 0.1))
            // Release at the original x without a matching final drag event.
            header.mouseUp(with: try mouse(.leftMouseUp, vertical, in: header))
            await completion?.value
            #expect(controller.mixer(track)?.gain.value == original,
                "returning to the press x must restore the exact original gain on release")
            #expect(controller.undoCount == before && controller.redoCount == 1)
            #expect(controller.faderPreview == nil && !header.clicks.active && controller.errorMessage == nil)
            await controller.redo()
            #expect(controller.mixer(track)?.pan.value == 0.25)
            await controller.undo()
            await controller.close(); await controller.open(url)
            #expect(controller.mixer(track)?.gain.value == original && controller.mixer(track)?.pan.value == 0,
                "zero-travel drags must preserve the original gain through save and reopen")
        }
        await controller.close()
    }

    @Test func zeroTravelIsAnExactIdentityForBothHeaderScales() {
        for value: Float in [0, .leastNonzeroMagnitude, 0.00000001, 0.0001, 0.001, 0.12345679, 1, 4] {
            #expect(MixScale.dragged(.gain, from: value, by: 0) == value)
            #expect(MixScale.dragged(.gain, from: value, by: -0.0) == value)
        }
        for value: Float in [-1, -0.12345679, 0, 0.12345679, 1] {
            #expect(MixScale.dragged(.pan, from: value, by: 0) == value)
        }
    }
}
