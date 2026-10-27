import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoMoveBatchHarness {
    let view: PianoGridView
    let snapshot: Snapshot
    let track: UUID
    let selected: Set<PianoNote.ID>

    init(snapshot: Snapshot, track: UUID, selected: Set<PianoNote.ID>) {
        self.snapshot = snapshot; self.track = track; self.selected = selected
        view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false, selection: selected)
        view.present(frame())
    }

    func frame(_ move: PianoMovePreview? = nil, resize: PianoResizePreview? = nil) -> PianoFrame {
        PianoFrame(input: CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(),
            width: 640, height: 400, scale: 1), piano: PianoRenderInput(track: track, focus: nil,
                camera: view.camera, key: DisplayKey(), preview: nil, selection: selected, move: move, resize: resize))
    }

    func point(_ note: PianoNote) -> CGPoint {
        CGPoint(x: view.pianoLayout.gridOrigin.x + (Double(note.start) / 705_600 + Double(note.length) / 705_600 / 2 - view.camera.originBeat) * view.camera.pixelsPerBeat,
                y: view.pianoLayout.gridOrigin.y + (view.camera.topPitch - Double(note.pitch) + 0.5) * view.camera.pixelsPerPitch)
    }

    func edgePoint(_ note: PianoNote, edge: PianoGesture.Mode) -> CGPoint {
        var result = point(note)
        let halfWidth = Double(note.length) / 705_600 / 2 * view.camera.pixelsPerBeat
        result.x += edge == .leftEdge ? -halfWidth + 1 : halfWidth - 1
        return result
    }

    func send(_ type: NSEvent.EventType, at point: CGPoint, flags: NSEvent.ModifierFlags = []) throws {
        let event = try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: flags,
            timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        switch type {
        case .leftMouseDown: view.mouseDown(with: event)
        case .leftMouseDragged: view.mouseDragged(with: event)
        default: view.mouseUp(with: event)
        }
    }
}
