import AppKit
import Carbon.HIToolbox
import EchodeBridge
import QuartzCore
import SwiftUI

struct PianoCanvas: NSViewRepresentable {
    let snapshot: Snapshot
    let track: UUID
    @Binding var focus: PianoNote.ID?
    let key: DisplayKey
    let busy: Bool
    let insert: (Int, Double, Double) -> Void
    let edit: (PianoNote.ID, NoteGeometry) -> Void
    let delete: (PianoNote.ID) -> Void
    var playback: PlaybackFeed? = nil
    var tick: Int64 = 0

    func makeNSView(context: Context) -> PianoRollView { PianoRollView(frame: .zero) }
    func updateNSView(_ view: PianoRollView, context: Context) {
        view.grid.onSelect = { focus = $0 }
        view.grid.onInsert = insert
        view.grid.onEdit = edit
        view.grid.onDelete = delete
        view.grid.setSnapshot(snapshot, track: track, focus: focus, key: key, busy: busy)
        view.grid.setPlayback(playback)
        view.needsDisplay = true
    }
    static func dismantleNSView(_ view: PianoRollView, coordinator: ()) { view.grid.stop() }
}

@MainActor
final class PianoRollView: NSView {
    let grid = PianoGridView(frame: .zero)
    private let labelWidth = 120.0
    private let rulerHeight = 24.0
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        addSubview(grid)
        grid.onCamera = { [weak self] in self?.needsDisplay = true }
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
    override func layout() {
        super.layout()
        grid.frame = NSRect(x: labelWidth, y: rulerHeight, width: max(1, bounds.width - labelWidth), height: max(1, bounds.height - rulerHeight))
        needsDisplay = true
    }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill(); bounds.fill()
        let camera = grid.camera
        let attributes: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedSystemFont(ofSize: 11, weight: .regular), .foregroundColor: NSColor.labelColor]
        (text("Pitch / Solfege") as NSString).draw(at: NSPoint(x: 5, y: 5), withAttributes: attributes)
        for pitch in 0...127 {
            let y = rulerHeight + (camera.topPitch - Double(pitch)) * camera.pixelsPerPitch
            guard y + camera.pixelsPerPitch > rulerHeight, y < bounds.height else { continue }
            let black = [1, 3, 6, 8, 10].contains(pitch % 12)
            (black ? NSColor.controlBackgroundColor : NSColor.textBackgroundColor).setFill()
            let top = max(rulerHeight, y)
            let bottom = min(bounds.height, y + camera.pixelsPerPitch - 1)
            NSRect(x: 0, y: top, width: labelWidth - 1, height: max(0, bottom - top)).fill()
            if y >= rulerHeight {
                let label = "\(noteName(pitch))  \(grid.key.syllable(pitch))"
                (label as NSString).draw(at: NSPoint(x: 6, y: y + 2), withAttributes: attributes)
            }
        }
        let first = Int(camera.originBeat.rounded(.up))
        let last = Int((camera.originBeat + grid.bounds.width / camera.pixelsPerBeat).rounded(.up))
        for beat in first...max(first, last) {
            let x = labelWidth + (Double(beat) - camera.originBeat) * camera.pixelsPerBeat
            (String(beat) as NSString).draw(at: NSPoint(x: x + 3, y: 5), withAttributes: attributes)
        }
    }
}

