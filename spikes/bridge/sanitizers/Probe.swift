import CBridgeSanitizerProbe
import Dispatch

@main
enum SanitizerProbe {
    static func main() {
        switch CommandLine.arguments.last {
        case "rust-memory": _ = eb_sanitizer_memory_fault()
        case "rust-thread": eb_sanitizer_thread_fault()
        case "swift-memory": swiftMemoryFault()
        case "swift-thread": swiftThreadFault()
        default:
            precondition(eb_sanitizer_clean() == 42)
            print("PASS sanitizer runtime and cross-language link")
        }
    }

    @inline(never)
    static func swiftMemoryFault() {
        // Deliberate use-after-free, only in an expected-failing child process.
        let pointer = UnsafeMutablePointer<UInt64>.allocate(capacity: 1)
        pointer.initialize(to: 42)
        pointer.deallocate()
        print(pointer.pointee)
    }

    @inline(never)
    static func swiftThreadFault() {
        let pointer = UnsafeMutablePointer<UInt64>.allocate(capacity: 1)
        pointer.initialize(to: 0)
        // Passing the numeric address deliberately bypasses Sendable here.
        // This fixture exists to prove that TSan detects the resulting race.
        let address = UInt(bitPattern: pointer)
        DispatchQueue.concurrentPerform(iterations: 2) { index in
            let shared = UnsafeMutablePointer<UInt64>(bitPattern: address)!
            for _ in 0..<100_000 { shared.pointee = UInt64(index) }
        }
        pointer.deallocate()
    }
}
