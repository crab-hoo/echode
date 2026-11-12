import AppKit
import EchodeBridge
import SwiftUI

/// The selected track's devices side by side in processing order, each with
/// all of its parameters. Selecting a track never waits for a plugin: names
/// and settings show at once, and each list fills in when its plugin answers.
struct WorkspaceDeviceView: View {
    let controller: ProjectController
    let add: () -> Void
    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(text("Devices")).font(.headline)
                Text(controller.tracks.first { $0.id == controller.selection }?.name ?? text("Select a track"))
                    .foregroundStyle(.secondary).lineLimit(1)
                Spacer()
            }.padding(10)
            Divider()
            if let document = controller.documentID, let track = controller.selection {
                let devices = controller.deviceRows(on: track)
                let states = Dictionary(
                    uniqueKeysWithValues: devices.compactMap { device in
                        controller.parameterState(device.id).map { (device.id, $0) }
                    })
                VStack(spacing: 0) {
                    DeviceStripCanvas(
                        controller: controller, devices: devices, track: track, document: document,
                        states: states, busy: controller.busy, locked: controller.historyBusy,
                        outputOpening: controller.outputOpening,
                        collapsed: controller.collapsedDevices,
                        changes: devices.map { states[$0.id].map(DeviceListChange.init) }, add: add)
                    Text(text("FX chain gesture help")).font(.caption).foregroundStyle(.secondary).padding(6)
                }
                .task(
                    id: DeviceViewKey(
                        document: document, track: track, running: devices.filter { !$0.offline }.map(\.id))
                ) {
                    await controller.showDevices(on: track).value
                }
            } else {
                Text(text("Select a track")).foregroundStyle(.secondary).frame(
                    maxWidth: .infinity, maxHeight: .infinity)
            }
        }.task { await controller.loadPluginNames() }
    }
}

/// What a list shows changed. Read in the body, so SwiftUI updates the strip.
private struct DeviceListChange: Equatable {
    let layout: Int
    let revision: Int
    let loading: Bool
    let error: String?
    let query: String
    @MainActor init(_ state: DeviceParameterState) {
        layout = state.layout
        revision = state.revision
        loading = state.loading
        error = state.error
        query = state.query
    }
}

/// A list is read again when the shown track or its running devices change.
private struct DeviceViewKey: Equatable {
    let document: DocumentID
    let track: UUID
    let running: [UUID]
}

private struct DeviceStripCanvas: NSViewRepresentable {
    let controller: ProjectController
    let devices: [TrackDevice]
    let track: UUID
    let document: DocumentID
    let states: [UUID: DeviceParameterState]
    let busy: Bool
    let locked: Bool
    let outputOpening: Bool
    let collapsed: Set<UUID>
    let changes: [DeviceListChange?]
    let add: () -> Void
    func makeNSView(context: Context) -> NSScrollView {
        let scroll = DeviceStripScrollView()
        scroll.hasHorizontalScroller = true
        scroll.hasVerticalScroller = false
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.documentView = DeviceStripView(frame: .zero)
        return scroll
    }
    func updateNSView(_ view: NSScrollView, context: Context) {
        guard let strip = view.documentView as? DeviceStripView else { return }
        let controller = controller
        let document = document
        let track = track
        strip.onEdit = { edit in Task { await controller.editDeviceChain(edit, documentID: document) } }
        strip.onInsert = { plugin in
            Task { await controller.addPluginOpeningWindow(plugin, on: track, documentID: document) }
        }
        strip.onOpenWindow = { device in Task { await controller.openPluginWindow(device, documentID: document) } }
        strip.onCollapse = { device, collapsed in
            controller.setDeviceCollapsed(device, collapsed: collapsed, documentID: document)
        }
        strip.onAdd = add
        strip.onBegin = { controller.beginParameter($0, device: $1, documentID: document) }
        strip.onChange = { controller.changeParameter($0, documentID: document) }
        strip.onEnd = { commit in Task { await controller.finishParameter(commit: commit, documentID: document) } }
        strip.onStep = { device, id, value in
            Task { await controller.setParameter(id, device: device, value: value, documentID: document) }
        }
        strip.onEnter = { device, id, text in
            await controller.enterParameter(text, id: id, device: device, documentID: document)
        }
        strip.onPin = { device, id, pinned in
            Task { await controller.setPinned(id, device: device, pinned: pinned, documentID: document) }
        }
        strip.onRetry = { device in controller.retryParameters(device, documentID: document) }
        strip.configure(
            devices: devices, track: track, document: document, busy: busy, locked: locked, states: states,
            outputOpening: outputOpening, collapsed: collapsed)
        view.needsLayout = true
    }
}

