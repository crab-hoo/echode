import AppKit
import EchodeBridge
import Foundation
import Synchronization
import Testing
@testable import EchodeAppCore

@MainActor
struct PlaybackStartTests {
    private let beat: Int64 = 705_600

    private func mouse(_ point: CGPoint, in view: NSView, flags: NSEvent.ModifierFlags = [], type: NSEvent.EventType = .leftMouseDown) throws -> NSEvent {
        try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: flags, timestamp: 0,
            windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
    }

    private nonisolated static func kind(_ command: AudioCommand) -> String {
        switch command {
        case .play: "play"
        case .pause: "pause"
        case .stop: "stop"
        case .setStart: "start"
        default: "other"
        }
    }

    /// Space stops at the remembered start and the play button pauses in place
    /// (ADR 0117, ADR 0155). Play is answered by a stand-in so no output opens;
    /// every other command reaches the product session through the bridge.
    @Test func contractSpaceStopsAtTheRememberedStartAndTheStartSurvivesReopening() async throws {
        let sent = Mutex<[String]>([])
        let controller = ProjectController { document, command in
            sent.withLock { $0.append(Self.kind(command)) }
            guard case .play = command else { return try await document.audio(command) }
            let state = try await document.audio(.status)
            return try JSONDecoder().decode(AudioState.self, from: Data("""
                {"playing":true,"running":false,"opening":false,"tick":\(state.tick),"start":\(state.start),"devices":[],"metronome":false}
                """.utf8))
        }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-start-\(UUID()).echode")
        defer { removeTestFile(url) }
        await controller.open(url, creating: true)
        #expect(controller.playbackStart == 0)
        await controller.setPlaybackStart(beat: 2)
        #expect(controller.playbackStart == 2 * beat && controller.position == 2 * beat)
        await controller.locate(beat: 3)
        #expect(controller.playbackStart == 2 * beat && controller.position == 3 * beat)

        await controller.togglePlayback()
        #expect(controller.playbackRequested)
        await controller.togglePlayback()
        #expect(sent.withLock { $0.last } == "stop")
        #expect(!controller.playbackRequested && controller.position == 2 * beat)

        await controller.togglePause()
        await controller.togglePause()
        #expect(sent.withLock { $0.suffix(2) } == ["play", "pause"])
        #expect(!controller.playbackRequested)
        let session = WorkspaceSession(controller: controller, state: WorkspaceState(defaults: .init()))
        #expect(session.enabled(.playback) && session.enabled(.pause) && session.enabled(.stop))

        // The start belongs to the package, outside Undo.
        #expect(controller.undoCount == 0)
        await controller.close()
        await controller.open(url)
        #expect(controller.playbackStart == 2 * beat && controller.position == 2 * beat)
        await controller.close()
    }

    /// A ruler click sets the start on the note grid; Shift releases the grid.
    /// The marker is drawn inside the ruler only.
    @Test func contractPianoRulerSetsTheStartOnTheGridAndDrawsItsMarker() async throws {
        let controller = ProjectController()
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-ruler-\(UUID()).echode")
        defer { removeTestFile(url) }
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let track = try #require(controller.selection)
        let snapshot = try #require(controller.snapshot)
        let grid = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        var requests: [Int64] = []
        grid.onSetStart = { requests.append($0) }
        var inserted = 0
        grid.onInsert = { _, _, _ in inserted += 1 }
        grid.setSnapshot(snapshot, track: track, focus: nil, key: DisplayKey(), busy: false)
        let input = CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(), width: 640, height: 400, scale: 1)
        grid.present(PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: grid.camera, key: DisplayKey(), preview: nil)))
        let layout = grid.pianoLayout
        let x = layout.gridOrigin.x + 1.3 * grid.camera.pixelsPerBeat

        // 1.3 beats is nearest the 1.25 sixteenth.
        grid.mouseDown(with: try mouse(CGPoint(x: x, y: layout.rulerHeight / 2), in: grid))
        grid.mouseUp(with: try mouse(CGPoint(x: x, y: layout.rulerHeight / 2), in: grid, type: .leftMouseUp))
        #expect(requests == [beat * 5 / 4])
        grid.mouseDown(with: try mouse(CGPoint(x: x, y: layout.rulerHeight / 2), in: grid, flags: .shift))
        grid.mouseUp(with: try mouse(CGPoint(x: x, y: layout.rulerHeight / 2), in: grid, flags: .shift, type: .leftMouseUp))
        #expect(requests.last == Tick.ticks(1.3))
        // The keyboard header and the note grid never move the start.
        grid.mouseDown(with: try mouse(CGPoint(x: layout.degreeWidth + 4, y: layout.rulerHeight / 2), in: grid))
        grid.mouseDown(with: try mouse(CGPoint(x: x, y: layout.rulerHeight + 100), in: grid))
        #expect(requests.count == 2 && inserted == 0)

        let undo = controller.undoCount
        await controller.setPlaybackStart(tick: try #require(requests.first))
        #expect(controller.playbackStart == beat * 5 / 4 && controller.undoCount == undo)
        var camera = grid.camera
        let marked = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera, key: DisplayKey(),
                                                                       preview: nil, start: controller.playbackStart))
        #expect(marked.startX == layout.gridOrigin.x + 1.25 * camera.pixelsPerBeat)
        #expect(marked.rulerInstances.count == 4)
        camera.originBeat = 2
        let scrolled = PianoFrame(input: input, piano: PianoRenderInput(track: track, focus: nil, camera: camera, key: DisplayKey(),
                                                                         preview: nil, start: controller.playbackStart))
        #expect(scrolled.startX == nil && scrolled.rulerInstances.count == 2)
        await controller.close()
    }
}
