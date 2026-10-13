import CEchodeFfi
import Foundation

/// A command the document thread rejected, or a package that could not be opened.
public struct BridgeError: Error, Sendable {
    public let message: String
}

/// One owned snapshot. The project stays readable after the document is closed.
///
/// The handle is an immutable `Arc` published by the document thread. This class
/// is `Sendable` because that storage does not change and Rust frees it once.
public final class Snapshot: @unchecked Sendable {
    private let handle: OpaquePointer

    fileprivate init(taking handle: OpaquePointer) {
        self.handle = handle
    }

    deinit {
        echode_snapshot_free(handle)
    }

    public var trackCount: Int {
        echode_snapshot_track_count(handle)
    }

    public func audioState() throws -> AudioState {
        try withExtendedLifetime(self) {
            var count = 0
            guard let bytes = echode_snapshot_audio(handle, &count) else {
                throw BridgeError(message: "audio state is unavailable")
            }
            return try JSONDecoder().decode(AudioState.self, from: Data(bytes: bytes, count: count))
        }
    }

    public func historyReport() throws -> HistoryReport {
        try withExtendedLifetime(self) {
            var count = 0
            guard let bytes = echode_snapshot_history(handle, &count) else {
                throw BridgeError(message: "history is unavailable")
            }
            return try JSONDecoder().decode(HistoryReport.self, from: Data(bytes: bytes, count: count))
        }
    }

    /// UUID of the track at `index` in arrange order.
    public func trackID(at index: Int) -> UUID? {
        guard index >= 0 else { return nil }
        var bytes = [UInt8](repeating: 0, count: 16)
        let code = bytes.withUnsafeMutableBufferPointer { buffer in
            echode_snapshot_track_id(handle, index, buffer.baseAddress)
        }
        guard code == 0 else { return nil }
        return bytes.withUnsafeBytes { raw in
            UUID(uuid: (
                raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
                raw[8], raw[9], raw[10], raw[11], raw[12], raw[13], raw[14], raw[15]
            ))
        }
    }

    public func trackName(at index: Int) -> String? {
        guard index >= 0 else { return nil }
        return withExtendedLifetime(self) {
            var count = 0
            guard let bytes = echode_snapshot_track_name(handle, index, &count) else { return nil }
            return String(decoding: UnsafeBufferPointer(start: bytes, count: count), as: UTF8.self)
        }
    }

    public func deviceCount(onTrack index: Int) -> Int {
        guard index >= 0 else { return 0 }
        return echode_snapshot_device_count(handle, index)
    }

    public func deviceID(onTrack track: Int, at device: Int) -> UUID? {
        guard track >= 0, device >= 0 else { return nil }
        var bytes = [UInt8](repeating: 0, count: 16)
        let code = bytes.withUnsafeMutableBufferPointer { buffer in
            echode_snapshot_device_id(handle, track, device, buffer.baseAddress)
        }
        guard code == 0 else { return nil }
        return bytes.withUnsafeBytes { raw in
            UUID(uuid: (
                raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
                raw[8], raw[9], raw[10], raw[11], raw[12], raw[13], raw[14], raw[15]
            ))
        }
    }

    /// CLAP id of an inserted device. The mixer returns nil.
    public func devicePlugin(onTrack track: Int, at device: Int) -> String? {
        guard track >= 0, device >= 0 else { return nil }
        return withExtendedLifetime(self) {
            var count = 0
            guard let bytes = echode_snapshot_device_plugin(handle, track, device, &count) else { return nil }
            return String(decoding: UnsafeBufferPointer(start: bytes, count: count), as: UTF8.self)
        }
    }

    /// Copies the visible arrange elements. The borrowed C buffer does not escape.
    public func visibleElements(
        originTick: Int64,
        widthTicks: Int64,
        originTrack: Double,
        heightTracks: Double,
        pixelsPerTick: Double,
        pixelsPerTrack: Double
    ) -> (elements: [ArrangeElement], dropped: Int) {
        withExtendedLifetime(self) {
            var written = 0
            var dropped = 0
            _ = echode_snapshot_visible(
                handle, originTick, widthTicks, originTrack, heightTracks,
                pixelsPerTick, pixelsPerTrack, nil, 0, &written, &dropped
            )
            let count = Int(dropped)
            guard count > 0 else { return ([], 0) }
            let buffer = UnsafeMutableBufferPointer<VisibleElement>.allocate(capacity: count)
            defer { buffer.deallocate() }
            _ = echode_snapshot_visible(
                handle, originTick, widthTicks, originTrack, heightTracks,
                pixelsPerTick, pixelsPerTrack, buffer.baseAddress, count, &written, &dropped
            )
            return (buffer.prefix(written).map(ArrangeElement.init), dropped)
        }
    }

