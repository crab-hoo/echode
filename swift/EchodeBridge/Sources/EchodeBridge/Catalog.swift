import CEchodeFfi
import Foundation

public struct PluginListing: Decodable, Identifiable, Sendable, Equatable {
    public let id: String
    public let name: String
    public let vendor: String
    public let version: String
    public let features: [String]
    /// True when this id appeared after the watch had already published a list.
    public let fresh: Bool
}

public enum PluginCatalog {
    /// Starts the folder watch. A later call returns the list the watch last published.
    public static func startWatch() throws {
        guard echode_plugin_watch_start() == 0 else { throw BridgeError(message: "plugin scan failed") }
    }

    /// Blocks until the published revision changes, or `timeout` elapses.
    public static func wait(after revision: UInt64, timeout: TimeInterval) -> UInt64 {
        let milliseconds = UInt32(clamping: Int((max(0.0, timeout) * 1_000).rounded()))
        return echode_plugin_watch_wait(revision, milliseconds)
    }

    /// Scans installed CLAP bundles in the helper process and returns their descriptors.
    public static func listings() throws -> [PluginListing] {
        var needed = 0
        let probe = echode_plugin_catalog(nil, 0, &needed)
        if probe == -1 { throw BridgeError(message: "plugin scan failed") }
        if needed == 0 { return [] }
        var data = Data(count: needed)
        var written = 0
        let code = data.withUnsafeMutableBytes { raw in
            echode_plugin_catalog(raw.baseAddress?.assumingMemoryBound(to: UInt8.self), needed, &written)
        }
        guard code == 0 else { throw BridgeError(message: "plugin scan failed") }
        return try JSONDecoder().decode([PluginListing].self, from: data.prefix(written))
    }
}
