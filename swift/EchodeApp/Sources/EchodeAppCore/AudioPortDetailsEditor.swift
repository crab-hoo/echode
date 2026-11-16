import AppKit
import EchodeBridge

/// A local draft: selecting a layout never queries or changes a plugin.
@MainActor
final class AudioPortDetailsEditor: NSView, NSTextFieldDelegate {
    let kind = NSPopUpButton(frame: .zero, pullsDown: false)
    let channelMap = NSTextField(string: "0,1")
    let ordering = NSPopUpButton(frame: .zero, pullsDown: false)
    let normalization = NSPopUpButton(frame: .zero, pullsDown: false)
    var onChange: (() -> Void)?
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        kind.addItems(withTitles: [text("Audio port automatic layout"), "Surround", "Ambisonic"])
        ordering.addItems(withTitles: ["FuMa", "ACN"])
        ordering.selectItem(at: 1)
        normalization.addItems(withTitles: ["MaxN", "SN3D", "N3D", "SN2D", "N2D"])
        normalization.selectItem(at: 1)
        for (control, label) in [
            (kind, "Audio port layout"), (ordering, "Ambisonic ordering"),
            (normalization, "Ambisonic normalization"),
        ] {
            control.target = self
            control.action = #selector(changed)
            control.setAccessibilityLabel(text(label))
        }
        channelMap.delegate = self
        channelMap.setAccessibilityLabel(text("Surround channel map"))
        channelMap.placeholderString = text("Surround channel map")
        channelMap.toolTip = text("Surround channel map help")
        channelMap.setAccessibilityHelp(text("Surround channel map help"))
        for view in [kind, channelMap, ordering, normalization] { addSubview(view) }
        updateVisibility()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func applying(to port: DeviceAudioPortChange) -> DeviceAudioPortChange? {
        let details: DeviceAudioPortDetails
        switch kind.indexOfSelectedItem {
        case 0: return port
        case 1:
            let parts = channelMap.stringValue.split(separator: ",", omittingEmptySubsequences: false)
            let map = parts.compactMap { part -> UInt8? in
                let value = part.trimmingCharacters(in: .whitespaces)
                guard !value.isEmpty, value.utf8.allSatisfy({ $0 >= 48 && $0 <= 57 }) else { return nil }
                return UInt8(value)
            }
            guard !map.isEmpty, map.count == parts.count, map.count == Int(port.channels) else { return nil }
            details = .surround(channelMap: map)
        case 2:
            guard ordering.indexOfSelectedItem >= 0, normalization.indexOfSelectedItem >= 0 else { return nil }
            details = .ambisonic(
                ordering: UInt32(ordering.indexOfSelectedItem), normalization: UInt32(normalization.indexOfSelectedItem)
            )
        default: return nil
        }
        return DeviceAudioPortChange(input: port.input, index: port.index, channels: port.channels, details: details)
    }

    override func layout() {
        super.layout()
        let selectorWidth = min(150, bounds.width)
        let remaining = max(0, bounds.width - selectorWidth - 6)
        kind.frame = CGRect(x: 0, y: 0, width: selectorWidth, height: 26)
        channelMap.frame = CGRect(x: selectorWidth + 6, y: 2, width: remaining, height: 24)
        ordering.frame = CGRect(x: selectorWidth + 6, y: 0, width: remaining / 2, height: 26)
        normalization.frame = CGRect(x: selectorWidth + 6 + remaining / 2, y: 0, width: remaining / 2, height: 26)
    }

    private func updateVisibility() {
        channelMap.isHidden = kind.indexOfSelectedItem != 1
        ordering.isHidden = kind.indexOfSelectedItem != 2
        normalization.isHidden = ordering.isHidden
    }

    func controlTextDidChange(_ obj: Notification) { onChange?() }
    @objc func changed() {
        updateVisibility()
        onChange?()
    }
}
