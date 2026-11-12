import CEchodeFfi
import Foundation

public struct OutputDevice: Decodable, Sendable, Equatable, Identifiable {
    public let id: UInt32
    public let uid: String
    public let name: String
    public let sampleRate: UInt32
    public let bufferFrames: UInt32
    public let channels: UInt32
    enum CodingKeys: String, CodingKey {
        case id, uid, name, channels
        case sampleRate = "sample_rate"
        case bufferFrames = "buffer_frames"
    }
}

/// One reply of the output, in the three parts that change at different rates.
/// Playback polls move the position; each sample audition request moves the
/// audition; the status changes on commands, devices and automatic mutes.
/// The app observes each part apart, so one part's readers ignore the others' changes (ADR 0289).
@dynamicMemberLookup
public struct AudioState: Decodable, Sendable, Equatable {
    public let status: AudioStatus
    /// The sample audition beside the project output, when one was ever requested.
    public let audition: AuditionStatus?
    public let tick: Int64
    public init() {
        status = AudioStatus()
        audition = nil
        tick = 0
    }
    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        tick = try container.decode(Int64.self, forKey: .tick)
        audition = try container.decodeIfPresent(AuditionStatus.self, forKey: .audition)
        status = try AudioStatus(from: decoder)
    }
    public subscript<Value>(dynamicMember keyPath: KeyPath<AudioStatus, Value>) -> Value { status[keyPath: keyPath] }
    enum CodingKeys: String, CodingKey { case tick, audition }
}

/// What the output reports apart from the position and the audition.
public struct AudioStatus: Decodable, Sendable, Equatable {
    public let playing: Bool
    public let running: Bool
    /// The remembered playback start. Stop returns here (ADR 0155).
    public let start: Int64
    /// A pause holds the playhead away from the start until play or stop.
    public let paused: Bool
    /// Helpers and media are being prepared for the shared audio output.
    /// Independent of transport intent and of each device's parameter reply.
    public let opening: Bool
    public let selectedUID: String?
    public let activeDevice: OutputDevice?
    public let devices: [OutputDevice]
    public let error: String?
    public let loopStart: Int64?
    public let loopEnd: Int64?
    public let metronome: Bool
    /// Tracks the output muted by itself because a plugin returned a NaN or an
    /// infinity. Not the tracks' own mute: starting playback clears it (ADR 0282).
    public let autoMutedTracks: [TrackAutoMute]
    /// The output muted the master because it ran past +18 dBFS (ADR 0282).
    public let masterAutoMute: MasterAutoMute?
    public init() {
        playing = false
        running = false
        opening = false
        start = 0
        paused = false
        selectedUID = nil
        activeDevice = nil
        devices = []
        error = nil
        loopStart = nil
        loopEnd = nil
        metronome = false
        autoMutedTracks = []
        masterAutoMute = nil
    }
    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        playing = try container.decode(Bool.self, forKey: .playing)
        running = try container.decode(Bool.self, forKey: .running)
        // Stand-in replies written before the start was reported omit these.
        start = try container.decodeIfPresent(Int64.self, forKey: .start) ?? 0
        paused = try container.decodeIfPresent(Bool.self, forKey: .paused) ?? false
        opening = try container.decode(Bool.self, forKey: .opening)
        selectedUID = try container.decodeIfPresent(String.self, forKey: .selectedUID)
        activeDevice = try container.decodeIfPresent(OutputDevice.self, forKey: .activeDevice)
        devices = try container.decode([OutputDevice].self, forKey: .devices)
        error = try container.decodeIfPresent(String.self, forKey: .error)
        loopStart = try container.decodeIfPresent(Int64.self, forKey: .loopStart)
        loopEnd = try container.decodeIfPresent(Int64.self, forKey: .loopEnd)
        metronome = try container.decode(Bool.self, forKey: .metronome)
        // Stand-in replies written before automatic mutes were reported omit these.
        autoMutedTracks = try container.decodeIfPresent([TrackAutoMute].self, forKey: .autoMutedTracks) ?? []
        masterAutoMute = try container.decodeIfPresent(MasterAutoMute.self, forKey: .masterAutoMute)
    }
    enum CodingKeys: String, CodingKey {
        case playing, running, opening, start, paused, devices, error, metronome
        case selectedUID = "selected_uid"
        case activeDevice = "active_device"
        case loopStart = "loop_start"
        case loopEnd = "loop_end"
        case autoMutedTracks = "auto_muted_tracks"
        case masterAutoMute = "master_auto_mute"
    }
}

