import CEchodeFfi
import Foundation

public struct PluginListing: Decodable, Identifiable, Sendable, Equatable {
    public let id: String
    public let name: String
    public let vendor: String
    public let version: String
    public let features: [String]
}

public enum PluginCatalog {
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
