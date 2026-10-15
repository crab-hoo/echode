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
    private var selected: UUID?
    private var camera = ArrangeCamera()
    private var headerWidth = 160.0
    private var navigation: CanvasNavigationState?
    private var navigationDocument: UUID?
    private var restoring = false
    var viewport: ArrangeCamera { camera }
    var onSelect: ((UUID) -> Void)?
    public override var isFlipped: Bool { true }

    public override init(frame: NSRect) {
        super.init(frame: frame)
        addSubview(timeline)
        addSubview(headers)
        addSubview(divider)
        divider.onDrag = { [weak self] in self?.resizeHeader(to: $0) }
        headers.onSelect = { [weak self] in self?.select($0) }
        timeline.onSelect = { [weak self] in self?.select($0) }
        headers.onScroll = { [weak self] in self?.scroll(x: $0, y: $1) }
        timeline.onScroll = { [weak self] in self?.scroll(x: $0, y: $1) }
        headers.onZoom = { [weak self] in self?.zoom(factor: $0, flags: $1, anchor: $2) }
        timeline.onZoom = { [weak self] in self?.zoom(factor: $0, flags: $1, anchor: $2) }
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    public override func layout() {
        super.layout()
        headerWidth = constrainedHeaderWidth(headerWidth)
        timeline.frame = bounds
        headers.frame = NSRect(x: 0, y: 0, width: headerWidth, height: bounds.height)
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

    public func setSnapshot(_ snapshot: Snapshot, selected: UUID?) {
        guard self.snapshot !== snapshot || self.selected != selected else { return }
        let changedSelection = self.selected != selected
        self.snapshot = snapshot
        self.selected = selected
        if changedSelection, !restoring, let selected,
           let index = (0..<snapshot.trackCount).first(where: { snapshot.trackID(at: $0) == selected }) {
            camera.reveal(track: index, trackCount: snapshot.trackCount, height: Double(bounds.height))
        }
        restoring = false
        headers.setSnapshot(snapshot, selected: selected)
        timeline.setSnapshot(snapshot, selected: selected)
        updateViewport()
    }
    public func setPlayback(_ playback: PlaybackFeed?) { timeline.setPlayback(playback) }
    public func noteTransport(_ generation: UInt64) { timeline.noteTransport(generation) }
    public func stop() { timeline.stop() }

    private func select(_ id: UUID) {
        guard let snapshot,
              (0..<snapshot.trackCount).contains(where: { snapshot.trackID(at: $0) == id }) else { return }
        setSnapshot(snapshot, selected: id)
        onSelect?(id)
    }
    private func scroll(x: Double, y: Double) {
        camera.scroll(x: x, y: y, trackCount: snapshot?.trackCount ?? 0, height: Double(bounds.height))
        updateViewport()
    }
    private func zoom(factor: Double, flags: NSEvent.ModifierFlags, anchor: CGPoint) {
        camera.zoom(factor: factor, horizontal: !flags.contains(.option) || flags.contains(.shift),
                    vertical: flags.contains(.option) || flags.contains(.shift),
                    anchor: anchor, trackCount: snapshot?.trackCount ?? 0, height: Double(bounds.height))
        updateViewport()
    }
    private func updateViewport() {
        camera.constrain(trackCount: snapshot?.trackCount ?? 0, height: Double(bounds.height))
        headers.setCamera(camera)
        var palette = ArrangeHeaderPalette()
        effectiveAppearance.performAsCurrentDrawingAppearance {
            palette.background = rgba(.controlBackgroundColor)
            palette.selected = rgba(.selectedContentBackgroundColor)
            palette.foreground = rgba(.labelColor)
            palette.selectedForeground = rgba(.alternateSelectedControlTextColor)
            palette.separator = rgba(.separatorColor)
        }
        timeline.setViewport(camera, header: ArrangeHeaderInput(width: headerWidth, palette: palette))
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