/// Why the output muted one track by itself (ADR 0282).
public struct TrackAutoMute: Decodable, Sendable, Equatable {
    public let track: UUID
    /// The device that returned the NaN or infinity.
    public let device: UUID
    /// The project sample of the first such value.
    public let sample: Int64
    public init(track: UUID, device: UUID, sample: Int64) {
        self.track = track
        self.device = device
        self.sample = sample
    }
}

/// Why the output muted the master by itself (ADR 0282).
public struct MasterAutoMute: Decodable, Sendable, Equatable {
    /// The first amplitude past the guard; `nil` for a NaN.
    public let peak: Float?
    public let sample: Int64
    public init(peak: Float?, sample: Int64) {
        self.peak = peak
        self.sample = sample
    }
}

/// The newest audition request and what became of it.
public struct AuditionStatus: Decodable, Sendable, Equatable {
    /// `superseded` appears only in history: a newer request came before it was heard.
    public enum Phase: String, Codable, Sendable { case idle, loading, sounding, ended, stopped, failed, superseded }
    public let request: UInt64
    /// The newest request the audition thread has acted on.
    public let handled: UInt64
    public let path: String?
    public let phase: Phase
    public let error: String?
    public let sampleRate: UInt32?
    /// The latest requests, oldest first, with the time each stage finished.
    public let history: [AuditionEntry]
    public let waveform: AuditionWaveform?
    public let settings: AuditionSettings?
    public let outputError: String?
    enum CodingKeys: String, CodingKey {
        case request, handled, path, phase, error, history, waveform, settings
        case sampleRate = "sample_rate"
        case outputError = "output_error"
    }
}

public struct AuditionSettings: Decodable, Sendable, Equatable {
    public let visible: Bool
    public let gain: Float
}

/// Progressive source peaks from the audition decoder, shared with its sound.
public struct AuditionWaveform: Decodable, Sendable, Equatable {
    public let frames: UInt64
    public let loadedFrames: UInt64
    /// Earlier buckets have not been read by a request starting inside the file.
    public let firstFrame: UInt64?
    public let sampleRate: UInt32
    public let framesPerPeak: UInt64
    public let complete: Bool
    /// Minimum and maximum across the source channels for each time bucket.
    public let peaks: [[Float]]
    enum CodingKeys: String, CodingKey {
        case frames, complete, peaks
        case loadedFrames = "loaded_frames"
        case sampleRate = "sample_rate"
        case framesPerPeak = "frames_per_peak"
        case firstFrame = "first_frame"
    }
}

/// The whole file's peaks for the browser, read apart from any audition so a
/// selection shows its shape at once and keeps it after the sound stops
/// (ADR 0163). It opens no output and touches no document.
public enum SampleOverview: Sendable, Equatable {
    case waveform(AuditionWaveform)
    case failed(String)
    /// A read with a larger request number started before this one finished.
    case cancelled

    private struct Reply: Decodable {
        let waveform: AuditionWaveform?
        let error: String?
        let cancelled: Bool?
    }

    /// Blocks while it reads the file; call it off the main actor. A larger
    /// `request` on the same `channel` stops this read; other channels do not.
    public static func read(_ url: URL, channel: UInt64, request: UInt64) -> SampleOverview {
        let path = Array(url.path.utf8)
        // 512 peak pairs fit well inside this; a larger reply asks once more.
        var capacity = 65_536
        for _ in 0..<2 {
            var data = Data(count: capacity)
            var written = 0
            let code = path.withUnsafeBufferPointer { path in
                data.withUnsafeMutableBytes { raw in
                    echode_sample_overview(
                        path.baseAddress, path.count, channel, request,
                        raw.baseAddress?.assumingMemoryBound(to: UInt8.self), capacity, &written)
                }
            }
            if code == 1 {
                capacity = written
                continue
            }
            guard code == 0, let reply = try? JSONDecoder().decode(Reply.self, from: data.prefix(written)) else {
                return .failed("sample overview failed")
            }
            if let waveform = reply.waveform { return .waveform(waveform) }
            if reply.cancelled == true { return .cancelled }
            return .failed(reply.error ?? "sample overview failed")
        }
        return .failed("sample overview did not fit")
    }

    /// Stops the channel's reads once its browser is gone.
    public static func end(channel: UInt64) { echode_sample_overview_end(channel) }
}

/// What made the UI ask for an audition. It is recorded, never acted on.
public enum AuditionInput: String, Codable, Sendable { case other, click, key }

