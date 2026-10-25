import Foundation

/// An exclusive tick range. This is transport state, outside model Undo.
public struct LoopRange: Sendable, Equatable {
    public let start: Int64
    public let end: Int64

    public init?(start: Int64, end: Int64) {
        guard start >= 0, end > start else { return nil }
        self.start = start; self.end = end
    }
}

public struct RulerLoop: Sendable, Equatable {
    public let range: LoopRange
    public let enabled: Bool

    public init(range: LoopRange, enabled: Bool) {
        self.range = range; self.enabled = enabled
    }
}

/// A click changes the start only on release; a drag never seeks or enables a loop.
struct RulerPress {
    let x: Double
    let originBeat: Double
    let pixelsPerBeat: Double
    let grid: Int64
    var edge: LoopEdge? = nil
    var originalRange: LoopRange? = nil
    private(set) var dragged = false
    private(set) var range: LoopRange?

    func tick(at x: Double, snapping: Bool) -> Int64? {
        guard let tick = PianoCamera.ticks(originBeat + x / pixelsPerBeat) else { return nil }
        let snapped = snapping ? PianoCamera.ticks((Double(tick) / Double(grid)).rounded() * Double(grid) / 705_600) ?? tick : tick
        return max(0, snapped)
    }

    mutating func update(x: Double, snapping: Bool) {
        dragged = dragged || abs(x - self.x) >= 3
        guard dragged else { range = nil; return }
        if let edge, let originalRange {
            range = resize(edge, original: originalRange, x: x, snapping: snapping)
        } else if let anchor = tick(at: self.x, snapping: snapping), let current = tick(at: x, snapping: snapping) {
            range = LoopRange(start: min(anchor, current), end: max(anchor, current))
        } else { range = nil }
    }

    private func resize(_ edge: LoopEdge, original: LoopRange, x: Double, snapping: Bool) -> LoopRange? {
        guard x != self.x else { return nil }
        let endpoint = edge == .start ? original.start : original.end
        let endpointX = (Double(endpoint) / 705_600 - originBeat) * pixelsPerBeat
        // Retain the grab offset, rather than jumping the edge to the pointer.
        guard var current = tick(at: endpointX + x - self.x, snapping: snapping) else { return nil }
        let result: LoopRange?
        if edge == .start {
            if current >= original.end {
                let limit = original.end - 1
                current = snapping ? limit / grid * grid : limit
            }
            result = LoopRange(start: current, end: original.end)
        } else {
            if current <= original.start {
                if snapping {
                    let next = (original.start / grid + 1).multipliedReportingOverflow(by: grid)
                    guard !next.overflow else { return nil }
                    current = next.partialValue
                } else { current = original.start + 1 }
            }
            result = LoopRange(start: original.start, end: current)
        }
        return result == original ? nil : result
    }
}

extension ProjectController {
    var rulerLoop: RulerLoop? {
        guard let start = PianoCamera.ticks(loopStartBeat), let end = PianoCamera.ticks(loopEndBeat),
              let range = LoopRange(start: start, end: end) else { return nil }
        return RulerLoop(range: range, enabled: audio.loopStart != nil)
    }

    func setLoopRange(_ range: LoopRange, documentID: UUID) async {
        guard self.documentID == documentID, !busy else { return }
        loopStartBeat = Double(range.start) / 705_600
        loopEndBeat = Double(range.end) / 705_600
        // Changing the draft must not turn an inactive loop on.
        await updateLoopBounds()
    }
}
