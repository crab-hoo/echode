import EchodeBridge
import Foundation

/// MIDI 1.0 entry values; stored notes retain the model's 16-bit representation.
struct NoteInputVelocity {
    private(set) var value = 100
    private(set) var generation: UInt64 = 0

    mutating func remember(_ value: Int) {
        guard ModelLimits.midi7.contains(value) else { return }
        self.value = value
        generation &+= 1
    }

    mutating func invalidate() { generation &+= 1 }

    mutating func finish(_ value: Int?, generation: UInt64) {
        // A later selection must win over a delayed journal response.
        if let value, self.generation == generation { remember(value) }
    }
}

struct PianoVelocityPreview: Sendable, Equatable {
    let id: PianoNote.ID
    let value: Int
    var values: [UUID: Int] = [:]

    func value(for note: PianoNote) -> Int? {
        values[note.id.note] ?? (id.note == note.id.note ? value : nil)
    }
}

struct PianoVelocityGesture {
    let id: PianoNote.ID
    let original: Int
    let y: Double

    /// Lane drags span the full MIDI range; note drags keep one point per step.
    var pointsPerStep = 1.0
    var originals: [PianoNote.ID: UInt32] = [:]
    func preview(y: Double) -> PianoVelocityPreview? {
        guard y.isFinite, self.y.isFinite, pointsPerStep.isFinite, pointsPerStep > 0 else { return nil }
        // Relative motion preserves the grabbed value and offset, independent of pitch zoom and time grid.
        let basis = originals.isEmpty ? [original] : originals.values.map { Int($0 >> 9) }
        let delta = Int(min(Double(ModelLimits.midi7.upperBound - (basis.max() ?? original)), max(Double(-(basis.min() ?? original)),
            ((self.y - y) / pointsPerStep).rounded())))
        var values: [UUID: Int] = [:]
        for (id, raw) in originals { values[id.note] = Int(raw >> 9) + delta }
        return PianoVelocityPreview(id: id, value: original + delta, values: values)
    }
}
