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
                Button(text("Add FX"), action: add).disabled(controller.busy || controller.selection == nil)
            }.padding(10)
            Divider()
            if let document = controller.documentID, let track = controller.selection {
                let devices = controller.deviceRows(on: track)
                let states = Dictionary(uniqueKeysWithValues: devices.compactMap { device in
                    controller.parameterState(device.id).map { (device.id, $0) }
                })
                VStack(spacing: 0) {
                    DeviceStripCanvas(controller: controller, devices: devices, track: track, document: document,
                                      states: states, busy: controller.busy, locked: controller.historyBusy,
                                      changes: devices.map { states[$0.id].map(DeviceListChange.init) })
                    Text(text("FX chain gesture help")).font(.caption).foregroundStyle(.secondary).padding(6)
                }
                .task(id: DeviceViewKey(document: document, track: track, running: devices.filter { !$0.offline }.map(\.id))) {
                    await controller.showDevices(on: track).value
                }
            } else {
                Text(text("Select a track")).foregroundStyle(.secondary).frame(maxWidth: .infinity, maxHeight: .infinity)
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
        layout = state.layout; revision = state.revision; loading = state.loading; error = state.error; query = state.query
    }
}

/// A list is read again when the shown track or its running devices change.
private struct DeviceViewKey: Equatable {
    let document: UUID
    let track: UUID
    let running: [UUID]
}

