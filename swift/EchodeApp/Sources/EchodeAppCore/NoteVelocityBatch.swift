import EchodeBridge
import Foundation

/// Immutable press-time targets. The controller validates every original before a single transaction.
struct PianoVelocityChange {
    let anchor: PianoNote.ID
    let originals: [PianoNote.ID: UInt32]
    let delta: Int

    var anchorValue: Int? { originals[anchor].map { Int($0 >> 9) + delta } }
    var isValid: Bool {
        guard !originals.isEmpty, originals[anchor] != nil, (-127...127).contains(delta), delta != 0 else { return false }
        return originals.allSatisfy { id, raw in
            raw <= UInt16.max && id.track == anchor.track && (0...127).contains(Int(raw >> 9) + delta)
        }
    }

    func actions(in snapshot: Snapshot?) -> [[String: Any]]? {
        guard isValid, originals.allSatisfy({ snapshot?.pianoNote($0.key)?.velocity == $0.value }) else { return nil }
        var seen: Set<UUID> = []
        return originals.keys.sorted { $0.descriptionKey < $1.descriptionKey }.compactMap { id -> [String: Any]? in
            guard seen.insert(id.note).inserted, let raw = originals[id] else { return nil }
            return ["type": "note.velocity", "track": id.track.uuidString, "clip": id.placement.uuidString,
                    "note": id.note.uuidString, "velocity": Int(raw >> 9) + delta]
        }
    }
}