/// The whole empty strip remains a drop target after resizing or deleting FX.
@MainActor
final class DeviceStripScrollView: NSScrollView {
    override func layout() {
        super.layout()
        guard let strip = documentView as? DeviceStripView else { return }
        let size = CGSize(width: max(contentSize.width, strip.contentWidth), height: contentSize.height)
        if strip.frame.size != size { strip.setFrameSize(size) }
    }
}

/// Native controls provide the same operations for keyboard and VoiceOver.
/// Dragging a device's title is an additional entry into the chain actions.
@MainActor
final class DeviceStripView: NSView {
    static let columnWidth = 300.0
    static let collapsedWidth = 200.0
    static let trailingWidth = 180.0
    private(set) var devices: [TrackDevice] = []
    private(set) var columns: [DeviceColumnView] = []
    /// Columns of devices shown before in this document. Returning to a track
    /// reuses them with their rows and scroll positions.
    private var cache: [UUID: DeviceColumnView] = [:]
    private var track: UUID?
    private var document: DocumentID?
    private var busy = false
    private let empty = NSTextField(labelWithString: "")
    let addButton = NSButton(title: text("Add FX"), target: nil, action: nil)
    var onAdd: (() -> Void)?
    var onEdit: ((DeviceChainEdit) -> Void)?
    var onInsert: ((String) -> Void)?
    /// A click on a device's name opens its plugin window.
    var onOpenWindow: ((UUID) -> Void)?
    var onCollapse: ((UUID, Bool) -> Void)?
    var onBegin: ((UInt32, UUID) -> Bool)?
    var onChange: ((Double) -> Void)?
    var onEnd: ((Bool) -> Void)?
    var onStep: ((UUID, UInt32, Double) -> Void)?
    /// Typed text for a parameter. False keeps the field open.
    var onEnter: ((UUID, UInt32, String) async -> Bool)?
    var onPin: ((UUID, UInt32, Bool) -> Void)?
    var onRetry: ((UUID) -> Void)?
    var contentWidth: Double { columns.reduce(Self.trailingWidth) { $0 + $1.preferredWidth } }
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        registerForDraggedTypes([FXDrag.deviceType, FXDrag.pluginType])
        setAccessibilityElement(false)
        empty.textColor = .secondaryLabelColor
        empty.alignment = .center
        addSubview(empty)
        addButton.bezelStyle = .rounded
        addButton.target = self
        addButton.action = #selector(addFX)
        addSubview(addButton)
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    /// Keeps the column of a device that stays, with its list and scroll position.
    func configure(
        devices: [TrackDevice], track: UUID, document: DocumentID, busy: Bool,
        locked: Bool = false, states: [UUID: DeviceParameterState] = [:], outputOpening: Bool = false,
        collapsed: Set<UUID> = []
    ) {
        if self.document != document { cache = [:] }
        self.devices = devices
        self.track = track
        self.document = document
        self.busy = busy
        addButton.isEnabled = !busy
        var next: [DeviceColumnView] = []
        for (index, device) in devices.enumerated() {
            let column =
                cache[device.id]
                ?? DeviceColumnView(
                    device: device, payload: FXDrag(document: document, track: track, device: device.id, plugin: nil))
            cache[device.id] = column
            if column.superview !== self { addSubview(column) }
            column.onEdit = { [weak self] edit in self?.onEdit?(edit) }
            column.onLeft = { [weak self] in
                self?.onEdit?(.reorder(device: device.id, before: index > 0 ? devices[index - 1].id : device.id))
            }
            column.onRight = { [weak self] in
                self?.onEdit?(
                    .reorder(device: device.id, before: index + 2 < devices.count ? devices[index + 2].id : nil))
            }
            column.onOpenWindow = { [weak self] in self?.onOpenWindow?(device.id) }
            column.onCollapse = { [weak self] collapsed in
                guard let self else { return }
                self.onCollapse?(device.id, collapsed)
                self.needsLayout = true
                self.enclosingScrollView?.needsLayout = true
            }
            column.onCopy = { [weak self] in
                self?.onEdit?(
                    .copy(
                        source: device.id, track: track, before: index + 1 < devices.count ? devices[index + 1].id : nil
                    ))
            }
            column.list.onBegin = { [weak self] id in self?.onBegin?(id, device.id) ?? false }
            column.list.onChange = { [weak self] value in self?.onChange?(value) }
            column.list.onEnd = { [weak self] commit in self?.onEnd?(commit) }
            column.list.onEnter = { [weak self] id, text in await self?.onEnter?(device.id, id, text) ?? false }
            column.list.onStep = { [weak self] id, value in self?.onStep?(device.id, id, value) }
            column.list.onPin = { [weak self] id, pinned in self?.onPin?(device.id, id, pinned) }
            column.onRetry = { [weak self] in self?.onRetry?(device.id) }
            column.configure(
                device: device, first: index == 0, last: index + 1 == devices.count,
                busy: busy, locked: locked, outputOpening: outputOpening, collapsed: collapsed.contains(device.id),
                state: states[device.id])
            next.append(column)
        }
        let kept = Set(next.map(ObjectIdentifier.init))
        for column in columns where !kept.contains(ObjectIdentifier(column)) { column.removeFromSuperview() }
        columns = next
        empty.stringValue = text(devices.isEmpty ? "No FX on this track" : "Drop FX here")
        needsLayout = true
        enclosingScrollView?.needsLayout = true
    }

