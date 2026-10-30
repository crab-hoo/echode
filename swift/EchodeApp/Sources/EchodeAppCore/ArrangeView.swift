import AppKit
import EchodeBridge
import QuartzCore
import SwiftUI

public struct ArrangeCanvas: NSViewRepresentable {
    public let snapshot: Snapshot
    @Binding public var selection: UUID?
    public var playback: PlaybackFeed?
    public var transportGeneration: UInt64 = 0
    public var navigation: CanvasNavigationState?
    public var documentID: UUID?
    public var selectedClip: UUID?
    public var faderPreview: FaderPreview?
    public var mixActions: MixActions?
    public var onSelectClip: ((UUID?) -> Void)?
    public var onClipEdit: ((ClipMixEdit) -> Void)?
    public var onOpenPiano: ((UUID, Int64) -> Void)?
    public var onOpenFX: ((UUID) -> Void)?
    public var onOpenPlugin: ((UUID, UUID) -> Void)?
    public var pluginNames: [String: String] = [:]
    public var onDropFX: ((UUID, TrackFXDrop) -> Void)?
    public var selectedTracks: Set<UUID>?
    public var onSelectTrack: ((UUID, TrackSelectionChange) -> Void)?
    public var collapsedTracks: Set<UUID> = []
    public var onFolderDisclosure: ((UUID, Bool) -> Void)?
    public var onDuplicateTracks: (() -> Void)?
    public var onDropAudio: ((AudioFileDrop) -> Bool)?
    public var onMoveClip: ((ClipMove) -> Void)?
    public var start: Int64?
    public var loop: RulerLoop?
    public var busy: Bool
    public var onSetStart: ((Int64) -> Void)?
    public var onSetLoop: ((LoopRange) -> Void)?
    public var followsPlayhead = false
    public var onManualScroll: (() -> Void)?

    public init(snapshot: Snapshot, selection: Binding<UUID?>, playback: PlaybackFeed? = nil, transportGeneration: UInt64 = 0, navigation: CanvasNavigationState? = nil, documentID: UUID? = nil, selectedClip: UUID? = nil, faderPreview: FaderPreview? = nil, mixActions: MixActions? = nil, onSelectClip: ((UUID?) -> Void)? = nil, onClipEdit: ((ClipMixEdit) -> Void)? = nil, onOpenFX: ((UUID) -> Void)? = nil, onDropFX: ((UUID, TrackFXDrop) -> Void)? = nil, selectedTracks: Set<UUID>? = nil, onSelectTrack: ((UUID, TrackSelectionChange) -> Void)? = nil,
                collapsedTracks: Set<UUID> = [], onFolderDisclosure: ((UUID, Bool) -> Void)? = nil, onDuplicateTracks: (() -> Void)? = nil, onOpenPlugin: ((UUID, UUID) -> Void)? = nil, pluginNames: [String: String] = [:], onDropAudio: ((AudioFileDrop) -> Bool)? = nil, onMoveClip: ((ClipMove) -> Void)? = nil,
                start: Int64? = nil, loop: RulerLoop? = nil, busy: Bool = false,
                onSetStart: ((Int64) -> Void)? = nil, onSetLoop: ((LoopRange) -> Void)? = nil,
                followsPlayhead: Bool = false, onManualScroll: (() -> Void)? = nil, onOpenPiano: ((UUID, Int64) -> Void)? = nil) {
        self.snapshot = snapshot
        _selection = selection
        self.playback = playback
        self.transportGeneration = transportGeneration
        self.navigation = navigation
        self.documentID = documentID
        self.selectedClip = selectedClip; self.faderPreview = faderPreview; self.mixActions = mixActions; self.onSelectClip = onSelectClip; self.onClipEdit = onClipEdit
        self.onOpenPiano = onOpenPiano
        self.onOpenFX = onOpenFX
        self.onDropFX = onDropFX
        self.selectedTracks = selectedTracks
        self.onSelectTrack = onSelectTrack
        self.collapsedTracks = collapsedTracks
        self.onFolderDisclosure = onFolderDisclosure
        self.onDuplicateTracks = onDuplicateTracks
        self.onOpenPlugin = onOpenPlugin
        self.pluginNames = pluginNames
        self.onDropAudio = onDropAudio
        self.onMoveClip = onMoveClip
        self.start = start; self.loop = loop; self.busy = busy
        self.onSetStart = onSetStart; self.onSetLoop = onSetLoop
        self.followsPlayhead = followsPlayhead; self.onManualScroll = onManualScroll
    }

