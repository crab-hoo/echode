import AppKit

/// Click rules shared by every value control (ADR 0153). A single click never
/// changes the value, and a drag moves it by the pointer's distance from the
/// press. A double click shows the default at once and records it when no
/// third click follows within the double-click interval. A third click puts
/// the original value back and opens a text field instead, so only the
/// confirmed result enters Undo.
@MainActor
final class ValueClicks<Target: Equatable> {
    enum Phase: Equatable {
        case idle
        /// Pressed once. Nothing changes until the pointer moves.
        case pressed(Target, CGPoint)
        case dragging(Target, CGPoint)
        /// The default is shown while the second press is held.
        case resetting(Target)
        /// The default is shown and a third click may still turn it into typing.
        case waiting(Target)
    }

    private(set) var phase = Phase.idle
    /// Starts a live preview of the value. False refuses the gesture.
    var begin: (Target) -> Bool = { _ in false }
    /// The pointer moved this far from the press while dragging.
    var drag: (Target, CGSize) -> Void = { _, _ in }
    /// Shows the default in the live preview.
    var reset: (Target) -> Void = { _ in }
    /// Ends the preview. True records it as one Undo step; false restores the value.
    var finish: (Bool) -> Void = { _ in }
    /// Opens the text field over the control.
    var type: (Target) -> Void = { _ in }
    /// How long a shown default waits for a third click.
    var interval = NSEvent.doubleClickInterval
    private var timer: DispatchWorkItem?

    var active: Bool { phase != .idle }

    /// `clicks` is the press's click count.
    func down(_ target: Target, at point: CGPoint, clicks: Int) {
        if case .waiting(let shown) = phase, clicks >= 3, shown == target {
            stopTimer()
            phase = .idle
            finish(false)
            type(target)
            return
        }
        settle()
        switch clicks {
        case ...1: phase = .pressed(target, point)
        case 2: if begin(target) { reset(target); phase = .resetting(target) }
        default: type(target)
        }
    }

    func dragged(to point: CGPoint) {
        switch phase {
        case .pressed(let target, let start):
            guard point != start else { return }
            guard begin(target) else { phase = .idle; return }
            phase = .dragging(target, start)
            drag(target, CGSize(width: point.x - start.x, height: point.y - start.y))
        case .dragging(let target, let start):
            drag(target, CGSize(width: point.x - start.x, height: point.y - start.y))
        case .idle, .resetting, .waiting:
            break
        }
    }

    func up(at point: CGPoint) {
        switch phase {
        case .pressed:
            phase = .idle
        case .dragging:
            dragged(to: point)
            phase = .idle
            finish(true)
        case .resetting(let target):
            phase = .waiting(target)
            let work = DispatchWorkItem { [weak self] in MainActor.assumeIsolated { self?.settle() } }
            timer = work
            DispatchQueue.main.asyncAfter(deadline: .now() + interval, execute: work)
        case .idle, .waiting:
            break
        }
    }

    /// Records a shown default or a drag now, for example before another
    /// control, key or document edit is used.
    func settle() {
        stopTimer()
        switch phase {
        case .dragging, .resetting, .waiting: phase = .idle; finish(true)
        case .pressed: phase = .idle
        case .idle: break
        }
    }

    /// Escape, a cancelled press, or the control leaving its window. The value
    /// goes back to what it was before the press.
    func cancel() {
        stopTimer()
        switch phase {
        case .dragging, .resetting, .waiting: phase = .idle; finish(false)
        case .pressed: phase = .idle
        case .idle: break
        }
    }

    private func stopTimer() {
        timer?.cancel()
        timer = nil
    }
}

/// The text field a third click opens over a value control (ADR 0153).
/// Return commits and Escape cancels. Leaving the field commits a usable
/// entry and drops one that is not. While it edits, plain keys stay in the
/// field, so Space types a space instead of starting playback.
@MainActor
final class ValueEntryField: NSTextField, NSTextFieldDelegate {
    private var commit: ((String) async -> Bool)?
    private var onClose: (() -> Void)?
    private weak var host: NSView?
    private(set) var closed = false
    private(set) var invalid = false
    /// The commit in flight, for callers that wait for its result.
    private(set) var pending: Task<Void, Never>?

