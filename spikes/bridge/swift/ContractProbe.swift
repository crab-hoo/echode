import CEchodeBridge
import Synchronization

// Probe handles stay on MainActor; the product's two unchecked wrappers suffice.
@MainActor
private final class LifetimeProbe {
    let handle: OpaquePointer
    init(_ document: CDocument) { handle = document.probeHandle() }
    isolated deinit { eb_probe_free(handle) }
    var counts: BridgeCounts { eb_probe_counts(handle) }
    func checkReleased() {
        // This is a synchronization barrier, not a performance threshold.
        eb_probe_wait_for_worker(handle)
        let counts = counts
        precondition(counts.documents == 0 && counts.workers == 0)
        precondition(counts.snapshots == 0 && counts.snapshot_handles == 0)
        precondition(counts.completions == 0 && CDocument.livePendingCalls == 0)
    }
}

@MainActor
private func cancelledAfterEnqueue() async throws -> LifetimeProbe {
    let document = try CDocument(noteCount: 0)
    let probe = LifetimeProbe(document)
    let changes = Mutex(0)
    document.pause()
    let pending = Task.detached { [weak document] in
        guard let document else { preconditionFailure("driver owns the document until completion") }
        let result = try await document.addTrack("cancel after enqueue", observer: { _ in changes.withLock { $0 += 1 } })
        precondition(result.withUnsafeNotes { $0.isEmpty })
        return result.revision
    }
    document.waitUntilPaused()
    pending.cancel()
    document.resume()
    let revision = try await pending.value
    precondition(revision == 1 && changes.withLock { $0 } == 1)
    return probe
}

@MainActor
private func failingWorkerDrainsReplies() async throws -> LifetimeProbe {
    let document = try CDocument(noteCount: 4)
    let probe = LifetimeProbe(document)
    let old = document.snapshot()
    document.pause()
    let first = Task.detached { [weak document] in
        guard let document else { preconditionFailure("driver owns the document until completion") }
        return try await document.addTrack("first").revision
    }
    document.waitUntilPaused()
    let (enqueued, signal) = AsyncStream<Void>.makeStream()
    let second = Task.detached { [weak document] in
        guard let document else { preconditionFailure("driver owns the document until completion") }
        return try await document.addTrack("queued", enqueued: { signal.yield(()); signal.finish() }).revision
    }
    for await _ in enqueued { break }
    document.failNext()
    document.resume()
    for task in [first, second] {
        do {
            _ = try await task.value
            preconditionFailure("a failed worker must finish every accepted call with an error")
        } catch CBridgeError.stopped {}
    }
    do {
        _ = try await document.addTrack("after failure")
        preconditionFailure("a failed worker cannot accept more edits")
    } catch CBridgeError.stopped {}
    precondition(document.snapshot().sharesProject(with: old))
    precondition(old.withUnsafeNotes { $0[3].start } == 2_880)
    return probe
}

@MainActor
private func closingQueueDrainsAcceptedWork() async throws -> LifetimeProbe {
    let document = try CDocument(noteCount: 1)
    let probe = LifetimeProbe(document)
    document.pause()
    let accepted = Task.detached { [weak document] in
        guard let document else { preconditionFailure("driver owns the document until completion") }
        return try await document.addTrack("accepted").revision
    }
    document.waitUntilPaused()
    document.closeQueue()
    do {
        _ = try await document.addTrack("refused")
        preconditionFailure("a closed queue must refuse new commands")
    } catch CBridgeError.stopped {}
    document.resume()
    let revision = try await accepted.value
    precondition(revision == 1)
    return probe
}

@MainActor
func checkBridgeContracts() async throws {
    // Repetition is a validation budget, never a product capacity limit.
    for _ in 0..<8 {
        try await cancelledAfterEnqueue().checkReleased()
        try await failingWorkerDrainsReplies().checkReleased()
        try await closingQueueDrainsAcceptedWork().checkReleased()
    }
    print("C ABI: PASS post-enqueue cancellation, panic/queue shutdown completion, empty views, all resource counts return to zero")
}
