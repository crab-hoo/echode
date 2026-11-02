import AppKit
import Carbon.HIToolbox
import EchodeBridge

/// Transparent input and accessibility region. Every visible header pixel is
/// drawn by the Metal surface underneath, together with its timeline lane.
@MainActor
final class ArrangeHeaderView: NSView {
    private var snapshot: Snapshot?
    private var selected: UUID?
    private var selectedTracks: Set<UUID> = []
    private var selectedIndex: Int?
    private(set) var camera = ArrangeCamera()
    private var accessible: [UUID: AccessibleTrackHeader] = [:]
    private var controlElements: [String: AccessibleMixControl] = [:]
    private var fxElements: [UUID: AccessibleTrackHeader] = [:]
    private var deviceElements: [UUID: AccessibleTrackHeader] = [:]
    private var overflowElements: [UUID: AccessibleTrackHeader] = [:]
    private var folderElements: [UUID: AccessibleTrackHeader] = [:]
    private var collapsedTracks: Set<UUID> = []
    var onSelect: ((UUID) -> Void)?
    var onSelectTrack: ((UUID, TrackSelectionChange) -> Void)?
    var onOpenFX: ((UUID) -> Void)?
    var onOpenPlugin: ((UUID, UUID) -> Void)?
    var pluginNames: [String: String] = [:]
    func headerDevices(on track: UUID) -> [TrackDevice] { snapshot?.devices(on: track) ?? [] }
    var onToggleFolder: ((UUID) -> Void)?
    var onDropFX: ((UUID, TrackFXDrop) -> Void)?
    var onDuplicateTracks: (() -> Void)?
    var documentID: DocumentID? {
        didSet {
            guard oldValue != documentID else { return }
            clicks.cancel()
            press = nil
            entry?.cancel()
            controlElements = [:]
        }
    }
    var onScroll: ((Double, Double) -> Void)?
    var onZoom: ((Double, NSEvent.ModifierFlags, CGPoint) -> Void)?
    var mixActions: MixActions?
    /// Fader and pan clicks, drags and typing (ADR 0153).
    let clicks = ValueClicks<MixTarget>()
    /// The control and value under the last press, which a drag moves from.
    private var press: (target: MixTarget, rect: CGRect, original: Float, document: DocumentID?, actions: MixActions)?
    /// A view update must not lend new callbacks to a gesture already in progress.
    private var gestureActions: MixActions?
    private(set) var entry: ValueEntryField?

