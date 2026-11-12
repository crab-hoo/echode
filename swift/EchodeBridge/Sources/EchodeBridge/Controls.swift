import CEchodeFfi
import Foundation

public enum MixerField: Int, Sendable, CaseIterable {
    case gain, pan, mute, solo
}

public struct MixerParameter: Sendable, Equatable {
    public let id: UUID
    public let value: Float
}

public struct TrackMix: Sendable, Equatable {
    public let gain: MixerParameter
    public let pan: MixerParameter
    public let mute: MixerParameter
    public let solo: MixerParameter
    public subscript(_ field: MixerField) -> MixerParameter {
        switch field {
        case .gain: gain
        case .pan: pan
        case .mute: mute
        case .solo: solo
        }
    }
}

public struct ClipMix: Sendable, Equatable, Identifiable {
    public let id: UUID
    public let track: UUID
    public let start: Int64
    public let length: Int64
    public let gain: Float
    public let fadeIn: Int64
    public let fadeOut: Int64
    public let isAudio: Bool
    public let fadeInTicks: Int64
    public let fadeOutTicks: Int64
}

extension Snapshot {
    public func flicks(at tick: Int64) -> Int64? {
        withExtendedLifetime(self) {
            var value: Int64 = 0
            return echode_snapshot_flicks(handle, tick, &value) == 0 ? value : nil
        }
    }
    public func mixer(onTrack index: Int) -> TrackMix? {
        guard index >= 0 else { return nil }
        var raw = MixerControls()
        guard withExtendedLifetime(self, { echode_snapshot_mixer_controls(handle, index, &raw) }) == 0 else {
            return nil
        }
        return TrackMix(
            gain: MixerParameter(id: controlUUID(raw.gain_id), value: raw.gain),
            pan: MixerParameter(id: controlUUID(raw.pan_id), value: raw.pan),
            mute: MixerParameter(id: controlUUID(raw.mute_id), value: raw.mute),
            solo: MixerParameter(id: controlUUID(raw.solo_id), value: raw.solo))
    }

    public func clipMix(_ id: UUID) -> ClipMix? {
        var bytes = id.uuid
        var raw = ClipControls()
        let status = withExtendedLifetime(self) {
            withUnsafeBytes(of: &bytes) { pointer in
                echode_snapshot_clip_controls(handle, pointer.bindMemory(to: UInt8.self).baseAddress, &raw)
            }
        }
        guard status == 0 else { return nil }
        return ClipMix(
            id: controlUUID(raw.clip_id), track: controlUUID(raw.track_id),
            start: raw.start, length: raw.length, gain: raw.gain,
            fadeIn: raw.fade_in, fadeOut: raw.fade_out, isAudio: raw.audio != 0,
            fadeInTicks: raw.fade_in_ticks, fadeOutTicks: raw.fade_out_ticks)
    }
}

private func controlUUID<T>(_ value: T) -> UUID {
    withUnsafeBytes(of: value) { bytes in
        UUID(
            uuid: (
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
            ))
    }
}