    override func layout() {
        super.layout()
        var start = 0.0
        for column in columns {
            column.frame = CGRect(x: start, y: 0, width: column.preferredWidth - 1, height: bounds.height)
            start += column.preferredWidth
        }
        addButton.frame = CGRect(
            x: start + 16, y: max(8, bounds.height / 2 - 32), width: Self.trailingWidth - 32, height: 28)
        empty.frame = CGRect(x: start + 8, y: addButton.frame.maxY + 8, width: Self.trailingWidth - 16, height: 20)
    }

    @objc private func addFX() { if !busy { onAdd?() } }

    private func targetBefore(_ point: CGPoint) -> UUID? {
        var start = 0.0
        for column in columns {
            if point.x < start + column.preferredWidth / 2 { return column.device.id }
            start += column.preferredWidth
        }
        return nil
    }

    func accept(_ pasteboard: NSPasteboard, at point: CGPoint, copying: Bool) -> Bool {
        guard point.x.isFinite, point.y.isFinite, point.x < Double(Int.max) / 2, !busy, let target = track,
            let payload = FXDrag.read(pasteboard), payload.document == document
        else { return false }
        if let plugin = payload.plugin {
            onInsert?(plugin)
            return onInsert != nil
        }
        if payload.track == target, !devices.contains(where: { $0.id == payload.device }) { return false }
        guard let edit = payload.operation(on: target, copying: copying, before: targetBefore(point)), let onEdit else {
            return false
        }
        onEdit(edit)
        return true
    }
    override func draggingEntered(_ sender: any NSDraggingInfo) -> NSDragOperation { dragOperation(sender) }
    override func draggingUpdated(_ sender: any NSDraggingInfo) -> NSDragOperation { dragOperation(sender) }
    private func dragOperation(_ sender: any NSDraggingInfo) -> NSDragOperation {
        guard !busy, let payload = FXDrag.read(sender.draggingPasteboard), payload.document == document else {
            return []
        }
        return payload.plugin != nil || payload.track != track || NSEvent.modifierFlags.contains(.command)
            ? .copy : .move
    }
    override func performDragOperation(_ sender: any NSDraggingInfo) -> Bool {
        accept(
            sender.draggingPasteboard, at: convert(sender.draggingLocation, from: nil),
            copying: NSEvent.modifierFlags.contains(.command))
    }
}

