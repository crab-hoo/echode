import Foundation

/// Submitted-frame callbacks can outlive their native renderer, or arrive
/// after newer callbacks. Never publish those into a replacement view state.
struct FrameFeedbackGate {
    private var generation: UUID?
    private var revision: UInt64 = 0
    mutating func restart() -> UUID {
        let token = UUID()
        generation = token
        revision = 0
        return token
    }
    mutating func invalidate() { generation = nil }
    mutating func accept(_ token: UUID, revision: UInt64) -> Bool {
        guard generation == token, revision >= self.revision else { return false }
        self.revision = revision
        return true
    }
}
