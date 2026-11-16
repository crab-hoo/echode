/// Owned, control-only CLAP port details. No pointer crosses the bridge.
public enum DeviceAudioPortDetails: Encodable, Sendable, Equatable {
    case surround(channelMap: [UInt8])
    case ambisonic(ordering: UInt32, normalization: UInt32)

    private enum CodingKeys: String, CodingKey {
        case type
        case channelMap = "channel_map"
        case ordering
        case normalization
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .surround(let map):
            try container.encode("surround", forKey: .type)
            try container.encode(map, forKey: .channelMap)
        case .ambisonic(let ordering, let normalization):
            try container.encode("ambisonic", forKey: .type)
            try container.encode(ordering, forKey: .ordering)
            try container.encode(normalization, forKey: .normalization)
        }
    }
}
