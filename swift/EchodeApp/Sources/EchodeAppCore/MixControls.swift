import EchodeBridge
import Foundation

public struct FaderPreview: Sendable, Equatable {
    public let document: UUID
    public let track: UUID
    public let field: MixerField
    public let parameter: UUID
    public let original: Float
    public var gain: Float
    public var pan: Float
    public var value: Float { field == .gain ? gain : pan }
}

@MainActor
public struct MixActions {
    public var begin: (UUID, MixerField) -> Bool
    public var change: (Float) -> Void
    public var finish: (Bool) -> Void
    public var set: (UUID, MixerField, Float) -> Void
    public init(begin: @escaping (UUID, MixerField) -> Bool, change: @escaping (Float) -> Void,
                finish: @escaping (Bool) -> Void, set: @escaping (UUID, MixerField, Float) -> Void) {
        self.begin = begin; self.change = change; self.finish = finish; self.set = set
    }
}

/// Shared drawing/hit geometry for the compact track controls.
struct TrackControlLayout: Sendable {
    let row: CGRect
    var visible: Bool { row.height >= 60 && row.width >= 120 }
    var mute: CGRect { CGRect(x: 8, y: row.minY + 29, width: 20, height: 18) }
    var solo: CGRect { CGRect(x: 31, y: row.minY + 29, width: 20, height: 18) }
    var gain: CGRect { CGRect(x: 59, y: row.minY + 29, width: max(22, (row.width - 76) * 0.62), height: 18) }
    var pan: CGRect { CGRect(x: gain.maxX + 8, y: row.minY + 29, width: max(18, row.width - gain.maxX - 18), height: 18) }
    var meter: CGRect { CGRect(x: 8, y: row.maxY - 9, width: max(0, row.width - 18), height: 3) }
    func rect(_ field: MixerField) -> CGRect {
        switch field { case .gain: gain; case .pan: pan; case .mute: mute; case .solo: solo }
    }
    func hit(_ point: CGPoint) -> MixerField? { visible ? MixerField.allCases.first { rect($0).contains(point) } : nil }
}

enum MixScale {
    /// Matches `CLIP_GAIN_MAX` in the document model (`2^20`).
    static let clipGainMax: Float = 1_048_576
    static func decibels(_ gain: Float) -> Double { gain > 0 ? 20 * log10(Double(gain)) : -.infinity }
    static func gain(_ position: Double) -> Float { position <= 0 ? 0 : Float(pow(10, (-60 + min(1, position) * (60 + 20 * log10(4.0))) / 20)) }
    static func position(_ gain: Float) -> Double { gain > 0 ? min(1, max(0, (decibels(gain) + 60) / (60 + 20 * log10(4.0)))) : 0 }
    /// Linear amplitude at -60 dB. Matches `DISPLAY_FLOOR` in the engine meter.
    /// The header bar and the inspector both treat this and anything quieter as silence.
    static let meterSilence: Float = 0.001
    static func meter(_ peak: Float) -> Double {
        guard peak > meterSilence else { return 0 }
        return min(1, max(0, (decibels(peak) + 60) / 60))
    }
    static func label(_ gain: Float) -> String { gain > 0 ? String(format: "%.1f dB", decibels(gain)) : "−∞ dB" }
    /// Peak readout. Unlike fader `label`, values at the meter floor stay at silence
    /// instead of counting down through ever smaller dB numbers.
    static func meterLabel(_ peak: Float) -> String {
        guard peak > meterSilence else { return "−∞ dB" }
        return String(format: "%.1f dB", decibels(peak))
    }
}

public enum ClipMixEdit: Sendable {
    case gain(UUID, Float)
    case fades(UUID, Int64, Int64)
}

enum ClipHandle: Sendable, Equatable { case gain, fadeIn, fadeOut }

struct ClipControlLayout: Sendable {
    let rect: CGRect
    let fadeIn: Double
    let fadeOut: Double
    static func make(_ clip: ClipMix, camera: ArrangeCamera, origin: Double, y: Double, height: Double,
                     fadeInTicks: Int64? = nil, fadeOutTicks: Int64? = nil) -> Self {
        let scale = camera.pixelsPerBeat / 705_600
        return Self(rect: CGRect(x: origin + (Double(clip.start) / 705_600 - camera.originBeat) * camera.pixelsPerBeat,
                                 y: y, width: Double(clip.length) * scale, height: max(1, height - 1)),
                    fadeIn: Double(fadeInTicks ?? clip.fadeInTicks) * scale,
                    fadeOut: Double(fadeOutTicks ?? clip.fadeOutTicks) * scale)
    }
    var gain: CGRect { CGRect(x: rect.midX - 12, y: rect.minY + 14, width: 24, height: 10) }
    var entering: CGRect { CGRect(x: min(rect.maxX - 4, rect.minX + fadeIn) - 4, y: rect.minY + 3, width: 8, height: 8) }
    var leaving: CGRect { CGRect(x: max(rect.minX + 4, rect.maxX - fadeOut) - 4, y: rect.minY + 3, width: 8, height: 8) }
    func hit(_ point: CGPoint) -> ClipHandle? {
        if entering.insetBy(dx: -3, dy: -3).contains(point), point.x <= rect.midX { return .fadeIn }
        if leaving.insetBy(dx: -3, dy: -3).contains(point) { return .fadeOut }
        if entering.insetBy(dx: -3, dy: -3).contains(point) { return .fadeIn }
        return gain.contains(point) ? .gain : nil
    }
}

struct ClipDraft: Sendable {
    let original: ClipMix
    var gain: Float
    var fadeIn: Int64
    var fadeOut: Int64
    var fadeInTicks: Int64
    var fadeOutTicks: Int64
    let handle: ClipHandle
    let down: CGPoint
    let layout: ClipControlLayout
}
