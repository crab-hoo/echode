import AppKit
import Carbon.HIToolbox
import EchodeBridge

/// Transparent input and accessibility region. Every visible header pixel is
/// drawn by the Metal surface underneath, together with its timeline lane.
@MainActor
final class ArrangeHeaderView: NSView {
    private var snapshot: Snapshot?
    private var selected: UUID?
    private var selectedIndex: Int?
    private(set) var camera = ArrangeCamera()
    private var accessible: [UUID: AccessibleTrackHeader] = [:]
    private var controlElements: [String: AccessibleMixControl] = [:]
    private var fxElements: [UUID: AccessibleTrackHeader] = [:]
    var onSelect: ((UUID) -> Void)?
    var onOpenFX: ((UUID) -> Void)?
    var onDropFX: ((UUID, TrackFXDrop) -> Void)?
    var documentID: UUID?
    var onScroll: ((Double, Double) -> Void)?
    var onZoom: ((Double, NSEvent.ModifierFlags, CGPoint) -> Void)?
    var mixActions: MixActions?
    private var drag: (track: UUID, field: MixerField, rect: CGRect)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        setAccessibilityElement(true)
        setAccessibilityRole(.list)
        setAccessibilityLabel(text("Track"))
        registerForDraggedTypes([FXDrag.deviceType, FXDrag.pluginType])
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    func setSnapshot(_ snapshot: Snapshot, selected: UUID?) {
        guard self.snapshot !== snapshot || self.selected != selected else { return }
        self.snapshot = snapshot
        self.selected = selected
        selectedIndex = (0..<snapshot.trackCount).first { snapshot.trackID(at: $0) == selected }
        NSAccessibility.post(element: self, notification: .selectedRowsChanged)
    }

    func setCamera(_ camera: ArrangeCamera) {
        guard self.camera != camera else { return }
        self.camera = camera
        NSAccessibility.post(element: self, notification: .layoutChanged)
    }