    public func makeNSView(context: Context) -> ArrangePaneView {
        ArrangePaneView(frame: .zero)
    }

    public func updateNSView(_ view: ArrangePaneView, context: Context) {
        view.setNavigation(navigation, documentID: documentID)
        view.onSelect = { selection = $0 }
        view.onSelectTrack = onSelectTrack
        view.onSelectClip = onSelectClip
        view.mixActions = mixActions
        view.onClipEdit = onClipEdit
        view.timeline.onOpenPiano = onOpenPiano
        view.onOpenFX = onOpenFX
        view.onOpenPlugin = onOpenPlugin
        view.pluginNames = pluginNames
        view.onDropFX = onDropFX
        view.onFolderDisclosure = onFolderDisclosure
        view.onDuplicateTracks = onDuplicateTracks
        view.timeline.audioDocument = documentID
        view.timeline.onDropAudio = onDropAudio
        view.onClipMove = onMoveClip
        view.timeline.setRuler(start: start, loop: loop, busy: busy)
        view.timeline.onSetStart = onSetStart
        view.timeline.onSetLoop = onSetLoop
        view.headers.documentID = documentID
        view.setControlSelection(clip: selectedClip, preview: faderPreview)
        view.setSnapshot(snapshot, selected: selection, selectedTracks: selectedTracks, collapsedTracks: collapsedTracks)
        view.setPlayback(playback)
        view.onManualScroll = onManualScroll
        view.setFollowing(followsPlayhead)
        view.noteTransport(transportGeneration)
    }

    public static func dismantleNSView(_ view: ArrangePaneView, coordinator: ()) { view.stop() }
}

@MainActor
public final class ArrangeView: NSView {
    private let metalLayer = CAMetalLayer()
    private var renderer: CanvasRenderer?
    private var feedback = FrameFeedbackGate()
    private(set) var snapshot: Snapshot?
    private var selected: UUID?
    private var elements: [ArrangeElement] = []
    // AppKit requires the view to retain virtual elements while they are shown.
    private var accessibleElements: [ArrangeElement.ID: AccessibleArrangeElement] = [:]
    private(set) var camera = ArrangeCamera()
    private var header: ArrangeHeaderInput?
    private var presentedTimelineOrigin = 0.0
    private var presentedCamera = ArrangeCamera()
    private var presentedSnapshot: Snapshot?
    private var clipDraft: ClipDraft?
    /// Clip gain and fade clicks, drags and typing (ADR 0153).
    let clicks = ValueClicks<ClipTarget>()
    /// The handle and edit destination captured on the press, before later view updates.
    private struct ClipPress {
        let target: ClipTarget
        let mix: ClipMix
        let layout: ClipControlLayout
        let document: UUID?
        let edit: ((ClipMixEdit) -> Void)?
    }
    private var press: ClipPress?
    private var gestureEdit: ((ClipMixEdit) -> Void)?
    private(set) var entry: ValueEntryField?
    var onOpenPiano: ((UUID, Int64) -> Void)?
    var onClipEdit: ((ClipMixEdit) -> Void)?
    private var playback: PlaybackFeed?
    private var transportGeneration: UInt64 = 0
    private var selectedClip: UUID?
    private var faderPreview: FaderPreview?
    var onSelectClip: ((UUID?) -> Void)?
    var onSelect: ((UUID) -> Void)?
    var onScroll: ((Double, Double) -> Void)?
    var onZoom: ((Double, NSEvent.ModifierFlags, CGPoint) -> Void)?
    var audioDocument: UUID? {
        didSet {
            guard oldValue != audioDocument else { return }
            audioDragDocument = nil
            cancelRuler()
            cancelClipControls()
        }
    }
    var onDropAudio: ((AudioFileDrop) -> Bool)?
    private var audioDragDocument: UUID?
    /// A press on a clip body and, once it travels, where the clip would land (ADR 0154).
    var movePress: ClipMovePress?
    var onClipMove: ((ClipMove) -> Void)?
    var ruler: ArrangeRulerInput?
    var rulerBusy = false
    var rulerPress: RulerPress?
    var onSetStart: ((Int64) -> Void)?
    var onSetLoop: ((LoopRange) -> Void)?
    private var presentedTimelineTop = 0.0

