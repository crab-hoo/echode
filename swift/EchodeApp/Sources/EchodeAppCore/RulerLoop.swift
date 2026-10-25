import Foundation

/// An exclusive tick range. This is transport state, outside model Undo.
struct LoopRange: Sendable, Equatable {
    let start: Int64
    let end: Int64

    init?(start: Int64, end: Int64) {
        guard start >= 0, end > start else { return nil }
        self.start = start; self.end = end
    }
}

struct RulerLoop: Sendable, Equatable {
    let range: LoopRange
    let enabled: Bool
}

/// A click changes the start only on release; a drag never seeks or enables a loop.
struct PianoRulerPress {
    let x: Double
    let camera: PianoCamera
    private(set) var dragged = false
    private(set) var range: LoopRange?

    mutating func update(x: Double, snapping: Bool) {
        dragged = dragged || abs(x - self.x) >= 3
        guard dragged,
              let anchor = camera.rulerTick(at: self.x, snapping: snapping),
              let current = camera.rulerTick(at: x, snapping: snapping) else { range = nil; return }
        range = LoopRange(start: min(anchor, current), end: max(anchor, current))
    }
}

extension ProjectController {
    var rulerLoop: RulerLoop? {
        guard let start = PianoCamera.ticks(loopStartBeat), let end = PianoCamera.ticks(loopEndBeat),
              let range = LoopRange(start: start, end: end) else { return nil }
        return RulerLoop(range: range, enabled: audio.loopStart != nil)
    }

    func setLoopRange(_ range: LoopRange, documentID: UUID) async {
        guard self.documentID == documentID, !busy, !audioBusy else { return }
        loopStartBeat = Double(range.start) / 705_600
        loopEndBeat = Double(range.end) / 705_600
        // Changing the draft must not turn an inactive loop on.
        if audio.loopStart != nil { await setLoop(enabled: true) }
    }
}
