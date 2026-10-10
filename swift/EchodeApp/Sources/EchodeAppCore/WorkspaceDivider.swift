import AppKit
import SwiftUI

struct WorkspaceDivider: NSViewRepresentable {
    let pane: WorkspacePane
    let value: Double
    let resize: (Double) -> Void
    let finish: () -> Void
    func makeNSView(context: Context) -> WorkspaceSplitter { WorkspaceSplitter(frame: .zero) }
    func updateNSView(_ view: WorkspaceSplitter, context: Context) {
        view.pane = pane; view.value = value; view.resize = resize; view.finish = finish
        view.setAccessibilityLabel(text(pane == .editor ? "Note Editor" : pane == .inspector ? "Inspector" : "Files"))
    }
}

@MainActor
final class WorkspaceSplitter: NSView {
    var pane = WorkspacePane.inspector
    var value = 0.0
    var resize: ((Double) -> Void)?
    var finish: (() -> Void)?
    private var start = CGPoint.zero
    private var initial = 0.0
    override init(frame: NSRect) {
        super.init(frame: frame)
        setAccessibilityElement(true); setAccessibilityRole(.splitter)
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
    override var acceptsFirstResponder: Bool { true }
    override func draw(_ dirtyRect: NSRect) { NSColor.separatorColor.setFill(); bounds.fill() }
    override func resetCursorRects() { addCursorRect(bounds, cursor: pane == .editor ? .resizeUpDown : .resizeLeftRight) }
    override func mouseDown(with event: NSEvent) { start = event.locationInWindow; initial = value; window?.makeFirstResponder(self) }
    override func mouseDragged(with event: NSEvent) {
        let delta = pane == .editor ? event.locationInWindow.y - start.y : event.locationInWindow.x - start.x
        resize?(initial + (pane == .browser ? -delta : delta))
    }
    override func mouseUp(with event: NSEvent) { finish?() }
    override func accessibilityValue() -> Any? { NSNumber(value: value) }
    override func accessibilityPerformIncrement() -> Bool { adjust(10) }
    override func accessibilityPerformDecrement() -> Bool { adjust(-10) }
    override func keyDown(with event: NSEvent) {
        switch event.keyCode {
        case 123: _ = adjust(pane == .browser ? 10 : -10)
        case 124: _ = adjust(pane == .browser ? -10 : 10)
        case 125: _ = adjust(-10)
        case 126: _ = adjust(10)
        default: super.keyDown(with: event)
        }
    }
    private func adjust(_ delta: Double) -> Bool { guard let resize else { return false }; resize(value + delta); finish?(); return true }
}
