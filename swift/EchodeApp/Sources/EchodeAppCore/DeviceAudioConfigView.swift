import AppKit
import EchodeBridge

/// Native, keyboard-accessible preset selection. Enumeration is always an
/// explicit button action because CLAP requires deactivation even for a read.
@MainActor
final class DeviceAudioConfigView: NSView {
    let read = NSButton(title: text("Read audio configurations"), target: nil, action: nil)
    let presets = NSPopUpButton(frame: .zero, pullsDown: false)
    let message = NSTextField(labelWithString: "")
    var onRead: (() -> Void)?
    var onSelect: ((UInt32) -> Void)?
    private var ids: [UInt32] = []
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        read.bezelStyle = .rounded
        read.target = self
        read.action = #selector(readPresets)
        read.toolTip = text("Audio configuration interruption help")
        read.setAccessibilityHelp(text("Audio configuration interruption help"))
        presets.target = self
        presets.action = #selector(selectPreset)
        message.textColor = .secondaryLabelColor
        message.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        message.lineBreakMode = .byTruncatingTail
        for view in [read, presets, message] { addSubview(view) }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func configure(_ state: DeviceAudioConfigState?, title: String, enabled: Bool) {
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
        presets.frame = CGRect(x: 0, y: 26, width: bounds.width, height: 24)
        message.frame = CGRect(x: 0, y: 26, width: bounds.width, height: 24)
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
