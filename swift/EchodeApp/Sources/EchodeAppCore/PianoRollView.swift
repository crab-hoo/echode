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
    var start: Int64? = nil
    var setStart: (Int64) -> Void = { _ in }
    var loop: RulerLoop? = nil
    var setLoop: (LoopRange) -> Void = { _ in }
    var playback: PlaybackFeed? = nil
    var transportGeneration: UInt64 = 0
    var navigation: CanvasNavigationState? = nil
    var documentID: UUID? = nil
    var followsPlayhead = false
    var onManualScroll: (() -> Void)? = nil

    func makeNSView(context: Context) -> PianoRollView { PianoRollView(frame: .zero) }
    func updateNSView(_ view: PianoRollView, context: Context) {
        view.grid.setNavigation(navigation, documentID: documentID)
        view.grid.onSelect = { focus = $0 }
        view.grid.onInsert = insert
        view.grid.onEdit = edit
        view.grid.onDelete = delete
        view.grid.onSetStart = setStart
        view.grid.setStart(start)
        view.grid.onSetLoop = setLoop
        view.grid.setLoop(loop)
        view.grid.setSnapshot(snapshot, track: track, focus: focus, key: key, busy: busy)
        view.grid.setPlayback(playback)
        view.grid.onManualScroll = onManualScroll
        view.grid.follower.setFollowing(followsPlayhead)
        view.grid.noteTransport(transportGeneration)
    }
    static func dismantleNSView(_ view: PianoRollView, coordinator: ()) { view.grid.stop() }
}

@MainActor
final class PianoRollView: NSView {
    let grid = PianoGridView(frame: .zero)
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        clipsToBounds = true
        addSubview(grid)
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
    override func layout() {
        super.layout()
        grid.frame = bounds
    }
}

