import AppKit
import EchodeBridge
import SwiftUI

struct PianoTrackSelector: NSViewRepresentable {
    let controller: ProjectController
    let rows: [PianoTrackRow]
    func makeNSView(context: Context) -> PianoTrackListView { PianoTrackListView() }
    func updateNSView(_ view: PianoTrackListView, context: Context) { view.show(controller, rows: rows) }
}

struct PianoTrackRow: Equatable {
    let target: PianoTrackTarget
    let name: String
    let visible: Bool
    let editing: Bool
    let solo: Bool
    let mute: Bool
    let enabled: Bool
}

@MainActor
final class PianoTrackListView: NSScrollView, NSTableViewDataSource, NSTableViewDelegate {
    let table = NSTableView()
    private(set) var rows: [PianoTrackRow] = []
    private weak var controller: ProjectController?

    init() {
        super.init(frame: .zero)
        table.addTableColumn(NSTableColumn(identifier: .init("piano.tracks")))
        table.headerView = nil
        table.rowHeight = 54
        table.allowsEmptySelection = true
        table.selectionHighlightStyle = .none
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.dataSource = self; table.delegate = self
        table.setAccessibilityLabel(text("Piano Tracks"))
        documentView = table
        hasVerticalScroller = true; autohidesScrollers = true
        drawsBackground = false; borderType = .noBorder
    }
    required init?(coder: NSCoder) { nil }

    func show(_ controller: ProjectController, rows suppliedRows: [PianoTrackRow]? = nil) {
        self.controller = controller
        let rows = suppliedRows ?? controller.pianoTrackRows
        guard rows != self.rows else { return }
        self.rows = rows
        table.reloadData()
    }
    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }
    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard rows.indices.contains(row) else { return nil }
        let cell = tableView.makeView(withIdentifier: PianoTrackCell.identifier, owner: nil) as? PianoTrackCell ?? PianoTrackCell()
        cell.show(rows[row], controller: controller)
        return cell
    }
}

@MainActor
final class PianoTrackCell: NSTableCellView {
    static let identifier = NSUserInterfaceItemIdentifier("piano.track")
    let name = NSTextField(labelWithString: "")
    let reference = NSButton(), edit = NSButton(), solo = NSButton(), mute = NSButton()
    private var row: PianoTrackRow?
    private weak var controller: ProjectController?
    private(set) var pending: Task<Bool, Never>?

    init() {
        super.init(frame: .zero)
        identifier = Self.identifier
        name.lineBreakMode = .byTruncatingTail
        let buttons = [reference, edit, solo, mute]
        for (button, title) in zip(buttons, ["Show", "Edit", "Solo", "Mute"]) {
            button.title = text(title)
            button.setButtonType(.pushOnPushOff)
            button.bezelStyle = .accessoryBarAction
            button.controlSize = .small
            button.target = self; button.action = #selector(activate(_:))
        }
        let controls = NSStackView(views: buttons)
        controls.spacing = 3
        let stack = NSStackView(views: [name, controls])
        stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 3
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 4),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }
    required init?(coder: NSCoder) { nil }

    func show(_ row: PianoTrackRow, controller: ProjectController?) {
        self.row = row; self.controller = controller
        name.stringValue = row.name
        for (button, title, active) in [(reference, "Show", row.visible), (edit, "Edit", row.editing),
                                        (solo, "Solo", row.solo), (mute, "Mute", row.mute)] {
            button.state = active ? .on : .off
            button.isEnabled = row.enabled
            button.setAccessibilityLabel("\(text(title)) \(row.name)")
            button.setAccessibilityIdentifier("piano.track.\(title.lowercased()).\(row.target.track)")
        }
        reference.toolTip = text("Show reference notes help")
        edit.toolTip = text("Edit piano track help")
    }

    @objc private func activate(_ button: NSButton) {
        guard let row, row.enabled, let controller, !controller.busy,
              controller.documentID == row.target.document,
              controller.tracks.contains(where: { $0.id == row.target.track }) else { return }
        if button === reference {
            controller.setPianoReference(row.target, visible: controller.hiddenPianoTracks.contains(row.target.track))
        } else if button === edit {
            _ = controller.openPiano(row.target)
        } else if button === solo || button === mute {
            let field: MixerField = button === solo ? .solo : .mute
            pending = Task { await controller.togglePianoMixer(row.target, field: field) }
        }
    }
}