@MainActor
final class PianoGridView: NSView {
    private let metalLayer = CAMetalLayer()
    private var renderer: CanvasRenderer?
    private var snapshot: Snapshot?
    private var playback: PlaybackFeed?
    private var track: UUID?
    private var focus: PianoNote.ID?
    private var announcedFocus: PianoNote.ID?
    private var busy = false
    private var elements: [PianoNote] = []
    private var accessible: [PianoNote.ID: PianoAccessibleNote] = [:]
    private var gesture: PianoGesture?
    private var preview: PianoPreview?
    private var caretTick: Int64 = 0
    private var caretPitch = 60
    private(set) var camera = PianoCamera()
    private(set) var key = DisplayKey()
    var onCamera: (() -> Void)?
    var onSelect: ((PianoNote.ID?) -> Void)?
    var onInsert: ((Int, Double, Double) -> Void)?
    var onEdit: ((PianoNote.ID, NoteGeometry) -> Void)?
    var onDelete: ((PianoNote.ID) -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel(text("Piano Roll"))
        setAccessibilityHelp(text("Piano keyboard help"))
        setAccessibilityCustomActions([
            NSAccessibilityCustomAction(name: text("Next Note")) { [weak self] in self?.navigate(backwards: false) ?? false },
            NSAccessibilityCustomAction(name: text("Previous Note")) { [weak self] in self?.navigate(backwards: true) ?? false },
            NSAccessibilityCustomAction(name: text("Insert Note")) { [weak self] in self?.insertAtCaret() ?? false },
        ])
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override func makeBackingLayer() -> CALayer { metalLayer }
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard window != nil else { stop(); return }
        if renderer == nil {
            do {
                renderer = try CanvasRenderer(layer: metalLayer, size: bounds.size, scale: window?.backingScaleFactor ?? 1,
                    onElements: { _ in }, onPiano: { [weak self] notes, version in
                        guard let self, self.snapshot === version else { return }
                        self.elements = notes
                        NSAccessibility.post(element: self, notification: .layoutChanged)
                        _ = self.accessibilityChildren()
                        if self.focus != self.announcedFocus {
                            if let focus = self.focus, let child = self.accessible[focus] {
                                NSAccessibility.post(element: child, notification: .focusedUIElementChanged)
                            }
                            self.announcedFocus = self.focus
                        }
                    })
            } catch {
                NSLog("Echode piano renderer: %@", String(describing: error))
                let label = NSTextField(labelWithString: text("Metal is unavailable"))
                label.frame = bounds; label.autoresizingMask = [.width, .height]; addSubview(label)
            }
        }
        refresh()
    }
    func stop() { renderer?.stop(); renderer = nil; cancelGesture() }
    func beginMeasurement() { renderer?.beginMeasurement() }
    func markMainBusy(_ busy: Bool) { renderer?.markMainBusy(busy) }
    var measurement: ArrangeMeasurement { renderer?.measurement ?? ArrangeMeasurement() }
    var rendererAvailable: Bool { renderer != nil }
    func setSnapshot(_ snapshot: Snapshot, track: UUID, focus: PianoNote.ID?, key: DisplayKey, busy: Bool) {
        let changed = self.snapshot !== snapshot || self.track != track
        if changed || busy { gesture = nil; preview = nil }
        if self.track != track { accessible = [:] }
        if changed { elements = [] }
        let needsRefresh = changed || self.focus != focus || self.key != key || self.busy != busy
        self.snapshot = snapshot; self.track = track; self.key = key; self.busy = busy
        self.focus = focus?.track == track ? focus : nil
        if needsRefresh {
            // Clicking an already visible, partly clipped note must not move
            // the camera under a drag. Reveal after edits or offscreen focus.
            if let focus = self.focus, (changed || !elements.contains(where: { $0.id == focus })),
               let note = snapshot.pianoNote(focus) { camera.reveal(note, width: bounds.width, height: bounds.height) }
            refresh()
        }
    }
    override func layout() { super.layout(); refresh() }
    override func viewDidChangeBackingProperties() { super.viewDidChangeBackingProperties(); refresh() }

