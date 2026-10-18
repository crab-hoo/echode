import EchodeBridge
import Foundation

/// Numeric text is a draft, not a model edit. Its identity includes the note,
/// so equal geometry on another note cannot reuse pending edits.
struct NoteDraft {
    private(set) var document: UUID?
    private(set) var track: UUID?
    private(set) var note: PianoNote.ID?
    private(set) var original: NoteGeometry?
    var pitch = 60
    var beat = "0"
    var length = "1"

    var geometry: NoteGeometry? {
        guard let beat = Self.number(beat), let length = Self.number(length),
              let start = PianoCamera.ticks(beat), let duration = PianoCamera.ticks(length) else { return nil }
        let value = NoteGeometry(start: start, length: duration, pitch: pitch)
        return value.isValid ? value : nil
    }
    var dirty: Bool { geometry != original }
    var canApply: Bool { note != nil && geometry != nil && geometry != original }
    var validation: String? { geometry == nil ? text("Note values help") : nil }

    mutating func synchronize(document: UUID?, track: UUID?, note: PianoNote?) {
        let geometry = note.map(NoteGeometry.init)
        guard self.document != document || self.track != track || self.note != note?.id || original != geometry else { return }
        self.document = document; self.track = track; self.note = note?.id; original = geometry
        reset()
    }

    mutating func reset() {
        pitch = original?.pitch ?? 60
        beat = Self.format(Double(original?.start ?? 0) / 705_600)
        length = Self.format(Double(original?.length ?? 705_600) / 705_600)
    }

    static func number(_ value: String, locale: Locale = .current) -> Double? {
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "−", with: "-")
            .replacingOccurrences(of: "–", with: "-")
        let scanner = Scanner(string: normalized)
        scanner.locale = locale
        guard let number = scanner.scanDouble(), scanner.isAtEnd, number.isFinite else { return nil }
        return number
    }
    private static func format(_ value: Double) -> String {
        value.formatted(.number.grouping(.never).precision(.fractionLength(0...9)))
    }
}
