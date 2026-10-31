/// Shows ThreadSanitizer the order a completion's continuation already gives.
///
/// The resume happens before the code after `await`. When the resume wins that
/// race, the Swift runtime carries on without the acquire TSan needs, so all that
/// the resuming thread wrote looks unordered until the task next changes executor.
/// `check-bridge-tsan` passes `-D ECHODE_TSAN`; other builds keep no calls or storage.
#if ECHODE_TSAN
final class ContinuationHandoff: Sendable {
    /// Before `resume`. What the resuming thread writes later stays unordered.
    func release() { tsanRelease(Unmanaged.passUnretained(self).toOpaque()) }
    /// After `await`. Each call has its own address, so calls stay unordered.
    func acquire() { tsanAcquire(Unmanaged.passUnretained(self).toOpaque()) }
}

@_silgen_name("__tsan_release") private func tsanRelease(_ address: UnsafeMutableRawPointer)
@_silgen_name("__tsan_acquire") private func tsanAcquire(_ address: UnsafeMutableRawPointer)
#else
struct ContinuationHandoff: Sendable {
    @inline(__always) func release() {}
    @inline(__always) func acquire() {}
}
#endif
