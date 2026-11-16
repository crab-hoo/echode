import AppKit
import EchodeBridge

/// Native, keyboard-accessible preset selection. Enumeration is always an
/// explicit button action because CLAP requires deactivation even for a read.
@MainActor
final class DeviceAudioConfigView: NSView {
    let read = NSButton(title: text("Read audio configurations"), target: nil, action: nil)
    let change = NSButton(title: text("Change audio ports"), target: nil, action: nil)
    let presets = NSPopUpButton(frame: .zero, pullsDown: false)
    let message = NSTextField(labelWithString: "")
    var onRead: (() -> Void)?
    var onSelect: ((UInt32) -> Void)?
    var onApplyPorts: (([DeviceAudioPortChange]) -> Void)?
    private(set) var portEditor: DeviceAudioPortEditor?
    private let popover = NSPopover()
    private weak var state: DeviceAudioConfigState?
    private var revision: Int?
    private var ids: [UInt32] = []
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        read.bezelStyle = .rounded
        read.target = self
        read.action = #selector(readPresets)
        read.toolTip = text("Audio configuration interruption help")
        read.setAccessibilityHelp(text("Audio configuration interruption help"))
        change.bezelStyle = .rounded
        change.target = self
        change.action = #selector(changePorts)
        change.toolTip = text("Audio configuration interruption help")
        change.setAccessibilityHelp(text("Audio configuration interruption help"))
        popover.behavior = .transient
        presets.target = self
        presets.action = #selector(selectPreset)
        message.textColor = .secondaryLabelColor
        message.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        message.lineBreakMode = .byTruncatingTail
        for view in [read, presets, message, change] { addSubview(view) }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func configure(_ state: DeviceAudioConfigState?, title: String, enabled: Bool) {
        if self.state !== state || revision != state?.revision || !enabled || state?.busy == true {
            popover.performClose(nil)
            portEditor = nil
        }
        self.state = state
        revision = state?.revision
        change.isEnabled = enabled && state != nil && state?.busy != true
        change.setAccessibilityLabel(text("Change audio ports") + " " + title)
        read.isEnabled = enabled && state != nil && state?.busy != true
        read.title = text(state?.busy == true ? "Reading audio configurations" : "Read audio configurations")
        read.setAccessibilityLabel(read.title + " " + title)
        ids = state?.list?.configs.map(\.id) ?? []
        presets.removeAllItems()
        presets.addItem(withTitle: text("Choose audio configuration"))
        for config in state?.list?.configs ?? [] {
            // Include the id: plugins may give several presets the same name.
            presets.addItem(withTitle: "\(config.name) [\(config.id)]")
        }
        if let current = state?.list?.currentID, let index = ids.firstIndex(of: current) {
            presets.selectItem(at: index + 1)
        } else {
            presets.selectItem(at: 0)
        }
        presets.isHidden = ids.isEmpty
        presets.isEnabled = enabled && state?.busy != true && !ids.isEmpty
        presets.setAccessibilityLabel(text("Choose audio configuration") + " " + title)
        message.stringValue =
            state?.error ?? (state?.list?.configs.isEmpty == true ? text("No audio configurations") : "")
        message.toolTip = message.stringValue
        message.setAccessibilityLabel(message.stringValue + " " + title)
        message.isHidden = message.stringValue.isEmpty
    }

    override func layout() {
        super.layout()
        read.frame = CGRect(x: 0, y: 0, width: bounds.width, height: 24)
        change.frame = CGRect(x: 0, y: 52, width: bounds.width, height: 24)
        presets.frame = CGRect(x: 0, y: 26, width: bounds.width, height: 24)
        message.frame = CGRect(x: 0, y: 26, width: bounds.width, height: 24)
    }

    @objc private func changePorts() {
        guard change.isEnabled, let state else { return }
        let revision = state.revision
        let editor = DeviceAudioPortEditor(frame: CGRect(x: 0, y: 0, width: 500, height: 320))
        let action = onApplyPorts
        editor.onApply = { [weak self, weak state, weak editor] requests in
            guard let self, let state, let editor, self.portEditor === editor, self.state === state,
                state.revision == revision, self.change.isEnabled
            else { return }
            self.popover.performClose(nil)
            self.portEditor = nil
            action?(requests)
        }
        editor.onCancel = { [weak self, weak editor] in
            guard let self, let editor, self.portEditor === editor else { return }
            self.popover.performClose(nil)
            self.portEditor = nil
        }
        portEditor = editor
        let controller = NSViewController()
        controller.view = editor
        popover.contentViewController = controller
        popover.contentSize = editor.frame.size
        if window != nil { popover.show(relativeTo: change.bounds, of: change, preferredEdge: .maxY) }
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        if newWindow == nil {
            popover.performClose(nil)
            portEditor = nil
        }
        super.viewWillMove(toWindow: newWindow)
    }

    @objc private func readPresets() {
        guard read.isEnabled else { return }
        onRead?()
    }

    @objc private func selectPreset() {
        let index = presets.indexOfSelectedItem - 1
        guard presets.isEnabled, ids.indices.contains(index) else { return }
        onSelect?(ids[index])
    }
}
