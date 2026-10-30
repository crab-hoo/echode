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
    public var toggle: ((UUID, MixerField) -> Void)?
    /// Numeric submission and relative steps are distinct from live single-track drags.
    public var enter: ((UUID, MixerField, Float) -> Void)?
    public var adjust: ((UUID, MixerField, Double) -> Void)?
    public init(begin: @escaping (UUID, MixerField) -> Bool, change: @escaping (Float) -> Void,
                finish: @escaping (Bool) -> Void, set: @escaping (UUID, MixerField, Float) -> Void, toggle: ((UUID, MixerField) -> Void)? = nil,
                enter: ((UUID, MixerField, Float) -> Void)? = nil, adjust: ((UUID, MixerField, Double) -> Void)? = nil) {
        self.begin = begin; self.change = change; self.finish = finish; self.set = set
        self.toggle = toggle
        self.enter = enter
        self.adjust = adjust
    }
}

/// One header fader or pan, the target of a click or drag.
struct MixTarget: Equatable {
    let track: UUID
    let field: MixerField
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
    static let clipGainMax = ModelLimits.clipGain.upperBound
    static let maximumGainDecibels = 20 * log10(Double(ModelLimits.gain.upperBound))
    static func decibels(_ gain: Float) -> Double { gain > 0 ? 20 * log10(Double(gain)) : -.infinity }
    static func gain(_ position: Double) -> Float { position <= 0 ? 0 : Float(pow(10, (-60 + min(1, position) * (60 + maximumGainDecibels)) / 20)) }
    static func position(_ gain: Float) -> Double { gain > 0 ? min(1, max(0, (decibels(gain) + 60) / (60 + maximumGainDecibels))) : 0 }
    /// Unity gain and the center.
    static func defaultValue(_ field: MixerField) -> Float { field == .gain ? 1 : 0 }
    /// A header drag. `fraction` is the pointer's travel over the control's
    /// width, so a press without movement keeps the value.
    static func dragged(_ field: MixerField, from original: Float, by fraction: Double) -> Float {
        // Display positions clip quiet gains; zero travel must not round-trip through that scale.
        if fraction == 0 { return original }
        if field == .gain { return gain(min(1, max(0, position(original) + fraction))) }
        return Float(min(Double(ModelLimits.pan.upperBound), max(Double(ModelLimits.pan.lowerBound), Double(original) + fraction * Double(ModelLimits.pan.upperBound - ModelLimits.pan.lowerBound))))
    }
    /// Linear amplitude at -60 dB. Matches `DISPLAY_FLOOR` in the engine meter.
    /// The header bar and the inspector both treat this and anything quieter as silence.
    static let meterSilence: Float = 0.001
    static func meter(_ peak: Float) -> Double {
        guard peak > meterSilence else { return 0 }
        return min(1, max(0, (decibels(peak) + 60) / 60))
    }
    static func label(_ gain: Float) -> String { gain > 0 ? String(format: "%.1f dB", decibels(gain)) : "−∞ dB" }
    /// Spoken pan. The inspector edits the same −100...100% range.
    static func panLabel(_ pan: Float) -> String { String(format: "%.1f%%", Double(pan) * 100) }
    /// Header accessibility speech. Mute and solo stay numeric checkboxes.
    static func spokenValue(_ field: MixerField, _ value: Float) -> String? {
        switch field {
        case .gain: label(value)
        case .pan: panLabel(value)
        case .mute, .solo: nil
        }
    }
    /// One header accessibility step. Volume moves by 1 dB and pan by 5%.
    /// The slider bottom is silence, so the first step up is 1 dB above the
    /// −60 dB end. A downward step that would stay on that end becomes silence.
    static func adjusted(_ field: MixerField, value: Float, direction: Double) -> Float {
        switch field {
        case .gain:
            let floor = -60.0
            let raised = Float(pow(10, (floor + 1) / 20))
            guard value > 0 else { return direction > 0 ? raised : 0 }
            let db = decibels(value) + direction
            if db <= floor { return direction > 0 ? raised : 0 }
            return min(ModelLimits.gain.upperBound, Float(pow(10, db / 20)))
        case .pan:
            return min(ModelLimits.pan.upperBound, max(ModelLimits.pan.lowerBound, value + Float(direction) * 0.05))
        case .mute, .solo:
            return value == 0 ? 1 : 0
        }
    }
    /// Peak readout. Unlike fader `label`, values at the meter floor stay at silence
    /// instead of counting down through ever smaller dB numbers.
    static func meterLabel(_ peak: Float) -> String {
        guard peak > meterSilence else { return "−∞ dB" }
        return String(format: "%.1f dB", decibels(peak))
    }
}

