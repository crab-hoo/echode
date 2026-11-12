/// Ephemeral note-entry state for one open document. Each model edit reserves
/// one history slot; selection alone changes the length without adding Undo.
struct NoteInputLength {
    struct Change {
        let before: Int64?
        let after: Int64
        let generation: UInt64
    }

    private(set) var length: Int64?
    private var generation: UInt64 = 0
    private var past: [Change?] = []
    private var future: [Change?] = []

    mutating func remember(_ length: Int64) {
        guard length > 0 else { return }
        self.length = length
        generation &+= 1
    }

    func change(to length: Int64?) -> Change? {
        guard let length, length > 0 else { return nil }
        return Change(before: self.length, after: length, generation: generation)
    }

    mutating func record(_ change: Change?) {
        past.append(change)
        future.removeAll()
        // A selection made while a journal write awaited must remain newer.
        if let change, generation == change.generation { remember(change.after) }
    }

    mutating func undo() {
        guard let entry = past.popLast() else { return }
        future.append(entry)
        if let entry {
            length = entry.before
            generation &+= 1
        }
    }

    mutating func redo() {
        guard let entry = future.popLast() else { return }
        past.append(entry)
        if let entry { remember(entry.after) }
    }
}

extension DocumentSession {
    /// All journaled mutations, including plugin state and revision restores,
    /// share this slot accounting with note input's non-persistent history.
    func recordEdit(inputLength: NoteInputLength.Change? = nil) {
        noteInput.record(inputLength)
        undoCount += 1
        redoCount = 0
    }
}