@MainActor
final class PianoGridView: NSView {
    private let metalLayer = CAMetalLayer()
    private var renderer: CanvasRenderer?
    private var feedback = FrameFeedbackGate()
    private var submittedFrame: PianoFrame?
    private var navigation: CanvasNavigationState?
    private var navigationDocument: UUID?
    private var snapshot: Snapshot?
    private var playback: PlaybackFeed?
    private var transportGeneration: UInt64 = 0
    private var track: UUID?
    private var focus: PianoNote.ID?
    private var announcedFocus: PianoNote.ID?
    private var revealOnLayout = false
    private var busy = false
    private var elements: [PianoNote] = []
    private var accessible: [PianoNote.ID: PianoAccessibleNote] = [:]
    private var accessibleVersion: [PianoNote.ID: UInt64] = [:]
    private var submittedVersion: UInt64 = 0
    /// Elements are built only for accessibility queries and focus changes.
    private(set) var accessibilityBuilds = 0
    private var gesture: PianoGesture?
    private var preview: PianoPreview?
    private var insertion: PianoInsertion?
    var displayedInsertion: NoteGeometry? { insertion?.shouldInsert == true ? insertion?.geometry : nil }
    /// Geometry already sent for a note whose snapshot has not caught up.
    private var stepped: [PianoNote.ID: NoteGeometry] = [:]
    private var caretTick: Int64 = 0
    private var caretPitch = 60
    private var start: Int64?
    private var loop: RulerLoop?
    private var rulerPress: RulerPress?
    private(set) var camera = PianoCamera()
    private(set) var key = DisplayKey()
    var pianoLayout: PianoLayout { PianoLayout(width: bounds.width, height: bounds.height) }
    var onSelect: ((PianoNote.ID?) -> Void)?
    var onInsert: ((Int, Double, Double) -> Void)?
    var onEdit: ((PianoNote.ID, NoteGeometry) -> Void)?
    var onDelete: ((PianoNote.ID) -> Void)?
    var onSetStart: ((Int64) -> Void)?
    var onSetLoop: ((LoopRange) -> Void)?
    let follower = PlayheadFollower()
    /// A time scroll turns following off (ADR 0159).
    var onManualScroll: (() -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        clipsToBounds = true
        follower.show = { [weak self] in self?.followPlayhead($0) }
        CanvasSurface.configure(self, layer: metalLayer)
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
    override func resignFirstResponder() -> Bool { cancelGesture(); return super.resignFirstResponder() }
    override func makeBackingLayer() -> CALayer { metalLayer }
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard window != nil else { stop(); return }
        follower.attach(to: self)
        if renderer == nil {
            do {
                let generation = feedback.restart()
                renderer = try CanvasRenderer(layer: metalLayer,
                    onElements: { _ in }, onPiano: { [weak self] frame in
                        guard let self, self.snapshot === frame.input.snapshot, self.track == frame.piano.track,
                              self.feedback.accept(generation, revision: frame.revision) else { return }
                        self.present(frame)
                    })
            } catch {
                NSLog("Echode piano renderer: %@", String(describing: error))
                let label = NSTextField(labelWithString: text("Metal is unavailable"))
                label.frame = bounds; label.autoresizingMask = [.width, .height]; addSubview(label)
            }
        }
        refresh()
    }
    func stop() { follower.detach(); feedback.invalidate(); renderer?.stop(); renderer = nil; submittedFrame = nil; elements = []; accessible = [:]; accessibleVersion = [:]; announcedFocus = nil; accessibilityBuilds = 0; cancelGesture() }
    func beginMeasurement() { renderer?.beginMeasurement() }
    func markMainBusy(_ busy: Bool) { renderer?.markMainBusy(busy) }
    var measurement: ArrangeMeasurement { renderer?.measurement ?? ArrangeMeasurement() }
    var rendererAvailable: Bool { renderer != nil }
    func setNavigation(_ navigation: CanvasNavigationState?, documentID: UUID? = nil) {
        self.navigation = navigation; navigationDocument = documentID ?? navigation?.document
    }
    func setSnapshot(_ snapshot: Snapshot, track: UUID, focus: PianoNote.ID?, key: DisplayKey, busy: Bool) {
        let trackChanged = self.track != track
        let oldGeometry = self.focus.flatMap { self.snapshot?.pianoNote($0).map(NoteGeometry.init) }
        let newGeometry = focus.flatMap { snapshot.pianoNote($0).map(NoteGeometry.init) }
        let needsReveal = trackChanged || (self.snapshot !== snapshot && oldGeometry != newGeometry) ||
            (self.focus != focus && !elements.contains(where: { $0.id == focus }))
        if trackChanged { rememberViewport(); stepped = [:] }
        let memory = trackChanged && navigation?.permits(document: navigationDocument, track: track) == true ? navigation?.pianos[track] : nil
        let restoredFocus = memory != nil && memory?.focus == focus && memory?.geometry == focus.flatMap { snapshot.pianoNote($0).map(NoteGeometry.init) }
        if trackChanged { camera = memory?.camera ?? PianoCamera(); caretTick = 0; caretPitch = 60 }
        let changed = self.snapshot !== snapshot || self.track != track
        if changed || busy { gesture = nil; preview = nil; insertion = nil; rulerPress = nil; revealOnLayout = false }
        if self.track != track { accessible = [:] }
        if changed { elements = []; submittedFrame = nil }
        let needsRefresh = changed || self.focus != focus || self.key != key || self.busy != busy
        self.snapshot = snapshot; self.track = track; self.key = key; self.busy = busy
        self.focus = focus?.track == track ? focus : nil
        syncStepped()
        if needsRefresh {
            // Clicking an already visible, partly clipped note must not move
            // the camera under a drag. Reveal after edits or offscreen focus.
            if let focus = self.focus, !restoredFocus, needsReveal,
               let note = snapshot.pianoNote(focus) {
                if pianoLayout.grid.width > 0, pianoLayout.grid.height > 0 {
                    camera.reveal(note, width: pianoLayout.grid.width, height: pianoLayout.grid.height)
                } else { revealOnLayout = true }
            }
            refresh()
        }
    }
    override func setFrameSize(_ newSize: NSSize) {
        if bounds.size != newSize { rulerPress = nil; insertion = nil }
        super.setFrameSize(newSize); refresh()
    }
    override func setBoundsSize(_ newSize: NSSize) {
        if bounds.size != newSize { rulerPress = nil; insertion = nil }
        super.setBoundsSize(newSize); refresh()
    }
    override func layout() {
        super.layout()
        if revealOnLayout, pianoLayout.grid.width > 0, pianoLayout.grid.height > 0,
           let focus, let note = snapshot?.pianoNote(focus) {
            camera.reveal(note, width: pianoLayout.grid.width, height: pianoLayout.grid.height)
            revealOnLayout = false
        }
        refresh()
    }
    override func viewDidChangeBackingProperties() { super.viewDidChangeBackingProperties(); refresh() }

