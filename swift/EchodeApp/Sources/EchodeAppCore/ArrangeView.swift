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

    public init(snapshot: Snapshot, selection: Binding<UUID?>, playback: PlaybackFeed? = nil, transportGeneration: UInt64 = 0, navigation: CanvasNavigationState? = nil, documentID: UUID? = nil, selectedClip: UUID? = nil, faderPreview: FaderPreview? = nil, mixActions: MixActions? = nil, onSelectClip: ((UUID?) -> Void)? = nil, onClipEdit: ((ClipMixEdit) -> Void)? = nil, onOpenFX: ((UUID) -> Void)? = nil, onDropFX: ((UUID, TrackFXDrop) -> Void)? = nil, selectedTracks: Set<UUID>? = nil, onSelectTrack: ((UUID, TrackSelectionChange) -> Void)? = nil,
                collapsedTracks: Set<UUID> = [], onFolderDisclosure: ((UUID, Bool) -> Void)? = nil, onDuplicateTracks: (() -> Void)? = nil, onOpenPlugin: ((UUID, UUID) -> Void)? = nil, pluginNames: [String: String] = [:], onDropAudio: ((AudioFileDrop) -> Bool)? = nil) {
        self.snapshot = snapshot
        _selection = selection
        self.playback = playback
        self.transportGeneration = transportGeneration
        self.navigation = navigation
        self.documentID = documentID
        self.selectedClip = selectedClip; self.faderPreview = faderPreview; self.mixActions = mixActions; self.onSelectClip = onSelectClip; self.onClipEdit = onClipEdit
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
        view.onOpenFX = onOpenFX
        view.onOpenPlugin = onOpenPlugin
        view.pluginNames = pluginNames
        view.onDropFX = onDropFX
        view.onFolderDisclosure = onFolderDisclosure
        view.onDuplicateTracks = onDuplicateTracks
        view.timeline.audioDocument = documentID
        view.timeline.onDropAudio = onDropAudio
        view.headers.documentID = documentID
        view.setControlSelection(clip: selectedClip, preview: faderPreview)
        view.setSnapshot(snapshot, selected: selection, selectedTracks: selectedTracks, collapsedTracks: collapsedTracks)
        view.setPlayback(playback)
        view.noteTransport(transportGeneration)
    }

    public static func dismantleNSView(_ view: ArrangePaneView, coordinator: ()) { view.stop() }
}

@MainActor
public final class ArrangeView: NSView {
    private let metalLayer = CAMetalLayer()
    private var renderer: CanvasRenderer?
    private var feedback = FrameFeedbackGate()
    private var snapshot: Snapshot?
    private var selected: UUID?
    private var elements: [ArrangeElement] = []
    // AppKit requires the view to retain virtual elements while they are shown.
    private var accessibleElements: [ArrangeElement.ID: AccessibleArrangeElement] = [:]
    private(set) var camera = ArrangeCamera()
    private var header: ArrangeHeaderInput?
    private var presentedTimelineOrigin = 0.0
    private var presentedCamera = ArrangeCamera()
    private var clipDraft: ClipDraft?
    var onClipEdit: ((ClipMixEdit) -> Void)?
    private var playback: PlaybackFeed?
    private var transportGeneration: UInt64 = 0
    private var selectedClip: UUID?
    private var faderPreview: FaderPreview?
    var onSelectClip: ((UUID?) -> Void)?
    var onSelect: ((UUID) -> Void)?
    var onScroll: ((Double, Double) -> Void)?
    var onZoom: ((Double, NSEvent.ModifierFlags, CGPoint) -> Void)?
    var audioDocument: UUID? { didSet { if oldValue != audioDocument { audioDragDocument = nil } } }
    var onDropAudio: ((AudioFileDrop) -> Bool)?
    private var audioDragDocument: UUID?