    override func scrollWheel(with event: NSEvent) {
        cancelGesture()
        camera.originBeat = max(0, camera.originBeat - Double(event.scrollingDeltaX) / camera.pixelsPerBeat)
        camera.topPitch = min(127, max(0, camera.topPitch + Double(event.scrollingDeltaY) / camera.pixelsPerPitch))
        refresh()
    }
    override func magnify(with event: NSEvent) {
        cancelGesture()
        let factor = max(0.1, 1 + Double(event.magnification))
        if event.modifierFlags.contains(.option) || event.modifierFlags.contains(.shift) {
            camera.pixelsPerPitch = min(80, max(12, camera.pixelsPerPitch * factor))
        }
        if !event.modifierFlags.contains(.option) || event.modifierFlags.contains(.shift) {
            camera.pixelsPerBeat = min(1200, max(8, camera.pixelsPerBeat * factor))
        }
        refresh()
    }
    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        guard !busy else { return }
        let point = convert(event.locationInWindow, from: nil)
        caretTick = max(0, camera.tick(at: point.x) ?? 0); caretPitch = camera.pitch(at: point.y)
        if let hit = elements.last(where: { NSRect(x: Double($0.x), y: Double($0.y), width: max(2, Double($0.width)), height: Double($0.height)).contains(point) }) {
            select(hit)
            let start = (Double(hit.start) / 705_600 - camera.originBeat) * camera.pixelsPerBeat
            let end = start + Double(hit.length) / 705_600 * camera.pixelsPerBeat
            let mode = PianoGesture.mode(at: point.x, start: start, end: end)
            gesture = PianoGesture(id: hit.id, original: NoteGeometry(hit), mode: mode, x: point.x, y: point.y)
        } else {
            focus = nil; onSelect?(nil); cancelGesture()
            if event.clickCount >= 2 { _ = insertAtCaret() }
        }
    }
    override func mouseDragged(with event: NSEvent) {
        guard let gesture, !busy else { return }
        let point = convert(event.locationInWindow, from: nil)
        preview = gesture.preview(x: point.x, y: point.y, camera: camera).map { PianoPreview(id: gesture.id, geometry: $0) }
        refresh()
    }
    override func mouseUp(with event: NSEvent) {
        let finished = preview
        let original = gesture?.original
        cancelGesture()
        if let finished, finished.geometry != original, !busy { onEdit?(finished.id, finished.geometry) }
    }
    override func keyDown(with event: NSEvent) {
        guard !event.modifierFlags.contains(.command) else { super.keyDown(with: event); return }
        let shift = event.modifierFlags.contains(.shift)
        switch Int(event.keyCode) {
        case kVK_Escape: cancelGesture()
        case kVK_Tab: if !navigate(backwards: shift) { super.keyDown(with: event) }
        case kVK_Return, kVK_ANSI_KeypadEnter: _ = insertAtCaret()
        case kVK_Delete, kVK_ForwardDelete: if !busy, let focus { onDelete?(focus) }
        case kVK_UpArrow: change(pitch: event.modifierFlags.contains(.option) ? 12 : 1)
        case kVK_DownArrow: change(pitch: event.modifierFlags.contains(.option) ? -12 : -1)
        case kVK_LeftArrow: change(time: -camera.grid, resize: shift)
        case kVK_RightArrow: change(time: camera.grid, resize: shift)
        default: super.keyDown(with: event)
        }
    }
    @discardableResult
    func navigate(backwards: Bool) -> Bool {
        guard let track, let note = snapshot?.pianoNeighbor(track: track, current: focus, backwards: backwards) else { return false }
        cancelGesture(); select(note)
        camera.reveal(note, width: bounds.width, height: bounds.height); refresh()
        return true
    }
    private func select(_ note: PianoNote) {
        focus = note.id; caretTick = max(0, note.start); caretPitch = note.pitch
        onSelect?(note.id); refresh()
    }
    private func insertAtCaret() -> Bool {
        guard !busy, track != nil else { return false }
        onInsert?(caretPitch, Double(caretTick) / 705_600, Double(camera.grid) / 705_600)
        return true
    }
    private func change(pitch: Int = 0, time: Int64 = 0, resize: Bool = false) {
        guard !busy, let focus, let note = snapshot?.pianoNote(focus) else { return }
        var value = NoteGeometry(note)
        value.pitch = min(127, max(0, value.pitch + pitch))
        let moved = (resize ? value.length : value.start).addingReportingOverflow(time)
        guard !moved.overflow else { return }
        if resize { value.length = max(camera.grid, moved.partialValue) } else { value.start = max(0, moved.partialValue) }
        if value.isValid { onEdit?(note.id, value) }
    }
    private func cancelGesture() { gesture = nil; preview = nil; refresh() }
    func setPlayback(_ playback: PlaybackFeed?) { self.playback = playback; refresh() }
    private func refresh() {
        onCamera?()
        guard let track else { return }
        renderer?.update(ArrangeInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(),
            width: bounds.width, height: bounds.height, scale: Double(window?.backingScaleFactor ?? 1),
            piano: PianoRenderInput(track: track, focus: focus, camera: camera, key: key, preview: preview), playback: playback))
    }

    override func accessibilityChildren() -> [Any]? {
        guard let window else { return [] }
        guard !elements.isEmpty else { return [] }
        var shown: [PianoNote.ID: PianoAccessibleNote] = [:]
        let children = elements.map { note in
            let child = accessible[note.id] ?? PianoAccessibleNote(actions: PianoAccessibilityActions())
            let rect = NSRect(x: Double(note.x), y: Double(note.y), width: max(2, Double(note.width)), height: Double(note.height))
            child.setAccessibilityElement(true); child.setAccessibilityRole(.button)
            child.setAccessibilityParent(self)
            child.setAccessibilityFrame(window.convertToScreen(convert(rect, to: nil)))
            child.setAccessibilityIdentifier("\(note.id.track)/\(note.id.placement)/\(note.id.note)")
            child.setAccessibilityLabel("\(noteName(note.pitch)) \(key.syllable(note.pitch)), \(text("Start beat")) \(Double(note.start) / 705_600), \(text("Length in beats")) \(Double(note.length) / 705_600)")
            child.setAccessibilityValue(note.id == focus ? text("Selected") : "")
            child.setAccessibilityFocused(note.id == focus)
            child.actions.press = { [weak self] in self?.select(note); return true }
            child.actions.increment = { [weak self] in self?.select(note); self?.change(pitch: 1); return true }
            child.actions.decrement = { [weak self] in self?.select(note); self?.change(pitch: -1); return true }
            child.setAccessibilityCustomActions([
                NSAccessibilityCustomAction(name: text("Delete Note")) { [weak self] in guard let self, !self.busy else { return false }; self.onDelete?(note.id); return true },
                NSAccessibilityCustomAction(name: text("Move Earlier")) { [weak self] in self?.select(note); if let self { self.change(time: -self.camera.grid) }; return true },
                NSAccessibilityCustomAction(name: text("Move Later")) { [weak self] in self?.select(note); if let self { self.change(time: self.camera.grid) }; return true },
                NSAccessibilityCustomAction(name: text("Shorten Note")) { [weak self] in self?.select(note); if let self { self.change(time: -self.camera.grid, resize: true) }; return true },
                NSAccessibilityCustomAction(name: text("Lengthen Note")) { [weak self] in self?.select(note); if let self { self.change(time: self.camera.grid, resize: true) }; return true },
            ])
            shown[note.id] = child; return child
        }
        accessible = shown
        return children
    }

}

@MainActor
private final class PianoAccessibilityActions {
    var press: (() -> Bool)?
    var increment: (() -> Bool)?
    var decrement: (() -> Bool)?
}

@MainActor
private final class PianoAccessibleNote: NSAccessibilityElement {
    nonisolated let actions: PianoAccessibilityActions
    init(actions: PianoAccessibilityActions) { self.actions = actions; super.init() }
    // Only the immutable, Sendable action holder crosses the Objective-C
    // callback boundary. Its mutable closures are confined to MainActor.
    override func accessibilityPerformPress() -> Bool { Task { @MainActor [actions] in _ = actions.press?() }; return true }
    override func accessibilityPerformIncrement() -> Bool { Task { @MainActor [actions] in _ = actions.increment?() }; return true }
    override func accessibilityPerformDecrement() -> Bool { Task { @MainActor [actions] in _ = actions.decrement?() }; return true }
}
