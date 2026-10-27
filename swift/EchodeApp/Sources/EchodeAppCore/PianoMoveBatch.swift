import EchodeBridge
import Foundation

/// A common displacement in project time; linked content is written only once.
struct PianoMoveChange: Sendable {
    let anchor: PianoNote.ID
    let originals: [PianoNote.ID: NoteGeometry]
    let time: Int64
    let pitch: Int

    func translated(_ original: NoteGeometry) -> NoteGeometry? {
        guard original.isValid, (-127...127).contains(pitch) else { return nil }
        let start = original.start.addingReportingOverflow(time)
        guard !start.overflow else { return nil }
        let value = NoteGeometry(start: start.partialValue, length: original.length, pitch: original.pitch + pitch)
        return value.isValid ? value : nil
    }

    var geometries: [PianoNote.ID: NoteGeometry]? {
        guard !originals.isEmpty, originals[anchor] != nil else { return nil }
        var result: [PianoNote.ID: NoteGeometry] = [:]
        for (id, original) in originals {
            guard id.track == anchor.track, let value = translated(original) else { return nil }
            result[id] = value
        }
        return result
    }

    func actions(in snapshot: Snapshot?) -> [[String: Any]]? {
        guard time != 0 || pitch != 0, let geometries,
              originals.allSatisfy({ snapshot?.pianoNote($0.key).map(NoteGeometry.init) == $0.value }) else { return nil }
        var seen: Set<UUID> = []
        // Prefer the grabbed placement when its content is also visible through a fixed link.
        let ids = [anchor] + originals.keys.filter { $0 != anchor }.sorted { $0.descriptionKey < $1.descriptionKey }
        return ids.compactMap { id -> [String: Any]? in
            guard seen.insert(id.note).inserted, let value = geometries[id] else { return nil }
            return ["type": "note.edit", "track": id.track.uuidString, "clip": id.placement.uuidString,
                    "note": id.note.uuidString, "start": value.start, "length": value.length, "pitch": value.pitch]
        }
    }
}

struct PianoMoveGesture {
    let anchor: PianoNote.ID
    let originals: [PianoNote.ID: NoteGeometry]
    let point: CGPoint
    let camera: PianoCamera

    func preview(at point: CGPoint, snapping: Bool) -> PianoMovePreview? {
        guard let original = originals[anchor], originals.values.allSatisfy(\.isValid),
              let moved = PianoGesture(id: anchor, original: original, mode: .move, x: self.point.x, y: self.point.y)
                .preview(x: point.x, y: point.y, camera: camera, snapping: snapping),
              let first = originals.values.map(\.start).min(),
              let last = originals.values.map({ $0.start + $0.length }).max(),
              let low = originals.values.map(\.pitch).min(), let high = originals.values.map(\.pitch).max() else { return nil }
        let time = min(Int64.max - last, max(-first, moved.start - original.start))
        let pitch = min(127 - high, max(-low, moved.pitch - original.pitch))
        let change = PianoMoveChange(anchor: anchor, originals: originals, time: time, pitch: pitch)
        guard change.geometries != nil else { return nil }
        return PianoMovePreview(change: change)
    }
}

struct PianoMovePreview: Sendable {
    let change: PianoMoveChange
    let noteIDs: Set<UUID>

    init(change: PianoMoveChange) {
        self.change = change
        noteIDs = Set(change.originals.keys.map(\.note))
    }

    func geometries(in notes: [PianoNote]) -> [NoteGeometry] {
        var values = change.geometries ?? [:]
        for note in notes where noteIDs.contains(note.id.note) && values[note.id] == nil {
            values[note.id] = change.translated(NoteGeometry(note))
        }
        return values.keys.sorted { $0.descriptionKey < $1.descriptionKey }.compactMap { values[$0] }
    }
}