    public func pianoNotes(track: UUID, originTick: Int64, widthTicks: Int64,
                           topPitch: Double, heightPitches: Double,
                           pixelsPerTick: Double, pixelsPerPitch: Double) -> [PianoNote] {
        withExtendedLifetime(self) {
            withIDs(track: track, placement: nil, note: nil) { track, _, _ in
                var written = 0
                var dropped = 0
                guard echode_snapshot_piano_visible(handle, track, originTick, widthTicks, topPitch, heightPitches,
                                                     pixelsPerTick, pixelsPerPitch, nil, 0, &written, &dropped) == 0,
                      dropped > 0 else { return [] }
                let buffer = UnsafeMutableBufferPointer<PianoNoteElement>.allocate(capacity: dropped)
                defer { buffer.deallocate() }
                guard echode_snapshot_piano_visible(handle, track, originTick, widthTicks, topPitch, heightPitches,
                                                     pixelsPerTick, pixelsPerPitch, buffer.baseAddress, buffer.count,
                                                     &written, &dropped) == 0 else { return [] }
                return buffer.prefix(written).map(PianoNote.init)
            }
        }
    }

    public func pianoNote(_ id: PianoNote.ID) -> PianoNote? {
        withIDs(track: id.track, placement: id.placement, note: id.note) { track, clip, note in
            var raw = PianoNoteElement()
            return echode_snapshot_piano_note(handle, track, clip, note, &raw) == 0 ? PianoNote(raw) : nil
        }
    }

    public func pianoNeighbor(track: UUID, current: PianoNote.ID?, backwards: Bool = false) -> PianoNote? {
        withIDs(track: track, placement: current?.placement, note: current?.note) { track, clip, note in
            var raw = PianoNoteElement()
            return echode_snapshot_piano_neighbor(handle, track, clip, note, backwards ? 1 : 0, &raw) == 0 ? PianoNote(raw) : nil
        }
    }
}

public struct PianoNote: Sendable, Identifiable {
    public struct ID: Sendable, Hashable {
        public let track: UUID
        public let placement: UUID
        public let note: UUID
    }
    public let id: ID
    public let start: Int64
    public let length: Int64
    public let windowStart: Int64
    public let windowEnd: Int64
    public let pitch: Int
    public let velocity: UInt32
    public let x: Float
    public let y: Float
    public let width: Float
    public let height: Float

    fileprivate init(_ raw: PianoNoteElement) {
        id = ID(track: UUID(uuid: raw.track_id), placement: UUID(uuid: raw.placement_id), note: UUID(uuid: raw.note_id))
        start = raw.start; length = raw.length; windowStart = raw.window_start; windowEnd = raw.window_end
        pitch = Int(raw.pitch); velocity = raw.velocity
        x = raw.x; y = raw.y; width = raw.w; height = raw.h
    }
}

private func withIDs<T>(track: UUID, placement: UUID?, note: UUID?,
                        _ body: (UnsafePointer<UInt8>, UnsafePointer<UInt8>?, UnsafePointer<UInt8>?) -> T) -> T {
    var trackBytes = track.uuid
    var clipBytes = (placement ?? track).uuid
    var noteBytes = (note ?? track).uuid
    return withUnsafeBytes(of: &trackBytes) { trackRaw in
        withUnsafeBytes(of: &clipBytes) { clipRaw in
            withUnsafeBytes(of: &noteBytes) { noteRaw in
                body(trackRaw.bindMemory(to: UInt8.self).baseAddress!,
                     placement == nil ? nil : clipRaw.bindMemory(to: UInt8.self).baseAddress!,
                     note == nil ? nil : noteRaw.bindMemory(to: UInt8.self).baseAddress!)
            }
        }
    }
}

/// Value objects keep the C layout and raw pointers inside the bridge.
public struct ArrangeElement: Sendable, Identifiable {
    public enum Role: UInt32, Sendable { case track = 0, clip = 1, note = 2 }
    public struct ID: Sendable, Hashable {
        public let track: UUID
        public let placement: UUID?
        public let note: UUID?
    }
    public let id: ID
    public let role: Role
    public let x: Float
    public let y: Float
    public let width: Float
    public let height: Float

    fileprivate init(_ raw: VisibleElement) {
        id = ID(
            track: UUID(uuid: raw.track_id),
            placement: raw.has_placement == 1 ? UUID(uuid: raw.placement_id) : nil,
            note: raw.has_note == 1 ? UUID(uuid: raw.note_id) : nil
        )
        role = Role(rawValue: raw.role)!
        x = raw.x
        y = raw.y
        width = raw.w
        height = raw.h
    }
}

extension Snapshot {
    /// Min and max for each column of an audio clip. An empty array means the
    /// placement has no stored waveform.
    public func waveform(placement: UUID, startTick: Int64, endTick: Int64, columns: Int) -> [SIMD2<Float>] {
        guard columns > 0 else { return [] }
        return withExtendedLifetime(self) {
            var identity = placement.uuid
            var mins = [Float](repeating: 0, count: columns)
            var maxs = [Float](repeating: 0, count: columns)
            let written = withUnsafeBytes(of: &identity) { raw in
                mins.withUnsafeMutableBufferPointer { minBuffer in
                    maxs.withUnsafeMutableBufferPointer { maxBuffer in
                        echode_snapshot_waveform(
                            handle, raw.bindMemory(to: UInt8.self).baseAddress,
                            startTick, endTick, minBuffer.baseAddress, maxBuffer.baseAddress, columns)
                    }
                }
            }
            guard written > 0 else { return [] }
            return (0..<Int(written)).map { SIMD2(mins[$0], maxs[$0]) }
        }
    }
}

