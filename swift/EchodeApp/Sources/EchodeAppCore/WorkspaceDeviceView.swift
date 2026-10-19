import AppKit
import EchodeBridge
import SwiftUI

struct WorkspaceDeviceView: View {
    let controller: ProjectController
    let add: () -> Void
    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(text("Track FX")).font(.headline)
                Text(controller.tracks.first { $0.id == controller.selection }?.name ?? text("Select a track"))
                    .foregroundStyle(.secondary).lineLimit(1)
                Spacer()
                Button(text("Add FX"), action: add).disabled(controller.busy || controller.selection == nil)
            }.padding(10)
            Divider()
            if let document = controller.documentID, let track = controller.selection {
                DeviceChainCanvas(devices: controller.deviceRows(on: track), track: track, document: document, busy: controller.busy,
                    edit: { edit in Task { await controller.editDeviceChain(edit, documentID: document) } },
                    insert: { plugin in Task { await controller.insertPlugin(plugin, on: track, documentID: document) } })
                Text(text("FX chain gesture help")).font(.caption).foregroundStyle(.secondary).padding(8)
            } else {
                Text(text("Select a track")).foregroundStyle(.secondary).frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }.task { await controller.loadPluginNames() }
    }
}

private struct DeviceChainCanvas: NSViewRepresentable {
    let devices: [TrackDevice]
    let track: UUID
    let document: UUID
    let busy: Bool
    let edit: (DeviceChainEdit) -> Void
    let insert: (String) -> Void
    func makeNSView(context: Context) -> NSScrollView {
        let scroll = DeviceChainScrollView()
        scroll.hasVerticalScroller = true; scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.documentView = DeviceChainListView(frame: .zero)
        return scroll
    }
    func updateNSView(_ view: NSScrollView, context: Context) {
        guard let list = view.documentView as? DeviceChainListView else { return }
        list.onEdit = edit; list.onInsert = insert
        list.configure(devices: devices, track: track, document: document, busy: busy)
        list.setFrameSize(CGSize(width: view.contentSize.width, height: max(view.contentSize.height, list.contentHeight)))
        list.autoresizingMask = [.width]
        view.needsLayout = true
    }
}

/// The whole empty editor remains a drop target after resizing or deleting FX.
@MainActor
final class DeviceChainScrollView: NSScrollView {
    override func layout() {
        super.layout()
        guard let list = documentView as? DeviceChainListView else { return }
        let size = CGSize(width: contentSize.width, height: max(contentSize.height, list.contentHeight))
        if list.frame.size != size { list.setFrameSize(size) }
    }
}