    public override init(frame: NSRect) {
        super.init(frame: frame)
        clipsToBounds = true
        registerForDraggedTypes([.fileURL])
        CanvasSurface.configure(self, layer: metalLayer)
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel(text("Arrange"))
        clicks.begin = { [weak self] target in
            guard let self, let press = self.press, press.target == target, press.document == self.audioDocument else { return false }
            self.gestureEdit = press.edit
            let mix = press.mix
            self.clipDraft = ClipDraft(original: mix, gain: mix.gain, fadeIn: mix.fadeIn, fadeOut: mix.fadeOut,
                fadeInTicks: mix.fadeInTicks, fadeOutTicks: mix.fadeOutTicks, handle: target.handle, layout: press.layout)
            self.refresh()
            return true
        }
        clicks.drag = { [weak self] _, offset in self?.dragClip(by: offset) }
        clicks.reset = { [weak self] _ in
            guard let self, var draft = self.clipDraft else { return }
            switch draft.handle {
            case .gain: draft.gain = 1
            case .fadeIn: draft.fadeIn = 0; draft.fadeInTicks = 0
            case .fadeOut: draft.fadeOut = 0; draft.fadeOutTicks = 0
            }
            self.clipDraft = draft; self.refresh()
        }
        clicks.finish = { [weak self] commit in self?.finishClip(commit: commit) }
        clicks.type = { [weak self] target in self?.openEntry(target) }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
    public override var isFlipped: Bool { true }
    public override var acceptsFirstResponder: Bool { true }
    public override func makeBackingLayer() -> CALayer { metalLayer }

    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard window != nil else { stop(); return }
        if renderer == nil {
            do {
                let generation = feedback.restart()
                renderer = try CanvasRenderer(layer: metalLayer,
                    onElements: { _ in }, onArrange: { [weak self] frame in
                        guard let self, self.snapshot === frame.input.snapshot, self.feedback.accept(generation, revision: frame.revision) else { return }
                        self.present(frame)
                        NSAccessibility.post(element: self, notification: .layoutChanged)
                    })
            } catch {
                NSLog("Echode arrange renderer: %@", String(describing: error))
                let label = NSTextField(labelWithString: text("Metal is unavailable"))
                label.frame = bounds
                label.autoresizingMask = [.width, .height]
                addSubview(label)
            }
        }
        refresh()
    }

    public func setSnapshot(_ snapshot: Snapshot, selected: UUID?) {
        guard self.snapshot !== snapshot || self.selected != selected else { return }
        cancelRuler()
        self.snapshot = snapshot
        clipDraft = nil
        movePress = nil
        self.selected = selected
        refresh()
    }
    func present(_ frame: ArrangeFrame) {
        guard snapshot === frame.input.snapshot else { return }
        presentedSnapshot = frame.input.snapshot
        elements = frame.elements; presentedTimelineOrigin = frame.timelineOrigin; presentedCamera = frame.camera
        presentedTimelineTop = frame.input.arrangeTop
    }
    func setControlSelection(clip: UUID?, preview: FaderPreview?) {
        guard selectedClip != clip || faderPreview != preview else { return }
        selectedClip = clip; faderPreview = preview; refresh()
    }

    /// Replaces the live feed. Equality is the document, not the playhead.
    public func setPlayback(_ playback: PlaybackFeed?) { guard self.playback != playback else { return }; self.playback = playback; refresh() }

