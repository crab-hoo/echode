import EchodeBridge
import Foundation

struct PianoVelocityBar: Sendable {
    let note: PianoNote
    let stem: CGRect
    let cap: CGRect
}

/// Shared point geometry for Metal, input and accessibility; independent of pitch zoom.
struct PianoVelocityLane: Sendable {
    let rect: CGRect
    var margin: Double { min(8, rect.height / 4) }
    var span: Double { max(0, rect.height - 2 * margin) }
    var baseline: Double { rect.maxY - margin }

    func bars(notes: [PianoNote], camera: PianoCamera, preview: PianoVelocityPreview?) -> [PianoVelocityBar] {
        guard rect.width > 0, span > 0 else { return [] }
        return notes.compactMap { note in
            // A sustained note entering from the left must not acquire a false onset.
            let start = max(note.start, note.windowStart)
            let x = rect.minX + (Double(start) / Tick.perBeat - camera.originBeat) * camera.pixelsPerBeat
            guard x >= rect.minX, x < rect.maxX else { return nil }
            let value = preview?.value(for: note) ?? Int(note.velocity >> 9)
            let y = baseline - span * Double(value) / Double(ModelLimits.midi7.upperBound)
            return PianoVelocityBar(
                note: note, stem: CGRect(x: x, y: y, width: 2, height: max(1, baseline - y)),
                cap: CGRect(x: x - 3, y: y - 3, width: 8, height: 6))
        }
    }

    func hit(at point: CGPoint, bars: [PianoVelocityBar], focus: PianoNote.ID?) -> PianoNote? {
        guard rect.contains(point) else { return nil }
        // Caps disambiguate chord tones with distinct values, even behind a selected stem.
        let caps = bars.filter { $0.cap.insetBy(dx: -2, dy: -2).contains(point) }
        let stems = caps.isEmpty ? bars.filter { $0.stem.insetBy(dx: -4, dy: -3).contains(point) } : caps
        return
            (stems.first { $0.note.id == focus }
            ?? stems.min {
                abs($0.cap.midY - point.y) < abs($1.cap.midY - point.y)
            })?.note
    }
}
