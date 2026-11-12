import CEchodeFfi
import Foundation
import Synchronization
import Testing

@testable import EchodeBridge

/// The real worker's resume always beats `await`, and the result is read straight
/// after it, as in `Document.audio`. `check-bridge-tsan` reports a race here when the
/// build loses `ContinuationHandoff`. These tests stay off the main actor: returning
/// there hops, and the hop shows TSan the order without any annotation.
struct DocumentCompletionOrderTests {
    @Test func aResultFromAResumeAheadOfTheAwaitIsOrdered() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-completion-order-\(UUID()).echode")
        defer { removeTestFile(url) }
        let document = try Document(creating: url.path)
        let relay = CompletionRelay()
        let snapshot = try await awaitCompletion { context in
            relay.pending = context
            _ = echode_document_query(document.handle, UInt32(ECHODE_DOCUMENT_SNAPSHOT), relay.context, relayCompletion)
            relay.waitForResume()
        }
        let tracks = snapshot.trackCount
        #expect(tracks == 0)
    }

}

/// The relaxed flag orders nothing for TSan. The relay is never freed, because the
/// free would look unordered with the worker's store.
private final class CompletionRelay {
    var pending: UnsafeMutableRawPointer?
    let resumed = Atomic(false)

    init() { _ = Unmanaged.passRetained(self) }

    var context: UnsafeMutableRawPointer { Unmanaged.passUnretained(self).toOpaque() }

    func waitForResume() {
        while !resumed.load(ordering: .relaxed) { sched_yield() }
    }
}

private func relayCompletion(
    _ context: UnsafeMutableRawPointer?,
    _ status: Int32,
    _ message: UnsafePointer<CChar>?,
    _ snapshot: OpaquePointer?
) {
    let relay = Unmanaged<CompletionRelay>.fromOpaque(context!).takeUnretainedValue()
    completeCommand(relay.pending, status, message, snapshot)
    relay.resumed.store(true, ordering: .relaxed)
}