/// One device: its name and switches, chain buttons, a search field and its
/// own parameter list. An offline device keeps its settings and lists nothing.
@MainActor
final class DeviceColumnView: NSView, NSDraggingSource {
    private(set) var device: TrackDevice
    let disclosure = NSButton()
    private(set) var isCollapsed = false
    var preferredWidth: Double { isCollapsed ? DeviceStripView.collapsedWidth : DeviceStripView.columnWidth }
    let title = NSTextField(labelWithString: "")
    let windowIndicator = NSImageView()
    let bypass = NSButton(), offline = NSButton()
    let left = NSButton(), right = NSButton(), copy = NSButton(), remove = NSButton()
    let search = NSSearchField()
    let list = DeviceParameterListView(frame: .zero)
    let message = NSTextField(wrappingLabelWithString: "")
    let latency = NSTextField(labelWithString: "")
    let retry = NSButton(title: text("Retry parameters"), target: nil, action: nil)
    private let payload: FXDrag
    private var origin: CGPoint?
    private var state: DeviceParameterState?
    private var locked = false
    private var first = true, last = true
    var dragEnabled = true
    var onEdit: ((DeviceChainEdit) -> Void)?
    var onLeft: (() -> Void)?, onRight: (() -> Void)?, onCopy: (() -> Void)?
    /// A click on the name, without a drag, opens the plugin's own window.
    var onOpenWindow: (() -> Void)?
    private var outputOpening = false
    var onCollapse: ((Bool) -> Void)?
    var onRetry: (() -> Void)?
    override var isFlipped: Bool { true }

