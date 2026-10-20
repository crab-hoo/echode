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

    public init(id: UInt32, flags: UInt32 = 0, name: String, module: String = "", min: Double, max: Double,
                defaultValue: Double, value: Double, text: String) {
        self.id = id; self.flags = flags; self.name = name; self.module = module
        self.min = min; self.max = max; self.defaultValue = defaultValue; self.value = value; self.text = text
    }

    enum CodingKeys: String, CodingKey { case id, flags, name, module, min, max, value, text, defaultValue = "default" }
}

/// A value read back after the plugin applied a change.
public struct DeviceParameterValue: Decodable, Sendable, Equatable {
    public let id: UInt32
    public let value: Double
    public let text: String
    public init(id: UInt32, value: Double, text: String) { self.id = id; self.value = value; self.text = text }
}

public struct DeviceParameterChange: Encodable, Sendable, Equatable {
    public let id: UInt32
    public let value: Double
    public init(id: UInt32, value: Double) { self.id = id; self.value = value }
}

/// What a device command returned. Only the field for that command is set.
public struct DeviceReport: Decodable, Sendable, Equatable {
    public let parameters: [DeviceParameter]?
    public let values: [DeviceParameterValue]?
    /// A commit recorded new plugin bytes as one Undo step.
    public let changed: Bool?
    public init(parameters: [DeviceParameter]? = nil, values: [DeviceParameterValue]? = nil, changed: Bool? = nil) {
        self.parameters = parameters; self.values = values; self.changed = changed
    }
}

public enum DeviceCommand: Encodable, Sendable, Equatable {
    /// Every parameter of the running plugin.
    case parameters(device: UUID)
    /// Values for the running plugin. The project is unchanged.
    case set(device: UUID, changes: [DeviceParameterChange])
    /// Records the plugin's current bytes when they differ from the project.
    case commit(device: UUID)

    enum CodingKeys: String, CodingKey { case type, device, changes }

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
