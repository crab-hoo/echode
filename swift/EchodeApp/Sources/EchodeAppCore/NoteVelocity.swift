import EchodeBridge

/// MIDI 1.0 entry values; stored notes retain the model's 16-bit representation.
struct NoteInputVelocity {
    private(set) var value = 100
    private(set) var generation: UInt64 = 0

    mutating func remember(_ value: Int) {
        guard (0...127).contains(value) else { return }
        self.value = value
        generation &+= 1
    }

    mutating func finish(_ value: Int?, generation: UInt64) {
        // A later selection must win over a delayed journal response.
        if let value, self.generation == generation { remember(value) }
    }
}

struct PianoVelocityPreview: Sendable, Equatable {
    let id: PianoNote.ID
    let value: Int
}

struct PianoVelocityGesture {
    let id: PianoNote.ID
    let original: Int
    let y: Double

    /// Lane drags span the full MIDI range; note drags keep one point per step.
    var pointsPerStep = 1.0
    func preview(y: Double) -> PianoVelocityPreview? {
        guard y.isFinite, self.y.isFinite, pointsPerStep.isFinite, pointsPerStep > 0 else { return nil }
        // Relative motion preserves the grabbed value and offset, independent of pitch zoom and time grid.
        let value = min(127, max(0, Double(original) + ((self.y - y) / pointsPerStep).rounded()))
        return PianoVelocityPreview(id: id, value: Int(value))
    }
}
