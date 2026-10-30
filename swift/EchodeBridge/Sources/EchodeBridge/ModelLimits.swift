import CEchodeFfi

/// Model units and bounds from the linked Rust core, not a second UI definition.
public enum ModelLimits {
    private static var model: ModelConstants { echode_model_constants() }
    public static let ticksPerQuarter = model.ticks_per_quarter
    public static let flicksPerSecond = model.flicks_per_second
    public static let gain: ClosedRange<Float> = 0...model.gain_max
    public static let pan: ClosedRange<Float> = model.pan_min...model.pan_max
    public static let clipGain: ClosedRange<Float> = 0...model.clip_gain_max
    public static let fadeRateLimit = model.fade_rate_limit
    public static let midi7: ClosedRange<Int> = 0...Int(model.midi7_max)
}

/// Integer musical time. Floating point beats belong at display/input boundaries only.
public struct Tick: RawRepresentable, Sendable, Equatable, Comparable, Hashable {
    public let rawValue: Int64
    public init(rawValue: Int64) { self.rawValue = rawValue }
    public init?(beats: Double) {
        guard let raw = Self.ticks(beats) else { return nil }
        self.init(rawValue: raw)
    }
    public var beats: Double { Double(rawValue) / Self.perBeat }
    public static let perQuarter = ModelLimits.ticksPerQuarter
    public static let perBeat = Double(perQuarter)

    /// Round once, ties away from zero; reject non-finite or overflowing inputs.
    public static func ticks(_ beats: Double) -> Int64? {
        Int64(exactly: (beats * perBeat).rounded())
    }
    public static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }
}