    override init(frame: NSRect) {
        super.init(frame: frame)
        setAccessibilityElement(true)
        setAccessibilityRole(.list)
        setAccessibilityLabel(text("Track"))
        registerForDraggedTypes([FXDrag.deviceType, FXDrag.pluginType])
        clicks.begin = { [weak self] target in
            guard let self, let press = self.press, press.target == target, press.document == self.documentID,
                  press.actions.begin(target.track, target.field) else { return false }
            self.gestureActions = press.actions
            return true
        }
        clicks.drag = { [weak self] target, offset in
            guard let press = self?.press, press.target == target else { return }
            let fraction = Double(offset.width) / max(1, Double(press.rect.width))
            if let dragged = self?.gestureActions?.dragged { dragged(fraction) }
            else { self?.gestureActions?.change(MixScale.dragged(target.field, from: press.original, by: fraction)) }
        }
        clicks.reset = { [weak self] target in self?.gestureActions?.change(MixScale.defaultValue(target.field)) }
        clicks.finish = { [weak self] commit in
            let actions = self?.gestureActions
            self?.gestureActions = nil
            actions?.finish(commit)
        }
        clicks.type = { [weak self] target in self?.openEntry(target) }
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    func setSnapshot(_ snapshot: Snapshot, selected: UUID?, selectedTracks: Set<UUID>? = nil, collapsedTracks: Set<UUID> = []) {
        let selectedTracks = selectedTracks ?? Set(selected.map { [$0] } ?? [])
        guard self.snapshot !== snapshot || self.selected != selected || self.selectedTracks != selectedTracks || self.collapsedTracks != collapsedTracks else { return }
        self.snapshot = snapshot
        self.selected = selected
        self.selectedTracks = selectedTracks
        self.collapsedTracks = collapsedTracks
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
        guard let index = rows.index(at: Double(point.y)), let id = snapshot?.trackID(at: index) else { clicks.settle(); return }
        if TrackControlLayout(row: rows.rect(at: index, width: Double(bounds.width))).hit(point).map({ $0 == .gain || $0 == .pan }) != true {
            clicks.settle()
        }
        if let tree = snapshot?.trackHierarchy(at: index), tree.hasChildren,
           TrackFolderLayout(row: rows.rect(at: index, width: bounds.width), depth: tree.depth, folder: true).disclosure.contains(point) {
            onToggleFolder?(id); return
        }
        let fx = TrackFXLayout(row: rows.rect(at: index, width: Double(bounds.width)))
        if fx.visible, fx.rect.contains(point) { select(id, change: .focus); onOpenFX?(id); return }
        let devices = headerDevices(on: id)
        let names = TrackFXNamesLayout(row: fx.row, count: devices.count, depth: snapshot?.trackHierarchy(at: index)?.depth ?? 0)
        if !names.name.isEmpty, names.name.contains(point), let device = devices.first {
            select(id, change: .focus); openHeaderDevice(device, track: id); return
        }
        if !names.overflow.isEmpty, names.overflow.contains(point) {
            select(id, change: .focus)
            deviceMenu(Array(devices.dropFirst()), track: id).popUp(positioning: nil, at: point, in: self)
            return
        }
        let controls = TrackControlLayout(row: rows.rect(at: index, width: Double(bounds.width)))
        if controls.hit(point) != nil { select(id, change: .focus) }
        else {
            select(id, change: event.modifierFlags.contains(.command) ? .toggle : event.modifierFlags.contains(.shift) ? .extend : .replace)
        }
        guard let field = controls.hit(point), let mix = snapshot?.mixer(onTrack: index), let mixActions else { clicks.settle(); return }
        if field == .mute || field == .solo {
            clicks.settle()
            if let toggle = mixActions.toggle { toggle(id, field) }
            else { mixActions.set(id, field, mix[field].value == 0 ? 1 : 0) }
        } else {
            let target = MixTarget(track: id, field: field)
            if case .waiting(target) = clicks.phase {} else {
                var captured = mixActions
                mixActions.capture?(&captured, id, field)
                press = (target, controls.rect(field), mix[field].value, documentID, captured)
            }
            clicks.down(target, at: point, clicks: event.clickCount)
        }
    }
    override func mouseDragged(with event: NSEvent) { clicks.dragged(to: convert(event.locationInWindow, from: nil)) }
    /// A right click on a selected row keeps the selection; on another row it selects that row.
    override func menu(for event: NSEvent) -> NSMenu? {
        let point = convert(event.locationInWindow, from: nil)
        clicks.settle()
        guard onDuplicateTracks != nil, let index = rows.index(at: Double(point.y)),
              let id = snapshot?.trackID(at: index) else { return nil }
        select(id, change: .focus)
        let menu = NSMenu()
        let duplicate = NSMenuItem(title: text("Duplicate Track"), action: #selector(duplicateTracks), keyEquivalent: "")
        duplicate.target = self
        menu.addItem(duplicate)
        return menu
    }
    @objc private func duplicateTracks() { onDuplicateTracks?() }
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
        case .remove, .bypass, .offline: return false
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
    override func mouseUp(with event: NSEvent) { clicks.up(at: convert(event.locationInWindow, from: nil)) }
    override func mouseCancelled(with event: NSEvent) { clicks.cancel() }
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window == nil { clicks.cancel(); entry?.cancel() }
    }

    /// The third click's field, in the inspector's units: dB with −∞, and percent.
    private func openEntry(_ target: MixTarget) {
        guard let snapshot, let index = (0..<snapshot.trackCount).first(where: { snapshot.trackID(at: $0) == target.track }),
              let mix = snapshot.mixer(onTrack: index), let name = snapshot.trackName(at: index), let actions = mixActions else { return }
        entry?.cancel()
        let layout = TrackControlLayout(row: rows.rect(at: index, width: Double(bounds.width)))
        let title = text(target.field == .gain ? "Volume" : "Pan") + " " + name
        entry = ValueEntryField.open(in: self, over: layout.rect(target.field), text: MixEntry.text(target.field, mix[target.field].value),
            label: title, submit: { typed in
                guard let value = MixEntry.value(target.field, typed) else { return false }
                if let enter = actions.enter { enter(target.track, target.field, value) }
                else if value != mix[target.field].value { actions.set(target.track, target.field, value) }
                return true
            }, onClose: { [weak self] in self?.entry = nil })
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
        if event.keyCode == kVK_Escape, clicks.active { clicks.cancel(); return }
        clicks.settle()
        guard !event.modifierFlags.contains(.command) else { super.keyDown(with: event); return }
        switch Int(event.keyCode) {
        case kVK_LeftArrow, kVK_RightArrow:
            guard let index = selectedIndex, let id = snapshot?.trackID(at: index), let tree = snapshot?.trackHierarchy(at: index) else { return }
            if tree.hasChildren && collapsedTracks.contains(id) == (event.keyCode == kVK_RightArrow) { onToggleFolder?(id) }
            else if event.keyCode == kVK_LeftArrow, let parent = tree.parent { onSelect?(parent) }
            else { super.keyDown(with: event) }
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

    private func select(_ id: UUID, change: TrackSelectionChange) {
        if let onSelectTrack { onSelectTrack(id, change) } else { onSelect?(id) }
    }

    override func accessibilityChildren() -> [Any]? {
        guard let snapshot, let window else { accessible = [:]; return [] }
        let document = documentID
        var retained: [UUID: AccessibleTrackHeader] = [:]
        var retainedControls: [String: AccessibleMixControl] = [:]
        var retainedFX: [UUID: AccessibleTrackHeader] = [:]
        var retainedFolders: [UUID: AccessibleTrackHeader] = [:]
        var retainedDevices: [UUID: AccessibleTrackHeader] = [:]
        var retainedOverflow: [UUID: AccessibleTrackHeader] = [:]
        let children: [AccessibleTrackHeader] = rows.visible.compactMap { index in
            guard let id = snapshot.trackID(at: index), let name = snapshot.trackName(at: index) else { return nil }
            let child = accessible[id] ?? AccessibleTrackHeader(actions: TrackHeaderActions())
            child.setAccessibilityElement(true)
            child.setAccessibilityRole(.row)
            child.setAccessibilityLabel(name)
            child.setAccessibilitySelected(selectedTracks.contains(id))
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
                    control.setAccessibilityMinValue(NSNumber(value: field == .pan ? ModelLimits.pan.lowerBound : ModelLimits.gain.lowerBound))
                    control.setAccessibilityMaxValue(NSNumber(value: field == .gain ? ModelLimits.gain.upperBound : ModelLimits.pan.upperBound))
                    control.setAccessibilityParent(child)
                    control.setAccessibilityIdentifier(key)
                    control.setAccessibilityFrame(window.convertToScreen(convert(layout.rect(field).intersection(bounds), to: nil)))
                    let actions = mixActions
                    control.actions.adjust = { [weak self, weak control] direction in
                        guard let self, self.documentID == document, let control else { return }
                        if (field == .gain || field == .pan), let adjust = actions?.adjust {
                            adjust(id, field, direction)
                            return
                        }
                        guard let next = control.actions.step.advance(field, direction: direction) else { return }
                        control.setAccessibilityValue(NSNumber(value: next))
                        if let spoken = MixScale.spokenValue(field, next) {
                            control.setAccessibilityValueDescription(spoken)
                        }
                        if (field == .mute || field == .solo), let toggle = actions?.toggle { toggle(id, field) }
                        else { actions?.set(id, field, next) }
                    }
                    retainedControls[key] = control
                    return control
                }
                child.setAccessibilityChildren(controls)
            } else { child.setAccessibilityChildren([]) }
            if let tree = snapshot.trackHierarchy(at: index), tree.hasChildren {
                let control = folderElements[id] ?? AccessibleTrackHeader(actions: TrackHeaderActions())
                let folder = TrackFolderLayout(row: rows.rect(at: index, width: bounds.width), depth: tree.depth, folder: true)
                control.setAccessibilityElement(true); control.setAccessibilityRole(.button)
                control.setAccessibilityLabel(text(collapsedTracks.contains(id) ? "Expand Folder" : "Collapse Folder") + " " + name)
                control.setAccessibilityIdentifier("\(id)/disclosure")
                control.setAccessibilityParent(child)
                control.setAccessibilityFrame(window.convertToScreen(convert(folder.disclosure.intersection(bounds), to: nil)))
                control.actions.press = { [weak self] in self?.onToggleFolder?(id) }
                child.setAccessibilityChildren((child.accessibilityChildren() ?? []) + [control])
                retainedFolders[id] = control
            }
            let fx = TrackFXLayout(row: rows.rect(at: index, width: Double(bounds.width)))
            if fx.visible, fx.rect.intersects(bounds) {
                let control = fxElements[id] ?? AccessibleTrackHeader(actions: TrackHeaderActions())
                control.setAccessibilityElement(true); control.setAccessibilityRole(.button)
                control.setAccessibilityLabel(text("Add FX") + " " + name)
                control.setAccessibilityIdentifier("\(id)/fx")
                control.setAccessibilityParent(child)
                control.setAccessibilityFrame(window.convertToScreen(convert(fx.rect.intersection(bounds), to: nil)))
                control.actions.press = { [weak self] in self?.select(id, change: .focus); self?.onOpenFX?(id) }
                child.setAccessibilityChildren((child.accessibilityChildren() ?? []) + [control])
                retainedFX[id] = control
            }
            let devices = headerDevices(on: id)
            let names = TrackFXNamesLayout(row: fx.row, count: devices.count, depth: snapshot.trackHierarchy(at: index)?.depth ?? 0)
            if !names.name.isEmpty, let device = devices.first {
                let control = deviceElements[device.id] ?? AccessibleTrackHeader(actions: TrackHeaderActions())
                control.setAccessibilityElement(true); control.setAccessibilityRole(.button)
                control.setAccessibilityLabel((pluginNames[device.plugin] ?? device.plugin) + " " + name)
                control.setAccessibilityEnabled(!device.offline)
                control.setAccessibilityIdentifier("\(device.id)/window")
                control.setAccessibilityParent(child)
                control.setAccessibilityFrame(window.convertToScreen(convert(names.name.intersection(bounds), to: nil)))
                control.actions.press = { [weak self] in
                    guard let self, self.documentID == document, let current = self.headerDevices(on: id).first(where: { $0.id == device.id }) else { return }
                    self.select(id, change: .focus); self.openHeaderDevice(current, track: id)
                }
                child.setAccessibilityChildren((child.accessibilityChildren() ?? []) + [control])
                retainedDevices[device.id] = control
            }
            if !names.overflow.isEmpty {
                let control = overflowElements[id] ?? AccessibleTrackHeader(actions: TrackHeaderActions())
                control.setAccessibilityElement(true); control.setAccessibilityRole(.menuButton)
                control.setAccessibilityLabel(text("FX") + " … " + name)
                control.setAccessibilityIdentifier("\(id)/fx-more")
                control.setAccessibilityParent(child)
                control.setAccessibilityFrame(window.convertToScreen(convert(names.overflow.intersection(bounds), to: nil)))
                control.actions.press = { [weak self] in
                    guard let self, self.documentID == document else { return }
                    self.select(id, change: .focus)
                    self.deviceMenu(Array(self.headerDevices(on: id).dropFirst()), track: id)
                        .popUp(positioning: nil, at: CGPoint(x: names.overflow.minX, y: names.overflow.maxY), in: self)
                }
                child.setAccessibilityChildren((child.accessibilityChildren() ?? []) + [control])
                retainedOverflow[id] = control
            }
            retained[id] = child
            return child
        }
        accessible = retained
        controlElements = retainedControls
        fxElements = retainedFX
        folderElements = retainedFolders
        deviceElements = retainedDevices
        overflowElements = retainedOverflow
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
    override func accessibilityPerformIncrement() -> Bool { adjust(1) }
    override func accessibilityPerformDecrement() -> Bool { adjust(-1) }
    override func accessibilityPerformPress() -> Bool { adjust(1) }
    private nonisolated func adjust(_ direction: Double) -> Bool {
        // AppKit's normal main-thread callback captures targets before returning.
        // An off-main accessibility client must first enter the UI actor.
        if Thread.isMainThread { MainActor.assumeIsolated { [actions] in actions.adjust?(direction) } }
        else { Task { @MainActor [actions] in actions.adjust?(direction) } }
        return true
    }
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
