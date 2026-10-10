import Foundation

@MainActor
private final class UIChanges {
    var revisions: [UInt64] = []
    func record(_ change: BridgeChange) { revisions.append(change.revision) }
}

private final class Observer: ChangeObserver, Sendable {
    let delivery: AsyncStream<BridgeChange>.Continuation
    let document: BridgeDocument
    init(_ delivery: AsyncStream<BridgeChange>.Continuation, _ document: BridgeDocument) {
        self.delivery = delivery
        self.document = document
    }
    func changed(change: BridgeChange) {
        precondition(document.snapshot().revision() >= change.revision)
        delivery.yield(change)
    }
}

private func checkOldSnapshot(_ document: BridgeDocument, _ observer: Observer) async throws {
    let old = document.snapshot()
    let same = document.snapshot()
    precondition(old.sharesProject(other: same))
    let new = try await document.addTrack(name: "piano", observer: observer)
    precondition(new.revision() == 1 && new.trackCount() == 1)
    precondition(old.revision() == 0 && old.trackCount() == 0)
    precondition(!old.sharesProject(other: new))
    precondition(document.liveSnapshots() == 2)
    let sum = await Task.detached {
        old.copiedNotes().reduce(Int64(0)) { $0 &+ $1.start }
    }.value
    precondition(sum == 4_799_952_000_000)
}

#if ASYNC_CALLBACK
private final class AsyncObserver: AsyncChangeObserver, Sendable {
    let ui: UIChanges
    init(_ ui: UIChanges) { self.ui = ui }
    func changed(change: BridgeChange) async { await ui.record(change) }
}
#endif

@main
enum UniProbe {
    @MainActor
    static func main() async throws {
        // Fixture size only, not an upper bound on documents or snapshots.
        let document = BridgeDocument(noteCount: 100_000)
        let ui = UIChanges()
        let (stream, delivery) = AsyncStream<BridgeChange>.makeStream()
        let observer = Observer(delivery, document)
        let consume = Task { @MainActor in
            for await change in stream { ui.record(change) }
        }
        try await checkOldSnapshot(document, observer)
        precondition(document.liveSnapshots() == 1)
        let revisions = try await withThrowingTaskGroup(of: UInt64.self) { group in
            for index in 0..<32 {
                group.addTask {
                    let snapshot = try await document.addTrack(name: "track \(index)", observer: observer)
                    return snapshot.revision()
                }
            }
            var result: [UInt64] = []
            for try await revision in group { result.append(revision) }
            return result.sorted()
        }
        delivery.finish()
        await consume.value
        precondition(revisions == Array(2...33).map(UInt64.init))
        precondition(ui.revisions == Array(1...33).map(UInt64.init))
        precondition(document.liveSnapshots() == 1)
        do {
            _ = try await document.addTrack(name: "", observer: observer)
            preconditionFailure("empty names must fail")
        } catch BridgeError.EmptyName {}
        precondition(document.snapshot().revision() == 33)

        func orphan() -> BridgeSnapshot { BridgeDocument(noteCount: 1).snapshot() }
        precondition(orphan().copiedNotes().first?.length == 480)
        print("UniFFI: PASS async, callbacks -> MainActor, 32 concurrent edits, errors, old snapshots, release")
        #if ASYNC_CALLBACK
        await notifyAsync(observer: AsyncObserver(ui))
        precondition(ui.revisions.last == 42)
        print("UniFFI: PASS async foreign callback")
        #endif

        if CommandLine.arguments.contains("--bench") {
            let clock = ContinuousClock()
            let handles = clock.measure {
                for _ in 0..<1_000 {
                    precondition(document.snapshot().noteCount() == 100_000)
                }
            }
            let arrays = clock.measure {
                for _ in 0..<100 {
                    let copied = document.snapshot().copiedNotes()
                    precondition(copied.reduce(Int64(0)) { $0 &+ $1.start } == 4_799_952_000_000)
                }
            }
            print("UniFFI: 1,000 snapshot handles + count reads: \(handles)")
            print("UniFFI: 100 copies + scans of a 100,000-note array: \(arrays)")
        }
    }
}
