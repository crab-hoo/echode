import CEchodeFfi
import Foundation

/// Where the Rust core appends panics (ADR 0238). Without it, a panic in an
/// FFI call aborts the app with its reason only on stderr.
public enum PanicLog {
    /// Installs the core's panic hook. Only the first call in a process counts.
    public static func keep(at file: URL) {
        file.path.withCString { echode_keep_panic_log($0) }
    }
}
