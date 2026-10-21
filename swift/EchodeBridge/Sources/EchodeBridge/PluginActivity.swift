import CEchodeFfi
import Foundation

/// A change a plugin reported on its own, for example from its window.
public struct PluginParameterEvent: Decodable, Sendable, Equatable {
    public enum Kind: String, Decodable, Sendable {
        /// The plugin changed the value. Read-only parameters are not reported.
        case value
        /// The user started adjusting the parameter in the plugin's window.
        case begin
        /// The user finished adjusting it.
        case end
    }

    public let id: UInt32
    public let kind: Kind
    public init(id: UInt32, kind: Kind) { self.id = id; self.kind = kind }
}

/// What one plugin did since the previous entry for it.
public struct PluginActivity: Decodable, Sendable, Equatable {
    public let device: UUID
    /// Existing DAW shortcuts forwarded by the focused plugin window.
    public let commands: [String]
    /// Gestures and value changes in the order the plugin reported them.
    public let events: [PluginParameterEvent]
    /// The current value and text of each parameter with a value event.
    public let values: [DeviceParameterValue]
    /// The plugin asked the host to read its parameters again.
    public let rescan: Bool
    /// Some events were dropped; the list must be read again and the change
    /// treated as finished.
    public let lost: Bool
    /// The plugin marked its state changed while its window was open.
    public let dirty: Bool
    /// The user or the plugin closed the window.
    public let closed: Bool
    /// The plugin's window was open when these changes arrived. Only then is
    /// a change without a gesture the user's edit.
    public let windowOpen: Bool

    public init(device: UUID, events: [PluginParameterEvent] = [], values: [DeviceParameterValue] = [],
                rescan: Bool = false, lost: Bool = false, dirty: Bool = false, closed: Bool = false,
                windowOpen: Bool = false, commands: [String] = []) {
        self.device = device; self.events = events; self.values = values
        self.rescan = rescan; self.lost = lost; self.dirty = dirty; self.closed = closed
        self.windowOpen = windowOpen
        self.commands = commands
    }

    enum CodingKeys: String, CodingKey {
        case device, commands, events, values, rescan, lost, dirty, closed, windowOpen = "window"
    }
}

/// Activity after a sequence number. Pass `next` to the following read.
public struct PluginActivityPage: Decodable, Sendable, Equatable {
    public let next: UInt64
    /// The document closed; no more activity will come.
    public let closed: Bool
    public let entries: [PluginActivity]
    public init(next: UInt64, closed: Bool = false, entries: [PluginActivity] = []) {
        self.next = next; self.closed = closed; self.entries = entries
    }
}

extension Document {
    /// Waits up to `timeout` for plugin activity after `after`. Blocks the
    /// calling thread, so call it off the main thread. Never waits for the
    /// document thread.
    public func pluginActivity(after: UInt64, timeout: TimeInterval) throws -> PluginActivityPage {
        let milliseconds = UInt32(clamping: Int((max(0.0, timeout) * 1_000).rounded()))
        var capacity = 4_096
        while true {
            var data = Data(count: capacity)
            var written = 0
            let code = data.withUnsafeMutableBytes { raw in
                echode_document_plugin_activity(handle, after, milliseconds,
                                                raw.baseAddress?.assumingMemoryBound(to: UInt8.self), capacity, &written)
            }
            switch code {
            case 0: return try JSONDecoder().decode(PluginActivityPage.self, from: data.prefix(written))
            case 1: capacity = max(written, capacity * 2)
            default: throw BridgeError(message: "plugin activity is unavailable")
            }
        }
    }
}