    public override init(frame: NSRect) {
        super.init(frame: frame)
        clipsToBounds = true
        registerForDraggedTypes([.fileURL])
        CanvasSurface.configure(self, layer: metalLayer)
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel(text("Arrange"))
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
        self.snapshot = snapshot
        clipDraft = nil
        self.selected = selected
        refresh()
    }
    func present(_ frame: ArrangeFrame) {
        guard snapshot === frame.input.snapshot else { return }
        elements = frame.elements; presentedTimelineOrigin = frame.timelineOrigin; presentedCamera = frame.camera
    }
    func setControlSelection(clip: UUID?, preview: FaderPreview?) {
        guard selectedClip != clip || faderPreview != preview else { return }
        selectedClip = clip; faderPreview = preview; refresh()
    }

    /// Replaces the live feed. Equality is the document, not the playhead.
    public func setPlayback(_ playback: PlaybackFeed?) { guard self.playback != playback else { return }; self.playback = playback; refresh() }

    /// One frame after play, pause, stop, or a stopped locate. Does not rebuild on position polls.
    public func noteTransport(_ generation: UInt64) {
        guard transportGeneration != generation else { return }
        transportGeneration = generation
        refresh()
    }

    func setViewport(_ camera: ArrangeCamera, header: ArrangeHeaderInput?) {
        guard self.camera != camera || self.header != header else { return }
        self.camera = camera
        self.header = header
        refresh()
    }

    public override func setFrameSize(_ newSize: NSSize) { super.setFrameSize(newSize); refresh() }
    public override func setBoundsSize(_ newSize: NSSize) { super.setBoundsSize(newSize); refresh() }
    public override func layout() { super.layout(); refresh() }
    public override func viewDidChangeBackingProperties() { super.viewDidChangeBackingProperties(); refresh() }

