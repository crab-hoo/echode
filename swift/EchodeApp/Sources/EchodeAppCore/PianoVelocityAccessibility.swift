import AppKit
import EchodeBridge

/// Built on accessibility queries, never on the render thread or frame notifications.
@MainActor
final class PianoVelocityAccessibility {
    private let group = NSAccessibilityElement()
    private var elements: [PianoNote.ID: PianoAccessibleNote] = [:]

    func clear() {
        elements = [:]
        group.setAccessibilityChildren([])
    }

    func element(
        frame: PianoFrame, focus: PianoNote.ID?, parent: NSView, convert: (NSRect) -> NSRect,
        select: @escaping (PianoNote) -> Void, change: @escaping (PianoNote.ID, Int) -> Bool
    ) -> NSAccessibilityElement {
        group.setAccessibilityElement(true)
        group.setAccessibilityRole(.group)
        group.setAccessibilityLabel(text("Velocity"))
        group.setAccessibilityParent(parent)
        group.setAccessibilityFrame(convert(frame.layout.velocity))
        let bars = frame.velocityBars
        let visible = Set(bars.map(\.note.id))
        elements = elements.filter { visible.contains($0.key) }
        let children = bars.map { bar -> PianoAccessibleNote in
            let note = bar.note
            let value = frame.velocity(for: note)
            let child = elements[note.id] ?? PianoAccessibleNote(actions: PianoAccessibilityActions())
            elements[note.id] = child
            child.setAccessibilityElement(true)
            child.setAccessibilityRole(.slider)
            child.setAccessibilityParent(group)
            child.setAccessibilityIdentifier("velocity/\(note.id.track)/\(note.id.placement)/\(note.id.note)")
            child.setAccessibilityLabel(
                "\(text("Velocity")), \(noteName(note.pitch)), \(text("Start beat")) \(Double(note.start) / Tick.perBeat)"
            )
            child.setAccessibilityValue(value)
            child.setAccessibilityMinValue(0)
            child.setAccessibilityMaxValue(ModelLimits.midi7.upperBound)
            child.setAccessibilityFocused(note.id == focus)
            child.setAccessibilityFrame(convert(bar.stem.union(bar.cap).intersection(frame.layout.velocity)))
            child.actions.press = {
                select(note)
                return true
            }
            child.actions.increment = { change(note.id, min(ModelLimits.midi7.upperBound, value + 1)) }
            child.actions.decrement = { change(note.id, max(0, value - 1)) }
            return child
        }
        group.setAccessibilityChildren(children)
        return group
    }
}
