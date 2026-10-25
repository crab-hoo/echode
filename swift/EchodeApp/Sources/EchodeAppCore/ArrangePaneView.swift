import AppKit
import EchodeBridge

/// One viewport and one Metal surface. The native children are transparent
/// input/accessibility regions; they never draw scrolling track content.
@MainActor
public final class ArrangePaneView: NSView {
    private let divider = ArrangeDividerView(frame: .zero)
    let headers = ArrangeHeaderView(frame: .zero)
    let timeline = ArrangeView(frame: .zero)
    private var snapshot: Snapshot?
    private var displaySnapshot: Snapshot?
    private var collapsedTracks: Set<UUID> = []
    private var selected: UUID?
    private var selectedTracks: Set<UUID> = []
    private var camera = ArrangeCamera()
    // Preserve the requested width; narrow layouts only constrain its display.
    private var headerWidth = 160.0
    private var navigation: CanvasNavigationState?
    private var navigationDocument: UUID?
    private var restoring = false
    let follower = PlayheadFollower()
    /// A time scroll in the headers or the timeline turns following off (ADR 0159).
    var onManualScroll: (() -> Void)?
    var viewport: ArrangeCamera { camera }
    // Selection can arrive before the timeline receives its first child layout.
    private var trackHeight: Double { max(0, Double(bounds.height) - ArrangeRulerInput.height) }
    var onSelect: ((UUID) -> Void)?
    var onSelectTrack: ((UUID, TrackSelectionChange) -> Void)? { didSet { headers.onSelectTrack = onSelectTrack } }
    var onSelectClip: ((UUID?) -> Void)? { didSet { timeline.onSelectClip = onSelectClip } }
    var mixActions: MixActions? { didSet { headers.mixActions = mixActions } }
    var onOpenFX: ((UUID) -> Void)? { didSet { headers.onOpenFX = onOpenFX } }
    var onOpenPlugin: ((UUID, UUID) -> Void)? { didSet { headers.onOpenPlugin = onOpenPlugin } }
    var pluginNames: [String: String] = [:] {
        didSet {
            guard oldValue != pluginNames else { return }
            headers.pluginNames = pluginNames; updateViewport()
        }
    }
    var onDropFX: ((UUID, TrackFXDrop) -> Void)? { didSet { headers.onDropFX = onDropFX } }
    var onDuplicateTracks: (() -> Void)? { didSet { headers.onDuplicateTracks = onDuplicateTracks } }
    var onFolderDisclosure: ((UUID, Bool) -> Void)?
    var onClipEdit: ((ClipMixEdit) -> Void)? { didSet { timeline.onClipEdit = onClipEdit } }
    var onClipMove: ((ClipMove) -> Void)? { didSet { timeline.onClipMove = onClipMove } }
    func setControlSelection(clip: UUID?, preview: FaderPreview?) { timeline.setControlSelection(clip: clip, preview: preview) }
    public override var isFlipped: Bool { true }

    public override init(frame: NSRect) {
        super.init(frame: frame)
        clipsToBounds = true
        timeline.setRuler(start: nil, loop: nil, busy: false)
        addSubview(timeline)
        addSubview(headers)
        addSubview(divider)
        divider.onDrag = { [weak self] in self?.resizeHeader(to: $0) }
        headers.onSelect = { [weak self] in self?.select($0) }
        timeline.onSelect = { [weak self] in self?.select($0) }
        headers.onToggleFolder = { [weak self] in self?.toggleFolder($0) }
        headers.onScroll = { [weak self] in self?.scroll(x: $0, y: $1) }
        timeline.onScroll = { [weak self] in self?.scroll(x: $0, y: $1) }
        headers.onZoom = { [weak self] in self?.zoom(factor: $0, flags: $1, anchor: $2) }
        timeline.onZoom = { [weak self] in self?.zoom(factor: $0, flags: $1, anchor: $2) }
        follower.show = { [weak self] in self?.followPlayhead($0) }
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    public override func layout() {
        super.layout()
        let headerWidth = constrainedHeaderWidth(headerWidth)
        timeline.frame = bounds
        let top = timeline.renderInput.arrangeTop
        let height = trackHeight
        headers.frame = NSRect(x: 0, y: top, width: headerWidth, height: height)
        divider.frame = NSRect(x: max(0, headerWidth - 3), y: 0, width: 7, height: bounds.height)
        divider.position = headerWidth
        updateViewport()
    }

    public override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        updateViewport()
    }

