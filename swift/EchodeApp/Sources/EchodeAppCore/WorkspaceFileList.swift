import AppKit
import EchodeBridge
import SwiftUI

/// The browser's rows. A click selects and takes keyboard focus on mouse
/// down, without waiting to see whether a double click follows; the second
/// click of a double click opens the row (ADR 0143).
struct SampleFileList: NSViewRepresentable {
    let files: [BrowserFile]
    let searching: Bool
    let selection: URL?
    let onSelect: (URL?, AuditionOrigin) -> Void
    let onOpen: (BrowserFile) -> Void
    var onAudition: (AuditionOrigin) -> Void = { _ in }
    func makeNSView(context: Context) -> FileListView { FileListView() }
    func updateNSView(_ view: FileListView, context: Context) {
        view.onSelect = onSelect
        view.onOpen = onOpen
        view.onAudition = onAudition
        view.show(files, searching: searching, selection: selection)
    }
}

@MainActor
final class FileListView: NSScrollView, NSTableViewDataSource, NSTableViewDelegate {
    let table = FileListTable()
    private(set) var files: [BrowserFile] = []
    private var searching = false
    /// Set while the browser moves the selection itself, which is not the user's choice.
    private var applying = false
    var onSelect: (URL?, AuditionOrigin) -> Void = { _, _ in }
    var onOpen: (BrowserFile) -> Void = { _ in }
    var onAudition: (AuditionOrigin) -> Void = { _ in }

