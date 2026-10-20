import AppKit
import EchodeBridge
import SwiftUI

/// Generic host controls for the parameters a running CLAP reports. The
/// plugin's own GUI is a separate, helper-owned window.
struct WorkspaceParameterView: View {
    let controller: ProjectController
    let devices: [TrackDevice]
    var body: some View {
        if let panel = controller.parameterPanel, panel.document == controller.documentID,
           let device = devices.first(where: { $0.id == panel.device }) {
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Text(text("Parameters")).font(.headline)
                    Text(device.title).foregroundStyle(.secondary).lineLimit(1).help(device.plugin)
                    Spacer()
                    Button(text("Close Parameters")) { controller.closeParameters() }
                }.padding(10)
                Divider()
                if panel.loading {
                    ProgressView(text("Loading parameters")).frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if let error = panel.error {
                    Text(error).foregroundStyle(.secondary).padding(10).frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if panel.parameters.isEmpty {
                    Text(text("No editable parameters")).foregroundStyle(.secondary).frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    DeviceParameterCanvas(parameters: panel.parameters, busy: controller.busy,
                        begin: { controller.beginParameter($0) },
                        change: { controller.changeParameter($0) },
                        end: { Task { await controller.finishParameter(commit: true) } },
                        step: { id, value in Task { await controller.setParameter(id, value: value) } })
                }
            }
        } else {
            Text(text("Choose parameters help")).foregroundStyle(.secondary).multilineTextAlignment(.center)
                .padding(10).frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

private struct DeviceParameterCanvas: NSViewRepresentable {
    let parameters: [DeviceParameter]
    let busy: Bool
    let begin: (UInt32) -> Bool
    let change: (Double) -> Void
    let end: () -> Void
    let step: (UInt32, Double) -> Void
    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true; scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.documentView = DeviceParameterListView(frame: .zero)
        return scroll
    }
    func updateNSView(_ view: NSScrollView, context: Context) {
        guard let list = view.documentView as? DeviceParameterListView else { return }
        list.onBegin = begin; list.onChange = change; list.onEnd = end; list.onStep = step
        list.configure(parameters: parameters, busy: busy)
        list.setFrameSize(CGSize(width: view.contentSize.width, height: max(view.contentSize.height, list.contentHeight)))
        list.autoresizingMask = [.width]
    }
}

/// Rows stay in place while their values change, so a slider keeps tracking
/// the pointer while the plugin reports each applied value.
@MainActor
final class DeviceParameterListView: NSView {
    static let rowHeight = 30.0
    private(set) var rows: [DeviceParameterRow] = []
    var onBegin: ((UInt32) -> Bool)?
    var onChange: ((Double) -> Void)?
    var onEnd: (() -> Void)?
    var onStep: ((UInt32, Double) -> Void)?
    var contentHeight: Double { Double(rows.count) * Self.rowHeight + 8 }
    override var isFlipped: Bool { true }

    func configure(parameters: [DeviceParameter], busy: Bool) {
        if rows.map(\.parameter.id) == parameters.map(\.id) {
            for (row, parameter) in zip(rows, parameters) { row.update(parameter, busy: busy) }
            return
        }
        rows.forEach { $0.removeFromSuperview() }
        rows = parameters.map { parameter in
            let row = DeviceParameterRow(parameter: parameter)
            row.onBegin = { [weak self] id in self?.onBegin?(id) ?? false }
            row.onChange = { [weak self] value in self?.onChange?(value) }
            row.onEnd = { [weak self] in self?.onEnd?() }
            row.onStep = { [weak self] id, value in self?.onStep?(id, value) }
            row.update(parameter, busy: busy)
            addSubview(row)
            return row
        }
        needsLayout = true
    }

    override func layout() {
        super.layout()
        for (index, row) in rows.enumerated() {
            row.frame = CGRect(x: 8, y: 4 + Double(index) * Self.rowHeight, width: max(0, bounds.width - 16), height: Self.rowHeight)
        }
    }
}

/// Reports the start and end of a pointer drag around AppKit's tracking loop.
@MainActor
final class ParameterSlider: NSSlider {
    var onBegin: (() -> Bool)?
    var onEnd: (() -> Void)?
    private(set) var dragging = false
    override func mouseDown(with event: NSEvent) {
        beginDrag()
        super.mouseDown(with: event)
        endDrag()
    }
    /// Actions sent between these calls are drag values, not single steps.
    func beginDrag() { dragging = onBegin?() ?? false }
    func endDrag() {
        guard dragging else { return }
        dragging = false
        onEnd?()
    }
}

@MainActor
final class DeviceParameterRow: NSView {
    private(set) var parameter: DeviceParameter
    let name: NSTextField
    let slider = ParameterSlider()
    let value = NSTextField(labelWithString: "")
    var onBegin: ((UInt32) -> Bool)?
    var onChange: ((Double) -> Void)?
    var onEnd: (() -> Void)?
    var onStep: ((UInt32, Double) -> Void)?
    override var isFlipped: Bool { true }

    init(parameter: DeviceParameter) {
        self.parameter = parameter
        name = NSTextField(labelWithString: parameter.name)
        super.init(frame: .zero)
        name.lineBreakMode = .byTruncatingTail
        name.toolTip = parameter.module.isEmpty ? parameter.name : parameter.module + " / " + parameter.name
        slider.minValue = parameter.min
        slider.maxValue = parameter.max
        let steps = parameter.max - parameter.min
        // Tick marks are drawn only when they stay readable; values are still
        // rounded for every stepped parameter.
        if parameter.stepped, steps >= 1, steps <= 64 {
            slider.numberOfTickMarks = Int(steps) + 1
            slider.allowsTickMarkValuesOnly = true
        }
        slider.isContinuous = true
        slider.target = self
        slider.action = #selector(moved)
        slider.onBegin = { [weak self] in
            guard let self else { return false }
            return self.onBegin?(self.parameter.id) ?? false
        }
        slider.onEnd = { [weak self] in self?.onEnd?() }
        slider.setAccessibilityLabel(parameter.name)
        value.alignment = .right
        value.font = .monospacedDigitSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        value.lineBreakMode = .byTruncatingTail
        for view in [name, slider, value] { addSubview(view) }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func update(_ parameter: DeviceParameter, busy: Bool) {
        self.parameter = parameter
        if !slider.dragging { slider.doubleValue = parameter.value }
        value.stringValue = parameter.text.isEmpty ? String(format: "%.4g", parameter.value) : parameter.text
        slider.setAccessibilityValueDescription(value.stringValue)
        slider.isEnabled = !parameter.readOnly && (!busy || slider.dragging)
    }

    override func layout() {
        super.layout()
        let labelWidth = min(160, bounds.width * 0.3)
        let valueWidth = min(96, bounds.width * 0.2)
        name.frame = CGRect(x: 0, y: 6, width: labelWidth, height: 18)
        value.frame = CGRect(x: max(0, bounds.width - valueWidth), y: 6, width: valueWidth, height: 18)
        slider.frame = CGRect(x: labelWidth + 8, y: 3, width: max(0, bounds.width - labelWidth - valueWidth - 16), height: 24)
    }

    @objc private func moved() {
        if slider.dragging { onChange?(slider.doubleValue) } else { onStep?(parameter.id, slider.doubleValue) }
    }
}