    /// Opens a field centered on `rect` in `host`, wide enough to read, with
    /// `text` selected. `commit` returns false when the entry is unusable.
    @discardableResult
    static func open(in host: NSView, over rect: CGRect, text: String, label: String,
                     commit: @escaping (String) async -> Bool, onClose: @escaping () -> Void = {}) -> ValueEntryField {
        let field = ValueEntryField(frame: frame(for: rect, in: host.bounds))
        field.commit = commit
        field.onClose = onClose
        field.host = host
        field.stringValue = text
        field.alignment = .right
        field.font = .monospacedDigitSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        field.bezelStyle = .roundedBezel
        field.isBezeled = true
        field.usesSingleLineMode = true
        field.cell?.isScrollable = true
        field.delegate = field
        field.setAccessibilityLabel(label)
        host.addSubview(field)
        host.window?.makeFirstResponder(field)
        field.currentEditor()?.selectAll(nil)
        return field
    }

    /// At least 72 by 22 points, centered on the control and kept inside the host.
    static func frame(for rect: CGRect, in bounds: CGRect) -> CGRect {
        let width = max(72, rect.width)
        let height = max(22, rect.height)
        var frame = CGRect(x: rect.midX - width / 2, y: rect.midY - height / 2, width: width, height: height)
        frame.origin.x = min(max(bounds.minX, frame.minX), max(bounds.minX, bounds.maxX - width))
        frame.origin.y = min(max(bounds.minY, frame.minY), max(bounds.minY, bounds.maxY - height))
        return frame.integral
    }

    /// Return. An unusable entry keeps the field open and marks it.
    func submit() { submit(leaving: false) }

    /// Escape: closes without a change.
    func cancel() { close() }

    private func submit(leaving: Bool) {
        guard !closed, pending == nil, let commit else { return }
        let entry = stringValue
        pending = Task { [weak self] in
            let accepted = await commit(entry)
            guard let self else { return }
            self.pending = nil
            if accepted || leaving { self.close() } else { self.mark(invalid: true) }
        }
    }

    private func close() {
        guard !closed else { return }
        closed = true
        commit = nil
        let focused = currentEditor() != nil
        abortEditing()
        if focused, let host, host.window != nil { host.window?.makeFirstResponder(host) }
        removeFromSuperview()
        onClose?()
        onClose = nil
    }

    private func mark(invalid: Bool) {
        self.invalid = invalid
        textColor = invalid ? .systemRed : .controlTextColor
        setAccessibilityValueDescription(invalid ? text("Enter a valid value in range") : nil)
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard let editor = currentEditor(), event.type == .keyDown else { return super.performKeyEquivalent(with: event) }
        let modifiers = event.modifierFlags.intersection([.command, .control])
        if modifiers.isEmpty {
            // Menu shortcuts without a modifier, such as Space, must not take typing.
            editor.keyDown(with: event)
            return true
        }
        if modifiers == .command, event.charactersIgnoringModifiers?.lowercased() == "z" {
            // The field's own typing is undone, never a document edit.
            if event.modifierFlags.contains(.shift) { editor.undoManager?.redo() } else { editor.undoManager?.undo() }
            return true
        }
        return super.performKeyEquivalent(with: event)
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        switch commandSelector {
        case #selector(NSResponder.insertNewline(_:)): submit(leaving: false); return true
        case #selector(NSResponder.cancelOperation(_:)): cancel(); return true
        default: return false
        }
    }

    func controlTextDidChange(_ obj: Notification) { if invalid { mark(invalid: false) } }

    func controlTextDidEndEditing(_ obj: Notification) { submit(leaving: true) }
}
