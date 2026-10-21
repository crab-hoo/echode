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
    func makeNSView(context: Context) -> FileListView { FileListView() }
    func updateNSView(_ view: FileListView, context: Context) {
        view.onSelect = onSelect
        view.onOpen = onOpen
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

    override var acceptsFirstResponder: Bool { true }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let row = row(at: convert(event.locationInWindow, from: nil))
        input = AuditionOrigin(input: .click, delayMicroseconds: Self.delay(event))
        defer { input = nil }
        guard row >= 0 else { deselectAll(nil); return }
        if selectedRow != row { selectRowIndexes([row], byExtendingSelection: false) }
        if event.clickCount == 2 { onOpen(row) }
    }

    override func keyDown(with event: NSEvent) {
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
        name.stringValue = file.url.lastPathComponent
        folder.stringValue = file.url.deletingLastPathComponent().path
        folder.isHidden = !showingFolder
        toolTip = file.url.path
    }
}
