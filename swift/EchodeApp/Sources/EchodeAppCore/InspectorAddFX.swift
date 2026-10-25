import AppKit
import SwiftUI

struct InspectorAddFX: NSViewRepresentable {
    let enabled: Bool
    let add: () -> Void

    func makeNSView(context: Context) -> InspectorAddFXButton {
        InspectorAddFXButton()
    }

    func updateNSView(_ button: InspectorAddFXButton, context: Context) {
        button.isEnabled = enabled
        button.add = add
    }
}

@MainActor
final class InspectorAddFXButton: NSButton {
    var add: () -> Void = {}

    init() {
        super.init(frame: .zero)
        title = text("Add FX")
        bezelStyle = .rounded
        setAccessibilityIdentifier("inspector-add-fx")
        target = self
        action = #selector(pressed)
    }

    required init?(coder: NSCoder) { nil }

    @objc private func pressed() {
        guard isEnabled else { return }
        add()
    }
}