/// The input behind an audition request, as the UI measured it.
public struct AuditionOrigin: Sendable, Equatable {
    public var input: AuditionInput
    /// From the input event to the request. Ignored for `other`.
    public var delayMicroseconds: UInt64
    public init(input: AuditionInput, delayMicroseconds: UInt64 = 0) {
        self.input = input
        self.delayMicroseconds = delayMicroseconds
    }
    public static let other = AuditionOrigin(input: .other)
}

/// One audition request. Stage times are microseconds after the request.
public struct AuditionEntry: Codable, Sendable, Equatable {
    public let request: UInt64
    public let path: String?
    public let input: AuditionInput
    public let inputDelayUs: UInt64?
    public let requestedAtMs: UInt64
    public let phase: AuditionStatus.Phase
    public let error: String?
    public let pickedUs: UInt64?
    public let firstDecodedUs: UInt64?
    public let decodedUs: UInt64?
    public let sourceFrames: UInt64?
    public let sourceRate: UInt32?
    public let startFrame: UInt64?
    public let outputUs: UInt64?
    public let openedOutput: Bool
    public let convertedUs: UInt64?
    public let handedUs: UInt64?
    public let heardUs: UInt64?
    public let starvedFrames: UInt64?
    /// Startup stages are settled after hearing and complete decoding, or a terminal phase.
    public var settled: Bool { (heardUs != nil && decodedUs != nil) || ![.loading, .sounding].contains(phase) }
    enum CodingKeys: String, CodingKey {
        case request, path, input, phase, error
        case inputDelayUs = "input_delay_us"
        case requestedAtMs = "requested_at_ms"
        case pickedUs = "picked_us"
        case decodedUs = "decoded_us"
        case firstDecodedUs = "first_decoded_us"
        case sourceFrames = "source_frames"
        case sourceRate = "source_rate"
        case outputUs = "output_us"
        case openedOutput = "opened_output"
        case startFrame = "start_frame"
        case convertedUs = "converted_us"
        case handedUs = "handed_us"
        case heardUs = "heard_us"
        case starvedFrames = "starved_frames"
    }
}

/// One member of a complete transient mixer preview.
public struct FaderValue: Encodable, Sendable, Equatable {
    public let track: UUID
    public let gain: Float
    public let pan: Float
    public init(track: UUID, gain: Float, pan: Float) {
        self.track = track
        self.gain = gain
        self.pan = pan
    }
}

public enum AudioCommand: Encodable, Sendable {
    case play, pause, stop
    case locate(tick: Int64)
    case output(uid: String?)
    case refresh, status
    /// Remembers where playback starts and stop returns, saved with the package.
    case setStart(tick: Int64)
    /// Opens the output stopped, helpers and media included, so the first play does not wait.
    case prepare
    /// Both ticks set the loop. Both nil clear it.
    case loop(start: Int64?, end: Int64?)
    case metronome(enabled: Bool)
    case previewFader(track: UUID, gain: Float, pan: Float)
    case previewFaders(values: [FaderValue])
    case clearFaderPreview
    enum CodingKeys: String, CodingKey { case type, tick, uid, start, end, enabled, track, gain, pan, values }
    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        let type: String
        switch self {
        case .play: type = "transport.play"
        case .pause: type = "transport.pause"
        case .stop: type = "transport.stop"
        case .locate(let tick):
            type = "transport.locate"
            try container.encode(tick, forKey: .tick)
        case .setStart(let tick):
            type = "transport.set_start"
            try container.encode(tick, forKey: .tick)
        case .output(let uid):
            type = "audio.output"
            try container.encode(uid, forKey: .uid)
        case .refresh: type = "audio.refresh"
        case .status: type = "audio.status"
        case .prepare: type = "audio.prepare"
        case .loop(let start, let end):
            type = "transport.loop"
            try container.encode(start, forKey: .start)
            try container.encode(end, forKey: .end)
        case .metronome(let enabled):
            type = "transport.metronome"
            try container.encode(enabled, forKey: .enabled)
        case .previewFader(let track, let gain, let pan):
            type = "audio.preview_fader"
            try container.encode(track, forKey: .track)
            try container.encode(gain, forKey: .gain)
            try container.encode(pan, forKey: .pan)
        case .previewFaders(let values):
            type = "audio.preview_faders"
            try container.encode(values, forKey: .values)
        case .clearFaderPreview: type = "audio.clear_fader_preview"
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
    public func peak(track: UUID) -> Float { document.trackPeak(track) }
    public static func == (lhs: Self, rhs: Self) -> Bool { lhs.document === rhs.document }
}
