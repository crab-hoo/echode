import CEchodeBridge
import Foundation

@MainActor
private final class UIChanges {
    var revisions: [UInt64] = []
    func record(_ change: CChange) { revisions.append(change.revision) }
}

private func checkOldSnapshot(_ document: CDocument) async throws {
    let old = document.snapshot()
    let retained = old.retained()
    precondition(old.sharesProject(with: retained))
    let new = try await document.addTrack("piano")
    precondition(new.revision == 1 && new.trackCount == 1)
    precondition(old.revision == 0 && old.trackCount == 0)
    precondition(!old.sharesProject(with: new))
    precondition(document.liveSnapshots == 2)
    // Read the old snapshot on a different executor after publishing a new one.
    let sum = await Task.detached { old.withUnsafeNotes { $0.reduce(Int64(0)) { $0 &+ $1.start } } }.value
    precondition(sum == 4_799_952_000_000)
    precondition(retained.withUnsafeNotes { $0.last?.pitch } == 31)
}

@main
enum CProbe {
    @MainActor
    static func main() async throws {
        // 100,000 is this measurement's fixture size, never a product limit.
        let document = try CDocument(noteCount: 100_000)
        precondition(MemoryLayout<BridgeNote>.stride == 24)
        precondition(MemoryLayout<BridgeNote>.size == 24 && MemoryLayout<BridgeNote>.alignment == 8)
        precondition(MemoryLayout<BridgeNote>.offset(of: \.pitch) == 16)
        precondition(MemoryLayout<BridgeStatus>.size == 4)
        try await checkOldSnapshot(document)
        precondition(document.liveSnapshots == 1)

        let ui = UIChanges()
        let (stream, delivery) = AsyncStream<CChange>.makeStream()
        let consume = Task { @MainActor in
            for await change in stream { ui.record(change) }
        }
        let observer: @Sendable (CChange) -> Void = { change in
            // Reentering Rust from a callback must not deadlock.
            precondition(document.snapshot().revision >= change.revision)
            delivery.yield(change)
        }
        let revisions = try await withThrowingTaskGroup(of: UInt64.self) { group in
            for index in 0..<32 {
                group.addTask {
                    let snapshot = try await document.addTrack("track \(index)", observer: observer)
                    precondition(snapshot.noteCount == 100_000)
                    return snapshot.revision
                }
            }
            var result: [UInt64] = []
            for try await revision in group { result.append(revision) }
            return result.sorted()
        }
        delivery.finish()
        await consume.value
        precondition(revisions == Array(2...33).map(UInt64.init))
        precondition(ui.revisions == Array(2...33).map(UInt64.init))
        precondition(document.liveSnapshots == 1)
        do {
            _ = try await document.addTrack("", observer: observer)
            preconditionFailure("empty names must fail")
        } catch CBridgeError.emptyName {}
        precondition(document.snapshot().revision == 33)

        // A snapshot's lifetime is independent of the document wrapper's lifetime.
        func orphan() throws -> CSnapshot { try CDocument(noteCount: 1).snapshot() }
        let orphaned = try orphan()
        precondition(orphaned.withUnsafeNotes { $0.first?.length } == 480)

        // Already cancelled calls never enter the document queue.
        let (gate, start) = AsyncStream<Void>.makeStream()
        let cancelled = Task {
            for await _ in gate { break }
            return try await document.addTrack("cancelled")
        }
        cancelled.cancel()
        start.yield(())
        start.finish()
        do {
            _ = try await cancelled.value
            preconditionFailure("pre-enqueue cancellation must fail")
        } catch is CancellationError {}
        precondition(document.snapshot().revision == 33)
        print(
            "C ABI: PASS async, callbacks -> MainActor, 32 concurrent edits, errors, cancellation, old snapshots, release"
        )

        #if BRIDGE_VALIDATION
            // Finish the earlier callbacks before checking global Swift context counts.
            let primaryProbe = document.probeHandle()
            document.closeQueue()
            eb_probe_wait_for_worker(primaryProbe)
            eb_probe_free(primaryProbe)
            try await checkBridgeContracts()
        #endif

        if CommandLine.arguments.contains("--bench") {
            let clock = ContinuousClock()
            let elapsed = clock.measure {
                for _ in 0..<1_000 {
                    let snapshot = document.snapshot()
                    precondition(snapshot.withUnsafeNotes { $0[99_999].start } == 95_999_040)
                }
            }
            print("C ABI: 1,000 snapshot handles + borrowed array reads: \(elapsed)")
            let scans = clock.measure {
                for _ in 0..<100 {
                    let sum = document.snapshot().withUnsafeNotes {
                        $0.reduce(Int64(0)) { $0 &+ $1.start }
                    }
                    precondition(sum == 4_799_952_000_000)
                }
            }
            print("C ABI: 100 borrowed scans of a 100,000-note array: \(scans)")
        }
    }
}
