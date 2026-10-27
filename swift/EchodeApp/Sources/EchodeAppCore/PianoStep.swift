import EchodeBridge
import Foundation

/// Key repeats carry a displacement, not stale geometry from the displayed frame.
struct PianoStep: Sendable {
    let ids: Set<PianoNote.ID>
    var time: Int64 = 0
    var pitch: Int = 0
    var resize = false

    func geometries(in snapshot: Snapshot?) -> [PianoNote.ID: NoteGeometry]? {
        guard !ids.isEmpty, Set(ids.map(\.track)).count == 1 else { return nil }
        var originals: [PianoNote.ID: NoteGeometry] = [:]
        for id in ids {
            guard let note = snapshot?.pianoNote(id), NoteGeometry(note).isValid else { return nil }
            originals[id] = NoteGeometry(note)
        }
        let values = Array(originals.values)
        if resize {
            guard pitch == 0, let shortest = values.map(\.length).min(),
                  let last = values.map({ $0.start + $0.length }).max() else { return nil }
            let delta = min(Int64.max - last, max(1 - shortest, time))
            return originals.mapValues { NoteGeometry(start: $0.start, length: $0.length + delta, pitch: $0.pitch) }
        }
        guard let first = values.map(\.start).min(), let last = values.map({ $0.start + $0.length }).max(),
              let low = values.map(\.pitch).min(), let high = values.map(\.pitch).max() else { return nil }
        let delta = min(Int64.max - last, max(-first, time))
        let interval = min(127 - high, max(-low, pitch))
        return originals.mapValues { NoteGeometry(start: $0.start + delta, length: $0.length, pitch: $0.pitch + interval) }
    }

    func actions(in snapshot: Snapshot?) -> [[String: Any]]? {
        guard let geometries = geometries(in: snapshot),
              ids.contains(where: { snapshot?.pianoNote($0).map(NoteGeometry.init) != geometries[$0] }) else { return nil }
        var seen: Set<UUID> = []
        return ids.sorted { $0.descriptionKey < $1.descriptionKey }.compactMap { id in
            guard seen.insert(id.note).inserted, let value = geometries[id] else { return nil }
            return ["type": "note.edit", "track": id.track.uuidString, "clip": id.placement.uuidString,
                    "note": id.note.uuidString, "start": value.start, "length": value.length, "pitch": value.pitch]
        }
    }
}
