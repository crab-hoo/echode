import AppKit
import EchodeBridge
import SwiftUI

/// The plugin candidates. As in the sample list, a press selects at once and
/// takes keyboard focus without waiting for a double click; the second click
/// of a double click inserts and closes, Return inserts and stays (ADR 0162).
struct PluginCandidateList: NSViewRepresentable {
    let plugins: [PluginListing]
    let selection: String?
    let document: DocumentID
    let onSelect: (String?) -> Void
    /// The plugin and whether the panel closes afterwards.
    let onInsert: (String, Bool) -> Void
    let onInsertOnSelected: (String) -> Void
    let canInsertOnSelected: Bool
    func makeNSView(context: Context) -> PluginListView { PluginListView() }
    func updateNSView(_ view: PluginListView, context: Context) {
        view.onSelect = onSelect
        view.table.onInsert = onInsert
        view.table.onInsertOnSelected = onInsertOnSelected
        view.table.canInsertOnSelected = canInsertOnSelected
        view.table.document = document
        view.show(plugins, selection: selection)
    }
}

@MainActor
final class PluginListView: NSScrollView, NSTableViewDataSource, NSTableViewDelegate {
    let table = PluginListTable()
    private(set) var plugins: [PluginListing] = []
    /// Set while the panel moves the selection itself.
    private var applying = false
    var onSelect: (String?) -> Void = { _ in }

    init() {
        super.init(frame: .zero)
        let column = NSTableColumn(identifier: .init("plugin"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.style = .inset
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.allowsMultipleSelection = false
        table.allowsEmptySelection = true
        table.rowHeight = 34
        table.dataSource = self
        table.delegate = self
        table.setAccessibilityLabel(text("Plugins"))
        table.plugin = { [weak self] row in
            guard let self, self.plugins.indices.contains(row) else { return nil }
            return self.plugins[row].id
        }
        documentView = table
        hasVerticalScroller = true
        autohidesScrollers = true
        drawsBackground = false
        borderType = .noBorder
    }

    required init?(coder: NSCoder) { nil }

    /// Rows are rebuilt only when the listed plugins change, not on every
    /// selection or panel update. Never reports a change.
    func show(_ plugins: [PluginListing], selection: String?) {
        applying = true
        defer { applying = false }
        if plugins != self.plugins {
            self.plugins = plugins
            table.reloadData()
        }
        let row = selection.flatMap { id in plugins.firstIndex { $0.id == id } }
        guard table.selectedRow != row ?? -1 else { return }
        if let row {
            table.selectRowIndexes([row], byExtendingSelection: false)
            table.scrollRowToVisible(row)
        } else {
            table.deselectAll(nil)
        }
    }

    func numberOfRows(in tableView: NSTableView) -> Int { plugins.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard plugins.indices.contains(row) else { return nil }
        let cell = tableView.makeView(withIdentifier: PluginRowView.identifier, owner: nil) as? PluginRowView ?? PluginRowView()
        cell.configure(plugins[row])
        return cell
    }

    func tableView(_ tableView: NSTableView, typeSelectStringFor tableColumn: NSTableColumn?, row: Int) -> String? {
        plugins.indices.contains(row) ? plugins[row].name : nil
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !applying else { return }
        let row = table.selectedRow
        onSelect(plugins.indices.contains(row) ? plugins[row].id : nil)
    }
}

@MainActor
final class PluginListTable: NSTableView {
    var onInsert: (String, Bool) -> Void = { _, _ in }
    var onInsertOnSelected: (String) -> Void = { _ in }
    var canInsertOnSelected = false
    var document: DocumentID?
    var plugin: (Int) -> String? = { _ in nil }
    private var dragDown: (NSEvent, String)?

    override var acceptsFirstResponder: Bool { true }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let row = row(at: convert(event.locationInWindow, from: nil))
        guard row >= 0, let id = plugin(row) else { dragDown = nil; deselectAll(nil); return }
        if selectedRow != row { selectRowIndexes([row], byExtendingSelection: false) }
        // Anywhere on the row counts, not only over its text.
        if event.clickCount == 2 { onInsert(id, true) }
        dragDown = event.clickCount == 1 ? (event, id) : nil
    }

    override func mouseDragged(with event: NSEvent) {
        guard let (down, id) = dragDown, let document,
              hypot(event.locationInWindow.x - down.locationInWindow.x, event.locationInWindow.y - down.locationInWindow.y) >= 4,
              let data = FXDrag(document: document, track: nil, device: nil, plugin: id).data else { return }
        dragDown = nil
        let item = NSPasteboardItem(); item.setData(data, forType: FXDrag.pluginType)
        let dragging = NSDraggingItem(pasteboardWriter: item)
        dragging.setDraggingFrame(CGRect(origin: convert(event.locationInWindow, from: nil), size: CGSize(width: 160, height: rowHeight)),
                                  contents: NSImage(systemSymbolName: "puzzlepiece.extension", accessibilityDescription: nil))
        beginDraggingSession(with: [dragging], event: event, source: self)
    }

    override func mouseUp(with event: NSEvent) { dragDown = nil }
    override func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation { .copy }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if window?.firstResponder === self, [36, 76].contains(event.keyCode),
           event.modifierFlags.isDisjoint(with: [.command, .option, .control, .shift]) {
            keyDown(with: event)
            return true
        }
        return super.performKeyEquivalent(with: event)
    }

