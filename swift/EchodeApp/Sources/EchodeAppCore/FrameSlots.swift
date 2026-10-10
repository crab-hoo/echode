import Synchronization

/// Three drawable frames can be in flight. This bounds GPU ownership, not the
/// number of tracks or instances; each slot's buffer grows to the visible data.
final class FrameSlots: Sendable {
    private let available = Mutex([0, 1, 2])

    func acquire() -> Int? { available.withLock { $0.popLast() } }
    func release(_ slot: Int) { available.withLock { $0.append(slot) } }
}
