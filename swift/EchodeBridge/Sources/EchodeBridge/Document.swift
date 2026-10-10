import CEchodeFfi

/// A command the document thread rejected, or a package that could not be opened.
public struct BridgeError: Error, Sendable {
    public let message: String
}

/// One owned snapshot. The project stays readable after the document is closed.
///
/// The handle is an immutable `Arc` published by the document thread. This class
/// is `Sendable` because that storage does not change and Rust frees it once.
public final class Snapshot: @unchecked Sendable {
    private let handle: OpaquePointer

    fileprivate init(taking handle: OpaquePointer) {
        self.handle = handle
    }

    deinit {
        echode_snapshot_free(handle)
    }

    public var trackCount: UInt32 {
        echode_snapshot_track_count(handle)
    }
}

private final class PendingCall: Sendable {
    let continuation: CheckedContinuation<Snapshot, any Error>

    init(_ continuation: CheckedContinuation<Snapshot, any Error>) {
        self.continuation = continuation
    }
}

private func completeCommand(
    _ context: UnsafeMutableRawPointer?,
    _ status: Int32,
    _ message: UnsafePointer<CChar>?,
    _ snapshot: OpaquePointer?
) {
    let pending = Unmanaged<PendingCall>.fromOpaque(context!).takeRetainedValue()
    if status == ECHODE_OK, let snapshot {
        pending.continuation.resume(returning: Snapshot(taking: snapshot))
    } else {
        if let snapshot {
            echode_snapshot_free(snapshot)
        }
        let text = message.map { String(cString: $0) } ?? "command failed"
        pending.continuation.resume(throwing: BridgeError(message: text))
    }
}

/// A package owned by the document thread.
///
/// Commands are queued and this call returns. Rust applies them on its thread
/// and completes exactly once. The pointer is `Sendable` for the same reason as
/// `Snapshot`: Swift does not mutate it, and the queue serializes edits.
public final class Document: @unchecked Sendable {
    private let handle: OpaquePointer

    /// Creates an empty package at `path` and opens it.
    public init(creating path: String) throws {
        let handle: OpaquePointer? = path.withCString { echode_document_create($0) }
        guard let handle else {
            throw BridgeError(message: "could not create the package")
        }
        self.handle = handle
    }

    deinit {
        echode_document_close(handle)
    }

    /// Queues `json`. A cancellation that arrives before the command is sent
    /// does not send it. A cancellation after that does not undo the edit.
    public func perform(_ json: String) async throws -> Snapshot {
        try Task.checkCancellation()
        return try await withCheckedThrowingContinuation { continuation in
            let pending = PendingCall(continuation)
            let context = Unmanaged.passRetained(pending).toOpaque()
            let bytes = Array(json.utf8)
            bytes.withUnsafeBufferPointer { buffer in
                _ = echode_document_submit(
                    handle,
                    buffer.baseAddress,
                    buffer.count,
                    context,
                    completeCommand
                )
            }
        }
    }

    /// Finishes commands already queued, then rejects new ones.
    public func shutdown() {
        echode_document_shutdown(handle)
    }
}
