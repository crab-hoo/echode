import CEchodeBridge
import Synchronization

struct CChange: Sendable {
    let revision: UInt64
    let trackCount: UInt64
}

enum CBridgeError: Error {
    case emptyName, invalidUTF8, stopped, invalidResult
}

// Rust owns immutable Project and note storage behind an Arc. Swift owns one
// handle, releases it exactly once, and never exposes a mutable pointer.
final class CSnapshot: @unchecked Sendable {
    private let handle: OpaquePointer

    fileprivate init(taking handle: OpaquePointer) {
        self.handle = handle
    }

    deinit { eb_snapshot_free(handle) }

    var revision: UInt64 { eb_snapshot_revision(handle) }
    var trackCount: UInt64 { eb_snapshot_track_count(handle) }
    var noteCount: Int { eb_snapshot_note_count(handle) }

    func retained() -> CSnapshot {
        CSnapshot(taking: eb_snapshot_retain(handle)!)
    }

    func sharesProject(with other: CSnapshot) -> Bool {
        eb_snapshot_shares_project(handle, other.handle)
    }

    // As with Swift's other withUnsafe APIs, the borrowed pointer must not escape.
    // This synchronous closure cannot suspend. Retain self for the entire read.
    func withUnsafeNotes<T>(_ body: (UnsafeBufferPointer<BridgeNote>) throws -> T) rethrows -> T {
        try withExtendedLifetime(self) {
            try body(UnsafeBufferPointer(start: eb_snapshot_notes(handle), count: noteCount))
        }
    }
}

private final class PendingCall: Sendable {
    private static let live = Mutex(0)
    static var liveCount: Int { live.withLock { $0 } }
    let continuation: CheckedContinuation<CSnapshot, any Error>
    let observer: @Sendable (CChange) -> Void

    init(_ continuation: CheckedContinuation<CSnapshot, any Error>,
         _ observer: @escaping @Sendable (CChange) -> Void) {
        self.continuation = continuation
        self.observer = observer
        Self.live.withLock { $0 += 1 }
    }
    deinit { Self.live.withLock { $0 -= 1 } }
}

// The context is retained before enqueueing and consumed exactly once here.
// Rust guarantees this completion even for a rejected command or a stopped queue.
private func completeTrack(_ context: UnsafeMutableRawPointer?, _ status: BridgeStatus,
                           _ handle: OpaquePointer?) {
    let pending = Unmanaged<PendingCall>.fromOpaque(context!).takeRetainedValue()
    if status == Ok, let handle {
        let snapshot = CSnapshot(taking: handle)
        pending.observer(CChange(revision: snapshot.revision, trackCount: snapshot.trackCount))
        pending.continuation.resume(returning: snapshot)
    } else {
        if let handle { eb_snapshot_free(handle) }
        let error: CBridgeError
        switch status {
        case EmptyName: error = .emptyName
        case InvalidUtf8: error = .invalidUTF8
        case Stopped: error = .stopped
        default: error = .invalidResult
        }
        pending.continuation.resume(throwing: error)
    }
}

// Only immutable ownership is stored in Swift. Rust serializes commands on its
// control thread and locks only while cloning the currently published Arc.
final class CDocument: @unchecked Sendable {
    private let handle: OpaquePointer

    init(noteCount: UInt32) throws {
        guard let handle = eb_document_new(noteCount) else { throw CBridgeError.stopped }
        self.handle = handle
    }
    deinit { eb_document_free(handle) }

    func snapshot() -> CSnapshot { CSnapshot(taking: eb_document_snapshot(handle)!) }
    var liveSnapshots: UInt64 { eb_document_live_snapshots(handle) }

    func addTrack(_ name: String,
                  observer: @escaping @Sendable (CChange) -> Void = { _ in },
                  enqueued: @escaping @Sendable () -> Void = {}) async throws -> CSnapshot {
        try Task.checkCancellation()
        // Cancellation after enqueueing does not roll back a committed edit.
        // The continuation still consumes the completion and its owned handle.
        return try await withCheckedThrowingContinuation { continuation in
            let pending = PendingCall(continuation, observer)
            let context = Unmanaged.passRetained(pending).toOpaque()
            Array(name.utf8).withUnsafeBufferPointer { bytes in
                eb_document_add_track(handle, bytes.baseAddress, bytes.count, context, completeTrack)
            }
            enqueued()
        }
    }

    #if BRIDGE_VALIDATION
    func probeHandle() -> OpaquePointer { eb_probe_new(handle)! }
    func pause() { eb_document_pause(handle) }
    func waitUntilPaused() { eb_document_wait_until_paused(handle) }
    func resume() { eb_document_resume(handle) }
    func failNext() { eb_document_fail_next(handle) }
    func closeQueue() { eb_document_close_queue(handle) }
    static var livePendingCalls: Int { PendingCall.liveCount }
    #endif
}