/// Header accessibility steps. The tree keeps showing the last snapshot while
/// an edit is in flight, so the next step starts from the value already sent.
struct MixStep: Equatable {
    var basis: Float = 0
    var pending = false

    mutating func adopt(_ published: Float) -> Float {
        if !pending || Self.same(basis, published) {
            basis = published
            pending = false
        }
        return basis
    }

    mutating func advance(_ field: MixerField, direction: Double) -> Float? {
        let next = MixScale.adjusted(field, value: basis, direction: direction)
        guard next != basis else { return nil }
        basis = next
        pending = true
        return next
    }

    static func same(_ lhs: Float, _ rhs: Float) -> Bool {
        if lhs == rhs { return true }
        guard lhs.isFinite, rhs.isFinite else { return false }
        let scale = max(abs(lhs), abs(rhs), 1)
        return abs(lhs - rhs) <= scale * 1e-5
    }
}

/// Inspector number text. Accepts the unit printed beside the field and the
/// minus sign used by the silence label.
enum MixEntry {
    static func number(_ text: String, unit: String, silence: Bool) -> Double? {
        var normalized = text.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "−", with: "-")
            .replacingOccurrences(of: "–", with: "-")
            .replacingOccurrences(of: ",", with: ".")
        if !unit.isEmpty, normalized.lowercased().hasSuffix(unit.lowercased()) {
            normalized = String(normalized.dropLast(unit.count)).trimmingCharacters(in: .whitespacesAndNewlines)
        }
        if silence, ["-∞", "-inf", "-infinity"].contains(normalized.lowercased()) { return -.infinity }
        guard let value = Double(normalized), value.isFinite else { return nil }
        return value
    }

    /// The header field's first text: the inspector's numbers without the unit.
    static func text(_ field: MixerField, _ value: Float) -> String {
        if field == .gain { return value > 0 ? String(format: "%.1f", MixScale.decibels(value)) : "−∞" }
        return String(format: "%.1f", Double(value) * 100)
    }

    /// A typed header value, in the inspector's range, or nil.
    static func value(_ field: MixerField, _ text: String) -> Float? {
        switch field {
        case .gain:
            guard let db = number(text, unit: "dB", silence: true) else { return nil }
            if db == -.infinity { return 0 }
            guard db >= -120, db <= MixScale.maximumGainDecibels else { return nil }
            return min(ModelLimits.gain.upperBound, Float(pow(10, db / 20)))
        case .pan:
            guard let percent = number(text, unit: "%", silence: false), (Double(ModelLimits.pan.lowerBound) * 100...Double(ModelLimits.pan.upperBound) * 100).contains(percent) else { return nil }
            return Float(percent / 100)
        case .mute, .solo:
            return nil
        }
    }
}

/// Whether the inspector should keep reading the post-fader peak.
enum MeterFollow {
    static func running(transportActive: Bool, playing: Bool, peak: Float) -> Bool {
        transportActive || playing || peak > MixScale.meterSilence
    }
}

public enum ClipMixEdit: Sendable {
    case gain(UUID, Float)
    case fades(UUID, Int64, Int64)
}

