import AppKit
import SwiftUI

struct SampleAuditionControls: NSViewRepresentable {
    let preferences: SampleAuditionPreferences
    let canPlay: Bool
    let onAutomatic: (Bool) -> Void
    let onGain: (Float) -> Void
    let onPlay: () -> Void
    let onStop: () -> Void

    func makeNSView(context: Context) -> AuditionControlsView { AuditionControlsView() }
    func updateNSView(_ view: AuditionControlsView, context: Context) {
        view.onAutomatic = onAutomatic; view.onGain = onGain
        view.onPlay = onPlay; view.onStop = onStop
        view.show(preferences: preferences, canPlay: canPlay)
    }
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: AuditionControlsView, context: Context) -> CGSize? {
        CGSize(width: proposal.width ?? 200, height: 52)
    }
}

@MainActor
final class AuditionControlsView: NSView {
    let automatic = NSButton(checkboxWithTitle: text("Auto Audition"), target: nil, action: nil)
    let volume = NSSlider(value: 1, minValue: 0, maxValue: 1, target: nil, action: nil)
    let play = NSButton(image: NSImage(systemSymbolName: "play.fill", accessibilityDescription: nil) ?? NSImage(), target: nil, action: nil)
    let stop = NSButton(image: NSImage(systemSymbolName: "stop.fill", accessibilityDescription: nil) ?? NSImage(), target: nil, action: nil)
    var onAutomatic: (Bool) -> Void = { _ in }
    var onGain: (Float) -> Void = { _ in }
    var onPlay: () -> Void = {}
    var onStop: () -> Void = {}

    override var intrinsicContentSize: NSSize { NSSize(width: NSView.noIntrinsicMetric, height: 52) }

    init() {
        super.init(frame: .zero)
        automatic.target = self; automatic.action = #selector(toggleAutomatic)
        volume.target = self; volume.action = #selector(changeGain); volume.isContinuous = true
        volume.setAccessibilityLabel(text("Audition Volume"))
        play.target = self; play.action = #selector(playSelected)
        stop.target = self; stop.action = #selector(stopAudition)
        play.setAccessibilityLabel(text("Audition Selected Sample")); play.toolTip = text("Audition Selected Sample") + " (→)"
        stop.setAccessibilityLabel(text("Stop Audition")); stop.toolTip = text("Stop Audition")
        for button in [automatic, play, stop] { button.controlSize = .small }
        for button in [play, stop] { button.bezelStyle = .inline }
        let actions = NSStackView(views: [automatic, NSView(), play, stop])
        actions.orientation = .horizontal
        let label = NSImageView(image: NSImage(systemSymbolName: "speaker.wave.2", accessibilityDescription: text("Audition Volume")) ?? NSImage())
        label.setAccessibilityElement(false)
        let gain = NSStackView(views: [label, volume])
        gain.orientation = .horizontal
        let stack = NSStackView(views: [actions, gain])
        stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 4
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor), stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor), stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            label.widthAnchor.constraint(equalToConstant: 18),
            actions.widthAnchor.constraint(equalTo: stack.widthAnchor), gain.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
    }

    required init?(coder: NSCoder) { nil }

    func show(preferences: SampleAuditionPreferences, canPlay: Bool) {
        automatic.state = preferences.automatic ? .on : .off
        volume.floatValue = preferences.gain
        volume.toolTip = preferences.gain == 0 ? "−∞ dB" : String(format: "%.1f dB", 20 * log10(preferences.gain))
        play.isEnabled = canPlay
    }

    @objc private func toggleAutomatic() { onAutomatic(automatic.state == .on) }
    @objc private func changeGain() { onGain(volume.floatValue) }
    @objc private func playSelected() { onPlay() }
    @objc private func stopAudition() { onStop() }
}
