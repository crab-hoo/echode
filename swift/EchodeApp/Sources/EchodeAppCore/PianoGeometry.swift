import EchodeBridge
import Foundation

struct DisplayKey: Sendable, Equatable {
    var tonic = 0
    var minor = false

    func syllable(_ pitch: Int) -> String {
        ["Do", "♯Do", "Re", "♭Mi", "Mi", "Fa", "♯Fa", "Sol", "♭La", "La", "♭Ti", "Ti"][(pitch - tonic + 132) % 12]
    }

    func inScale(_ pitch: Int) -> Bool {
        (minor ? [0, 2, 3, 5, 7, 8, 10] : [0, 2, 4, 5, 7, 9, 11]).contains((pitch - tonic + 132) % 12)
    }
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
