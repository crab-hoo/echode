import EchodeBridge
import Foundation

/// A common edge displacement, not a length ratio; each opposite edge stays fixed.
struct PianoResizeChange: Sendable {
    let anchor: PianoNote.ID
    let originals: [PianoNote.ID: NoteGeometry]
    let edge: PianoGesture.Mode
    let delta: Int64

    func resized(_ original: NoteGeometry) -> NoteGeometry? {
        guard original.isValid, edge != .move else { return nil }
        let start = original.start.addingReportingOverflow(edge == .leftEdge ? delta : 0)
        let length = edge == .leftEdge ? original.length.subtractingReportingOverflow(delta)
                                      : original.length.addingReportingOverflow(delta)
        guard !start.overflow, !length.overflow else { return nil }
        let value = NoteGeometry(start: start.partialValue, length: length.partialValue, pitch: original.pitch)
        return value.isValid ? value : nil
    }

    var geometries: [PianoNote.ID: NoteGeometry]? {
        guard !originals.isEmpty, originals[anchor] != nil else { return nil }
        var result: [PianoNote.ID: NoteGeometry] = [:]
        for (id, original) in originals {
            guard id.track == anchor.track, let value = resized(original) else { return nil }
            result[id] = value
        }
        return result
    }

    func actions(in snapshot: Snapshot?) -> [EditAction]? {
        guard delta != 0, let geometries,
              originals.allSatisfy({ snapshot?.pianoNote($0.key).map(NoteGeometry.init) == $0.value }) else { return nil }
        var seen: Set<UUID> = []
        // The grabbed placement owns any fixed-window growth for shared content.
        let ids = [anchor] + originals.keys.filter { $0 != anchor }.sorted { $0.descriptionKey < $1.descriptionKey }
        return ids.compactMap { id -> EditAction? in
            guard seen.insert(id.note).inserted, let value = geometries[id] else { return nil }
            return .editNote(track: id.track.uuidString, clip: id.placement.uuidString,
                             note: id.note.uuidString, pitch: UInt8(value.pitch), start: value.start, length: value.length)
        }
    }
}

struct PianoResizeGesture {
    let anchor: PianoNote.ID
    let originals: [PianoNote.ID: NoteGeometry]
    let edge: PianoGesture.Mode
    let point: CGPoint
    let camera: PianoCamera

    func preview(at point: CGPoint, snapping: Bool) -> PianoResizePreview? {
        guard edge != .move, let original = originals[anchor], originals.values.allSatisfy(\.isValid),
              camera.grid > 0, camera.pixelsPerBeat.isFinite, camera.pixelsPerBeat > 0,
              point.x.isFinite, point.y.isFinite, self.point.x.isFinite, self.point.y.isFinite,
              let displacement = Tick.ticks((point.x - self.point.x) / camera.pixelsPerBeat),
              let shortest = originals.values.map(\.length).min(),
              let first = originals.values.map(\.start).min(),
              let last = originals.values.map({ $0.start + $0.length }).max() else { return nil }
        let basis = edge == .leftEdge ? original.start : original.start + original.length
        let moved = basis.addingReportingOverflow(displacement)
        guard !moved.overflow else { return nil }
        let target = snapping ? camera.snapped(moved.partialValue) : moved.partialValue
        let proposed = target.subtractingReportingOverflow(basis)
        guard !proposed.overflow else { return nil }
        let delta = edge == .leftEdge ? min(shortest - 1, max(-first, proposed.partialValue))
                                     : min(Int64.max - last, max(1 - shortest, proposed.partialValue))
        let change = PianoResizeChange(anchor: anchor, originals: originals, edge: edge, delta: delta)
        guard change.geometries != nil else { return nil }
        return PianoResizePreview(change: change)
    }
}

struct PianoResizePreview: Sendable {
    let change: PianoResizeChange

    func geometries(in notes: [PianoNote]) -> [NoteGeometry] {
        var values = change.geometries ?? [:]
        let noteIDs = Set(change.originals.keys.map(\.note))
        for note in notes where noteIDs.contains(note.id.note) && values[note.id] == nil {
            values[note.id] = change.resized(NoteGeometry(note))
        }
        return values.keys.sorted { $0.descriptionKey < $1.descriptionKey }.compactMap { values[$0] }
    }
}
