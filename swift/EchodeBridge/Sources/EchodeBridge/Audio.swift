import CEchodeFfi
import Foundation

public struct OutputDevice: Decodable, Sendable, Equatable, Identifiable {
    public let id: UInt32
    public let uid: String
    public let name: String
    public let sampleRate: UInt32
    public let bufferFrames: UInt32
    public let channels: UInt32
    enum CodingKeys: String, CodingKey { case id, uid, name, channels, sampleRate = "sample_rate", bufferFrames = "buffer_frames" }
}

public struct AudioState: Decodable, Sendable, Equatable {
    public let playing: Bool
    public let running: Bool
    public let tick: Int64
    public let selectedUID: String?
    public let activeDevice: OutputDevice?
    public let devices: [OutputDevice]
    public let error: String?
    public init() {
        playing = false; running = false; tick = 0; selectedUID = nil; activeDevice = nil; devices = []; error = nil
    }
    enum CodingKeys: String, CodingKey {
        case playing, running, tick, devices, error, selectedUID = "selected_uid", activeDevice = "active_device"
    }
}

public enum AudioCommand: Encodable, Sendable {
    case play, pause, stop, locate(tick: Int64), output(uid: String?), refresh, status
    enum CodingKeys: String, CodingKey { case type, tick, uid }
    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        let type: String
        switch self {
        case .play: type = "transport.play"
        case .pause: type = "transport.pause"
        case .stop: type = "transport.stop"
        case .locate(let tick): type = "transport.locate"; try container.encode(tick, forKey: .tick)
        case .output(let uid): type = "audio.output"; try container.encode(uid, forKey: .uid)
        case .refresh: type = "audio.refresh"
        case .status: type = "audio.status"
        }
        try container.encode(type, forKey: .type)
    }
}

public struct PlaybackPosition: Sendable {
    public let tick: Int64
    public let playing: Bool
}

/// Retains the document's live position storage while a renderer reads it.
/// The query is atomic and does not depend on MainActor polling.
public struct PlaybackFeed: Sendable {
    private let document: Document
    init(_ document: Document) { self.document = document }
    public var position: PlaybackPosition { document.playbackPosition }
}
