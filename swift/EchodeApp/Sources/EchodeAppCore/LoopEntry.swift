import EchodeBridge

/// Beat text belongs to the input boundary, never to the shared transport draft.
struct LoopEntry: Equatable {
    var start: String
    var end: String
    private let original: LoopRange
    private let shownStart: String
    private let shownEnd: String

    init(range: LoopRange) {
        original = range
        shownStart = NoteDraft.format(Tick(rawValue: range.start).beats)
        shownEnd = NoteDraft.format(Tick(rawValue: range.end).beats)
        start = shownStart
        end = shownEnd
    }

    var range: LoopRange? {
        // Display text can be rounded, especially beyond Double's exact integer range.
        // Leaving one field alone must preserve that endpoint, even when the other changes.
        let startTick = start == shownStart ? original.start : NoteDraft.number(start).flatMap(Tick.ticks)
        let endTick = end == shownEnd ? original.end : NoteDraft.number(end).flatMap(Tick.ticks)
        guard let startTick, let endTick else { return nil }
        return LoopRange(start: startTick, end: endTick)
    }
}