    init(device: TrackDevice, payload: FXDrag) {
        self.device = device
        self.payload = payload
        super.init(frame: .zero)
        wantsLayer = true
        layer?.borderWidth = 1
        layer?.borderColor = NSColor.separatorColor.cgColor
        layer?.cornerRadius = 6
        title.lineBreakMode = .byTruncatingMiddle
        title.font = .boldSystemFont(ofSize: NSFont.systemFontSize)
        windowIndicator.identifier = NSUserInterfaceItemIdentifier("plugin-window-open")
        windowIndicator.image = NSImage(
            systemSymbolName: "macwindow", accessibilityDescription: text("Plugin window open"))
        windowIndicator.contentTintColor = .controlAccentColor
        windowIndicator.setAccessibilityElement(true)
        windowIndicator.setAccessibilityRole(.image)
        disclosure.identifier = NSUserInterfaceItemIdentifier("device-disclosure")
        disclosure.bezelStyle = .disclosure
        disclosure.setButtonType(.pushOnPushOff)
        disclosure.target = self
        disclosure.action = #selector(toggleDisclosure)
        for (button, name, action) in [
            (bypass, "Bypass", #selector(bypassFX)), (offline, "Offline", #selector(offlineFX)),
        ] {
            button.setButtonType(.switch)
            button.title = text(name)
            button.target = self
            button.action = action
        }
        offline.toolTip = text("Offline FX help")
        for (button, symbol, name, action) in [
            (left, "arrow.left", "Move FX Earlier", #selector(moveEarlier)),
            (right, "arrow.right", "Move FX Later", #selector(moveLater)),
            (copy, "plus.square.on.square", "Duplicate FX", #selector(copyFX)),
            (remove, "trash", "Remove FX", #selector(removeFX)),
        ] {
            button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: text(name))
            button.imagePosition = .imageOnly
            button.bezelStyle = .texturedRounded
            button.target = self
            button.action = action
            button.toolTip = text(name)
        }
        search.placeholderString = text("Search parameters")
        search.sendsSearchStringImmediately = true
        search.target = self
        search.action = #selector(searched)
        message.textColor = .secondaryLabelColor
        message.alignment = .center
        latency.identifier = NSUserInterfaceItemIdentifier("plugin-latency")
        latency.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        latency.textColor = .secondaryLabelColor
        addSubview(latency)
        retry.identifier = NSUserInterfaceItemIdentifier("retry-parameters")
        retry.bezelStyle = .rounded
        retry.target = self
        retry.action = #selector(retryList)
        addSubview(retry)
        for view in [
            disclosure, title, windowIndicator, offline, bypass, left, right, copy, remove, search, list, message,
        ] { addSubview(view) }
        configure(device: device, first: true, last: true, busy: false, locked: false, state: nil)
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func configure(
        device: TrackDevice, first: Bool, last: Bool, busy: Bool, locked: Bool, outputOpening: Bool = false,
        collapsed: Bool = false, state: DeviceParameterState?
    ) {
        self.device = device
        self.state = state
        self.locked = locked
        self.outputOpening = outputOpening
        self.first = first
        self.last = last
        disclosure.isEnabled = !busy
        if collapsed, let window, let responder = window.firstResponder as? NSView,
            responder.isDescendant(of: list) || responder.isDescendant(of: search)
                || responder === search.currentEditor() || responder === retry
        {
            // Hidden parameter controls must not keep consuming workspace keys.
            window.makeFirstResponder(nil)
        }
        isCollapsed = collapsed
        disclosure.state = collapsed ? .off : .on
        let disclosureLabel = text(collapsed ? "Expand device" : "Collapse device") + " " + device.title
        disclosure.setAccessibilityLabel(disclosureLabel)
        disclosure.toolTip = disclosureLabel
        title.stringValue = device.title
        windowIndicator.isHidden = !device.windowOpen || device.offline
        windowIndicator.toolTip = text("Plugin window open")
        windowIndicator.setAccessibilityLabel(text("Plugin window open") + " " + device.title)
        // The tooltip names the plugin; opening its window is the click and the accessibility action.
        title.toolTip = device.plugin
        title.setAccessibilityLabel(device.title)
        title.setAccessibilityCustomActions(
            device.offline
                ? []
                : [
                    NSAccessibilityCustomAction(name: text("Open plugin window")) { [weak self] in
                        self?.openWindow() ?? false
                    }
                ])
        title.textColor = device.bypassed || device.offline ? .secondaryLabelColor : .labelColor
        bypass.state = device.bypassed ? .on : .off
        offline.state = device.offline ? .on : .off
        bypass.setAccessibilityLabel(text("Bypass") + " " + device.title)
        offline.setAccessibilityLabel(text("Offline") + " " + device.title)
        for (button, name) in [
            (left, "Move FX Earlier"), (right, "Move FX Later"), (copy, "Duplicate FX"), (remove, "Remove FX"),
        ] {
            button.setAccessibilityLabel(text(name) + " " + device.title)
        }
        left.isEnabled = !busy && !first
        right.isEnabled = !busy && !last
        copy.isEnabled = !busy
        remove.isEnabled = !busy
        dragEnabled = !busy
        bypass.isEnabled = !busy
        offline.isEnabled = !busy
        search.isEnabled = !device.offline
        search.setAccessibilityLabel(text("Search parameters") + " " + device.title)
        if search.stringValue != (state?.query ?? "") { search.stringValue = state?.query ?? "" }
        latency.isHidden = device.offline || state?.error != nil || state?.latencySamples == nil
        latency.stringValue = state?.latencySamples.map { String(format: text("Plugin latency samples"), $0) } ?? ""
        latency.toolTip = text("Plugin latency uncompensated")
        latency.setAccessibilityLabel(latency.stringValue + " " + device.title)
        latency.setAccessibilityHelp(text("Plugin latency uncompensated"))
        // Offline devices keep settings but have no running plugin to read.
        let note: String? =
            if device.offline {
                text("Offline device parameters")
            } else if let state, let error = state.error {
                error
            } else if state == nil || state?.loading == true {
                text(outputOpening ? "Preparing audio output" : "Loading parameters")
            } else if state?.parameters.isEmpty == true {
                text("No editable parameters")
            } else {
                nil
            }
        message.stringValue = note ?? ""
        message.setAccessibilityLabel(note.map { $0 + " " + device.title })
        retry.isHidden = isCollapsed || device.offline || state?.error == nil || state?.loading == true
        retry.isEnabled = !busy && !retry.isHidden
        retry.setAccessibilityLabel(text("Retry parameters") + " " + device.title)
        search.isHidden = isCollapsed
        message.isHidden = isCollapsed || note == nil
        list.isHidden = isCollapsed || note != nil
        if let state, note == nil, !isCollapsed {
            list.configure(state: state, pinned: Set(device.pinned), locked: locked)
        }
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let width = bounds.width - 16
        disclosure.frame = CGRect(x: 6, y: 6, width: 22, height: 24)
        let titleWidth = max(0, bounds.width - 40 - (isCollapsed ? 0 : 96))
        let indicatorWidth = windowIndicator.isHidden ? 0.0 : 22.0
        title.frame = CGRect(x: 32, y: 8, width: max(0, titleWidth - indicatorWidth), height: 20)
        windowIndicator.frame = CGRect(x: title.frame.maxX + 2, y: 8, width: 18, height: 18)
        offline.frame = CGRect(x: max(8, bounds.width - 100), y: isCollapsed ? 34 : 6, width: 92, height: 24)
        bypass.frame = CGRect(x: 8, y: 34, width: isCollapsed ? 76 : min(100, max(0, width - 140)), height: 24)
        for (index, button) in [left, right, copy, remove].enumerated() {
            button.frame = CGRect(
                x: max(8, bounds.width - 8 - Double(4 - index) * 32), y: isCollapsed ? 64 : 32, width: 28, height: 26)
        }
        let latencyHeight = latency.isHidden ? 0.0 : 22.0
        latency.frame = CGRect(x: 8, y: isCollapsed ? 96 : 64, width: max(0, width), height: 20)
        search.frame = CGRect(x: 8, y: 64 + latencyHeight, width: max(0, width), height: 24)
        let body = CGRect(
            x: 4, y: 94 + latencyHeight, width: max(0, bounds.width - 8),
            height: max(0, bounds.height - 98 - latencyHeight))
        list.frame = body
        message.frame = body.insetBy(dx: 8, dy: 8)
        if !retry.isHidden {
            message.frame.size.height = max(0, message.frame.height - 40)
        }
        retry.frame = CGRect(x: 12, y: max(body.minY, body.maxY - 38), width: max(0, bounds.width - 24), height: 28)
    }

    @objc private func toggleDisclosure() {
        guard dragEnabled else { return }
        configure(
            device: device, first: first, last: last,
            busy: false, locked: locked, outputOpening: outputOpening, collapsed: !isCollapsed, state: state)
        onCollapse?(isCollapsed)
    }

    @objc private func retryList() {
        guard retry.isEnabled, !retry.isHidden else { return }
        onRetry?()
    }

    @objc private func moveEarlier() { onLeft?() }
    @objc private func moveLater() { onRight?() }
    @objc private func copyFX() { onCopy?() }
    @objc private func removeFX() { onEdit?(.remove(device.id)) }
    @objc private func bypassFX() { onEdit?(.bypass(device: device.id, bypassed: bypass.state == .on)) }
    @objc private func offlineFX() { onEdit?(.offline(device: device.id, offline: offline.state == .on)) }
    @objc private func searched() {
        guard let state else { return }
        state.query = search.stringValue
        list.configure(state: state, pinned: Set(device.pinned), locked: locked)
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let hit = super.hitTest(point)
        return hit === title ? self : hit
    }
    /// Only the header starts a drag; the list below belongs to the sliders.
    override func mouseDown(with event: NSEvent) {
        origin = convert(event.locationInWindow, from: nil).y < 32 ? event.locationInWindow : nil
    }
    override func mouseDragged(with event: NSEvent) {
        guard dragEnabled, let origin,
            hypot(event.locationInWindow.x - origin.x, event.locationInWindow.y - origin.y) >= 4,
            let data = payload.data
        else { return }
        self.origin = nil
        let item = NSPasteboardItem()
        item.setData(data, forType: payload.type)
        let dragging = NSDraggingItem(pasteboardWriter: item)
        dragging.setDraggingFrame(
            CGRect(x: 0, y: 0, width: bounds.width, height: 60),
            contents: NSImage(systemSymbolName: "puzzlepiece.extension", accessibilityDescription: nil))
        beginDraggingSession(with: [dragging], event: event, source: self)
    }
    override func mouseUp(with event: NSEvent) {
        defer { origin = nil }
        guard origin != nil, title.frame.contains(convert(event.locationInWindow, from: nil)) else { return }
        openWindow()
    }
    @discardableResult
    private func openWindow() -> Bool {
        guard !device.offline, let onOpenWindow else { return false }
        onOpenWindow()
        return true
    }
    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext)
        -> NSDragOperation
    {
        context == .withinApplication ? [.copy, .move] : []
    }
    func ignoreModifierKeys(for session: NSDraggingSession) -> Bool { true }
}
