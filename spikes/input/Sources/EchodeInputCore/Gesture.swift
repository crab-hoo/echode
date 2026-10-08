/// Trackpad gesture decisions that do not touch AppKit.
///
/// Pinch axes stay independent: a change in the horizontal finger span never
/// changes the vertical zoom. The benchmark population is not involved here;
/// these are input rules, not a limit on the DAW.

public struct FingerSpan: Equatable, Sendable {
    public var x: Double
    public var y: Double

    public init(x: Double, y: Double) {
        self.x = x
        self.y = y
    }
}

public struct Zoom: Equatable, Sendable {
    public var x: Double
    public var y: Double

    public init(x: Double, y: Double) {
        self.x = x
        self.y = y
    }
}

public struct TrackpadTouch: Equatable, Sendable {
    public var x: Double
    public var y: Double
    public var deviceWidth: Double
    public var deviceHeight: Double

    public init(x: Double, y: Double, deviceWidth: Double, deviceHeight: Double) {
        self.x = x
        self.y = y
        self.deviceWidth = deviceWidth
        self.deviceHeight = deviceHeight
    }
}

/// Distance between two fingers on the trackpad surface, in points.
///
/// Trackpad touches are indirect, so their position in a view is not usable.
/// `normalized` is 0...1 across the pad, and `deviceWidth`/`deviceHeight` are
/// the pad size in points.
public func fingerSpan(first: TrackpadTouch, second: TrackpadTouch) -> FingerSpan {
    let width = first.deviceWidth > 1 ? first.deviceWidth : 1_000
    let height = first.deviceHeight > 1 ? first.deviceHeight : 1_000
    return FingerSpan(
        x: abs(first.x - second.x) * width,
        y: abs(first.y - second.y) * height
    )
}
public let minimumPinchSpan = 8.0
public let minimumZoom = 0.25
public let maximumZoom = 8.0

/// A pinch is uniform: both axes share one scale, taken from the distance
/// between the fingers at the start of the gesture. Comparing each frame with
/// the previous one makes the zoom jump when the fingers cross and the span
/// passes through a small value.
///
/// Directional mode locks to one axis only after the gesture is clearly
/// horizontal or vertical. A normal pinch stays uniform, because a one-axis
/// pinch is hard to aim.
public enum PinchMode: Equatable, Sendable {
    case uniform
    case directional
}

public enum PinchAxis: Equatable, Sendable {
    case uniform
    case horizontal
    case vertical
}

public struct PinchGesture: Equatable, Sendable {
    public var startSpan: FingerSpan
    public var startZoom: Zoom
    public var mode: PinchMode
    public var axis: PinchAxis
    public var axisLocked: Bool
    public var peakX: Double
    public var peakY: Double
    public var heldZoom: Zoom

    public init(span: FingerSpan, zoom: Zoom, mode: PinchMode) {
        startSpan = span
        startZoom = zoom
        self.mode = mode
        axis = .uniform
        axisLocked = mode == .uniform
        peakX = span.x
        peakY = span.y
        heldZoom = zoom
    }

    public mutating func update(span: FingerSpan) -> Zoom {
        peakX = max(peakX, span.x)
        peakY = max(peakY, span.y)
        if mode == .directional, !axisLocked {
            decideAxis(span)
            if axisLocked {
                // Samples before the axis was clear were noise. Scale from here,
                // so the zoom does not move and then snap when the axis locks.
                startSpan = span
                peakX = span.x
                peakY = span.y
            }
            return startZoom
        }
        if crossed(span) {
            return heldZoom
        }
        var zoom = startZoom
        switch axis {
        case .uniform:
            let factor = scale(from: distance(startSpan), to: distance(span))
            zoom.x = clampZoom(startZoom.x * factor)
            zoom.y = clampZoom(startZoom.y * factor)
        case .horizontal:
            zoom.x = clampZoom(startZoom.x * scale(from: startSpan.x, to: span.x))
        case .vertical:
            zoom.y = clampZoom(startZoom.y * scale(from: startSpan.y, to: span.y))
        }
        heldZoom = zoom
        return zoom
    }

    private mutating func decideAxis(_ span: FingerSpan) {
        let horizontal = abs(span.x - startSpan.x)
        let vertical = abs(span.y - startSpan.y)
        let movement = max(horizontal, vertical)
        guard movement >= 12 else { return }
        if horizontal >= vertical * 2 {
            axis = .horizontal
        } else if vertical >= horizontal * 2 {
            axis = .vertical
        } else {
            axis = .uniform
        }
        axisLocked = true
    }

    private func crossed(_ span: FingerSpan) -> Bool {
        switch axis {
        case .horizontal:
            return peakX >= minimumPinchSpan && span.x < peakX * 0.35
        case .vertical:
            return peakY >= minimumPinchSpan && span.y < peakY * 0.35
        case .uniform:
            let xCollapsed = peakX >= minimumPinchSpan && span.x < peakX * 0.35
            let yCollapsed = peakY >= minimumPinchSpan && span.y < peakY * 0.35
            return xCollapsed != yCollapsed
        }
    }
}

private func distance(_ span: FingerSpan) -> Double {
    (span.x * span.x + span.y * span.y).squareRoot()
}

private func scale(from start: Double, to current: Double) -> Double {
    let baseline = max(start, minimumPinchSpan)
    guard baseline > 0, current.isFinite else { return 1 }
    return max(current, 0) / baseline
}

private func clampZoom(_ value: Double) -> Double {
    min(maximumZoom, max(minimumZoom, value))
}

public func crossedGridLine(from: Double, to: Double, spacing: Double) -> Bool {
    guard spacing > 0, from.isFinite, to.isFinite else { return false }
    return (from / spacing).rounded(.down) != (to / spacing).rounded(.down)
}

public struct ComputerKeyboard: Equatable, Sendable {
    public private(set) var notes: Set<Int> = []

    public init() {}

    public mutating func press(_ character: Character) {
        guard let note = midiNote(for: character) else { return }
        notes.insert(note)
    }

    public mutating func release(_ character: Character) {
        guard let note = midiNote(for: character) else { return }
        notes.remove(note)
    }

    public var heldNotes: [Int] {
        notes.sorted()
    }
}

public struct ChordPressure: Equatable, Sendable {
    public var notes: [Int]
    public var pressure: Double

    public init(notes: [Int], pressure: Double) {
        self.notes = notes
        self.pressure = pressure
    }
}

/// Pressure is reported only while at least one computer-keyboard note is held.
/// The trackpad value is clamped to 0...1.
public func chordPressure(notes: Set<Int>, pressure: Double) -> ChordPressure? {
    guard !notes.isEmpty, pressure.isFinite else { return nil }
    return ChordPressure(
        notes: notes.sorted(),
        pressure: min(1, max(0, pressure))
    )
}

public func noteName(_ midi: Int) -> String {
    let names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
    let index = ((midi % 12) + 12) % 12
    return "\(names[index])\(midi / 12 - 1)"
}

public func midiNote(for character: Character) -> Int? {
    switch character.lowercased().first {
    case "a": 48
    case "w": 49
    case "s": 50
    case "e": 51
    case "d": 52
    case "f": 53
    case "t": 54
    case "g": 55
    case "y": 56
    case "h": 57
    case "u": 58
    case "j": 59
    case "k": 60
    case "z": 36
    case "x": 38
    case "c": 40
    case "v": 41
    case "b": 43
    case "n": 45
    case "m": 47
    default: nil
    }
}
