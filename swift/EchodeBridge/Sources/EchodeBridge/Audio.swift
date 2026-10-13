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
    public let loopStart: Int64?
    public let loopEnd: Int64?
    public let metronome: Bool
    public init() {
        playing = false; running = false; tick = 0; selectedUID = nil; activeDevice = nil; devices = []; error = nil
        loopStart = nil; loopEnd = nil; metronome = false
    }
    enum CodingKeys: String, CodingKey {
        case playing, running, tick, devices, error, metronome, selectedUID = "selected_uid", activeDevice = "active_device"
        case loopStart = "loop_start", loopEnd = "loop_end"
    }
}

public enum AudioCommand: Encodable, Sendable {
    case play, pause, stop, locate(tick: Int64), output(uid: String?), refresh, status
    /// Both ticks set the loop. Both nil clear it.
    case loop(start: Int64?, end: Int64?)
    case metronome(enabled: Bool)
    enum CodingKeys: String, CodingKey { case type, tick, uid, start, end, enabled }
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
        case .loop(let start, let end):
            type = "transport.loop"
            try container.encode(start, forKey: .start)
            try container.encode(end, forKey: .end)
        case .metronome(let enabled):
            type = "transport.metronome"
            try container.encode(enabled, forKey: .enabled)
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
public struct PlaybackFeed: Sendable, Equatable {
    private let document: Document
    init(_ document: Document) { self.document = document }
    public var position: PlaybackPosition { document.playbackPosition }
    public static func == (lhs: Self, rhs: Self) -> Bool { lhs.document === rhs.document }
}