    override func keyDown(with event: NSEvent) {
        // Return and keypad Enter insert and keep the panel open.
        if [36, 76].contains(event.keyCode), event.modifierFlags.isDisjoint(with: [.command, .option, .control, .shift]) {
            if let id = plugin(selectedRow) { onInsert(id, false) }
            return
        }
        super.keyDown(with: event)
    }

    /// The pressed row is selected, then offered for every selected track.
    override func menu(for event: NSEvent) -> NSMenu? {
        let row = row(at: convert(event.locationInWindow, from: nil))
        guard let id = plugin(row) else { return nil }
        if selectedRow != row { selectRowIndexes([row], byExtendingSelection: false) }
        let menu = NSMenu()
        let item = NSMenuItem(title: text("Insert on selected tracks"), action: canInsertOnSelected ? #selector(insertOnSelected(_:)) : nil, keyEquivalent: "")
        item.target = self
        item.representedObject = id
        item.isEnabled = canInsertOnSelected
        menu.autoenablesItems = false
        menu.addItem(item)
        return menu
    }

    @objc private func insertOnSelected(_ sender: NSMenuItem) {
        if let id = sender.representedObject as? String { onInsertOnSelected(id) }
    }
}

final class PluginRowView: NSTableCellView {
    static let identifier = NSUserInterfaceItemIdentifier("plugin-row")
    private let name = NSTextField(labelWithString: "")
    private let fresh = NSTextField(labelWithString: text("New plugin"))
    private let detail = NSTextField(labelWithString: "")

    init() {
        super.init(frame: .zero)
        identifier = Self.identifier
        textField = name
        name.lineBreakMode = .byTruncatingTail
        for label in [fresh, detail] {
            label.font = .preferredFont(forTextStyle: .caption1)
            label.textColor = .secondaryLabelColor
        }
        detail.lineBreakMode = .byTruncatingTail
        let title = NSStackView(views: [name, fresh])
        title.orientation = .horizontal
        title.spacing = 6
        let stack = NSStackView(views: [title, detail])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 2
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 2),
            stack.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -2),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    required init?(coder: NSCoder) { nil }

    func configure(_ plugin: PluginListing) {
        name.stringValue = plugin.name
        fresh.isHidden = !plugin.fresh
        detail.stringValue = [plugin.vendor, plugin.id].filter { !$0.isEmpty }.joined(separator: " · ")
        setAccessibilityLabel([plugin.name, plugin.fresh ? text("New plugin") : nil, detail.stringValue].compactMap { $0 }.joined(separator: ", "))
    }
}
