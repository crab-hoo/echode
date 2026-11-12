import CEchodeFfi
import Foundation

/// One CLAP parameter as the running plugin describes it. Values are in the
/// plugin's own units between `min` and `max`; `text` is the plugin's display.
public struct DeviceParameter: Decodable, Sendable, Equatable, Identifiable {
    public let id: UInt32
    public let flags: UInt32
    public let name: String
    public let module: String
    public let min: Double
    public let max: Double
    public let defaultValue: Double
    public var value: Double
    public var text: String
    /// CLAP flag bits, as the helper reported them.
    public var stepped: Bool { flags & (1 << 0) != 0 }
    public var hidden: Bool { flags & (1 << 2) != 0 }
    public var readOnly: Bool { flags & (1 << 3) != 0 }
    public var isEnum: Bool { flags & (1 << 16) != 0 }

    public init(
        id: UInt32, flags: UInt32 = 0, name: String, module: String = "", min: Double, max: Double,
        defaultValue: Double, value: Double, text: String
    ) {
        self.id = id
        self.flags = flags
        self.name = name
        self.module = module
        self.min = min
        self.max = max
        self.defaultValue = defaultValue
        self.value = value
        self.text = text
    }

    enum CodingKeys: String, CodingKey {
        case id, flags, name, module, min, max, value, text
        case defaultValue = "default"
    }
}

/// A value read back after the plugin applied a change.
public struct DeviceParameterValue: Decodable, Sendable, Equatable {
    public let id: UInt32
    public let value: Double
    public let text: String
    public init(id: UInt32, value: Double, text: String) {
        self.id = id
        self.value = value
        self.text = text
    }
}

/// How the plugin read typed text for one parameter. Nothing changed in it.
public struct DeviceParameterParse: Decodable, Sendable, Equatable {
    public let id: UInt32
    /// The value in the plugin's units. Nil when the plugin refused the text
    /// or has no converter.
    public let value: Double?
    /// False when the plugin has no `text_to_value`: read a plain number.
    public let converts: Bool
    public init(id: UInt32, value: Double?, converts: Bool) {
        self.id = id
        self.value = value
        self.converts = converts
    }
}

public struct DeviceParameterChange: Encodable, Sendable, Equatable {
    public let id: UInt32
    public let value: Double
    public init(id: UInt32, value: Double) {
        self.id = id
        self.value = value
    }
}

/// What showing a plugin's own window did in its helper.
public struct DeviceWindowReport: Decodable, Sendable, Equatable {
    /// This request created the window. False when an open window was raised.
    public let created: Bool
    /// On screen. False while the window waits for Echode to be the active app.
    public let visible: Bool
    /// The window took keyboard focus. Showing never does.
    public let key: Bool
    public init(created: Bool, visible: Bool, key: Bool) {
        self.created = created
        self.visible = visible
        self.key = key
    }
}

/// What a device command returned. Only the field for that command is set.
public struct DeviceReport: Decodable, Sendable, Equatable {
    public let parameters: [DeviceParameter]?
    public let values: [DeviceParameterValue]?
    /// A commit recorded new plugin bytes as one Undo step.
    public let changed: Bool?
    public let window: DeviceWindowReport?
    /// A close request finished; there is no window now.
    public let closed: Bool?
    public let parsed: DeviceParameterParse?
    /// Device-rate samples declared by the helper; not yet compensated.
    public let latencySamples: UInt32?
    public init(
        parameters: [DeviceParameter]? = nil, values: [DeviceParameterValue]? = nil, changed: Bool? = nil,
        window: DeviceWindowReport? = nil, closed: Bool? = nil, parsed: DeviceParameterParse? = nil,
        latencySamples: UInt32? = nil
    ) {
        self.parameters = parameters
        self.values = values
        self.changed = changed
        self.window = window
        self.closed = closed
        self.parsed = parsed
        self.latencySamples = latencySamples
    }
    enum CodingKeys: String, CodingKey {
        case parameters, values, changed, window, closed, parsed
        case latencySamples = "latency_samples"
    }
}

public enum DeviceCommand: Encodable, Sendable, Equatable {
    /// Every parameter of the running plugin.
    case parameters(device: UUID)
    /// Values for the running plugin. The project is unchanged.
    case set(device: UUID, changes: [DeviceParameterChange])
    /// Records the plugin's current bytes when they differ from the project.
    case commit(device: UUID)
    /// Reads typed text in the plugin's units. Nothing changes.
    case parse(device: UUID, id: UInt32, text: String)
    /// Opens the plugin's own window, or raises it, without taking focus.
    case showWindow(device: UUID, title: String)
    /// Closes the plugin's own window. Never starts a helper to do it.
    case closeWindow(device: UUID)

    enum CodingKeys: String, CodingKey { case type, device, changes, title, id, text }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .parameters(let device):
            try container.encode("device.parameters", forKey: .type)
            try container.encode(device, forKey: .device)
        case .set(let device, let changes):
            try container.encode("device.parameter.set", forKey: .type)
            try container.encode(device, forKey: .device)
            try container.encode(changes, forKey: .changes)
        case .commit(let device):
            try container.encode("device.parameter.commit", forKey: .type)
            try container.encode(device, forKey: .device)
        case .parse(let device, let id, let text):
            try container.encode("device.parameter.parse", forKey: .type)
            try container.encode(device, forKey: .device)
            try container.encode(id, forKey: .id)
            try container.encode(text, forKey: .text)
        case .showWindow(let device, let title):
            try container.encode("device.gui.show", forKey: .type)
            try container.encode(device, forKey: .device)
            try container.encode(title, forKey: .title)
        case .closeWindow(let device):
            try container.encode("device.gui.close", forKey: .type)
            try container.encode(device, forKey: .device)
        }
    }
}

extension Snapshot {
    /// The report of the device command this snapshot completed. Others are empty.
    public func deviceReport() throws -> DeviceReport {
        try withExtendedLifetime(self) {
            var count = 0
            guard let bytes = echode_snapshot_device(handle, &count) else {
                throw BridgeError(message: "device report is unavailable")
            }
            return try JSONDecoder().decode(DeviceReport.self, from: Data(bytes: bytes, count: count))
        }
    }
}