    init() {
        super.init(frame: .zero)
        let column = NSTableColumn(identifier: .init("file"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.style = .inset
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.allowsMultipleSelection = false
        table.allowsEmptySelection = true
        table.rowHeight = 22
        table.dataSource = self
        table.delegate = self
        table.setAccessibilityLabel(text("Files"))
        table.onOpen = { [weak self] row in
            guard let self, self.files.indices.contains(row) else { return }
            self.onOpen(self.files[row])
        }
        table.onAudition = { [weak self] origin in self?.onAudition(origin) }
        table.dragURL = { [weak self] row in
            guard let self, self.files.indices.contains(row), self.files[row].audio else { return nil }
            return self.files[row].url
        }
        documentView = table
        hasVerticalScroller = true
        autohidesScrollers = true
        drawsBackground = false
        borderType = .noBorder
    }

    required init?(coder: NSCoder) { nil }

    /// Shows `files` and the browser's selection. Never reports a change.
    func show(_ files: [BrowserFile], searching: Bool, selection: URL?) {
        applying = true
        defer { applying = false }
        if files.map(\.url) != self.files.map(\.url) || searching != self.searching {
            self.files = files
            self.searching = searching
            table.rowHeight = searching ? 34 : 22
            table.reloadData()
        }
        let row = selection.flatMap { url in files.firstIndex { $0.url == url } }
        guard table.selectedRow != row ?? -1 else { return }
        if let row {
            table.selectRowIndexes([row], byExtendingSelection: false)
            table.scrollRowToVisible(row)
        } else {
            table.deselectAll(nil)
        }
    }

    func numberOfRows(in tableView: NSTableView) -> Int { files.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard files.indices.contains(row) else { return nil }
        let cell = tableView.makeView(withIdentifier: FileRowView.identifier, owner: nil) as? FileRowView ?? FileRowView()
        cell.configure(files[row], showingFolder: searching)
        return cell
    }

    /// Typing a name selects the row, as a key input.
    func tableView(_ tableView: NSTableView, typeSelectStringFor tableColumn: NSTableColumn?, row: Int) -> String? {
        files.indices.contains(row) ? files[row].url.lastPathComponent : nil
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !applying else { return }
        let row = table.selectedRow
        onSelect(files.indices.contains(row) ? files[row].url : nil, table.input ?? .other)
    }
}

@MainActor
final class FileListTable: NSTableView {
    /// The click or key being handled, which a selection change reports.
    fileprivate(set) var input: AuditionOrigin?
    var onOpen: (Int) -> Void = { _ in }
    var onAudition: (AuditionOrigin) -> Void = { _ in }
    var dragURL: (Int) -> URL? = { _ in nil }
    private var dragDown: (NSEvent, any NSPasteboardWriting)?

    func pasteboardWriter(for row: Int) -> NSPasteboardWriting? { dragURL(row).map { $0 as NSURL } }

    override var acceptsFirstResponder: Bool { true }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let row = row(at: convert(event.locationInWindow, from: nil))
        input = AuditionOrigin(input: .click, delayMicroseconds: Self.delay(event))
        defer { input = nil }
        guard row >= 0 else { dragDown = nil; deselectAll(nil); return }
        if selectedRow != row { selectRowIndexes([row], byExtendingSelection: false) }
        if event.clickCount == 2 { onOpen(row) }
        dragDown = event.clickCount == 1 ? pasteboardWriter(for: row).map { (event, $0) } : nil
    }

    override func mouseDragged(with event: NSEvent) {
        guard let (down, writer) = dragDown,
              hypot(event.locationInWindow.x - down.locationInWindow.x, event.locationInWindow.y - down.locationInWindow.y) >= 4 else { return }
        dragDown = nil
        let item = NSDraggingItem(pasteboardWriter: writer)
        item.setDraggingFrame(CGRect(origin: convert(event.locationInWindow, from: nil), size: CGSize(width: 120, height: rowHeight)), contents: NSImage(systemSymbolName: "waveform", accessibilityDescription: nil))
        beginDraggingSession(with: [item], event: event, source: self)
    }

    override func mouseUp(with event: NSEvent) { dragDown = nil }
    override func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation { .copy }

    override func keyDown(with event: NSEvent) {
        if event.keyCode == 124, event.modifierFlags.isDisjoint(with: [.command, .option, .control, .shift]) {
            if !event.isARepeat { onAudition(AuditionOrigin(input: .key, delayMicroseconds: Self.delay(event))) }
            return
        }
        // Return and keypad Enter open the row, as a double click does.
        if [36, 76].contains(event.keyCode), event.modifierFlags.isDisjoint(with: [.command, .option, .control, .shift]) {
            if selectedRow >= 0 { onOpen(selectedRow) }
            return
        }
        input = AuditionOrigin(input: .key, delayMicroseconds: Self.delay(event))
        defer { input = nil }
        super.keyDown(with: event)
    }

    /// From the event to now, on the uptime clock that stamps events.
    static func delay(_ event: NSEvent) -> UInt64 {
        UInt64(max(0, ProcessInfo.processInfo.systemUptime - event.timestamp) * 1_000_000)
    }
}

final class FileRowView: NSTableCellView {
    static let identifier = NSUserInterfaceItemIdentifier("file-row")
    private let icon = NSImageView()
    private let name = NSTextField(labelWithString: "")
    private let folder = NSTextField(labelWithString: "")

    init() {
        super.init(frame: .zero)
        identifier = Self.identifier
        imageView = icon
        textField = name
        name.lineBreakMode = .byTruncatingMiddle
        folder.lineBreakMode = .byTruncatingHead
        folder.font = .preferredFont(forTextStyle: .caption1)
        folder.textColor = .secondaryLabelColor
        let text = NSStackView(views: [name, folder])
        text.orientation = .vertical
        text.alignment = .leading
        text.spacing = 0
        let row = NSStackView(views: [icon, text])
        row.orientation = .horizontal
        row.spacing = 6
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        NSLayoutConstraint.activate([
            row.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 2),
            row.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -2),
            row.centerYAnchor.constraint(equalTo: centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 16),
        ])
    }

    required init?(coder: NSCoder) { nil }

    func configure(_ file: BrowserFile, showingFolder: Bool) {
        icon.image = NSImage(systemSymbolName: file.icon, accessibilityDescription: nil)
        name.stringValue = file.url.lastPathComponent + (file.midi ? " — " + text("MIDI import not available yet") : "")
        name.textColor = file.midi ? .secondaryLabelColor : .labelColor
        folder.stringValue = file.url.deletingLastPathComponent().path
        folder.isHidden = !showingFolder
        toolTip = file.url.path
    }
}
