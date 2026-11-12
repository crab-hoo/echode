import CEchodeFfi
import Foundation

/// The product log (ADR 0322). The Rust core writes each record to the unified
/// log (subsystem `io.echode`) and appends it to an NDJSON file that
/// `make debug-report` copies. Never write from an audio callback.
public enum EchodeLog {
    public enum Level: UInt32, Sendable {
        case error = 1
        case warning = 2
        case info = 3
        case debug = 4
    }

    /// The app's file beside the panic and audition logs.
    public static var file: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/Echode/echode.ndjson")
    }

    /// The size past which every diagnostic NDJSON file moves aside.
    public static let fileLimit = UInt64(ECHODE_LOG_FILE_LIMIT)

    /// Installs the core's log. Only the first call in a process counts; nil
    /// keeps the unified log without a file.
    public static func install(at file: URL?) {
        if let file {
            file.path.withCString { echode_log_install($0) }
        } else {
            echode_log_install(nil)
        }
    }

    /// Logs `message` with the target `app::<category>`.
    public static func write(_ level: Level, _ category: String, _ message: String) {
        category.withCString { category in
            message.withCString { message in
                _ = echode_log_write(level.rawValue, category, message)
            }
        }
    }

    /// Waits until the file holds every record logged so far, up to two seconds.
    public static func flush() { echode_log_flush() }

    /// Appends whole NDJSON lines to `file` under the rotation rule every
    /// diagnostic log shares: past `limit` bytes the file moves to
    /// `<name>.1.<extension>` first.
    @discardableResult
    public static func append(_ lines: Data, to file: URL, limit: UInt64 = fileLimit) -> Bool {
        file.path.withCString { path in
            lines.withUnsafeBytes { bytes in
                echode_log_append_lines(path, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count, limit)
            }
        }
    }
}