private final class PendingCall: Sendable {
    let continuation: CheckedContinuation<Snapshot, any Error>

    init(_ continuation: CheckedContinuation<Snapshot, any Error>) {
        self.continuation = continuation
    }
}

private func completeCommand(
    _ context: UnsafeMutableRawPointer?,
    _ status: Int32,
    _ message: UnsafePointer<CChar>?,
    _ snapshot: OpaquePointer?
) {
    let pending = Unmanaged<PendingCall>.fromOpaque(context!).takeRetainedValue()
    if status == ECHODE_OK, let snapshot {
        pending.continuation.resume(returning: Snapshot(taking: snapshot))
    } else {
        if let snapshot {
            echode_snapshot_free(snapshot)
        }
        let text = message.map { String(cString: $0) } ?? "command failed"
        pending.continuation.resume(throwing: BridgeError(message: text))
    }
}

/// A package owned by the document thread.
///
/// Commands are queued and this call returns. Rust applies them on its thread
/// and completes exactly once. The pointer is `Sendable` for the same reason as
/// `Snapshot`: Swift does not mutate it, and the queue serializes edits.
public final class Document: @unchecked Sendable {
    private let handle: OpaquePointer

    /// Creates an empty package at `path` and opens it.
    public init(creating path: String) throws {
        let handle: OpaquePointer? = path.withCString { echode_document_create($0) }
        guard let handle else {
            throw BridgeError(message: "could not create the package")
        }
        self.handle = handle
    }

    /// Opens an existing package and replays its durable journal.
    public init(opening path: String) throws {
        let handle = path.withCString { echode_document_open($0) }
        guard let handle else { throw BridgeError(message: "could not recover the package") }
        self.handle = handle
    }

    deinit {
        echode_document_close(handle)
    }

    /// Queues `json`. A cancellation that arrives before the command is sent
    /// does not send it. A cancellation after that does not undo the edit.
    public func perform(_ json: String) async throws -> Snapshot {
        try Task.checkCancellation()
        return try await withCheckedThrowingContinuation { continuation in
            let pending = PendingCall(continuation)
            let context = Unmanaged.passRetained(pending).toOpaque()
            let bytes = Array(json.utf8)
            bytes.withUnsafeBufferPointer { buffer in
                _ = echode_document_submit(
                    handle,
                    buffer.baseAddress,
                    buffer.count,
                    context,
                    completeCommand
                )
            }
        }
    }

    public func snapshot() async throws -> Snapshot {
        try await query(UInt32(ECHODE_DOCUMENT_SNAPSHOT))
    }

    public var playbackFeed: PlaybackFeed { PlaybackFeed(self) }

    var playbackPosition: PlaybackPosition {
        var value = TransportPosition()
        _ = echode_document_position(handle, &value)
        return PlaybackPosition(tick: value.tick, playing: value.playing != 0)
    }

    public func audio(_ command: AudioCommand) async throws -> AudioState {
        try Task.checkCancellation()
        let bytes = Array(try JSONEncoder().encode(command))
        let snapshot: Snapshot = try await withCheckedThrowingContinuation { continuation in
            let context = Unmanaged.passRetained(PendingCall(continuation)).toOpaque()
            bytes.withUnsafeBufferPointer { buffer in
                _ = echode_document_audio(handle, buffer.baseAddress, buffer.count, context, completeCommand)
            }
        }
        return try snapshot.audioState()
    }

    public func history(_ command: HistoryCommand) async throws -> Snapshot {
        try Task.checkCancellation()
        let bytes = Array(try JSONEncoder().encode(command))
        return try await withCheckedThrowingContinuation { continuation in
            let context = Unmanaged.passRetained(PendingCall(continuation)).toOpaque()
            bytes.withUnsafeBufferPointer { buffer in
                _ = echode_document_history(handle, buffer.baseAddress, buffer.count, context, completeCommand)
            }
        }
    }

    public func undo() async throws -> Snapshot {
        try await query(UInt32(ECHODE_DOCUMENT_UNDO))
    }

    public func redo() async throws -> Snapshot {
        try await query(UInt32(ECHODE_DOCUMENT_REDO))
    }

    private func query(_ operation: UInt32) async throws -> Snapshot {
        try Task.checkCancellation()
        return try await withCheckedThrowingContinuation { continuation in
            let context = Unmanaged.passRetained(PendingCall(continuation)).toOpaque()
            _ = echode_document_query(handle, operation, context, completeCommand)
        }
    }

    /// Finishes commands already queued, then rejects new ones.
    public func shutdown() {
        echode_document_shutdown(handle)
    }
}
