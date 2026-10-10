import Foundation

/// The arrange pane owns this viewport. Headers and the renderer consume the
/// same value; neither has an independent vertical scroll position.
struct ArrangeCamera: Sendable, Equatable {
    var originBeat = 0.0
    var originTrack = 0.0
    var pixelsPerBeat = 72.0
    var pixelsPerTrack = 64.0

    mutating func constrain(trackCount: Int, height: Double) {
        if !originBeat.isFinite { originBeat = 0 }
        if !originTrack.isFinite { originTrack = 0 }
        originBeat = max(0, originBeat)
        let visible = max(0, height) / pixelsPerTrack
        originTrack = min(max(0, originTrack), max(0, Double(trackCount) - visible))
    }

    mutating func scroll(x: Double, y: Double, trackCount: Int, height: Double) {
        guard x.isFinite, y.isFinite else { return }
        originBeat -= x / pixelsPerBeat
        originTrack -= y / pixelsPerTrack
        constrain(trackCount: trackCount, height: height)
    }

    mutating func zoom(factor: Double, horizontal: Bool, vertical: Bool,
                       anchor: CGPoint, trackCount: Int, height: Double) {
        guard factor.isFinite, factor > 0 else { return }
        if horizontal {
            let beat = originBeat + Double(anchor.x) / pixelsPerBeat
            pixelsPerBeat = min(1200, max(8, pixelsPerBeat * factor))
            originBeat = beat - Double(anchor.x) / pixelsPerBeat
        }
        if vertical {
            let track = originTrack + Double(anchor.y) / pixelsPerTrack
            pixelsPerTrack = min(240, max(16, pixelsPerTrack * factor))
            originTrack = track - Double(anchor.y) / pixelsPerTrack
        }
        constrain(trackCount: trackCount, height: height)
    }

    mutating func reveal(track: Int, trackCount: Int, height: Double) {
        guard track >= 0, track < trackCount, height > 0 else { return }
        let visible = height / pixelsPerTrack
        if Double(track) < originTrack { originTrack = Double(track) }
        else if Double(track + 1) > originTrack + visible {
            originTrack = visible >= 1 ? Double(track + 1) - visible : Double(track)
        }
        constrain(trackCount: trackCount, height: height)
    }

    func rows(trackCount: Int, height: Double) -> ArrangeRows {
        ArrangeRows(origin: originTrack, rowHeight: pixelsPerTrack,
                    height: height, count: trackCount)
    }
}

/// Half-open row geometry in view points, including partially visible lanes.
struct ArrangeRows: Sendable {
    let origin: Double
    let rowHeight: Double
    let height: Double
    let count: Int

    var visible: Range<Int> {
        guard count > 0, origin.isFinite, origin >= 0,
              rowHeight.isFinite, rowHeight > 0, height.isFinite, height > 0,
              origin < Double(count) else { return 0..<0 }
        let first = Int(origin.rounded(.down))
        let end = min(Double(count), (origin + height / rowHeight).rounded(.up))
        let last = end >= Double(count) ? count : Int(end)
        return first..<max(first, last)
    }

    func rect(at index: Int, width: Double) -> CGRect {
        CGRect(x: 0, y: (Double(index) - origin) * rowHeight,
               width: width, height: rowHeight)
    }

    func index(at y: Double) -> Int? {
        guard y.isFinite, y >= 0, y < height, rowHeight > 0 else { return nil }
        let value = (origin + y / rowHeight).rounded(.down)
        guard value.isFinite, value >= 0, value < Double(count) else { return nil }
        return Int(value)
    }
}
