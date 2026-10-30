import AppKit
import EchodeBridge
import Foundation
import Synchronization
import Testing
@testable import EchodeAppCore

private actor HeldStart {
    private var continuation: CheckedContinuation<Void, Never>?
    func wait(_ started: AsyncStream<Void>.Continuation) async {
        await withCheckedContinuation {
            continuation = $0
            started.yield()
        }
    }
    func release() { continuation?.resume(); continuation = nil }
}

@MainActor
struct TransportOrderTests {
    private let beat: Int64 = 705_600

    private nonisolated static func name(_ command: AudioCommand) -> String {
        switch command {
        case .setStart(let tick): "start \(tick / 705_600)"
        case .loop(let start, let end): "loop \(start.map { String($0 / 705_600) } ?? "-") \(end.map { String($0 / 705_600) } ?? "-")"
        case .metronome(let enabled): "metronome \(enabled)"
        case .output: "output"
        default: "other"
        }
    }

    /// Moving the start while the bridge is still answering keeps the transport,
    /// rulers and commands enabled. Later commands wait their turn instead of
    /// being dropped, and a toggle reads the state the earlier one left
    /// (ADR 0160). The first start is held across the same await as a slow
    /// bridge request; every command then reaches the product session.
    @Test func contractStartChangesKeepTransportControlsEnabledAndInOrder() async throws {
        let started = AsyncStream<Void>.makeStream()
        let hold = HeldStart()
        let sent = Mutex<[String]>([])
        let first = Mutex(true)
        let controller = ProjectController { document, command in
            if case .status = command { return try await document.audio(command) }
            sent.withLock { $0.append(Self.name(command)) }
            if case .setStart = command, first.withLock({ let held = $0; $0 = false; return held }) {
                await hold.wait(started.continuation)
            }
            return try await document.audio(command)
        }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-transport-order-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let document = try #require(controller.documentID), snapshot = try #require(controller.snapshot)
        let session = WorkspaceSession(controller: controller, state: WorkspaceState(defaults: .init()))
        let commands: [WorkspaceCommand] = [.playback, .stop, .loop, .metronome, .output, .newProject, .openProject]
        #expect(commands.allSatisfy { session.enabled($0) })

        // The arrange ruler receives the same lock the workspace passes it.
        let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 900, height: 200))
        pane.layout(); pane.setSnapshot(snapshot, selected: controller.selection)
        let ruler = pane.timeline
        ruler.audioDocument = document
        var clicked: [Int64] = []
        ruler.onSetStart = { clicked.append($0) }
        func click(_ position: Double) throws {
            let x = ruler.renderInput.header!.width + 1 + (position - ruler.camera.originBeat) * ruler.camera.pixelsPerBeat
            for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
                let event = try #require(NSEvent.mouseEvent(with: type, location: ruler.convert(CGPoint(x: x, y: 10), to: nil),
                    modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
                if type == .leftMouseDown { ruler.mouseDown(with: event) } else { ruler.mouseUp(with: event) }
            }
        }

        ruler.setRuler(start: controller.playbackStart, loop: controller.rulerLoop, busy: controller.transportLocked)
        try click(1)
        let tick = try #require(clicked.last)
        let firstStart = Task { await controller.setPlaybackStart(tick: tick) }
        for await _ in started.stream { break }
        #expect(controller.audioBusy) // the first start is still in flight
        #expect(!controller.transportLocked)
        #expect(commands.allSatisfy { session.enabled($0) })
        ruler.setRuler(start: controller.playbackStart, loop: controller.rulerLoop, busy: controller.transportLocked)
        try click(2)
        #expect(clicked == [beat, beat * 2], "the ruler dropped a click while the start was sent")

        // Requests made meanwhile are queued, not dropped, and keep their order.
        let second = Task { await controller.setPlaybackStart(tick: beat * 2) }
        let range = try #require(LoopRange(start: beat, end: beat * 3))
        let loop = Task {
            controller.loopRange = range
            await controller.toggleLoop()
        }
        let ranged = Task { await controller.setLoopRange(range, documentID: document) }
        let metronomeOn = Task { await controller.toggleMetronome() }
        let metronomeOff = Task { await controller.toggleMetronome() }
        for _ in 0..<20 { await Task.yield() }
        #expect(sent.withLock { $0 } == ["start 1"])

        await hold.release()
        await firstStart.value
        for task in [second, loop, ranged, metronomeOn, metronomeOff] { await task.value }
        #expect(sent.withLock { $0 } == ["start 1", "start 2", "loop 1 3", "loop 1 3", "metronome true", "metronome false"])
        #expect(controller.playbackStart == beat * 2 && controller.audio.tick == beat * 2)
        #expect(controller.audio.loopStart == beat && controller.audio.loopEnd == beat * 3)
        #expect(!controller.audio.metronome && !controller.audioBusy)
        await controller.close()
    }
}