enum ClipHandle: Sendable, Equatable { case gain, fadeIn, fadeOut }

/// One clip handle, the target of a click or drag.
struct ClipTarget: Equatable {
    let clip: UUID
    let handle: ClipHandle
}

/// Typed clip values in the inspector's units and range.
enum ClipEntry {
    /// The inspector's fade limit: the longest fade the model accepts at 768 kHz.
    static let maximumFadeMilliseconds = Double(UInt32.max) / Double(ModelLimits.fadeRateLimit) * 1000
    static let flicksPerMillisecond = Double(ModelLimits.flicksPerSecond) / 1000

    static func text(_ handle: ClipHandle, _ mix: ClipMix) -> String {
        switch handle {
        case .gain: mix.gain > 0 ? String(format: "%.1f", MixScale.decibels(mix.gain)) : "−∞"
        case .fadeIn: String(format: "%.1f", Double(mix.fadeIn) / flicksPerMillisecond)
        case .fadeOut: String(format: "%.1f", Double(mix.fadeOut) / flicksPerMillisecond)
        }
    }

    static func edit(_ handle: ClipHandle, _ mix: ClipMix, _ typed: String) -> ClipMixEdit? {
        switch handle {
        case .gain:
            guard let db = MixEntry.number(typed, unit: "dB", silence: true) else { return nil }
            if db == -.infinity { return .gain(mix.id, 0) }
            guard db >= -120, db <= 20 * log10(Double(MixScale.clipGainMax)) else { return nil }
            return .gain(mix.id, min(MixScale.clipGainMax, Float(pow(10, db / 20))))
        case .fadeIn, .fadeOut:
            guard let ms = MixEntry.number(typed, unit: "ms", silence: false), ms >= 0, ms <= maximumFadeMilliseconds else { return nil }
            let flicks = Int64((ms * flicksPerMillisecond).rounded())
            return handle == .fadeIn ? .fades(mix.id, flicks, mix.fadeOut) : .fades(mix.id, mix.fadeIn, flicks)
        }
    }

    static func changes(_ edit: ClipMixEdit, _ mix: ClipMix) -> Bool {
        switch edit {
        case .gain(_, let gain): gain != mix.gain
        case .fades(_, let entering, let leaving): entering != mix.fadeIn || leaving != mix.fadeOut
        }
    }
}

struct ClipControlLayout: Sendable {
    let rect: CGRect
    let fadeIn: Double
    let fadeOut: Double
    static func make(_ clip: ClipMix, camera: ArrangeCamera, origin: Double, y: Double, height: Double,
                     fadeInTicks: Int64? = nil, fadeOutTicks: Int64? = nil) -> Self {
        let scale = camera.pixelsPerBeat / Tick.perBeat
        return Self(rect: CGRect(x: origin + (Double(clip.start) / Tick.perBeat - camera.originBeat) * camera.pixelsPerBeat,
                                 y: y, width: Double(clip.length) * scale, height: max(1, height - 1)),
                    fadeIn: Double(fadeInTicks ?? clip.fadeInTicks) * scale,
                    fadeOut: Double(fadeOutTicks ?? clip.fadeOutTicks) * scale)
    }
    var gain: CGRect { CGRect(x: rect.midX - 12, y: rect.minY + 14, width: 24, height: 10) }
    // Both handles stay inside the clip, so a zero fade, which placed audio
    // starts with, can still be grabbed. Drags are relative (ADR 0161).
    var entering: CGRect { CGRect(x: min(rect.maxX - 4, max(rect.minX + 4, rect.minX + fadeIn)) - 4, y: rect.minY + 3, width: 8, height: 8) }
    var leaving: CGRect { CGRect(x: max(rect.minX + 4, min(rect.maxX - 4, rect.maxX - fadeOut)) - 4, y: rect.minY + 3, width: 8, height: 8) }
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
    let layout: ClipControlLayout
}
