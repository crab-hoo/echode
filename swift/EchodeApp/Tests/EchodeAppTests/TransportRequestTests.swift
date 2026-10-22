import EchodeBridge
import Foundation
import Synchronization
import Testing
@testable import EchodeAppCore

private actor DelayedAudioReply {
    private var continuation: CheckedContinuation<AudioState, Never>?
    func wait(_ started: AsyncStream<Void>.Continuation) async -> AudioState {
        await withCheckedContinuation {
            continuation = $0
            started.yield()
        }
    }
    func finish(_ state: AudioState) { continuation?.resume(returning: state); continuation = nil }
}

@MainActor
struct TransportRequestTests {
    @Test func contractStopAndSpaceRemainAvailableWhilePlayIsPreparing() async throws {
        for useSpace in [false, true] {
            let started = AsyncStream<Void>.makeStream()
            let reply = DelayedAudioReply()
            let controller = ProjectController { document, command in
                if case .play = command {
                    // The acknowledgement is held across the same await as a
                    // slow bridge request. No HAL device or timer is needed.
                    return await reply.wait(started.continuation)
                }
                return try await document.audio(command)
            }
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-pending-\(UUID()).echode")
            defer { try? FileManager.default.removeItem(at: url) }
            await controller.open(url, creating: true)
            let session = WorkspaceSession(controller: controller, state: WorkspaceState())
            let play = Task { await controller.togglePlayback() }
            for await _ in started.stream { break }
            #expect(controller.audioBusy && controller.playbackRequested)
            #expect(session.enabled(.playback) && session.enabled(.stop))
            #expect(!session.enabled(.output))
            if useSpace { await controller.togglePlayback() }
            else { await controller.stopPlayback() }
            #expect(!controller.playbackRequested)
            #expect(controller.audioBusy) // the old load is still in flight
            let latePlay = try JSONDecoder().decode(AudioState.self, from: Data("""
                {"playing":true,"running":true,"opening":false,"tick":123,"devices":[],"metronome":false}
                """.utf8))
            await reply.finish(latePlay)
            await play.value
            #expect(!controller.audioBusy && !controller.audio.playing)
            #expect(!controller.playbackRequested && controller.audio.tick == 0)
            await controller.close()
        }
    }

    /// The app prepares the output when a project opens, so the first play does
    /// not wait (ADR 0147). Tests and probes leave it off and never reach HAL.
    @Test func contractTheAppPreparesTheOutputWhenAProjectOpens() async throws {
        for prepares in [true, false] {
            let prepared = Mutex(0)
            let controller = ProjectController { document, command in
                // The bridge would open the output; no HAL device is needed here.
                if case .prepare = command { prepared.withLock { $0 += 1 }; return AudioState() }
                return try await document.audio(command)
            }
            controller.preparesOutput = prepares
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-prepare-\(UUID()).echode")
            defer { try? FileManager.default.removeItem(at: url) }
            await controller.open(url, creating: true)
            #expect(!controller.busy && controller.snapshot != nil)
            let deadline = ContinuousClock.now + .seconds(5)
            while prepares, prepared.withLock({ $0 }) == 0, ContinuousClock.now < deadline {
                try await Task.sleep(for: .milliseconds(1))
            }
            for _ in 0..<20 { await Task.yield() }
            #expect(prepared.withLock { $0 } == (prepares ? 1 : 0))
            #expect(!controller.audio.playing && controller.pendingPlayback == nil)
            await controller.close()
        }
    }
}