    /// One frame after play, pause, stop, or a stopped locate. Does not rebuild on position polls.
    /// Returns whether the generation was new.
    @discardableResult
    public func noteTransport(_ generation: UInt64) -> Bool {
        guard transportGeneration != generation else { return false }
        transportGeneration = generation
        refresh()
        return true
    }

    func setViewport(_ camera: ArrangeCamera, header: ArrangeHeaderInput?) {
        guard self.camera != camera || self.header != header else { return }
        cancelRuler()
        self.camera = camera
        self.header = header
        refresh()
    }

    public override func setFrameSize(_ newSize: NSSize) {
        if bounds.size != newSize { cancelRuler() }
        super.setFrameSize(newSize); refresh()
    }
    public override func setBoundsSize(_ newSize: NSSize) {
        if bounds.size != newSize { cancelRuler() }
        super.setBoundsSize(newSize); refresh()
    }
    public override func resignFirstResponder() -> Bool { cancelRuler(); return super.resignFirstResponder() }
    public override func layout() { super.layout(); refresh() }
    public override func viewDidChangeBackingProperties() { super.viewDidChangeBackingProperties(); refresh() }

    public func stop() {
        cancelRuler()
        cancelClipControls()
        feedback.invalidate(); renderer?.stop(); renderer = nil; elements = []; accessibleElements = [:]; clipDraft = nil; audioDragDocument = nil; movePress = nil
    }

    // Explicit, hardware-only probe; the app never enables continuous redraw
    // or blocks MainActor during normal editing.
    public func beginMeasurement() { renderer?.beginMeasurement() }
    public func markMainBusy(_ busy: Bool) { renderer?.markMainBusy(busy) }
    public var measurement: ArrangeMeasurement { renderer?.measurement ?? ArrangeMeasurement() }
    public var rendererAvailable: Bool { renderer != nil }

    public override func draggingEntered(_ sender: any NSDraggingInfo) -> NSDragOperation {
        audioDragDocument = audioDocument
        return draggingUpdated(sender)
    }
    public override func draggingUpdated(_ sender: any NSDraggingInfo) -> NSDragOperation {
        guard sender.draggingSourceOperationMask.contains(.copy), let document = audioDragDocument,
              audioDrop(sender.draggingPasteboard, at: convert(sender.draggingLocation, from: nil),
                        flags: NSEvent.modifierFlags, document: document) != nil else { return [] }
        return .copy
    }
    public override func draggingExited(_ sender: (any NSDraggingInfo)?) { audioDragDocument = nil }
    public override func prepareForDragOperation(_ sender: any NSDraggingInfo) -> Bool { draggingUpdated(sender) == .copy }
    public override func performDragOperation(_ sender: any NSDraggingInfo) -> Bool {
        defer { audioDragDocument = nil }
        guard sender.draggingSourceOperationMask.contains(.copy), let document = audioDragDocument else { return false }
        return acceptAudio(sender.draggingPasteboard, at: convert(sender.draggingLocation, from: nil),
                           flags: NSEvent.modifierFlags, document: document)
    }

    public override func scrollWheel(with event: NSEvent) {
        cancelRuler()
        let delta = WheelScroll.points(x: Double(event.scrollingDeltaX), y: Double(event.scrollingDeltaY),
            precise: event.hasPreciseScrollingDeltas, lineWidth: camera.pixelsPerBeat, lineHeight: camera.pixelsPerTrack)
        if let onScroll { onScroll(delta.x, delta.y); return }
        camera.scroll(x: delta.x, y: delta.y, trackCount: snapshot?.trackCount ?? 0, height: renderInput.arrangeHeight)
        refresh()
    }

    public override func magnify(with event: NSEvent) {
        cancelRuler()
        let factor = max(0.1, 1 + Double(event.magnification))
        var anchor = convert(event.locationInWindow, from: nil)
        anchor.x -= header.map { $0.width + 1 } ?? 0
        anchor.x = max(0, anchor.x)
        anchor.y = max(0, anchor.y - renderInput.arrangeTop)
        if let onZoom { onZoom(factor, event.modifierFlags, anchor); return }
        camera.zoom(factor: factor, horizontal: !event.modifierFlags.contains(.option) || event.modifierFlags.contains(.shift),
                    vertical: event.modifierFlags.contains(.option) || event.modifierFlags.contains(.shift),
                    anchor: anchor, trackCount: snapshot?.trackCount ?? 0, height: renderInput.arrangeHeight)
        refresh()
    }

