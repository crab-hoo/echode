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
    var inputLength: () -> Int64? = { nil }
    var velocity: (PianoNote.ID, Int) -> Void = { _, _ in }
    var velocities: ((PianoVelocityChange) -> Void)? = nil
    var moves: ((PianoMoveChange) -> Void)? = nil
    var resizes: ((PianoResizeChange) -> Void)? = nil
    var steps: ((PianoStep) -> Void)? = nil
    var references: [UUID] = []
    var location: PianoLocation? = nil
    var selection: Set<PianoNote.ID>? = nil
    var selectNotes: ((Set<PianoNote.ID>) -> Void)? = nil
    var deleteNotes: ((Set<PianoNote.ID>) -> Void)? = nil

    func makeNSView(context: Context) -> PianoRollView { PianoRollView(frame: .zero) }
    func updateNSView(_ view: PianoRollView, context: Context) {
        view.grid.setNavigation(navigation, documentID: documentID)
        view.grid.onSelect = { focus = $0 }
        view.grid.onSelection = selectNotes
        view.grid.onDeleteNotes = deleteNotes
        view.grid.onInsert = insert
        view.grid.inputLength = inputLength
        view.grid.onEdit = edit
        view.grid.onVelocity = velocity
        view.grid.onVelocities = velocities
        view.grid.onMoves = moves
        view.grid.onResizes = resizes
        view.grid.onSteps = steps
        view.grid.onDelete = delete
        view.grid.onSetStart = setStart
        view.grid.setStart(start)
        view.grid.onSetLoop = setLoop
        view.grid.setLoop(loop)
        view.grid.setReferences(references)
        view.grid.setSnapshot(snapshot, track: track, focus: focus, key: key, busy: busy, selection: selection)
        view.grid.setLocation(location)
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
    private(set) var selectedNotes: Set<PianoNote.ID> = []
    private(set) var selectionPress: PianoSelectionPress?
    var displayedSelection: Set<PianoNote.ID> {
        guard let selectionPress, selectionPress.dragged else { return selectedNotes }
        return selectionPress.notes(in: elements, layout: pianoLayout)
    }
    private var announcedFocus: PianoNote.ID?
    private var revealOnLayout = false
    private var busy = false
    private var elements: [PianoNote] = []
    private var references: [UUID] = []
    private var shownLocation: UUID?
    private var accessible: [PianoNote.ID: PianoAccessibleNote] = [:]
    private let velocityAccessibility = PianoVelocityAccessibility()
    private var accessibleVersion: [PianoNote.ID: UInt64] = [:]
    private var submittedVersion: UInt64 = 0
    /// Elements are built only for accessibility queries and focus changes.
    private(set) var accessibilityBuilds = 0
    private var gesture: PianoGesture?
    private var preview: PianoPreview?
    private var velocityGesture: PianoVelocityGesture?
    private(set) var velocityPreview: PianoVelocityPreview?
    private var moveGesture: PianoMoveGesture?
    private(set) var movePreview: PianoMovePreview?
    private var resizeGesture: PianoResizeGesture?
    private(set) var resizePreview: PianoResizePreview?
    private var insertion: PianoInsertion?
    var displayedInsertion: NoteGeometry? { insertion?.geometry }
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
    var onVelocity: ((PianoNote.ID, Int) -> Void)?
    var onVelocities: ((PianoVelocityChange) -> Void)?
    var onMoves: ((PianoMoveChange) -> Void)?
    var onResizes: ((PianoResizeChange) -> Void)?
    var onSteps: ((PianoStep) -> Void)?
    var onDelete: ((PianoNote.ID) -> Void)?
    var onSelection: ((Set<PianoNote.ID>) -> Void)?
    var onDeleteNotes: ((Set<PianoNote.ID>) -> Void)?
    var onSetStart: ((Int64) -> Void)?
    var onSetLoop: ((LoopRange) -> Void)?
    let follower = PlayheadFollower()
    /// A time scroll turns following off (ADR 0159).
    var onManualScroll: (() -> Void)?
    /// Read at press time, so selection and pending edits cannot leave a stale copy.
    var inputLength: () -> Int64? = { nil }

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
                        guard let self, self.snapshot === frame.input.snapshot, self.track == frame.piano.track, self.references == frame.piano.references,
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
    func stop() { velocityAccessibility.clear(); follower.detach(); feedback.invalidate(); renderer?.stop(); renderer = nil; submittedFrame = nil; elements = []; accessible = [:]; accessibleVersion = [:]; announcedFocus = nil; accessibilityBuilds = 0; cancelGesture() }
    func beginMeasurement() { renderer?.beginMeasurement() }
    func markMainBusy(_ busy: Bool) { renderer?.markMainBusy(busy) }
    var measurement: ArrangeMeasurement { renderer?.measurement ?? ArrangeMeasurement() }
    var rendererAvailable: Bool { renderer != nil }
    func setNavigation(_ navigation: CanvasNavigationState?, documentID: UUID? = nil) {
        let document = documentID ?? navigation?.document
        if self.navigation !== navigation || navigationDocument != document {
            // Never write the departing document's viewport into its replacement.
            cancelGesture(); selectedNotes = []; focus = nil; track = nil; camera = PianoCamera()
        }
        self.navigation = navigation; navigationDocument = document
    }
    func setSnapshot(_ snapshot: Snapshot, track: UUID, focus: PianoNote.ID?, key: DisplayKey, busy: Bool, selection: Set<PianoNote.ID>? = nil) {
        let trackChanged = self.track != track
        let oldGeometry = self.focus.flatMap { self.snapshot?.pianoNote($0).map(NoteGeometry.init) }
        let newGeometry = focus.flatMap { snapshot.pianoNote($0).map(NoteGeometry.init) }
        let needsReveal = trackChanged || (self.snapshot !== snapshot && oldGeometry != newGeometry) ||
            (self.focus != focus && !elements.contains(where: { $0.id == focus }))
        if trackChanged { rememberViewport(); stepped = [:] }
        let canRestore = navigation?.permits(document: navigationDocument, track: track) == true
        let sharedCamera = canRestore ? navigation?.pianoCamera : nil
        let restoredFocus = trackChanged && sharedCamera != nil
        if trackChanged { camera = sharedCamera ?? PianoCamera(); caretTick = 0; caretPitch = 60 }
        let changed = self.snapshot !== snapshot || self.track != track
        if changed || busy { selectionPress = nil; velocityGesture = nil; velocityPreview = nil; gesture = nil; preview = nil; insertion = nil; rulerPress = nil; revealOnLayout = false }
        if changed || busy { cancelNoteTransforms() }
        if self.track != track { accessible = [:]; velocityAccessibility.clear() }
        if changed { elements = []; submittedFrame = nil }
        let selected = (selection ?? Set(focus.map { [$0] } ?? [])).filter { $0.track == track && snapshot.pianoNote($0) != nil }
        if selectedNotes != selected { velocityGesture = nil; velocityPreview = nil }
        if selectedNotes != selected { cancelNoteTransforms() }
        let needsRefresh = changed || selectedNotes != selected || self.focus != focus || self.key != key || self.busy != busy
        self.snapshot = snapshot; self.track = track; self.key = key; self.busy = busy
        selectedNotes = selected
        self.focus = selected.count == 1 ? selected.first : nil
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
        if bounds.size != newSize { selectionPress = nil; rulerPress = nil; insertion = nil; velocityGesture = nil; velocityPreview = nil }
        if bounds.size != newSize { cancelNoteTransforms() }
        super.setFrameSize(newSize); refresh()
    }
    override func setBoundsSize(_ newSize: NSSize) {
        if bounds.size != newSize { selectionPress = nil; rulerPress = nil; insertion = nil; velocityGesture = nil; velocityPreview = nil }
        if bounds.size != newSize { cancelNoteTransforms() }
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
              submittedFrame.piano.track == track, submittedFrame.piano.references == references, submittedFrame.piano.preview == nil, submittedFrame.piano.insertion == nil, submittedFrame.piano.velocity == nil, submittedFrame.piano.move == nil, submittedFrame.piano.resize == nil else { return }
        let panePoint = convert(event.locationInWindow, from: nil)
        if pianoLayout.ruler.contains(panePoint) {
            // Defer the click until release so a loop drag never seeks.
            cancelGesture()
            rulerPress = RulerPress(x: pianoLayout.gridPoint(panePoint).x, originBeat: camera.originBeat,
                                    pixelsPerBeat: camera.pixelsPerBeat, grid: camera.grid,
                                    edge: rulerHandles.hit(at: panePoint), originalRange: loop?.range)
            return
        }
        if pianoLayout.velocity.contains(panePoint) {
            cancelGesture()
            if let hit = submittedFrame.velocityLane.hit(at: panePoint, bars: submittedFrame.velocityBars, focus: focus) {
                if event.modifierFlags.contains(.command) { toggleSelection(hit); return }
                beginVelocity(hit, y: pianoLayout.gridPoint(panePoint).y, pointsPerStep: submittedFrame.velocityLane.span / 127)
            }
            return
        }
        guard pianoLayout.grid.contains(panePoint) else { return }
        stepped = [:]
        let point = pianoLayout.gridPoint(panePoint)
        caretTick = max(0, camera.tick(at: point.x) ?? 0); caretPitch = camera.pitch(at: point.y)
        if let hit = elements.last(where: { NSRect(x: Double($0.x), y: Double($0.y), width: max(2, Double($0.width)), height: Double($0.height)).contains(point) }) {
            if event.modifierFlags.contains(.command) {
                cancelGesture()
                let ids = selectedNotes.contains(hit.id) ? selectedNotes : [hit.id]
                var originals: [PianoNote.ID: NoteGeometry] = [:]
                for id in ids {
                    guard let note = snapshot?.pianoNote(id) else { return }
                    originals[id] = NoteGeometry(note)
                }
                let copies = Dictionary(uniqueKeysWithValues: Set(ids.map(\.note)).map { ($0, UUID()) })
                moveGesture = PianoMoveGesture(anchor: hit.id, originals: originals, point: point, camera: camera, copies: copies)
                return
            }
            if event.modifierFlags.contains(.option), !event.modifierFlags.contains(.command) {
                gesture = nil; preview = nil
                beginVelocity(hit, y: point.y)
                return
            }
            let start = (Double(hit.start) / 705_600 - camera.originBeat) * camera.pixelsPerBeat
            let end = start + Double(hit.length) / 705_600 * camera.pixelsPerBeat
            let mode = PianoGesture.mode(at: point.x, start: start, end: end)
            let groupedMove = mode == .move && selectedNotes.count > 1 && selectedNotes.contains(hit.id)
            if groupedMove {
                var originals: [PianoNote.ID: NoteGeometry] = [:]
                for id in selectedNotes {
                    guard let note = snapshot?.pianoNote(id) else { return }
                    originals[id] = NoteGeometry(note)
                }
                moveGesture = PianoMoveGesture(anchor: hit.id, originals: originals, point: point, camera: camera)
                return
            }
            let groupedResize = mode != .move && selectedNotes.count > 1 && selectedNotes.contains(hit.id)
            if groupedResize {
                var originals: [PianoNote.ID: NoteGeometry] = [:]
                for id in selectedNotes {
                    guard let note = snapshot?.pianoNote(id) else { return }
                    originals[id] = NoteGeometry(note)
                }
                resizeGesture = PianoResizeGesture(anchor: hit.id, originals: originals, edge: mode, point: point, camera: camera)
                return
            }
            select(hit)
            gesture = PianoGesture(id: hit.id, original: NoteGeometry(hit), mode: mode, x: point.x, y: point.y)
        } else {
            // The first release already inserts. A double click must not add
            // another note while the new model frame is still in flight.
            guard event.clickCount == 1 else { return }
            guard !event.modifierFlags.contains(.command) else { return }
            replaceSelection([]); cancelGesture()
            insertion = PianoInsertion(x: point.x, y: point.y, camera: camera,
                                       snapping: !event.modifierFlags.contains(.shift), length: inputLength())
            refresh()
        }
    }
    override func mouseDragged(with event: NSEvent) {
        if let moveGesture {
            guard !busy else { cancelGesture(); return }
            let point = pianoLayout.gridPoint(convert(event.locationInWindow, from: nil))
            if movePreview != nil || hypot(point.x - moveGesture.point.x, point.y - moveGesture.point.y) >= 3 {
                movePreview = moveGesture.preview(at: point, snapping: !event.modifierFlags.contains(.shift))
            }
            refresh(); return
        }
        if let resizeGesture {
            guard !busy else { cancelGesture(); return }
            let point = pianoLayout.gridPoint(convert(event.locationInWindow, from: nil))
            if resizePreview != nil || abs(point.x - resizeGesture.point.x) >= 3 {
                resizePreview = resizeGesture.preview(at: point, snapping: !event.modifierFlags.contains(.shift))
            }
            refresh(); return
        }
        if let velocityGesture {
            guard !busy else { cancelGesture(); return }
            velocityPreview = velocityGesture.preview(y: pianoLayout.gridPoint(convert(event.locationInWindow, from: nil)).y)
            refresh(); return
        }
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
        if let moveGesture {
            let point = pianoLayout.gridPoint(convert(event.locationInWindow, from: nil))
            let finished = movePreview == nil ? nil : moveGesture.preview(at: point, snapping: !event.modifierFlags.contains(.shift))
            let click = movePreview == nil && !moveGesture.copies.isEmpty ? snapshot?.pianoNote(moveGesture.anchor) : nil
            cancelGesture()
            if let finished, !busy, !finished.change.copies.isEmpty || finished.change.time != 0 || finished.change.pitch != 0 {
                onMoves?(finished.change)
            } else if let click, !busy { toggleSelection(click) }
            return
        }
        if let resizeGesture {
            let point = pianoLayout.gridPoint(convert(event.locationInWindow, from: nil))
            let finished = resizePreview == nil ? nil : resizeGesture.preview(at: point, snapping: !event.modifierFlags.contains(.shift))
            cancelGesture()
            if let finished, !busy, finished.change.delta != 0 { onResizes?(finished.change) }
            return
        }
        if let velocityGesture {
            let finished = velocityPreview == nil ? nil : velocityGesture.preview(y: pianoLayout.gridPoint(convert(event.locationInWindow, from: nil)).y)
            cancelGesture()
            if let finished { _ = commitVelocity(finished, gesture: velocityGesture) }
            return
        }
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
            if let geometry = insertion.geometry, !busy {
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
    private func beginVelocity(_ note: PianoNote, y: Double, pointsPerStep: Double = 1) {
        let grouped = selectedNotes.count > 1 && selectedNotes.contains(note.id)
        if !grouped { select(note) }
        var originals: [PianoNote.ID: UInt32] = [:]
        if grouped {
            for id in selectedNotes {
                guard let current = snapshot?.pianoNote(id) else { return }
                originals[id] = current.velocity
            }
        }
        velocityGesture = PianoVelocityGesture(id: note.id, original: Int(note.velocity >> 9), y: y,
                                               pointsPerStep: pointsPerStep, originals: originals)
    }
    @discardableResult
    private func commitVelocity(_ finished: PianoVelocityPreview, gesture: PianoVelocityGesture) -> Bool {
        guard !busy, finished.value != gesture.original else { return false }
        if gesture.originals.isEmpty {
            guard let onVelocity else { return false }
            onVelocity(finished.id, finished.value)
        } else {
            guard let onVelocities else { return false }
            onVelocities(PianoVelocityChange(anchor: finished.id, originals: gesture.originals,
                                             delta: finished.value - gesture.original))
        }
        return true
    }
    private func notePreview(with event: NSEvent) -> PianoPreview? {
        guard let gesture else { return nil }
        let point = pianoLayout.gridPoint(convert(event.locationInWindow, from: nil))
        return gesture.preview(x: point.x, y: point.y, camera: camera,
                               snapping: !event.modifierFlags.contains(.shift))
            .map { PianoPreview(id: gesture.id, geometry: $0) }
    }
    override func keyDown(with event: NSEvent) {
        guard !event.modifierFlags.contains(.command) || Int(event.keyCode) == kVK_Escape else { super.keyDown(with: event); return }
        let shift = event.modifierFlags.contains(.shift)
        switch Int(event.keyCode) {
        case kVK_Escape: if moveGesture != nil || resizeGesture != nil || selectionPress != nil || velocityGesture != nil || gesture != nil || preview != nil || insertion != nil || rulerPress != nil { cancelGesture() } else { super.keyDown(with: event) }
        case kVK_Tab: if !navigate(backwards: shift) { super.keyDown(with: event) }
        case kVK_Return, kVK_ANSI_KeypadEnter: super.keyDown(with: event)
        case kVK_Delete, kVK_ForwardDelete: deleteSelection()
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
    private func replaceSelection(_ ids: Set<PianoNote.ID>) {
        if ids != selectedNotes { velocityGesture = nil; velocityPreview = nil }
        if ids != selectedNotes { cancelNoteTransforms() }
        selectedNotes = ids.filter { $0.track == track && snapshot?.pianoNote($0) != nil }
        focus = selectedNotes.count == 1 ? selectedNotes.first : nil
        if let onSelection { onSelection(selectedNotes) } else { onSelect?(focus) }
        refresh()
    }

    private func toggleSelection(_ note: PianoNote) {
        var ids = selectedNotes
        if !ids.insert(note.id).inserted { ids.remove(note.id) }
        replaceSelection(ids)
    }

    private func deleteSelection() {
        guard !busy, selectionPress == nil, !selectedNotes.isEmpty else { return }
        if let onDeleteNotes { onDeleteNotes(selectedNotes) }
        else if selectedNotes.count == 1, let id = selectedNotes.first { onDelete?(id) }
    }

    private var hasCurrentNoteFrame: Bool {
        guard !busy, let frame = submittedFrame else { return false }
        return frame.input.snapshot === snapshot && frame.piano.track == track && frame.piano.references == references && frame.piano.camera == camera &&
            frame.layout == pianoLayout && frame.piano.preview == nil && frame.piano.insertion == nil && frame.piano.velocity == nil && frame.piano.move == nil && frame.piano.resize == nil
    }

    override func rightMouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        cancelGesture()
        let point = convert(event.locationInWindow, from: nil)
        guard hasCurrentNoteFrame, pianoLayout.grid.contains(point) else { return }
        selectionPress = PianoSelectionPress(at: point)
    }

    override func rightMouseDragged(with event: NSEvent) {
        guard var press = selectionPress, !busy else { return }
        press.update(convert(event.locationInWindow, from: nil))
        selectionPress = press; refresh()
    }

    override func rightMouseUp(with event: NSEvent) {
        guard var press = selectionPress, !busy else { return }
        let point = convert(event.locationInWindow, from: nil)
        press.update(point)
        selectionPress = nil
        if press.dragged {
            replaceSelection(press.notes(in: elements, layout: pianoLayout))
        } else if let menu = noteContextMenu(at: point), window != nil {
            NSMenu.popUpContextMenu(menu, with: event, for: self)
        }
        refresh()
    }

    /// Also used by windowless regressions; creating a menu never opens a window.
    func noteContextMenu(at point: CGPoint) -> NSMenu? {
        guard hasCurrentNoteFrame, pianoLayout.grid.contains(point), let snapshot else { return nil }
        let local = pianoLayout.gridPoint(point)
        guard let note = elements.last(where: {
            CGRect(x: Double($0.x), y: Double($0.y), width: max(2, Double($0.width)), height: Double($0.height)).contains(local)
        }) else { replaceSelection([]); return nil }
        if !selectedNotes.contains(note.id) { select(note) }
        let menu = NSMenu()
        menu.autoenablesItems = false
        let item = NSMenuItem(title: text(selectedNotes.count == 1 ? "Delete Note" : "Delete Notes"),
                              action: #selector(deleteFromMenu(_:)), keyEquivalent: "")
        item.target = self
        item.representedObject = PianoNoteDeletion(snapshot: snapshot, ids: selectedNotes)
        menu.addItem(item)
        return menu
    }

    @objc func deleteFromMenu(_ item: NSMenuItem) {
        guard !busy, let target = item.representedObject as? PianoNoteDeletion,
              snapshot === target.snapshot, target.ids.allSatisfy({ $0.track == track && snapshot?.pianoNote($0) != nil }) else { return }
        if let onDeleteNotes { onDeleteNotes(target.ids) }
        else if target.ids.count == 1, let id = target.ids.first { onDelete?(id) }
    }
    private func select(_ note: PianoNote) {
        guard note.id.track == track, snapshot?.pianoNote(note.id) != nil else { return }
        caretTick = max(0, note.start); caretPitch = note.pitch
        replaceSelection([note.id])
    }
    private func insertAtCaret() -> Bool {
        guard !busy, track != nil else { return false }
        onInsert?(caretPitch, Double(caretTick) / 705_600, Double(inputLength() ?? camera.grid) / 705_600)
        return true
    }
    private func change(pitch: Int = 0, time: Int64 = 0, resize: Bool = false) {
        guard !busy, selectionPress == nil, velocityGesture == nil, gesture == nil,
              moveGesture == nil, resizeGesture == nil, insertion == nil, rulerPress == nil else { return }
        if let onSteps {
            guard !selectedNotes.isEmpty else { return }
            onSteps(PianoStep(ids: selectedNotes, time: time, pitch: pitch, resize: resize))
            return
        }
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
    private func cancelNoteTransforms() { moveGesture = nil; movePreview = nil; resizeGesture = nil; resizePreview = nil }
    private func cancelGesture() { selectionPress = nil; velocityGesture = nil; velocityPreview = nil; gesture = nil; preview = nil; insertion = nil; rulerPress = nil; cancelNoteTransforms(); refresh() }
    func setLocation(_ location: PianoLocation?) {
        guard let location, location.target.document == navigationDocument, location.target.track == track,
              location.tick >= 0, shownLocation != location.id, navigation?.pianoLocationID != location.id else { return }
        shownLocation = location.id; navigation?.pianoLocationID = location.id
        cancelGesture()
        camera.originBeat = Double(location.tick) / 705_600
        caretTick = location.tick
        refresh()
    }
    func setReferences(_ tracks: [UUID]) {
        guard references != tracks else { return }
        references = tracks; refresh()
    }
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
        guard moveGesture == nil, resizeGesture == nil, selectionPress == nil, velocityGesture == nil, gesture == nil, insertion == nil, rulerPress == nil else { return }
        let span = pianoLayout.grid.width / camera.pixelsPerBeat
        guard let origin = PlayheadFollow.origin(showing: tick, from: camera.originBeat, span: span) else { return }
        camera.originBeat = origin
        refresh()
    }
    private func rememberViewport() {
        guard let track, bounds.width > 0, bounds.height > 0, navigation?.permits(document: navigationDocument, track: track) == true else { return }
        navigation?.pianoCamera = camera
    }
    private func refresh() {
        window?.invalidateCursorRects(for: self)
        let scale = Double(window?.backingScaleFactor ?? 1)
        CanvasSurface.resize(metalLayer, size: canvasDrawableSize(width: bounds.width, height: bounds.height, scale: scale), scale: scale)
        rememberViewport()
        guard let track else { return }
        renderer?.update(CanvasInput(snapshot: snapshot, selected: track, camera: ArrangeCamera(),
            width: bounds.width, height: bounds.height, scale: scale,
            piano: PianoRenderInput(track: track, focus: focus, camera: camera, key: key, preview: preview, start: start, loop: displayedLoop, insertion: displayedInsertion, velocity: velocityPreview, selection: displayedSelection, marquee: selectionPress?.rect, move: movePreview, resize: resizePreview, references: references), playback: playback,
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
        guard let submittedFrame else { return [] }
        let children = elements.map { element(for: $0, in: submittedFrame, convert: convert) }
        let visible = Set(elements.map(\.id))
        accessible = accessible.filter { visible.contains($0.key) }
        accessibleVersion = accessibleVersion.filter { visible.contains($0.key) }
        let lane = velocityAccessibility.element(frame: submittedFrame, focus: focus, parent: self, convert: convert,
            select: { [weak self] note in
                guard let self, self.snapshot === submittedFrame.input.snapshot else { return }
                self.select(note)
            }, change: { [weak self] id, value in
                guard let self, !self.busy, self.snapshot === submittedFrame.input.snapshot,
                      self.track == id.track, let note = self.snapshot?.pianoNote(id),
                      Int(note.velocity >> 9) != value else { return false }
                self.beginVelocity(note, y: 0)
                guard let gesture = self.velocityGesture else { return false }
                let finished = gesture.preview(y: Double(gesture.original - value))
                self.cancelGesture()
                return finished.map { self.commitVelocity($0, gesture: gesture) } ?? false
            })
        return children + [lane]
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
        child.setAccessibilityValue(selectedNotes.contains(note.id) ? text("Selected") : "")
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
        child.setAccessibilityLabel("\(noteName(note.pitch))\(degree), \(text("Start beat")) \(Double(note.start) / 705_600), \(text("Length in beats")) \(Double(note.length) / 705_600), \(text("Velocity")) \(frame.velocity(for: note))")
        applyAccessibility(child, note: note, frame: frame, convert: convert)
        child.actions.press = { [weak self] in self?.select(note); return true }
        child.actions.increment = { [weak self] in self?.select(note); self?.change(pitch: 1); return true }
        child.actions.decrement = { [weak self] in self?.select(note); self?.change(pitch: -1); return true }
        child.setAccessibilityCustomActions([
            NSAccessibilityCustomAction(name: text("Toggle Note Selection")) { [weak self] in
                guard let self, !self.busy, self.track == note.id.track,
                      self.snapshot === frame.input.snapshot else { return false }
                self.toggleSelection(note); return true
            },
            NSAccessibilityCustomAction(name: text("Delete Note")) { [weak self] in
                guard let self, !self.busy, self.track == note.id.track, self.snapshot?.pianoNote(note.id) != nil else { return false }
                if !self.selectedNotes.contains(note.id) { self.select(note) }
                self.deleteSelection(); return true
            },
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
