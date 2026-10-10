import EchodeBridge
import Foundation

struct DisplayKey: Sendable, Equatable {
    var tonic = 0
    var minor = false

    func degree(_ pitch: Int) -> Int? {
        let interval = ((pitch - tonic) % 12 + 12) % 12
        return (minor ? [0, 2, 3, 5, 7, 8, 10] : [0, 2, 4, 5, 7, 9, 11]).firstIndex(of: interval).map { $0 + 1 }
    }

    func degreeLabel(_ pitch: Int) -> String? { degree(pitch).flatMap(Self.numeral) }

    /// Uppercase Roman numerals for both modes. The tonic is I.
    static func numeral(_ degree: Int) -> String? {
        switch degree {
        case 1: "I"
        case 2: "II"
        case 3: "III"
        case 4: "IV"
        case 5: "V"
        case 6: "VI"
        case 7: "VII"
        default: nil
        }
    }
}

/// All positions are in pane coordinates; editing uses grid-local coordinates.
struct PianoLayout: Sendable, Equatable {
    let width: Double
    let height: Double
    /// 36pt fits VII and the I–VII header in the 12pt UI font.
    static let degreeColumn = 36.0
    var degreeWidth: Double { min(Self.degreeColumn, width) }
    var keyboardWidth: Double { min(44, max(0, width - degreeWidth)) }
    var rulerHeight: Double { min(24, height) }
    var gridOrigin: CGPoint { CGPoint(x: degreeWidth + keyboardWidth, y: rulerHeight) }
    var grid: CGRect { CGRect(origin: gridOrigin, size: CGSize(width: max(0, width - gridOrigin.x), height: max(0, height - rulerHeight))) }
    var keyboard: CGRect { CGRect(x: degreeWidth, y: rulerHeight, width: keyboardWidth, height: grid.height) }
    var degrees: CGRect { CGRect(x: 0, y: rulerHeight, width: degreeWidth, height: grid.height) }
    var ruler: CGRect { CGRect(x: gridOrigin.x, y: 0, width: grid.width, height: rulerHeight) }

    func gridPoint(_ point: CGPoint) -> CGPoint { CGPoint(x: point.x - gridOrigin.x, y: point.y - gridOrigin.y) }
    func paneRect(_ rect: CGRect) -> CGRect { rect.offsetBy(dx: gridOrigin.x, dy: gridOrigin.y) }
}

public struct NoteGeometry: Sendable, Equatable {
    public var start: Int64
    public var length: Int64
    public var pitch: Int
    public init(start: Int64, length: Int64, pitch: Int) {
        self.start = start; self.length = length; self.pitch = pitch
    }
    init(_ note: PianoNote) { self.init(start: note.start, length: note.length, pitch: note.pitch) }
    var isValid: Bool { (0...127).contains(pitch) && start >= 0 && length > 0 && !start.addingReportingOverflow(length).overflow }
}

struct PianoCamera: Sendable, Equatable {
    var originBeat = 0.0
    var topPitch = 64.0
    var pixelsPerBeat = 72.0
    var pixelsPerPitch = 20.0
    var grid: Int64 = 176_400 // A sixteenth note; time units are quarter-note ticks.

    func tick(at x: Double) -> Int64? { Self.ticks(originBeat + x / pixelsPerBeat).map(snapped) }
    func pitch(at y: Double) -> Int { min(127, max(0, Int((topPitch - y / pixelsPerPitch).rounded(.up)))) }
    func snapped(_ tick: Int64) -> Int64 { Self.ticks((Double(tick) / Double(grid)).rounded() * Double(grid) / 705_600) ?? tick }
    static func ticks(_ beat: Double) -> Int64? { Int64(exactly: (beat * 705_600).rounded()) }

    mutating func zoom(factor: Double, horizontal: Bool, vertical: Bool, anchor: CGPoint) {
        guard factor.isFinite, factor > 0, anchor.x.isFinite, anchor.y.isFinite else { return }
        if horizontal {
            let beat = originBeat + anchor.x / pixelsPerBeat
            pixelsPerBeat = min(1200, max(8, pixelsPerBeat * factor))
            originBeat = max(0, beat - anchor.x / pixelsPerBeat)
        }
        if vertical {
            let pitch = topPitch - anchor.y / pixelsPerPitch
            pixelsPerPitch = min(80, max(12, pixelsPerPitch * factor))
            topPitch = min(127, max(0, pitch + anchor.y / pixelsPerPitch))
        }
    }

    mutating func reveal(_ note: PianoNote, width: Double, height: Double) {
        let start = Double(max(note.start, note.windowStart)) / 705_600
        let end = min(Double(note.start) + Double(note.length), Double(note.windowEnd)) / 705_600
        let span = width / pixelsPerBeat
        if start < originBeat || end > originBeat + span { originBeat = max(0, end - span); if start < originBeat { originBeat = max(0, start) } }
        if Double(note.pitch) > topPitch { topPitch = Double(note.pitch) }
        if (topPitch - Double(note.pitch) + 1) * pixelsPerPitch > height {
            topPitch = min(127, Double(note.pitch) + max(1, height / pixelsPerPitch) - 1)
        }
    }
}

struct PianoGesture: Sendable {
    enum Mode: Sendable, Equatable { case move, leftEdge, rightEdge }
    let id: PianoNote.ID
    let original: NoteGeometry
    let mode: Mode
    let x: Double
    let y: Double

    static func mode(at x: Double, start: Double, end: Double) -> Mode {
        let handle = min(6, max(0, end - start) / 4)
        if abs(x - end) <= handle { return .rightEdge }
        if abs(x - start) <= handle { return .leftEdge }
        return .move
    }

    func preview(x: Double, y: Double, camera: PianoCamera) -> NoteGeometry? {
        guard let delta = PianoCamera.ticks((x - self.x) / camera.pixelsPerBeat) else { return nil }
        var value = original
        switch mode {
        case .move:
            let start = original.start.addingReportingOverflow(delta)
            guard !start.overflow else { return nil }
            value.start = max(0, camera.snapped(start.partialValue))
            value.pitch = min(127, max(0, original.pitch + Int(((self.y - y) / camera.pixelsPerPitch).rounded())))
        case .rightEdge:
            let end = original.start.addingReportingOverflow(original.length)
            let moved = end.partialValue.addingReportingOverflow(delta)
            guard !end.overflow, !moved.overflow else { return nil }
            let length = camera.snapped(moved.partialValue).subtractingReportingOverflow(original.start)
            guard !length.overflow else { return nil }
            value.length = max(camera.grid, length.partialValue)
        case .leftEdge:
            let moved = original.start.addingReportingOverflow(delta)
            let end = original.start.addingReportingOverflow(original.length)
            guard !moved.overflow, !end.overflow else { return nil }
            value.start = min(end.partialValue - min(camera.grid, original.length), max(0, camera.snapped(moved.partialValue)))
            value.length = end.partialValue - value.start
        }
        return value.isValid ? value : nil
    }
}

struct PianoPreview: Sendable {
    let id: PianoNote.ID
    let geometry: NoteGeometry
}

struct PianoRenderInput: Sendable {
    let track: UUID
    let focus: PianoNote.ID?
    let camera: PianoCamera
    let key: DisplayKey
    let preview: PianoPreview?
}
