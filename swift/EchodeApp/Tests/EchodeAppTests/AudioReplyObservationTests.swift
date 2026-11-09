import EchodeBridge
import Foundation
import Observation
import Synchronization
import Testing
@testable import EchodeAppCore

/// Replies only; this fixture never opens HAL, an audition output or a helper.
private func reply(tick: Int64, audition: UInt64? = nil, metronome: Bool = false) throws -> AudioState {
    let sample = audition.map { #","audition":{"request":\#($0),"handled":\#($0),"phase":"sounding","history":[]}"# } ?? ""
    return try JSONDecoder().decode(AudioState.self, from: Data(
        #"{"playing":true,"running":true,"opening":false,"tick":\#(tick),"devices":[],"metronome":\#(metronome)\#(sample)}"#.utf8))
}

/// Whether `change` notifies an observer of what `read` reads.
@MainActor private func notifies(_ read: () -> Void, when change: () throws -> Void) rethrows -> Bool {
    let fired = Mutex(false)
    withObservationTracking(read) { fired.withLock { $0 = true } }
    try change()
    return fired.withLock { $0 }
}

/// A playback poll used to re-evaluate the whole workspace 20 times a second,
/// 3.4 ms each with 128 tracks (ADR 0289). Each part of a reply now notifies
/// only its own readers.
@MainActor
struct AudioReplyObservationTests {
    @Test func contractEachReplyPartNotifiesOnlyItsOwnReaders() throws {
        let session = DocumentSession(auditionLog: AuditionLog())
        session.receive(try reply(tick: 0, audition: 1))
        let status = { _ = session.audio; _ = session.outputOpening; _ = session.playbackStart }

        #expect(try !notifies({ status(); _ = session.audition }) { session.receive(try reply(tick: 240, audition: 1)) })
        #expect(try notifies({ _ = session.position }) { session.receive(try reply(tick: 480, audition: 1)) })

        #expect(try !notifies({ status(); _ = session.position }) { session.receive(try reply(tick: 480, audition: 2)) })
        #expect(try notifies({ _ = session.audition }) { session.receive(try reply(tick: 480, audition: 3)) })

        #expect(try !notifies({ _ = session.position; _ = session.audition }) { session.receive(try reply(tick: 480, audition: 3, metronome: true)) })
        #expect(try notifies({ _ = session.audio }) { session.receive(try reply(tick: 480, audition: 3)) })
        #expect(session.position == 480 && session.audition?.request == 3 && !session.audio.metronome)
    }

    @Test func contractPlaybackPollsReachOnlyThePositionDisplay() async throws {
        let ticks = Mutex<Int64>(0)
        let controller = ProjectController(sendAudio: { _, _ in try reply(tick: ticks.withLock { $0 += 240; return $0 }) })
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-poll-observation-\(UUID()).echode")
        defer { removeTestFile(url) }
        await controller.open(url, creating: true)
        let observing = Task { await controller.observeAudio() }
        defer { observing.cancel() }
        try await wait("a playing poll") { controller.position >= 480 }

        let fired = Mutex(false)
        // What the workspace, the mix inspector, the rulers and the browser read.
        withObservationTracking {
            _ = controller.audio; _ = controller.audition; _ = controller.autoMuteReasons; _ = controller.rulerLoop
            _ = controller.playbackRequested; _ = controller.outputOpening; _ = controller.playbackStart
        } onChange: { fired.withLock { $0 = true } }
        let reached = controller.position
        try await wait("two more polls") { controller.position >= reached + 480 }
        #expect(!fired.withLock { $0 })
        #expect(controller.audio.playing && controller.playbackRequested)
        await controller.close()
    }

    private func wait(_ phase: String, _ ready: () -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(5)
        while !ready() {
            try #require(ContinuousClock.now < deadline, "Status did not arrive: \(phase)")
            try await Task.sleep(for: .milliseconds(5))
        }
    }
}