    override func scrollWheel(with event: NSEvent) {
        cancelGesture()
        let delta = WheelScroll.points(x: Double(event.scrollingDeltaX), y: Double(event.scrollingDeltaY),
            precise: event.hasPreciseScrollingDeltas, lineWidth: camera.pixelsPerBeat, lineHeight: camera.pixelsPerPitch)
        if PlayheadFollow.isTimeScroll(x: delta.x, y: delta.y) {
            // Released here first so the next frame does not page back.
            follower.setFollowing(false)
            onManualScroll?()
        }
        camera.originBeat = max(0, camera.originBeat - delta.x / camera.pixelsPerBeat)
        camera.topPitch = min(127, max(0, camera.topPitch + delta.y / camera.pixelsPerPitch))
        refresh()
    }
    override func magnify(with event: NSEvent) {
        cancelGesture()
        let factor = max(0.1, 1 + Double(event.magnification))
        let point = pianoLayout.gridPoint(convert(event.locationInWindow, from: nil))
        camera.zoom(factor: factor, horizontal: !event.modifierFlags.contains(.option) || event.modifierFlags.contains(.shift),
                    vertical: event.modifierFlags.contains(.option) || event.modifierFlags.contains(.shift),
                    anchor: CGPoint(x: max(0, point.x), y: max(0, point.y)))
        refresh()
    }
    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        guard !busy, let submittedFrame, submittedFrame.piano.camera == camera,
              submittedFrame.layout == pianoLayout, submittedFrame.input.snapshot === snapshot,
              submittedFrame.piano.track == track, submittedFrame.piano.preview == nil, submittedFrame.piano.insertion == nil else { return }
        let panePoint = convert(event.locationInWindow, from: nil)
        if pianoLayout.ruler.contains(panePoint) {
            // Defer the click until release so a loop drag never seeks.
            cancelGesture()
            rulerPress = RulerPress(x: pianoLayout.gridPoint(panePoint).x, originBeat: camera.originBeat,
                                    pixelsPerBeat: camera.pixelsPerBeat, grid: camera.grid,
                                    edge: rulerHandles.hit(at: panePoint), originalRange: loop?.range)
            return
        }
        guard pianoLayout.grid.contains(panePoint) else { return }
        stepped = [:]
        let point = pianoLayout.gridPoint(panePoint)
        caretTick = max(0, camera.tick(at: point.x) ?? 0); caretPitch = camera.pitch(at: point.y)
        if let hit = elements.last(where: { NSRect(x: Double($0.x), y: Double($0.y), width: max(2, Double($0.width)), height: Double($0.height)).contains(point) }) {
            select(hit)
            let start = (Double(hit.start) / 705_600 - camera.originBeat) * camera.pixelsPerBeat
            let end = start + Double(hit.length) / 705_600 * camera.pixelsPerBeat
            let mode = PianoGesture.mode(at: point.x, start: start, end: end)
            gesture = PianoGesture(id: hit.id, original: NoteGeometry(hit), mode: mode, x: point.x, y: point.y)
        } else {
            focus = nil; onSelect?(nil); cancelGesture()
            insertion = PianoInsertion(x: point.x, y: point.y, camera: camera,
                                       snapping: !event.modifierFlags.contains(.shift), insertsOnClick: event.clickCount >= 2)
            refresh()
        }
    }
    override func mouseDragged(with event: NSEvent) {
        if var press = rulerPress {
            guard !busy else { cancelGesture(); return }
            press.update(x: pianoLayout.gridPoint(convert(event.locationInWindow, from: nil)).x,
                         snapping: !event.modifierFlags.contains(.shift))
            rulerPress = press; refresh(); return
        }
        if var insertion {
            guard !busy else { cancelGesture(); return }
            let point = pianoLayout.gridPoint(convert(event.locationInWindow, from: nil))
            insertion.update(x: point.x, y: point.y, snapping: !event.modifierFlags.contains(.shift))
            self.insertion = insertion; refresh(); return
        }
        guard gesture != nil, !busy else { return }
        preview = notePreview(with: event)
        refresh()
    }
    override func mouseUp(with event: NSEvent) {
        if var press = rulerPress {
            press.update(x: pianoLayout.gridPoint(convert(event.locationInWindow, from: nil)).x,
                         snapping: !event.modifierFlags.contains(.shift))
            cancelGesture()
            guard !busy, press.originBeat == camera.originBeat, press.pixelsPerBeat == camera.pixelsPerBeat, press.grid == camera.grid else { return }
            if press.dragged {
                if let range = press.range { onSetLoop?(range) }
            } else if press.edge == nil, let tick = camera.rulerTick(at: press.x, snapping: !event.modifierFlags.contains(.shift)) {
                onSetStart?(tick)
            }
            return
        }
        if var insertion {
            let point = pianoLayout.gridPoint(convert(event.locationInWindow, from: nil))
            insertion.update(x: point.x, y: point.y, snapping: !event.modifierFlags.contains(.shift))
            cancelGesture()
            if insertion.shouldInsert, let geometry = insertion.geometry, !busy {
                caretTick = geometry.start; caretPitch = geometry.pitch
                onInsert?(geometry.pitch, Double(geometry.start) / 705_600, Double(geometry.length) / 705_600)
            }
            return
        }
        // A click alone must not quantize an off-grid note. After a drag, use
        // the release position and modifiers rather than the last drawn preview.
        let finished = preview == nil ? nil : notePreview(with: event)
        let original = gesture?.original
        cancelGesture()
        if let finished, finished.geometry != original, !busy { onEdit?(finished.id, finished.geometry) }
    }
    private func notePreview(with event: NSEvent) -> PianoPreview? {
        guard let gesture else { return nil }
        let point = pianoLayout.gridPoint(convert(event.locationInWindow, from: nil))
        return gesture.preview(x: point.x, y: point.y, camera: camera,
                               snapping: !event.modifierFlags.contains(.shift))
            .map { PianoPreview(id: gesture.id, geometry: $0) }
    }
    override func keyDown(with event: NSEvent) {
        guard !event.modifierFlags.contains(.command) else { super.keyDown(with: event); return }
        let shift = event.modifierFlags.contains(.shift)
        switch Int(event.keyCode) {
        case kVK_Escape: if gesture != nil || preview != nil || insertion != nil || rulerPress != nil { cancelGesture() } else { super.keyDown(with: event) }
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
        camera.reveal(note, width: max(1, pianoLayout.grid.width), height: max(1, pianoLayout.grid.height)); refresh()
        return true
    }
    private func select(_ note: PianoNote) {
        guard note.id.track == track, snapshot?.pianoNote(note.id) != nil else { return }
        focus = note.id; caretTick = max(0, note.start); caretPitch = note.pitch
        onSelect?(note.id); refresh()
    }
    private func insertAtCaret() -> Bool {
        guard !busy, track != nil else { return false }
        onInsert?(caretPitch, Double(caretTick) / 705_600, Double(camera.grid) / 705_600)
        return true
    }
    private func change(pitch: Int = 0, time: Int64 = 0, resize: Bool = false) {
        // The previous edit may still be in flight. Step from the geometry
        // already sent, and let the controller queue the write.
        guard let focus, let current = noteBasis(focus) else { return }
        var value = current
        value.pitch = min(127, max(0, value.pitch + pitch))
        let moved = (resize ? value.length : value.start).addingReportingOverflow(time)
        guard !moved.overflow else { return }
        if resize { value.length = max(camera.grid, moved.partialValue) } else { value.start = max(0, moved.partialValue) }
        guard value.isValid, value != current else { return }
        stepped[focus] = value
        onEdit?(focus, value)
    }
    private func noteBasis(_ id: PianoNote.ID) -> NoteGeometry? {
        if let geometry = stepped[id] { return geometry }
        return snapshot?.pianoNote(id).map(NoteGeometry.init)
    }
    private func syncStepped() {
        stepped = stepped.filter { id, geometry in
            guard let note = snapshot?.pianoNote(id) else { return false }
            return NoteGeometry(note) != geometry
        }
    }
    private func cancelGesture() { gesture = nil; preview = nil; insertion = nil; rulerPress = nil; refresh() }
    /// The remembered start drawn on the ruler. Redraws only when it changes.
    func setStart(_ start: Int64?) { guard self.start != start else { return }; self.start = start; refresh() }
    func setLoop(_ loop: RulerLoop?) {
        guard self.loop != loop else { return }
        rulerPress = nil; self.loop = loop; refresh()
    }
    var rulerHandles: RulerHandles {
        RulerHandles(range: displayedLoop?.range, ruler: pianoLayout.ruler,
                     originBeat: camera.originBeat, pixelsPerBeat: camera.pixelsPerBeat)
    }
    override func mouseCancelled(with event: NSEvent) { cancelGesture() }
    override func resetCursorRects() {
        for handle in rulerHandles.handles { addCursorRect(handle.hitRect, cursor: .resizeLeftRight) }
    }
    var displayedLoop: RulerLoop? {
        if let range = rulerPress?.range { return RulerLoop(range: range, enabled: loop?.enabled ?? false) }
        return loop
    }
    /// Replaces the live feed. Equality is the document, not the playhead.
    func setPlayback(_ playback: PlaybackFeed?) {
        follower.feed = playback
        guard self.playback != playback else { return }; self.playback = playback; refresh()
    }

    /// One frame after play, pause, stop, or a stopped locate. Does not rebuild on position polls.
    func noteTransport(_ generation: UInt64) {
        guard transportGeneration != generation else { return }
        transportGeneration = generation
        follower.noteTransport()
        refresh()
    }

    /// Pages the time origin so a moving playhead stays shown (ADR 0159).
    /// A note or ruler press keeps its view; the first frame after it pages.
    private func followPlayhead(_ tick: Int64) {
        guard gesture == nil, insertion == nil, rulerPress == nil else { return }
        let span = pianoLayout.grid.width / camera.pixelsPerBeat
        guard let origin = PlayheadFollow.origin(showing: tick, from: camera.originBeat, span: span) else { return }
        camera.originBeat = origin
        refresh()
    }
    private func rememberViewport() {
        guard let track, bounds.width > 0, bounds.height > 0, navigation?.permits(document: navigationDocument, track: track) == true else { return }
        navigation?.pianos[track] = PianoViewportMemory(camera: camera, focus: focus,
            geometry: focus.flatMap { noteBasis($0) })
    }
    private func refresh() {
        window?.invalidateCursorRects(for: self)
        let scale = Double(window?.backingScaleFactor ?? 1)
        CanvasSurface.resize(metalLayer, size: canvasDrawableSize(width: bounds.width, height: bounds.height, scale: scale), scale: scale)
        rememberViewport()
        guard let track else { return }
        renderer?.update(CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(),
            width: bounds.width, height: bounds.height, scale: scale,
            piano: PianoRenderInput(track: track, focus: focus, camera: camera, key: key, preview: preview, start: start, loop: displayedLoop, insertion: displayedInsertion), playback: playback,
            displayFramesPerSecond: Double(window?.screen?.maximumFramesPerSecond ?? 60)))
    }

    /// Called for each accepted render frame. Layout changes are announced,
    /// but elements are not built until accessibility asks for them.
    func present(_ frame: PianoFrame) {
        submittedFrame = frame
        elements = frame.notes
        submittedVersion &+= 1
        NSAccessibility.post(element: self, notification: .layoutChanged)
        if focus != announcedFocus {
            if let focus, let note = elements.first(where: { $0.id == focus }) {
                NSAccessibility.post(element: element(for: note, in: frame, convert: screenRect),
                                     notification: .focusedUIElementChanged)
            }
            announcedFocus = focus
        }
    }

    override func accessibilityChildren() -> [Any]? {
        guard window != nil else { return [] }
        return buildAccessibilityChildren(convert: screenRect)
    }

    /// Builds (or reuses) the visible elements. Exposed without a window so the
    /// labels, actions, frames, and build count can be checked.
    func buildAccessibilityChildren(convert: (NSRect) -> NSRect) -> [Any] {
        guard !elements.isEmpty, let submittedFrame else { return [] }
        let children = elements.map { element(for: $0, in: submittedFrame, convert: convert) }
        let visible = Set(elements.map(\.id))
        accessible = accessible.filter { visible.contains($0.key) }
        accessibleVersion = accessibleVersion.filter { visible.contains($0.key) }
        return children
    }

    private func screenRect(_ rect: NSRect) -> NSRect {
        guard let window else { return rect }
        return window.convertToScreen(convert(rect, to: nil))
    }

    private func element(for note: PianoNote, in frame: PianoFrame, convert: (NSRect) -> NSRect) -> PianoAccessibleNote {
        if let existing = accessible[note.id], accessibleVersion[note.id] == submittedVersion {
            // Reuse the element but refresh the frame and focus-dependent state:
            // a window move or selection change does not advance the draw version.
            applyAccessibility(existing, note: note, frame: frame, convert: convert)
            return existing
        }
        let child = buildAccessibilityElement(note, frame: frame, existing: accessible[note.id], convert: convert)
        accessible[note.id] = child
        accessibleVersion[note.id] = submittedVersion
        return child
    }

    private func applyAccessibility(_ child: PianoAccessibleNote, note: PianoNote, frame: PianoFrame,
                                    convert: (NSRect) -> NSRect) {
        let rect = frame.layout.paneRect(NSRect(x: Double(note.x), y: Double(note.y), width: max(2, Double(note.width)), height: Double(note.height)))
        child.setAccessibilityFrame(convert(rect))
        child.setAccessibilityValue(note.id == focus ? text("Selected") : "")
        child.setAccessibilityFocused(note.id == focus)
    }

    func buildAccessibilityElement(_ note: PianoNote, frame: PianoFrame, existing: PianoAccessibleNote? = nil,
                                   convert: (NSRect) -> NSRect) -> PianoAccessibleNote {
        accessibilityBuilds += 1
        let child = existing ?? PianoAccessibleNote(actions: PianoAccessibilityActions())
        child.setAccessibilityElement(true); child.setAccessibilityRole(.button)
        child.setAccessibilityParent(self)
        child.setAccessibilityIdentifier("\(note.id.track)/\(note.id.placement)/\(note.id.note)")
        let degree = frame.piano.key.degreeLabel(note.pitch).map { ", \(text("Scale degree")) \($0)" } ?? ""
        child.setAccessibilityLabel("\(noteName(note.pitch))\(degree), \(text("Start beat")) \(Double(note.start) / 705_600), \(text("Length in beats")) \(Double(note.length) / 705_600)")
        applyAccessibility(child, note: note, frame: frame, convert: convert)
        child.actions.press = { [weak self] in self?.select(note); return true }
        child.actions.increment = { [weak self] in self?.select(note); self?.change(pitch: 1); return true }
        child.actions.decrement = { [weak self] in self?.select(note); self?.change(pitch: -1); return true }
        child.setAccessibilityCustomActions([
            NSAccessibilityCustomAction(name: text("Delete Note")) { [weak self] in guard let self, !self.busy, self.track == note.id.track, self.snapshot?.pianoNote(note.id) != nil else { return false }; self.onDelete?(note.id); return true },
            NSAccessibilityCustomAction(name: text("Move Earlier")) { [weak self] in self?.select(note); if let self { self.change(time: -self.camera.grid) }; return true },
            NSAccessibilityCustomAction(name: text("Move Later")) { [weak self] in self?.select(note); if let self { self.change(time: self.camera.grid) }; return true },
            NSAccessibilityCustomAction(name: text("Shorten Note")) { [weak self] in self?.select(note); if let self { self.change(time: -self.camera.grid, resize: true) }; return true },
            NSAccessibilityCustomAction(name: text("Lengthen Note")) { [weak self] in self?.select(note); if let self { self.change(time: self.camera.grid, resize: true) }; return true },
        ])
        return child
    }

}

@MainActor
final class PianoAccessibilityActions {
    var press: (() -> Bool)?
    var increment: (() -> Bool)?
    var decrement: (() -> Bool)?
}

@MainActor
final class PianoAccessibleNote: NSAccessibilityElement {
    nonisolated let actions: PianoAccessibilityActions
    init(actions: PianoAccessibilityActions) { self.actions = actions; super.init() }
    // Only the immutable, Sendable action holder crosses the Objective-C
    // callback boundary. Its mutable closures are confined to MainActor.
    override func accessibilityPerformPress() -> Bool { Task { @MainActor [actions] in _ = actions.press?() }; return true }
    override func accessibilityPerformIncrement() -> Bool { Task { @MainActor [actions] in _ = actions.increment?() }; return true }
    override func accessibilityPerformDecrement() -> Bool { Task { @MainActor [actions] in _ = actions.decrement?() }; return true }
}