    func setNavigation(_ navigation: CanvasNavigationState?, documentID: UUID? = nil) {
        let document = documentID ?? navigation?.document
        guard self.navigation !== navigation || navigationDocument != document else { return }
        self.navigation = navigation; navigationDocument = document
        if navigation?.permits(document: document) == true, let saved = navigation?.arrange {
            camera = saved; headerWidth = navigation?.headerWidth ?? 160; restoring = true
        }
    }

    public func setSnapshot(_ snapshot: Snapshot, selected: UUID?, selectedTracks: Set<UUID>? = nil, collapsedTracks: Set<UUID>? = nil) {
        let selectedTracks = selectedTracks ?? Set(selected.map { [$0] } ?? [])
        let collapsed = collapsedTracks ?? self.collapsedTracks
        guard self.snapshot !== snapshot || self.selected != selected || self.selectedTracks != selectedTracks || self.collapsedTracks != collapsed else { return }
        let changedSelection = self.selected != selected
        if self.snapshot !== snapshot || self.collapsedTracks != collapsed {
            guard let projected = snapshot.arrangeProjection(collapsedTracks: collapsed) else { return }
            displaySnapshot = projected
        }
        guard let displaySnapshot else { return }
        self.snapshot = snapshot
        self.collapsedTracks = collapsed
        self.selected = selected
        self.selectedTracks = selectedTracks
        if changedSelection, !restoring, let selected,
           let index = (0..<displaySnapshot.trackCount).first(where: { displaySnapshot.trackID(at: $0) == selected }) {
            // Reveal from what is shown. Selecting an already visible track
            // must not clamp a taller pane's remembered origin.
            var display = displayCamera
            let before = display.originTrack
            display.reveal(track: index, trackCount: displaySnapshot.trackCount, height: trackHeight)
            if display.originTrack != before { camera.originTrack = display.originTrack }
        }
        restoring = false
        headers.setSnapshot(displaySnapshot, selected: selected, selectedTracks: selectedTracks, collapsedTracks: collapsed)
        timeline.setSnapshot(displaySnapshot, selected: selected)
        updateViewport()
    }
    public func setPlayback(_ playback: PlaybackFeed?) { timeline.setPlayback(playback); follower.feed = playback }
    public func noteTransport(_ generation: UInt64) {
        if timeline.noteTransport(generation) { follower.noteTransport() }
    }
    func setFollowing(_ following: Bool) { follower.setFollowing(following) }
    public func stop() { follower.detach(); timeline.stop() }
    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil { follower.attach(to: self) } else { follower.detach() }
    }

    /// Pages the requested origin so a moving playhead stays shown (ADR 0159).
    private func followPlayhead(_ tick: Int64) {
        guard !timeline.holdsViewport else { return }
        let span = max(0, Double(bounds.width) - timeline.renderInput.timelineOrigin) / camera.pixelsPerBeat
        guard let origin = PlayheadFollow.origin(showing: tick, from: camera.originBeat, span: span) else { return }
        camera.originBeat = origin
        updateViewport()
    }

    private func toggleFolder(_ id: UUID) {
        guard let snapshot,
              let index = (0..<snapshot.trackCount).first(where: { snapshot.trackID(at: $0) == id }),
              snapshot.trackHierarchy(at: index)?.hasChildren == true else { return }
        var collapsed = collapsedTracks
        if !collapsed.insert(id).inserted { collapsed.remove(id) }
        setSnapshot(snapshot, selected: selected, selectedTracks: selectedTracks, collapsedTracks: collapsed)
        onFolderDisclosure?(id, collapsed.contains(id))
        if let displaySnapshot {
            let visible = Set((0..<displaySnapshot.trackCount).compactMap { displaySnapshot.trackID(at: $0) })
            if !selectedTracks.isSubset(of: visible) {
                setSnapshot(snapshot, selected: id, selectedTracks: [id])
                onSelect?(id)
            }
        }
    }

    private func select(_ id: UUID) {
        guard let snapshot,
              (0..<snapshot.trackCount).contains(where: { snapshot.trackID(at: $0) == id }) else { return }
        setSnapshot(snapshot, selected: id)
        onSelect?(id)
    }
    private var displayCamera: ArrangeCamera {
        var display = camera
        display.constrain(trackCount: displaySnapshot?.trackCount ?? 0, height: trackHeight)
        return display
    }
    private func scroll(x: Double, y: Double) {
        guard x.isFinite, y.isFinite else { return }
        if PlayheadFollow.isTimeScroll(x: x, y: y) {
            // Released here first so the next frame does not page back.
            follower.setFollowing(false)
            onManualScroll?()
        }
        camera.originBeat -= x / camera.pixelsPerBeat
        if y != 0 {
            // Start from the displayed rows so a taller pane does not create a
            // dead zone between the remembered and visible origin.
            var display = displayCamera
            display.originTrack -= y / display.pixelsPerTrack
            display.constrain(trackCount: displaySnapshot?.trackCount ?? 0, height: trackHeight)
            camera.originTrack = display.originTrack
        }
        updateViewport()
    }
    private func zoom(factor: Double, flags: NSEvent.ModifierFlags, anchor: CGPoint) {
        guard factor.isFinite, factor > 0 else { return }
        let horizontal = !flags.contains(.option) || flags.contains(.shift)
        let vertical = flags.contains(.option) || flags.contains(.shift)
        if horizontal {
            let beat = camera.originBeat + Double(anchor.x) / camera.pixelsPerBeat
            camera.pixelsPerBeat = min(1200, max(8, camera.pixelsPerBeat * factor))
            camera.originBeat = beat - Double(anchor.x) / camera.pixelsPerBeat
        }
        if vertical {
            var display = displayCamera
            let track = display.originTrack + Double(anchor.y) / display.pixelsPerTrack
            camera.pixelsPerTrack = min(240, max(16, display.pixelsPerTrack * factor))
            display.pixelsPerTrack = camera.pixelsPerTrack
            display.originTrack = track - Double(anchor.y) / display.pixelsPerTrack
            display.constrain(trackCount: displaySnapshot?.trackCount ?? 0, height: trackHeight)
            camera.originTrack = display.originTrack
        }
        updateViewport()
    }
    private func updateViewport() {
        // The requested origin survives height changes; only the display copy
        // is clamped. See ADR 0149 (JNK-07).
        camera.sanitize()
        let display = displayCamera
        headers.setCamera(display)
        var palette = ArrangeHeaderPalette()
        effectiveAppearance.performAsCurrentDrawingAppearance {
            palette.background = rgba(.controlBackgroundColor)
            palette.selected = rgba(.selectedContentBackgroundColor)
            palette.foreground = rgba(.labelColor)
            palette.selectedForeground = rgba(.alternateSelectedControlTextColor)
            palette.separator = rgba(.separatorColor)
        }
        timeline.setViewport(display, header: ArrangeHeaderInput(width: constrainedHeaderWidth(headerWidth), palette: palette, selectedTracks: selectedTracks, collapsedTracks: collapsedTracks, pluginNames: pluginNames))
        if bounds.width > 0, bounds.height > 0, navigation?.permits(document: navigationDocument) == true {
            navigation?.arrange = camera; navigation?.headerWidth = headerWidth
        }
    }

    func resizeHeader(to width: Double) {
        headerWidth = constrainedHeaderWidth(width)
        needsLayout = true
        layoutSubtreeIfNeeded()
    }
    private func constrainedHeaderWidth(_ width: Double) -> Double {
        let minimum = min(120, Double(bounds.width) / 2)
        let maximum = max(minimum, min(240, Double(bounds.width) - 250))
        return min(maximum, max(minimum, width))
    }
    private func rgba(_ color: NSColor) -> SIMD4<Float> {
        guard let color = color.usingColorSpace(.deviceRGB) else { return SIMD4(1, 1, 1, 1) }
        return SIMD4(Float(color.redComponent), Float(color.greenComponent), Float(color.blueComponent), Float(color.alphaComponent))
    }
}

@MainActor
private final class ArrangeDividerView: NSView {
    var onDrag: ((Double) -> Void)?
    var position = 160.0
    private var dragOffset = 0.0
    override init(frame: NSRect) {
        super.init(frame: frame)
        setAccessibilityElement(true)
        setAccessibilityRole(.splitter)
        setAccessibilityLabel(text("Track"))
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
    override func accessibilityValue() -> Any? { NSNumber(value: position) }
    override func accessibilityPerformIncrement() -> Bool { onDrag?(position + 10); return onDrag != nil }
    override func accessibilityPerformDecrement() -> Bool { onDrag?(position - 10); return onDrag != nil }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .resizeLeftRight) }
    override func mouseDown(with event: NSEvent) {
        if let parent = superview { dragOffset = Double(parent.convert(event.locationInWindow, from: nil).x) - position }
    }
    override func mouseDragged(with event: NSEvent) {
        if let parent = superview { onDrag?(Double(parent.convert(event.locationInWindow, from: nil).x) - dragOffset) }
    }
}
