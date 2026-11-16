import AppKit
import EchodeBridge

/// A draft only: editing this form never calls a plugin. Apply sends the entire
/// list once, so multi-port changes use one inactive CLAP call and one Undo edit.
@MainActor
final class DeviceAudioPortEditor: NSView {
    let add = NSButton(title: text("Add audio port change"), target: nil, action: nil)
    let apply = NSButton(title: text("Apply audio port changes"), target: nil, action: nil)
    let cancel = NSButton(title: text("Cancel"), target: nil, action: nil)
    private let help = NSTextField(wrappingLabelWithString: text("Audio port changes help"))
    private let scroll = NSScrollView()
    private let contents = AudioPortRows(frame: .zero)
    private(set) var rows: [AudioPortChangeRow] = []
    var onApply: (([DeviceAudioPortChange]) -> Void)?
    var onCancel: (() -> Void)?
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.documentView = contents
        for (button, action) in [
            (add, #selector(addRow)), (apply, #selector(applyChanges)), (cancel, #selector(cancelChanges)),
        ] {
            button.bezelStyle = .rounded
            button.target = self
            button.action = action
        }
        apply.toolTip = text("Audio configuration interruption help")
        apply.setAccessibilityHelp(text("Audio configuration interruption help"))
        for view in [help, scroll, add, apply, cancel] { addSubview(view) }
        addRow()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    var requests: [DeviceAudioPortChange]? {
        let values = rows.compactMap(\.request)
        return !values.isEmpty && values.count == rows.count ? values : nil
    }

    @objc func addRow() {
        let row = AudioPortChangeRow(frame: .zero)
        row.onChange = { [weak self] in self?.validate() }
        row.onRemove = { [weak self, weak row] in
            guard let self, let row else { return }
            self.rows.removeAll { $0 === row }
            row.removeFromSuperview()
            self.validate()
        }
        rows.append(row)
        contents.addSubview(row)
        validate()
    }

    private func validate() {
        apply.isEnabled = requests != nil
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let width = max(0, bounds.width - 24)
        help.frame = CGRect(x: 12, y: 12, width: width, height: 74)
        scroll.frame = CGRect(x: 12, y: 94, width: width, height: max(0, bounds.height - 150))
        contents.frame = CGRect(x: 0, y: 0, width: scroll.contentSize.width, height: Double(rows.count) * 34)
        for (index, row) in rows.enumerated() {
            row.frame = CGRect(x: 0, y: Double(index) * 34, width: contents.bounds.width, height: 32)
        }
        add.frame = CGRect(x: 12, y: bounds.height - 44, width: 150, height: 30)
        cancel.frame = CGRect(x: bounds.width - 230, y: bounds.height - 44, width: 80, height: 30)
        apply.frame = CGRect(x: bounds.width - 146, y: bounds.height - 44, width: 134, height: 30)
    }

    @objc private func applyChanges() {
        guard apply.isEnabled, let requests else { return }
        onApply?(requests)
    }
    @objc private func cancelChanges() { onCancel?() }
}

@MainActor
private final class AudioPortRows: NSView {
    override var isFlipped: Bool { true }
}

@MainActor
final class AudioPortChangeRow: NSView, NSTextFieldDelegate {
    let direction = NSPopUpButton(frame: .zero, pullsDown: false)
    let index = NSTextField(string: "0")
    let channels = NSTextField(string: "2")
    let remove = NSButton(title: text("Remove"), target: nil, action: nil)
    var onChange: (() -> Void)?
    var onRemove: (() -> Void)?
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        direction.addItems(withTitles: [text("Audio port input"), text("Audio port output")])
        direction.setAccessibilityLabel(text("Audio port direction"))
        direction.target = self
        direction.action = #selector(changed)
        for (field, label) in [(index, "Audio port index"), (channels, "Audio port channels")] {
            field.delegate = self
            field.placeholderString = text(label)
            field.toolTip = text(label)
            field.setAccessibilityLabel(text(label))
        }
        remove.bezelStyle = .rounded
        remove.target = self
        remove.action = #selector(removeRow)
        for view in [direction, index, channels, remove] { addSubview(view) }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    var request: DeviceAudioPortChange? {
        func number(_ field: NSTextField) -> UInt32? {
            let value = field.stringValue
            guard !value.isEmpty, value.utf8.allSatisfy({ $0 >= 48 && $0 <= 57 }) else { return nil }
            return UInt32(value)
        }
        guard let index = number(index), let channels = number(channels) else { return nil }
        return DeviceAudioPortChange(input: direction.indexOfSelectedItem == 0, index: index, channels: channels)
    }

    override func layout() {
        super.layout()
        let fieldWidth = max(0, (bounds.width - 196) / 2)
        direction.frame = CGRect(x: 0, y: 2, width: 104, height: 26)
        index.frame = CGRect(x: 110, y: 4, width: fieldWidth, height: 24)
        channels.frame = CGRect(x: 116 + fieldWidth, y: 4, width: fieldWidth, height: 24)
        remove.frame = CGRect(x: bounds.width - 74, y: 2, width: 74, height: 28)
    }

    func controlTextDidChange(_ obj: Notification) { onChange?() }
    @objc private func changed() { onChange?() }
    @objc private func removeRow() { onRemove?() }
}
