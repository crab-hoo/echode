import EchodeBridge
import Foundation
import Synchronization

/// The document is still private to open(), with workspace input locked.
public struct RecoveryPrompt: Sendable, Equatable {
    public let url: URL
    let id: UUID
    public let notice: RecoveryNotice
    public var title: String { text("The latest saved version could not be read") }
    public var message: String {
        "\(url.lastPathComponent)\n\n" + text("An older version was recovered. Recent edits may be missing. Cancel and preserve a copy of the project before continuing. Continuing saves the recovered version as a new checkpoint.")
            + "\n\n\(text("Unreadable version")): \(notice.tip)\n\(text("Recovered version")): \(notice.base)\n\(text("Applied journal sequence")): \(notice.applied_seq)"
    }
}

/// EditTurn uses an ordered task: cancellation must also reach that task's open.
final class RecoveryOpenRequest: Sendable {
    let id = UUID()
    private let cancelled = Mutex(false)
    var isCancelled: Bool { cancelled.withLock { $0 } }
    func cancel() { cancelled.withLock { $0 = true } }
}

extension ProjectController {
    func confirmRecovery(_ prompt: RecoveryPrompt, request: RecoveryOpenRequest) async -> Bool {
        guard !request.isCancelled else { return false }
        return await withCheckedContinuation { reply in
            recoveryReply = reply
            recoveryPrompt = prompt
        }
    }

    /// Called only by the explicit alert choices, cancellation or view teardown.
    public func respondToRecovery(_ prompt: RecoveryPrompt? = nil, accept: Bool) {
        if let prompt, recoveryPrompt?.id != prompt.id { return }
        let reply = recoveryReply
        recoveryReply = nil
        recoveryPrompt = nil
        reply?.resume(returning: accept)
    }
}
