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

    public init(snapshot: Snapshot, selection: Binding<UUID?>, playback: PlaybackFeed? = nil, transportGeneration: UInt64 = 0, navigation: CanvasNavigationState? = nil, documentID: UUID? = nil) {
        self.snapshot = snapshot
        _selection = selection
        self.playback = playback
        self.transportGeneration = transportGeneration
        self.navigation = navigation
        self.documentID = documentID
    }

    public func makeNSView(context: Context) -> ArrangePaneView {
        ArrangePaneView(frame: .zero)
    }

    public func updateNSView(_ view: ArrangePaneView, context: Context) {
        view.setNavigation(navigation, documentID: documentID)
        view.onSelect = { selection = $0 }
        view.setSnapshot(snapshot, selected: selection)
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
    private var accessibleElements: [ArrangeElement.ID: NSAccessibilityElement] = [:]
    private(set) var camera = ArrangeCamera()
    private var header: ArrangeHeaderInput?
    private var presentedTimelineOrigin = 0.0
    private var playback: PlaybackFeed?
    private var transportGeneration: UInt64 = 0
    var onSelect: ((UUID) -> Void)?
    var onScroll: ((Double, Double) -> Void)?
    var onZoom: ((Double, NSEvent.ModifierFlags, CGPoint) -> Void)?

    public override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
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
                renderer = try CanvasRenderer(layer: metalLayer, size: bounds.size, scale: window?.backingScaleFactor ?? 1,
                    onElements: { _ in }, onArrange: { [weak self] frame in
                        guard let self, self.snapshot === frame.input.snapshot, self.feedback.accept(generation, revision: frame.revision) else { return }
                        self.elements = frame.elements
                        self.presentedTimelineOrigin = frame.timelineOrigin
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
        self.selected = selected
        refresh()
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

    public override func layout() { super.layout(); refresh() }
    public override func viewDidChangeBackingProperties() { super.viewDidChangeBackingProperties(); refresh() }

    public func stop() { feedback.invalidate(); renderer?.stop(); renderer = nil; elements = []; accessibleElements = [:] }

    // Explicit, hardware-only probe; the app never enables continuous redraw
    // or blocks MainActor during normal editing.
    public func beginMeasurement() { renderer?.beginMeasurement() }
    public func markMainBusy(_ busy: Bool) { renderer?.markMainBusy(busy) }
    public var measurement: ArrangeMeasurement { renderer?.measurement ?? ArrangeMeasurement() }
    public var rendererAvailable: Bool { renderer != nil }

    public override func scrollWheel(with event: NSEvent) {
        let x = Double(event.scrollingDeltaX), y = Double(event.scrollingDeltaY)
        if let onScroll { onScroll(x, y); return }
        camera.scroll(x: x, y: y, trackCount: snapshot?.trackCount ?? 0, height: Double(bounds.height))
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
           let id = snapshot.trackID(at: index) { onSelect?(id) }
    }

    public override func accessibilityChildren() -> [Any]? {
        guard let window else { accessibleElements = [:]; return [] }
        var visible: [ArrangeElement.ID: NSAccessibilityElement] = [:]
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
            let child = accessibleElements[element.id] ?? NSAccessibilityElement()
            child.setAccessibilityElement(true)
            child.setAccessibilityRole(.staticText)
            child.setAccessibilityFrame(screen)
            child.setAccessibilityLabel(label)
            child.setAccessibilityParent(self)
            child.setAccessibilityIdentifier("\(element.id.track)/\(element.id.placement?.uuidString ?? "")/\(element.id.note?.uuidString ?? "")")
            visible[element.id] = child
            return child
        }
        accessibleElements = visible
        return children
    }

    var renderInput: CanvasInput {
        let scale = window?.backingScaleFactor ?? 1
        return CanvasInput(snapshot: snapshot, selected: selected, camera: camera,
                            width: Double(bounds.width), height: Double(bounds.height), scale: Double(scale), playback: playback, header: header)
    }
    private func refresh() { renderer?.update(renderInput) }
}