/// Native controls provide the same operations for keyboard and VoiceOver.
/// Row dragging is an additional entry into the durable chain actions.
@MainActor
final class DeviceChainListView: NSView {
    static let rowHeight = 42.0
    private(set) var devices: [TrackDevice] = []
    private var track: UUID?
    private var document: UUID?
    private var busy = false
    var onEdit: ((DeviceChainEdit) -> Void)?
    var onInsert: ((String) -> Void)?
    var contentHeight: Double { max(Self.rowHeight, Double(devices.count + 1) * Self.rowHeight) }
    override var isFlipped: Bool { true }
    override init(frame: NSRect) {
        super.init(frame: frame)
        registerForDraggedTypes([FXDrag.deviceType, FXDrag.pluginType])
        setAccessibilityElement(false)
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func configure(devices: [TrackDevice], track: UUID, document: UUID, busy: Bool) {
        guard self.devices != devices || self.track != track || self.document != document || self.busy != busy else { return }
        self.devices = devices; self.track = track; self.document = document; self.busy = busy
        subviews.forEach { $0.removeFromSuperview() }
        for (index, device) in devices.enumerated() {
            let row = DeviceChainRow(device: device, payload: FXDrag(document: document, track: track, device: device.id, plugin: nil))
            row.up.isEnabled = !busy && index > 0
            row.down.isEnabled = !busy && index + 1 < devices.count
            row.copy.isEnabled = !busy; row.remove.isEnabled = !busy; row.dragEnabled = !busy
            row.onUp = { [weak self] in self?.onEdit?(.reorder(device: device.id, before: index > 0 ? devices[index - 1].id : device.id)) }
            row.onDown = { [weak self] in self?.onEdit?(.reorder(device: device.id, before: index + 2 < devices.count ? devices[index + 2].id : nil)) }
            row.onCopy = { [weak self] in self?.onEdit?(.copy(source: device.id, track: track, before: index + 1 < devices.count ? devices[index + 1].id : nil)) }
            row.onRemove = { [weak self] in self?.onEdit?(.remove(device.id)) }
            addSubview(row)
        }
        let empty = NSTextField(labelWithString: text(devices.isEmpty ? "No FX on this track" : "Drop FX here"))
        empty.textColor = .secondaryLabelColor; empty.alignment = .center
        addSubview(empty)
        needsLayout = true
    }
    override func layout() {
        super.layout()
        for (index, view) in subviews.enumerated() {
            view.frame = CGRect(x: 8, y: Double(index) * Self.rowHeight, width: max(0, bounds.width - 16), height: Self.rowHeight)
        }
    }
    private func targetBefore(_ point: CGPoint) -> UUID? {
        let index = Int(max(0, (point.y / Self.rowHeight + 0.5).rounded(.down)))
        return index < devices.count ? devices[index].id : nil
    }
    func accept(_ pasteboard: NSPasteboard, at point: CGPoint, copying: Bool) -> Bool {
        guard point.x.isFinite, point.y.isFinite, point.y < Double(Int.max) / 2, !busy, let target = track, let payload = FXDrag.read(pasteboard), payload.document == document else { return false }
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

@MainActor
final class DeviceChainRow: NSView, NSDraggingSource {
    let up = NSButton(), down = NSButton(), copy = NSButton(), remove = NSButton()
    private let label: NSTextField
    private let payload: FXDrag
    private var origin: CGPoint?
    var dragEnabled = true
    var onUp: (() -> Void)?, onDown: (() -> Void)?, onCopy: (() -> Void)?, onRemove: (() -> Void)?
    override var isFlipped: Bool { true }
    init(device: TrackDevice, payload: FXDrag) {
        self.payload = payload; label = NSTextField(labelWithString: device.title)
        super.init(frame: .zero)
        label.lineBreakMode = .byTruncatingMiddle; label.setAccessibilityLabel(device.title); label.toolTip = device.plugin
        addSubview(label)
        for (button, symbol, title, action) in [
            (up, "arrow.up", "Move FX Up", #selector(moveUpFX)), (down, "arrow.down", "Move FX Down", #selector(moveDownFX)),
            (copy, "plus.square.on.square", "Duplicate FX", #selector(copyFX)), (remove, "trash", "Remove FX", #selector(removeFX))
        ] {
            button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: text(title))
            button.imagePosition = .imageOnly; button.bezelStyle = .texturedRounded
            button.target = self; button.action = action; button.toolTip = text(title)
            button.setAccessibilityLabel(text(title) + " " + device.title)
            addSubview(button)
        }
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
    override func layout() {
        super.layout()
        let controls = [up, down, copy, remove]
        let start = max(0, bounds.width - 136)
        label.frame = CGRect(x: 4, y: 11, width: max(0, start - 10), height: 20)
        for (index, control) in controls.enumerated() { control.frame = CGRect(x: start + Double(index) * 34, y: 7, width: 30, height: 28) }
    }
    @objc private func moveUpFX() { onUp?() }
    @objc private func moveDownFX() { onDown?() }
    @objc private func copyFX() { onCopy?() }
    @objc private func removeFX() { onRemove?() }
    override func hitTest(_ point: NSPoint) -> NSView? {
        let hit = super.hitTest(point)
        return hit === label ? self : hit
    }
    override func mouseDown(with event: NSEvent) { origin = event.locationInWindow }
    override func mouseDragged(with event: NSEvent) {
        guard dragEnabled, let origin, hypot(event.locationInWindow.x - origin.x, event.locationInWindow.y - origin.y) >= 4, let data = payload.data else { return }
        self.origin = nil
        let item = NSPasteboardItem(); item.setData(data, forType: payload.type)
        let dragging = NSDraggingItem(pasteboardWriter: item)
        dragging.setDraggingFrame(bounds, contents: NSImage(systemSymbolName: "puzzlepiece.extension", accessibilityDescription: nil))
        beginDraggingSession(with: [dragging], event: event, source: self)
    }
    override func mouseUp(with event: NSEvent) { origin = nil }
    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        context == .withinApplication ? [.copy, .move] : []
    }
    func ignoreModifierKeys(for session: NSDraggingSession) -> Bool { true }
}