    public func stop() { feedback.invalidate(); renderer?.stop(); renderer = nil; elements = []; accessibleElements = [:]; clipDraft = nil; audioDragDocument = nil }

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
        let delta = WheelScroll.points(x: Double(event.scrollingDeltaX), y: Double(event.scrollingDeltaY),
            precise: event.hasPreciseScrollingDeltas, lineWidth: camera.pixelsPerBeat, lineHeight: camera.pixelsPerTrack)
        if let onScroll { onScroll(delta.x, delta.y); return }
        camera.scroll(x: delta.x, y: delta.y, trackCount: snapshot?.trackCount ?? 0, height: Double(bounds.height))
        refresh()
    }

    public override func magnify(with event: NSEvent) {
        let factor = max(0.1, 1 + Double(event.magnification))
        var anchor = convert(event.locationInWindow, from: nil)
        anchor.x -= header.map { $0.width + 1 } ?? 0
        anchor.x = max(0, anchor.x)
        if let onZoom { onZoom(factor, event.modifierFlags, anchor); return }
        camera.zoom(factor: factor, horizontal: !event.modifierFlags.contains(.option) || event.modifierFlags.contains(.shift),
                    vertical: event.modifierFlags.contains(.option) || event.modifierFlags.contains(.shift),
                    anchor: anchor, trackCount: snapshot?.trackCount ?? 0, height: Double(bounds.height))
        refresh()
    }

    public override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let point = convert(event.locationInWindow, from: nil)
        // Track selection uses current shared geometry, even before the next
        // GPU frame replaces the previous frame's accessible elements.
        if let snapshot,
           let index = camera.rows(trackCount: snapshot.trackCount, height: Double(bounds.height)).index(at: Double(point.y)),
           let id = snapshot.trackID(at: index) {
            onSelect?(id)
            let clip = elements.last { $0.role == .clip && $0.id.track == id &&
                CGRect(x: presentedTimelineOrigin + Double($0.x), y: Double($0.y), width: Double($0.width), height: Double($0.height)).contains(point) }
            onSelectClip?(clip?.id.placement)
            if let element = clip, let id = element.id.placement, selectedClip == id,
               let mix = snapshot.clipMix(id), mix.isAudio {
                let layout = ClipControlLayout.make(mix, camera: presentedCamera, origin: presentedTimelineOrigin,
                    y: Double(element.y), height: Double(element.height))
                if let handle = layout.hit(point) {
                    clipDraft = ClipDraft(original: mix, gain: mix.gain, fadeIn: mix.fadeIn, fadeOut: mix.fadeOut,
                        fadeInTicks: mix.fadeInTicks, fadeOutTicks: mix.fadeOutTicks, handle: handle, down: point, layout: layout)
                }
            }
        }
    }
    public override func mouseDragged(with event: NSEvent) {
        guard var draft = clipDraft, let snapshot else { return }
        let point = convert(event.locationInWindow, from: nil)
        let unchanged = draft.handle == .gain ? point.y == draft.down.y : point.x == draft.down.x
        if unchanged {
            draft.gain = draft.original.gain
            draft.fadeIn = draft.original.fadeIn; draft.fadeOut = draft.original.fadeOut
            draft.fadeInTicks = draft.original.fadeInTicks; draft.fadeOutTicks = draft.original.fadeOutTicks
            clipDraft = draft
            refresh(); return
        }
        switch draft.handle {
        case .gain:
            let movement = Double(point.y - draft.down.y)
            let initial = draft.original.gain > 0 ? MixScale.decibels(draft.original.gain) : -60
            let db = initial - movement * 0.25
            draft.gain = db <= min(-60, initial) ? 0 : min(MixScale.clipGainMax,
                Float(pow(10, min(max(24, initial), db) / 20)))
        case .fadeIn, .fadeOut:
            let fraction = min(1, max(0, (point.x - draft.layout.rect.minX) / max(1, draft.layout.rect.width)))
            let offset = fraction >= 1 ? draft.original.length : Int64((Double(draft.original.length) * fraction).rounded(.down))
            let tick = draft.original.start + offset
            guard let at = snapshot.flicks(at: tick), let first = snapshot.flicks(at: draft.original.start),
                  let last = snapshot.flicks(at: draft.original.start + draft.original.length) else { return }
            if draft.handle == .fadeIn { draft.fadeIn = max(0, at - first); draft.fadeInTicks = offset }
            else { draft.fadeOut = max(0, last - at); draft.fadeOutTicks = draft.original.length - offset }
        }
        clipDraft = draft; refresh()
    }
    public override func mouseUp(with event: NSEvent) {
        guard clipDraft != nil else { return }
        mouseDragged(with: event)
        guard let draft = clipDraft else { return }
        clipDraft = nil; refresh()
        switch draft.handle {
        case .gain: onClipEdit?(.gain(draft.original.id, draft.gain))
        case .fadeIn, .fadeOut: onClipEdit?(.fades(draft.original.id, draft.fadeIn, draft.fadeOut))
        }
    }
    public override func mouseCancelled(with event: NSEvent) { clipDraft = nil; refresh() }
    public override func keyDown(with event: NSEvent) {
        if event.keyCode == 53, clipDraft != nil { clipDraft = nil; refresh(); return }
        super.keyDown(with: event)
    }

    public override func accessibilityChildren() -> [Any]? {
        guard let window else { accessibleElements = [:]; return [] }
        var visible: [ArrangeElement.ID: AccessibleArrangeElement] = [:]
        let children = elements.map { element in
            let local = NSRect(x: presentedTimelineOrigin + Double(element.x), y: CGFloat(element.y),
                               width: CGFloat(element.width), height: CGFloat(element.height))
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

    var renderInput: CanvasInput {
        let scale = window?.backingScaleFactor ?? 1
        return CanvasInput(snapshot: snapshot, selected: selected, camera: camera,
                            width: Double(bounds.width), height: Double(bounds.height), scale: Double(scale), playback: playback, header: header,
                            selectedClip: selectedClip, faderPreview: faderPreview, clipDraft: clipDraft,
                            displayFramesPerSecond: Double(window?.screen?.maximumFramesPerSecond ?? 60))
    }
    private func refresh() {
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