    public override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let point = convert(event.locationInWindow, from: nil)
        if pressRuler(at: point) { clicks.settle(); return }
        // A shown default is recorded before this press can change the selection.
        if event.clickCount < 3 { clicks.settle() }
        // Track selection uses current shared geometry, even before the next
        // GPU frame replaces the previous frame's accessible elements.
        if let snapshot,
           let index = camera.rows(trackCount: snapshot.trackCount, height: renderInput.arrangeHeight).index(at: Double(point.y) - renderInput.arrangeTop),
           let id = snapshot.trackID(at: index) {
            onSelect?(id)
            let clip = elements.last { $0.role == .clip && $0.id.track == id &&
                CGRect(x: presentedTimelineOrigin + Double($0.x), y: presentedTimelineTop + Double($0.y), width: Double($0.width), height: Double($0.height)).contains(point) }
            onSelectClip?(clip?.id.placement)
            let isAudio = clip?.id.placement.flatMap { snapshot.clipMix($0)?.isAudio } ?? false
            if event.clickCount == 2, !rulerBusy, !isAudio, let onOpenPiano,
               presentedSnapshot === snapshot, presentedCamera == camera,
               event.modifierFlags.isDisjoint(with: [.command, .option, .control]),
               let tick = Tick.ticks(camera.originBeat + (point.x - renderInput.timelineOrigin) / camera.pixelsPerBeat), tick >= 0 {
                finishClipMove(at: nil, flags: []); onOpenPiano(id, tick); return
            }
            if let element = clip, let id = element.id.placement, selectedClip == id,
               let mix = snapshot.clipMix(id), mix.isAudio {
                let layout = ClipControlLayout.make(mix, camera: presentedCamera, origin: presentedTimelineOrigin,
                    y: presentedTimelineTop + Double(element.y), height: Double(element.height))
                if let handle = layout.hit(point) {
                    let target = ClipTarget(clip: id, handle: handle)
                    if case .waiting(target) = clicks.phase {} else { press = ClipPress(target: target, mix: mix, layout: layout, document: audioDocument, edit: onClipEdit) }
                    clicks.down(target, at: point, clicks: event.clickCount)
                    return
                }
            }
            // The clip body moves the clip; its handles keep their own gestures.
            if onClipMove != nil, let id = clip?.id.placement, let mix = snapshot.clipMix(id) {
                clicks.settle()
                pressClip(mix, at: point)
                return
            }
        }
        clicks.settle()
    }
    public override func mouseDragged(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if rulerPress != nil { dragRuler(to: point, flags: event.modifierFlags); return }
        if movePress != nil { dragClip(to: point, flags: event.modifierFlags); return }
        clicks.dragged(to: point)
    }
    public override func mouseUp(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if rulerPress != nil { finishRuler(at: point, flags: event.modifierFlags); return }
        if movePress != nil { finishClipMove(at: point, flags: event.modifierFlags); return }
        clicks.up(at: point)
    }
    public override func mouseCancelled(with event: NSEvent) { cancelRuler(); finishClipMove(at: nil, flags: []); clicks.cancel() }
    public override func resetCursorRects() {
        for handle in rulerHandles.handles { addCursorRect(handle.hitRect, cursor: .resizeLeftRight) }
    }
    public override func performKeyEquivalent(with event: NSEvent) -> Bool {
        // A local arrange shortcut must never capture a text field, piano editor or another pane's keys.
        if window?.firstResponder === self, duplicateClip(with: event) { return true }
        return super.performKeyEquivalent(with: event)
    }

    private func duplicateClip(with event: NSEvent) -> Bool {
        guard event.modifierFlags.intersection([.command, .option, .control, .shift]) == .command,
              event.charactersIgnoringModifiers?.lowercased() == "d",
              window == nil || window?.firstResponder === self else { return false }
        // Holding the shortcut must not keep replacing the previous copy before a new frame arrives.
        if event.isARepeat { return true }
        guard !rulerBusy, rulerPress == nil, movePress == nil, !clicks.active, entry == nil,
              let document = audioDocument, let selectedClip, let clip = snapshot?.clipMix(selectedClip),
              clip.track == selected, let start = ClipMoveTarget.adjacentStart(start: clip.start, length: clip.length),
              let onClipMove else { return false }
        onClipMove(ClipMove(document: document, clip: clip.id, track: clip.track, start: start, copy: true))
        return true
    }

    public override func keyDown(with event: NSEvent) {
        if event.keyCode == 53, rulerPress != nil { cancelRuler(); return }
        if event.keyCode == 53, movePress != nil { finishClipMove(at: nil, flags: []); return }
        if event.keyCode == 53, clicks.active { clicks.cancel(); return }
        if duplicateClip(with: event) { return }
        clicks.settle()
        super.keyDown(with: event)
    }

    /// Gain follows the vertical travel; a fade handle follows the horizontal
    /// travel from where it was, so a press without movement keeps the value.
    private func dragClip(by offset: CGSize) {
        guard var draft = clipDraft, let snapshot else { return }
        let original = draft.original
        if draft.handle == .gain ? offset.height == 0 : offset.width == 0 {
            draft.gain = original.gain
            draft.fadeIn = original.fadeIn; draft.fadeOut = original.fadeOut
            draft.fadeInTicks = original.fadeInTicks; draft.fadeOutTicks = original.fadeOutTicks
            clipDraft = draft
            refresh(); return
        }
        switch draft.handle {
        case .gain:
            let initial = original.gain > 0 ? MixScale.decibels(original.gain) : -60
            let db = initial - Double(offset.height) * 0.25
            draft.gain = db <= min(-60, initial) ? 0 : min(MixScale.clipGainMax,
                Float(pow(10, min(max(24, initial), db) / 20)))
        case .fadeIn, .fadeOut:
            let rect = draft.layout.rect
            let handle = draft.handle == .fadeIn ? rect.minX + draft.layout.fadeIn : rect.maxX - draft.layout.fadeOut
            let fraction = min(1, max(0, (handle + offset.width - rect.minX) / max(1, rect.width)))
            let ticks = fraction >= 1 ? original.length : Int64((Double(original.length) * fraction).rounded(.down))
            guard let at = snapshot.flicks(at: original.start + ticks), let first = snapshot.flicks(at: original.start),
                  let last = snapshot.flicks(at: original.start + original.length) else { return }
            if draft.handle == .fadeIn { draft.fadeIn = max(0, at - first); draft.fadeInTicks = ticks }
            else { draft.fadeOut = max(0, last - at); draft.fadeOutTicks = original.length - ticks }
        }
        clipDraft = draft; refresh()
    }

    private func cancelClipControls() {
        clicks.cancel()
        press = nil
        gestureEdit = nil
        clipDraft = nil
        entry?.cancel()
    }

    /// A commit sends the shown value to the press's destination; cancellation only drops the draft.
    private func finishClip(commit: Bool) {
        let edit = gestureEdit
        gestureEdit = nil
        guard let draft = clipDraft else { return }
        clipDraft = nil; refresh()
        guard commit else { return }
        switch draft.handle {
        case .gain: edit?(.gain(draft.original.id, draft.gain))
        case .fadeIn, .fadeOut: edit?(.fades(draft.original.id, draft.fadeIn, draft.fadeOut))
        }
    }

    /// The third click's field, in the inspector's units: dB for gain and
    /// milliseconds for a fade.
    private func openEntry(_ target: ClipTarget) {
        guard let press, press.target == target, press.document == audioDocument, let mix = snapshot?.clipMix(target.clip) else { return }
        entry?.cancel()
        let rect: CGRect
        let title: String
        switch target.handle {
        case .gain: rect = press.layout.gain; title = text("Clip Gain")
        case .fadeIn: rect = press.layout.entering; title = text("Fade In")
        case .fadeOut: rect = press.layout.leaving; title = text("Fade Out")
        }
        entry = ValueEntryField.open(in: self, over: rect, text: ClipEntry.text(target.handle, mix), label: title,
            commit: { [weak self] typed in
                guard let self, self.audioDocument == press.document,
                      let edit = ClipEntry.edit(target.handle, mix, typed) else { return false }
                if ClipEntry.changes(edit, mix) { press.edit?(edit) }
                return true
            }, onClose: { [weak self] in self?.entry = nil })
    }

    /// Local VoiceOver geometry from the last presented frame, not the live camera.
    func accessibilityRect(for element: ArrangeElement) -> CGRect {
        CGRect(x: presentedTimelineOrigin + Double(element.x), y: presentedTimelineTop + Double(element.y),
               width: Double(element.width), height: Double(element.height))
            .intersection(CGRect(x: presentedTimelineOrigin, y: presentedTimelineTop,
                width: max(0, bounds.width - presentedTimelineOrigin), height: max(0, bounds.height - presentedTimelineTop)))
    }

    public override func accessibilityChildren() -> [Any]? {
        guard let window else { accessibleElements = [:]; return [] }
        var visible: [ArrangeElement.ID: AccessibleArrangeElement] = [:]
        let children = elements.compactMap { element -> AccessibleArrangeElement? in
            let local = accessibilityRect(for: element)
            guard !local.isEmpty, !local.isNull else { return nil }
            let screen = window.convertToScreen(convert(local, to: nil))
            let label: String
            switch element.role {
            case .track: label = text("Track")
            case .clip: label = text("Clip")
            case .note: label = text("Note")
            }
            let child = accessibleElements[element.id] ?? AccessibleArrangeElement(actions: ArrangeAccessibilityActions())
            child.setAccessibilityElement(true)
            child.setAccessibilityRole(element.role == .clip ? .button : .staticText)
            child.setAccessibilitySelected(element.role == .clip && element.id.placement == selectedClip)
            child.setAccessibilityFrame(screen)
            child.setAccessibilityLabel(label)
            child.setAccessibilityParent(self)
            child.setAccessibilityIdentifier("\(element.id.track)/\(element.id.placement?.uuidString ?? "")/\(element.id.note?.uuidString ?? "")")
            child.actions.press = element.role == .clip ? { [weak self] in
                self?.onSelect?(element.id.track)
                self?.onSelectClip?(element.id.placement)
            } : nil
            visible[element.id] = child
            return child
        }
        accessibleElements = visible
        return children
    }

    /// A press, drag, typed value or file drop placed against the current view.
    /// Following the playhead waits until it ends (ADR 0159).
    var holdsViewport: Bool { rulerPress != nil || movePress != nil || clicks.active || entry != nil || audioDragDocument != nil }

    var renderInput: CanvasInput {
        let scale = window?.backingScaleFactor ?? 1
        return CanvasInput(snapshot: snapshot, selected: selected, camera: camera,
                            width: Double(bounds.width), height: Double(bounds.height), scale: Double(scale), playback: playback, header: header,
                            selectedClip: selectedClip, faderPreview: faderPreview, clipDraft: clipDraft, clipMove: movePress?.preview,
                            arrangeRuler: ruler.map { ArrangeRulerInput(start: $0.start, loop: displayedLoop) },
                            displayFramesPerSecond: Double(window?.screen?.maximumFramesPerSecond ?? 60))
    }
    func refresh() {
        window?.invalidateCursorRects(for: self)
        let input = renderInput
        CanvasSurface.resize(metalLayer, size: input.drawableSize, scale: input.scale)
        renderer?.update(input)
    }
}

@MainActor
private final class ArrangeAccessibilityActions { var press: (() -> Void)? }

@MainActor
private final class AccessibleArrangeElement: NSAccessibilityElement {
    nonisolated let actions: ArrangeAccessibilityActions
    init(actions: ArrangeAccessibilityActions) { self.actions = actions; super.init() }
    override func accessibilityPerformPress() -> Bool {
        Task { @MainActor [actions] in actions.press?() }
        return true
    }
}
