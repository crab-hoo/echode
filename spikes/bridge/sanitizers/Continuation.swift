import Foundation
import Synchronization

/// Controls for `ContinuationHandoff`, with a resume that always beats `await`.
///
/// Nothing else may order the two threads for TSan, or the controls prove nothing:
/// the flag is relaxed, is not a lazy global (`swift_once` is an edge from the
/// first thread to touch it) and is never freed (a free would race the store).
/// The read stays in the awaiting function: returning to the main actor hops, and
/// the hop shows TSan the order without any annotation.
@main
enum ContinuationProbe {
    static func main() async {
        switch CommandLine.arguments.last {
        case "unannotated": print(await readAfterResume(annotated: false, lateWrite: false))
        case "annotated": print(await readAfterResume(annotated: true, lateWrite: false))
        case "late-write": print(await readAfterResume(annotated: true, lateWrite: true))
        default: preconditionFailure("unknown mode")
        }
    }

    static func readAfterResume(annotated: Bool, lateWrite: Bool) async -> UInt64 {
        let handoff = ContinuationHandoff()
        let resumed = Unmanaged.passRetained(Flag()).takeUnretainedValue()
        let value: Value = await withCheckedContinuation { continuation in
            Thread {
                let value = Value()
                if annotated { handoff.release() }
                continuation.resume(returning: value)
                if lateWrite { writeAfterResume(value) }
                resumed.state.store(true, ordering: .relaxed)
            }.start()
            while !resumed.state.load(ordering: .relaxed) {}
        }
        if annotated { handoff.acquire() }
        return value.number
    }

    @inline(never)
    static func writeAfterResume(_ value: Value) { value.number = 43 }

    final class Value: @unchecked Sendable {
        var number: UInt64 = 42
    }

    final class Flag: Sendable {
        let state = Atomic(false)
    }
}
