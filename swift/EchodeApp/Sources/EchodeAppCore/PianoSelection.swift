import EchodeBridge
import Foundation

/// A right-button press never edits the project. Commit its selection only on release.
struct PianoSelectionPress {
    let origin: CGPoint
    private(set) var point: CGPoint
    private(set) var dragged = false

    init(at point: CGPoint) { origin = point; self.point = point }

    mutating func update(_ point: CGPoint) {
        guard point.x.isFinite, point.y.isFinite else { return }
        self.point = point
        dragged = dragged || hypot(point.x - origin.x, point.y - origin.y) >= 3
    }

    var rect: CGRect? {
        guard dragged else { return nil }
        return CGRect(x: min(origin.x, point.x), y: min(origin.y, point.y),
                      width: max(1, abs(point.x - origin.x)), height: max(1, abs(point.y - origin.y)))
    }

    func notes(in notes: [PianoNote], layout: PianoLayout) -> Set<PianoNote.ID> {
        guard let rect else { return [] }
        let area = rect.intersection(layout.grid)
        guard !area.isNull, !area.isEmpty else { return [] }
        return Set(notes.filter { note in
            let visible = layout.paneRect(CGRect(x: Double(note.x), y: Double(note.y),
                width: max(2, Double(note.width)), height: Double(note.height))).intersection(layout.grid)
            return !visible.isNull && !visible.isEmpty && visible.intersects(area)
        }.map(\.id))
    }
}

extension PianoNote.ID {
    /// Stable transaction order, independent of Set iteration and frame query order.
    var descriptionKey: String { "\(track)/\(placement)/\(note)" }
}

struct PianoNoteDeletion {
    let snapshot: Snapshot
    let ids: Set<PianoNote.ID>
}
