import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoGridTests {
    @Test func contractAutomaticGridFollowsZoomAndDrivesNoteInput() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-grid-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Keys")
        let track = try #require(controller.pianoTrack), snapshot = try #require(controller.snapshot)
        let navigation = CanvasNavigationState()
        navigation.prepare(document: controller.documentID, tracks: [track])
        navigation.pianoCamera = PianoCamera(originBeat: 4)
        let view = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { view.stop() }
        view.setNavigation(navigation, documentID: controller.documentID)
        view.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false)
        let anchor = CGPoint(x: 144, y: 80), before = view.camera
        let event = GridMagnifyEvent()
        event.point = view.convert(CGPoint(x: view.pianoLayout.gridOrigin.x + anchor.x,
                                          y: view.pianoLayout.gridOrigin.y + anchor.y), to: nil)
        view.magnify(with: event)
        #expect(view.camera.grid == 88_200, "Automatic note grid must refine with horizontal zoom")
        #expect(view.camera.pixelsPerBeat == 144)
        #expect(abs(view.camera.originBeat + anchor.x / view.camera.pixelsPerBeat -
                    (before.originBeat + anchor.x / before.pixelsPerBeat)) < 0.000001)
        #expect(view.camera.topPitch == before.topPitch && view.camera.pixelsPerPitch == before.pixelsPerPitch)
        #expect(navigation.pianoCamera == view.camera)
        let frame = gridFrame(view, snapshot: snapshot, track: track)
        view.present(frame)
        #expect(frame.gridTicks.prefix(3).map { $0 } == [3_528_000, 3_616_200, 3_704_400])
        #expect(frame.gridInstances.count == frame.rows.count + frame.gridLineTicks.count,
                "The rendered note grid must include the snapping subdivisions")
        let undo = controller.undoCount, document = try #require(controller.documentID)
        var writes: [Task<Void, Never>] = []
        view.onInsert = { pitch, beat, length in
            writes.append(Task { await controller.insertNote(pitch: pitch, beat: beat, length: length, on: track, documentID: document) })
        }
        // 5.2 beats snaps to 5.25; an unremembered click uses the new grid width.
        let point = CGPoint(x: view.pianoLayout.gridOrigin.x + 0.2 * 144,
                            y: view.pianoLayout.gridOrigin.y + 90)
        view.mouseDown(with: try gridMouse(view, .leftMouseDown, point))
        view.mouseUp(with: try gridMouse(view, .leftMouseUp, point))
        #expect(writes.count == 1)
        for write in writes { await write.value }
        let note = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        #expect(note.start == 3_704_400 && note.length == 88_200)
        #expect(controller.undoCount == undo + 1)
        #expect(controller.noteInput.length == 88_200)
        await controller.undo()
        #expect(controller.snapshot?.pianoNeighbor(track: track, current: nil) == nil)
        await controller.redo()
        #expect(controller.snapshot?.pianoNote(note.id).map(NoteGeometry.init) == NoteGeometry(note))
        await controller.close()
        await controller.open(url)
        #expect(controller.snapshot?.pianoNote(note.id).map(NoteGeometry.init) == NoteGeometry(note))
        await controller.close()
    }

    @Test func straightGridBoundariesAndDenseFixedLinesStayBounded() throws {
        for (pixels, ticks) in [(8.0, Int64(2_822_400)), (18, 705_600), (36, 352_800),
                                (72, 176_400), (144, 88_200), (288, 44_100), (576, 22_050), (1200, 22_050)] {
            #expect(PianoCamera(pixelsPerBeat: pixels).grid == ticks)
            #expect(PianoGrid.automatic.ticks(pixelsPerBeat: pixels.nextDown) >= ticks)
        }
        #expect(PianoGrid.automatic.ticks(pixelsPerBeat: .nan) == 176_400)
        #expect(PianoGrid.automatic.ticks(pixelsPerBeat: 0) == 176_400)
        for mode in PianoGrid.choices.dropFirst() {
            var camera = PianoCamera(originBeat: 20, gridMode: mode)
            let grid = camera.grid
            camera.zoom(factor: 100, horizontal: true, vertical: false, anchor: CGPoint(x: 80, y: 20))
            #expect(camera.grid == grid)
            camera.zoom(factor: 0.0001, horizontal: true, vertical: false, anchor: .zero)
            #expect(camera.grid == grid)
        }
        let track = UUID()
        let input = CanvasInput(snapshot: nil, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
        let camera = PianoCamera(originBeat: 1234.3, pixelsPerBeat: 8, gridMode: .fixed(22_050))
        let frame = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera, key: DisplayKey(), preview: nil))
        #expect(frame.gridTicks.count <= Int(frame.layout.grid.width / 4) + 1)
        #expect(frame.gridTicks.allSatisfy { $0 % camera.grid == 0 })
        #expect(zip(frame.gridTicks, frame.gridTicks.dropFirst()).allSatisfy { Double($1 - $0) / 705_600 * 8 >= 4 })
        #expect(camera.grid == 22_050)
    }
}

// Synthesized input exercises the native pinch handler without a window or device.
final class GridMagnifyEvent: NSEvent, @unchecked Sendable {
    var point = CGPoint.zero
    var flags: NSEvent.ModifierFlags = []
    var amount: CGFloat = 1
    override var locationInWindow: NSPoint { point }
    override var modifierFlags: NSEvent.ModifierFlags { flags }
    override var magnification: CGFloat { amount }
}

@MainActor
func gridFrame(_ view: PianoGridView, snapshot: Snapshot, track: UUID) -> PianoFrame {
    PianoFrame(input: CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(),
                                 width: view.bounds.width, height: view.bounds.height, scale: 1),
               piano: PianoRenderInput(track: track, focus: nil, camera: view.camera, key: DisplayKey(), preview: nil))
}

@MainActor
func gridMouse(_ view: PianoGridView, _ type: NSEvent.EventType, _ point: CGPoint, flags: NSEvent.ModifierFlags = []) throws -> NSEvent {
    try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: flags,
                                   timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
}