    private var rows: ArrangeRows {
        camera.rows(trackCount: snapshot?.trackCount ?? 0, height: Double(bounds.height))
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let point = convert(event.locationInWindow, from: nil)
        guard let index = rows.index(at: Double(point.y)), let id = snapshot?.trackID(at: index) else { return }
        onSelect?(id)
        let fx = TrackFXLayout(row: rows.rect(at: index, width: Double(bounds.width)))
        if fx.visible, fx.rect.contains(point) { onOpenFX?(id); return }
        let controls = TrackControlLayout(row: rows.rect(at: index, width: Double(bounds.width)))
        guard let field = controls.hit(point), let mix = snapshot?.mixer(onTrack: index), let mixActions else { return }
        if field == .mute || field == .solo {
            mixActions.set(id, field, mix[field].value == 0 ? 1 : 0)
        } else if mixActions.begin(id, field) {
            drag = (id, field, controls.rect(field))
            updateDrag(point)
        }
    }
    override func mouseDragged(with event: NSEvent) { updateDrag(convert(event.locationInWindow, from: nil)) }
    func acceptFX(_ pasteboard: NSPasteboard, at point: CGPoint, copying: Bool) -> Bool {
        guard point.y.isFinite, let payload = FXDrag.read(pasteboard), payload.document == documentID,
              let index = rows.index(at: Double(point.y)), let track = snapshot?.trackID(at: index), let onDropFX else { return false }
        if let plugin = payload.plugin { onDropFX(track, .plugin(plugin)); return true }
        guard let sourceTrack = payload.track, let source = payload.device,
              snapshot?.devices(on: sourceTrack).contains(where: { $0.id == source }) == true,
              let operation = payload.operation(on: track, copying: copying, before: nil) else { return false }
        switch operation {
        case .copy(let source, _, _): onDropFX(track, .copyDevice(source))
        case .reorder(let source, _): onDropFX(track, .moveToEnd(source))
        case .remove, .bypass: return false
        }
        return true
    }
    override func draggingEntered(_ sender: any NSDraggingInfo) -> NSDragOperation { fxDragOperation(sender) }
    override func draggingUpdated(_ sender: any NSDraggingInfo) -> NSDragOperation { fxDragOperation(sender) }
    private func fxDragOperation(_ sender: any NSDraggingInfo) -> NSDragOperation {
        let point = convert(sender.draggingLocation, from: nil)
        guard let payload = FXDrag.read(sender.draggingPasteboard), payload.document == documentID,
              let index = rows.index(at: Double(point.y)), let track = snapshot?.trackID(at: index) else { return [] }
        return payload.plugin != nil || payload.track != track || NSEvent.modifierFlags.contains(.command) ? .copy : .move
    }
    override func performDragOperation(_ sender: any NSDraggingInfo) -> Bool {
        acceptFX(sender.draggingPasteboard, at: convert(sender.draggingLocation, from: nil), copying: NSEvent.modifierFlags.contains(.command))
    }
    override func mouseUp(with event: NSEvent) {
        guard drag != nil else { return }
        updateDrag(convert(event.locationInWindow, from: nil)); drag = nil; mixActions?.finish(true)
    }
    override func mouseCancelled(with event: NSEvent) {
        guard drag != nil else { return }
        drag = nil; mixActions?.finish(false)
    }
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window == nil, drag != nil { drag = nil; mixActions?.finish(false) }
    }
    private func updateDrag(_ point: CGPoint) {
        guard let drag else { return }
        let position = min(1, max(0, (point.x - drag.rect.minX) / drag.rect.width))
        mixActions?.change(drag.field == .gain ? MixScale.gain(position) : Float(position * 2 - 1))
    }
    override func scrollWheel(with event: NSEvent) {
        let delta = WheelScroll.points(x: Double(event.scrollingDeltaX), y: Double(event.scrollingDeltaY),
            precise: event.hasPreciseScrollingDeltas, lineWidth: camera.pixelsPerBeat, lineHeight: camera.pixelsPerTrack)
        onScroll?(delta.x, delta.y)
    }
    override func magnify(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        // Headers have no time coordinate. Time zoom anchors at the grid edge.
        onZoom?(max(0.1, 1 + Double(event.magnification)), event.modifierFlags, CGPoint(x: 0, y: point.y))
    }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == kVK_Escape, drag != nil { drag = nil; mixActions?.finish(false); return }
        guard !event.modifierFlags.contains(.command) else { super.keyDown(with: event); return }
        switch Int(event.keyCode) {
        case kVK_UpArrow, kVK_DownArrow, kVK_Home, kVK_End: interpretKeyEvents([event])
        default: super.keyDown(with: event)
        }
    }
    override func moveUp(_ sender: Any?) { select(index: max(0, (selectedIndex ?? 1) - 1)) }
    override func moveDown(_ sender: Any?) { select(index: (selectedIndex ?? -1) + 1) }
    override func moveToBeginningOfDocument(_ sender: Any?) { select(index: 0) }
    override func moveToEndOfDocument(_ sender: Any?) { select(index: (snapshot?.trackCount ?? 0) - 1) }

    private func select(index: Int) {
        if let id = snapshot?.trackID(at: index) { onSelect?(id) }
    }

    override func accessibilityChildren() -> [Any]? {
        guard let snapshot, let window else { accessible = [:]; return [] }
        var retained: [UUID: AccessibleTrackHeader] = [:]
        var retainedControls: [String: AccessibleMixControl] = [:]
        var retainedFX: [UUID: AccessibleTrackHeader] = [:]
        let children: [AccessibleTrackHeader] = rows.visible.compactMap { index in
            guard let id = snapshot.trackID(at: index), let name = snapshot.trackName(at: index) else { return nil }
            let child = accessible[id] ?? AccessibleTrackHeader(actions: TrackHeaderActions())
            child.setAccessibilityElement(true)
            child.setAccessibilityRole(.row)
            child.setAccessibilityLabel(name)
            child.setAccessibilitySelected(id == selected)
            child.setAccessibilityIdentifier(id.uuidString)
            child.setAccessibilityParent(self)
            let rect = rows.rect(at: index, width: Double(bounds.width)).intersection(bounds)
            child.setAccessibilityFrame(window.convertToScreen(convert(rect, to: nil)))
            child.actions.press = { [weak self] in self?.onSelect?(id) }
            let layout = TrackControlLayout(row: rows.rect(at: index, width: Double(bounds.width)))
            if layout.visible, let mix = snapshot.mixer(onTrack: index) {
                let controls = MixerField.allCases.map { field in
                    let key = "\(id)/\(field.rawValue)"
                    let control = controlElements[key] ?? AccessibleMixControl(actions: MixAccessibilityActions())
                    let title: String
                    switch field { case .gain: title = "Volume"; case .pan: title = "Pan"; case .mute: title = "Mute"; case .solo: title = "Solo" }
                    control.setAccessibilityElement(true)
                    control.setAccessibilityRole(field == .gain || field == .pan ? .slider : .checkBox)
                    control.setAccessibilityLabel(text(title) + " " + name)
                    let shown = control.actions.step.adopt(mix[field].value)
                    control.setAccessibilityValue(NSNumber(value: shown))
                    if let spoken = MixScale.spokenValue(field, shown) {
                        control.setAccessibilityValueDescription(spoken)
                    }
                    control.setAccessibilityMinValue(NSNumber(value: field == .pan ? -1 : 0))
                    control.setAccessibilityMaxValue(NSNumber(value: field == .gain ? 4 : 1))
                    control.setAccessibilityParent(child)
                    control.setAccessibilityIdentifier(key)
                    control.setAccessibilityFrame(window.convertToScreen(convert(layout.rect(field).intersection(bounds), to: nil)))
                    control.actions.adjust = { [weak self, weak control] direction in
                        guard let self, let control, let next = control.actions.step.advance(field, direction: direction) else { return }
                        control.setAccessibilityValue(NSNumber(value: next))
                        if let spoken = MixScale.spokenValue(field, next) {
                            control.setAccessibilityValueDescription(spoken)
                        }
                        self.mixActions?.set(id, field, next)
                    }
                    retainedControls[key] = control
                    return control
                }
                child.setAccessibilityChildren(controls)
            } else { child.setAccessibilityChildren([]) }
            let fx = TrackFXLayout(row: rows.rect(at: index, width: Double(bounds.width)))
            if fx.visible, fx.rect.intersects(bounds) {
                let control = fxElements[id] ?? AccessibleTrackHeader(actions: TrackHeaderActions())
                control.setAccessibilityElement(true); control.setAccessibilityRole(.button)
                control.setAccessibilityLabel(text("Add FX") + " " + name)
                control.setAccessibilityIdentifier("\(id)/fx")
                control.setAccessibilityParent(child)
                control.setAccessibilityFrame(window.convertToScreen(convert(fx.rect.intersection(bounds), to: nil)))
                control.actions.press = { [weak self] in self?.onSelect?(id); self?.onOpenFX?(id) }
                child.setAccessibilityChildren((child.accessibilityChildren() ?? []) + [control])
                retainedFX[id] = control
            }
            retained[id] = child
            return child
        }
        accessible = retained
        controlElements = retainedControls
        fxElements = retainedFX
        return children
    }
}

@MainActor
private final class MixAccessibilityActions {
    var adjust: ((Double) -> Void)?
    var step = MixStep()
}

@MainActor
private final class AccessibleMixControl: NSAccessibilityElement {
    nonisolated let actions: MixAccessibilityActions
    init(actions: MixAccessibilityActions) { self.actions = actions; super.init() }
    override func accessibilityPerformIncrement() -> Bool { Task { @MainActor [actions] in actions.adjust?(1) }; return true }
    override func accessibilityPerformDecrement() -> Bool { Task { @MainActor [actions] in actions.adjust?(-1) }; return true }
    override func accessibilityPerformPress() -> Bool { Task { @MainActor [actions] in actions.adjust?(1) }; return true }
}

@MainActor
private final class TrackHeaderActions {
    var press: (() -> Void)?
}

@MainActor
private final class AccessibleTrackHeader: NSAccessibilityElement {
    nonisolated let actions: TrackHeaderActions
    init(actions: TrackHeaderActions) { self.actions = actions; super.init() }
    override func accessibilityPerformPress() -> Bool {
        Task { @MainActor [actions] in actions.press?() }
        return true
    }
}