private struct DeviceStripCanvas: NSViewRepresentable {
    let controller: ProjectController
    let devices: [TrackDevice]
    let track: UUID
    let document: UUID
    let states: [UUID: DeviceParameterState]
    let busy: Bool
    let locked: Bool
    let changes: [DeviceListChange?]
    func makeNSView(context: Context) -> NSScrollView {
        let scroll = DeviceStripScrollView()
        scroll.hasHorizontalScroller = true; scroll.hasVerticalScroller = false
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
        strip.onInsert = { plugin in Task { await controller.addPluginOpeningWindow(plugin, on: track, documentID: document) } }
        strip.onOpenWindow = { device in Task { await controller.openPluginWindow(device, documentID: document) } }
        strip.onBegin = { controller.beginParameter($0, device: $1) }
        strip.onChange = { controller.changeParameter($0) }
        strip.onEnd = { Task { await controller.finishParameter(commit: true) } }
        strip.onStep = { device, id, value in Task { await controller.setParameter(id, device: device, value: value) } }
        strip.onPin = { device, id, pinned in Task { await controller.setPinned(id, device: device, pinned: pinned, documentID: document) } }
        strip.configure(devices: devices, track: track, document: document, busy: busy, locked: locked, states: states)
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
    static let trailingWidth = 180.0
    private(set) var devices: [TrackDevice] = []
    private(set) var columns: [DeviceColumnView] = []
    /// Columns of devices shown before in this document. Returning to a track
    /// reuses them with their rows and scroll positions.
    private var cache: [UUID: DeviceColumnView] = [:]
    private var track: UUID?
    private var document: UUID?
    private var busy = false
    private let empty = NSTextField(labelWithString: "")
    var onEdit: ((DeviceChainEdit) -> Void)?
    var onInsert: ((String) -> Void)?
    /// A click on a device's name opens its plugin window.
    var onOpenWindow: ((UUID) -> Void)?
    var onBegin: ((UInt32, UUID) -> Bool)?
    var onChange: ((Double) -> Void)?
    var onEnd: (() -> Void)?
    var onStep: ((UUID, UInt32, Double) -> Void)?
    var onPin: ((UUID, UInt32, Bool) -> Void)?
    var contentWidth: Double { Double(columns.count) * Self.columnWidth + Self.trailingWidth }
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        registerForDraggedTypes([FXDrag.deviceType, FXDrag.pluginType])
        setAccessibilityElement(false)
        empty.textColor = .secondaryLabelColor; empty.alignment = .center
        addSubview(empty)
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    /// Keeps the column of a device that stays, with its list and scroll position.
    func configure(devices: [TrackDevice], track: UUID, document: UUID, busy: Bool,
                   locked: Bool = false, states: [UUID: DeviceParameterState] = [:]) {
        if self.document != document { cache = [:] }
        self.devices = devices; self.track = track; self.document = document; self.busy = busy
        var next: [DeviceColumnView] = []
        for (index, device) in devices.enumerated() {
            let column = cache[device.id] ?? DeviceColumnView(
                device: device, payload: FXDrag(document: document, track: track, device: device.id, plugin: nil))
            cache[device.id] = column
            if column.superview !== self { addSubview(column) }
            column.onEdit = { [weak self] edit in self?.onEdit?(edit) }
            column.onLeft = { [weak self] in
                self?.onEdit?(.reorder(device: device.id, before: index > 0 ? devices[index - 1].id : device.id))
            }
            column.onRight = { [weak self] in
                self?.onEdit?(.reorder(device: device.id, before: index + 2 < devices.count ? devices[index + 2].id : nil))
            }
            column.onOpenWindow = { [weak self] in self?.onOpenWindow?(device.id) }
            column.onCopy = { [weak self] in
                self?.onEdit?(.copy(source: device.id, track: track, before: index + 1 < devices.count ? devices[index + 1].id : nil))
            }
            column.list.onBegin = { [weak self] id in self?.onBegin?(id, device.id) ?? false }
            column.list.onChange = { [weak self] value in self?.onChange?(value) }
            column.list.onEnd = { [weak self] in self?.onEnd?() }
            column.list.onStep = { [weak self] id, value in self?.onStep?(device.id, id, value) }
            column.list.onPin = { [weak self] id, pinned in self?.onPin?(device.id, id, pinned) }
            column.configure(device: device, first: index == 0, last: index + 1 == devices.count,
                             busy: busy, locked: locked, state: states[device.id])
            next.append(column)
        }
        let kept = Set(next.map(ObjectIdentifier.init))
        for column in columns where !kept.contains(ObjectIdentifier(column)) { column.removeFromSuperview() }
        columns = next
        empty.stringValue = text(devices.isEmpty ? "No FX on this track" : "Drop FX here")
        needsLayout = true
    }

    override func layout() {
        super.layout()
        for (index, column) in columns.enumerated() {
            column.frame = CGRect(x: Double(index) * Self.columnWidth, y: 0, width: Self.columnWidth - 1, height: bounds.height)
        }
        let start = Double(columns.count) * Self.columnWidth
        empty.frame = CGRect(x: start + 8, y: max(0, bounds.height / 2 - 10), width: max(0, bounds.width - start - 16), height: 20)
    }

    private func targetBefore(_ point: CGPoint) -> UUID? {
        let index = Int(max(0, (point.x / Self.columnWidth + 0.5).rounded(.down)))
        return index < devices.count ? devices[index].id : nil
    }

    func accept(_ pasteboard: NSPasteboard, at point: CGPoint, copying: Bool) -> Bool {
        guard point.x.isFinite, point.y.isFinite, point.x < Double(Int.max) / 2, !busy, let target = track,
              let payload = FXDrag.read(pasteboard), payload.document == document else { return false }
        if let plugin = payload.plugin { onInsert?(plugin); return onInsert != nil }
        if payload.track == target, !devices.contains(where: { $0.id == payload.device }) { return false }
        guard let edit = payload.operation(on: target, copying: copying, before: targetBefore(point)), let onEdit else { return false }
        onEdit(edit); return true
    }
    override func draggingEntered(_ sender: any NSDraggingInfo) -> NSDragOperation { dragOperation(sender) }
    override func draggingUpdated(_ sender: any NSDraggingInfo) -> NSDragOperation { dragOperation(sender) }
    private func dragOperation(_ sender: any NSDraggingInfo) -> NSDragOperation {
        guard !busy, let payload = FXDrag.read(sender.draggingPasteboard), payload.document == document else { return [] }
        return payload.plugin != nil || payload.track != track || NSEvent.modifierFlags.contains(.command) ? .copy : .move
    }
    override func performDragOperation(_ sender: any NSDraggingInfo) -> Bool {
        accept(sender.draggingPasteboard, at: convert(sender.draggingLocation, from: nil), copying: NSEvent.modifierFlags.contains(.command))
    }
}

/// One device: its name and switches, chain buttons, a search field and its
/// own parameter list. An offline device keeps its settings and lists nothing.
@MainActor
final class DeviceColumnView: NSView, NSDraggingSource {
    private(set) var device: TrackDevice
    let title = NSTextField(labelWithString: "")
    let bypass = NSButton(), offline = NSButton()
    let left = NSButton(), right = NSButton(), copy = NSButton(), remove = NSButton()
    let search = NSSearchField()
    let list = DeviceParameterListView(frame: .zero)
    let message = NSTextField(wrappingLabelWithString: "")
    private let payload: FXDrag
    private var origin: CGPoint?
    private var state: DeviceParameterState?
    private var locked = false
    var dragEnabled = true
    var onEdit: ((DeviceChainEdit) -> Void)?
    var onLeft: (() -> Void)?, onRight: (() -> Void)?, onCopy: (() -> Void)?
    /// A click on the name, without a drag, opens the plugin's own window.
    var onOpenWindow: (() -> Void)?
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
        for (button, name, action) in [(bypass, "Bypass", #selector(bypassFX)), (offline, "Offline", #selector(offlineFX))] {
            button.setButtonType(.switch); button.title = text(name)
            button.target = self; button.action = action
        }
        offline.toolTip = text("Offline FX help")
        for (button, symbol, name, action) in [
            (left, "arrow.left", "Move FX Earlier", #selector(moveEarlier)), (right, "arrow.right", "Move FX Later", #selector(moveLater)),
            (copy, "plus.square.on.square", "Duplicate FX", #selector(copyFX)), (remove, "trash", "Remove FX", #selector(removeFX))
        ] {
            button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: text(name))
            button.imagePosition = .imageOnly; button.bezelStyle = .texturedRounded
            button.target = self; button.action = action; button.toolTip = text(name)
        }
        search.placeholderString = text("Search parameters")
        search.sendsSearchStringImmediately = true
        search.target = self; search.action = #selector(searched)
        message.textColor = .secondaryLabelColor
        message.alignment = .center
        for view in [title, offline, bypass, left, right, copy, remove, search, list, message] { addSubview(view) }
        configure(device: device, first: true, last: true, busy: false, locked: false, state: nil)
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func configure(device: TrackDevice, first: Bool, last: Bool, busy: Bool, locked: Bool, state: DeviceParameterState?) {
        self.device = device
        self.state = state
        self.locked = locked
        title.stringValue = device.title
        // The tooltip names the plugin; opening its window is the click and the accessibility action.
        title.toolTip = device.plugin
        title.setAccessibilityLabel(device.title)
        title.setAccessibilityCustomActions(device.offline ? [] : [
            NSAccessibilityCustomAction(name: text("Open plugin window")) { [weak self] in self?.openWindow() ?? false }
        ])
        title.textColor = device.bypassed || device.offline ? .secondaryLabelColor : .labelColor
        bypass.state = device.bypassed ? .on : .off
        offline.state = device.offline ? .on : .off
        bypass.setAccessibilityLabel(text("Bypass") + " " + device.title)
        offline.setAccessibilityLabel(text("Offline") + " " + device.title)
        for (button, name) in [(left, "Move FX Earlier"), (right, "Move FX Later"), (copy, "Duplicate FX"), (remove, "Remove FX")] {
            button.setAccessibilityLabel(text(name) + " " + device.title)
        }
        left.isEnabled = !busy && !first
        right.isEnabled = !busy && !last
        copy.isEnabled = !busy; remove.isEnabled = !busy; dragEnabled = !busy
        bypass.isEnabled = !busy; offline.isEnabled = !busy
        search.isEnabled = !device.offline
        search.setAccessibilityLabel(text("Search parameters") + " " + device.title)
        if search.stringValue != (state?.query ?? "") { search.stringValue = state?.query ?? "" }
        // Offline devices keep settings but have no running plugin to read.
        let note: String? = if device.offline {
            text("Offline device parameters")
        } else if let state, let error = state.error {
            error
        } else if state == nil || state?.loading == true {
            text("Loading parameters")
        } else if state?.parameters.isEmpty == true {
            text("No editable parameters")
        } else {
            nil
        }
        message.stringValue = note ?? ""
        message.isHidden = note == nil
        list.isHidden = note != nil
        if let state, note == nil { list.configure(state: state, pinned: Set(device.pinned), locked: locked) }
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let width = bounds.width - 16
        title.frame = CGRect(x: 8, y: 8, width: max(0, width - 96), height: 20)
        offline.frame = CGRect(x: max(8, bounds.width - 100), y: 6, width: 92, height: 24)
        bypass.frame = CGRect(x: 8, y: 34, width: min(100, max(0, width - 140)), height: 24)
        for (index, button) in [left, right, copy, remove].enumerated() {
            button.frame = CGRect(x: max(8, bounds.width - 8 - Double(4 - index) * 32), y: 32, width: 28, height: 26)
        }
        search.frame = CGRect(x: 8, y: 64, width: max(0, width), height: 24)
        let body = CGRect(x: 4, y: 94, width: max(0, bounds.width - 8), height: max(0, bounds.height - 98))
        list.frame = body
        message.frame = body.insetBy(dx: 8, dy: 8)
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
        guard dragEnabled, let origin, hypot(event.locationInWindow.x - origin.x, event.locationInWindow.y - origin.y) >= 4, let data = payload.data else { return }
        self.origin = nil
        let item = NSPasteboardItem(); item.setData(data, forType: payload.type)
        let dragging = NSDraggingItem(pasteboardWriter: item)
        dragging.setDraggingFrame(CGRect(x: 0, y: 0, width: bounds.width, height: 60), contents: NSImage(systemSymbolName: "puzzlepiece.extension", accessibilityDescription: nil))
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
    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        context == .withinApplication ? [.copy, .move] : []
    }
    func ignoreModifierKeys(for session: NSDraggingSession) -> Bool { true }
}
