import AppKit
import EchodeBridge

/// Generic host controls for every parameter one running CLAP reports. Only
/// rows on screen exist as views: scrolling reuses them, and a value change
/// refreshes only those rows. The plugin's own GUI is a separate,
/// helper-owned window.
@MainActor
final class DeviceParameterListView: NSView, NSTableViewDataSource, NSTableViewDelegate {
    static let rowHeight = 30.0
    let scroll = ParameterScrollView()
    let table = NSTableView()
    private(set) var state: DeviceParameterState?
    /// Indices into the state's parameters, in display order.
    private(set) var order: [Int] = []
    private var pinned: Set<UInt32> = []
    private var shown: (layout: Int, query: String, pinned: Set<UInt32>)?
    private var locked = false
    var onBegin: ((UInt32) -> Bool)?
    var onChange: ((Double) -> Void)?
    var onEnd: (() -> Void)?
    var onStep: ((UInt32, Double) -> Void)?
    var onPin: ((UInt32, Bool) -> Void)?
    private static let rowID = NSUserInterfaceItemIdentifier("parameter")

    override init(frame: NSRect) {
        super.init(frame: frame)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("value"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.style = .plain
        table.rowHeight = Self.rowHeight
        table.intercellSpacing = .zero
        table.backgroundColor = .clear
        table.selectionHighlightStyle = .none
        table.focusRingType = .none
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.dataSource = self
        table.delegate = self
        table.setAccessibilityLabel(text("Parameters"))
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        addSubview(scroll)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    /// Rebuilds rows only when the listed ids, the query or the pins changed.
    /// Otherwise refreshes the rows on screen.
    func configure(state: DeviceParameterState, pinned: Set<UInt32>, locked: Bool) {
        let key = (state.layout, state.query, pinned)
        self.locked = locked
        if self.state !== state || shown.map({ $0 != key }) ?? true {
            self.state = state
            self.pinned = pinned
            shown = key
            order = state.rows(pinned: pinned)
            table.reloadData()
        } else {
            refreshVisibleRows()
        }
    }

    /// The row views that exist now. Rows scrolled away have none.
    var visibleRows: [DeviceParameterRow] {
        let range = table.rows(in: table.visibleRect)
        guard range.length > 0 else { return [] }
        return (range.location..<range.location + range.length).compactMap {
            table.view(atColumn: 0, row: $0, makeIfNecessary: false) as? DeviceParameterRow
        }
    }

    /// The row for a parameter, scrolling it on screen first.
    func reveal(_ id: UInt32) -> DeviceParameterRow? {
        guard let state, let row = order.firstIndex(where: { state.parameters[$0].id == id }) else { return nil }
        table.scrollRowToVisible(row)
        return table.view(atColumn: 0, row: row, makeIfNecessary: true) as? DeviceParameterRow
    }

    private func refreshVisibleRows() {
        guard let state else { return }
        for row in visibleRows {
            guard let parameter = state.parameter(row.parameter.id) else { continue }
            row.configure(parameter, pinned: pinned.contains(parameter.id), locked: locked)
        }
    }

    override func layout() {
        super.layout()
        scroll.frame = bounds
    }

    func numberOfRows(in tableView: NSTableView) -> Int { order.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let state, order.indices.contains(row) else { return nil }
        let parameter = state.parameters[order[row]]
        let view = tableView.makeView(withIdentifier: Self.rowID, owner: nil) as? DeviceParameterRow ?? {
            let made = DeviceParameterRow(parameter: parameter)
            made.identifier = Self.rowID
            made.onBegin = { [weak self] id in self?.onBegin?(id) ?? false }
            made.onChange = { [weak self] value in self?.onChange?(value) }
            made.onEnd = { [weak self] in self?.onEnd?() }
            made.onStep = { [weak self] id, value in self?.onStep?(id, value) }
            made.onPin = { [weak self] id, value in self?.onPin?(id, value) }
            return made
        }()
        view.configure(parameter, pinned: pinned.contains(parameter.id), locked: locked)
        return view
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }
}

/// A horizontal swipe over a parameter list moves along the device chain.
@MainActor
final class ParameterScrollView: NSScrollView {
    override func scrollWheel(with event: NSEvent) {
        if abs(event.scrollingDeltaX) > abs(event.scrollingDeltaY), let outer = enclosingScrollView {
            outer.scrollWheel(with: event)
        } else {
            super.scrollWheel(with: event)
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

/// One reusable row. A table hands it to a different parameter as rows
/// scroll, so every property that depends on the parameter is set again.
@MainActor
final class DeviceParameterRow: NSView {
    private(set) var parameter: DeviceParameter
    let pin = NSButton()
    let name = NSTextField(labelWithString: "")
    let slider = ParameterSlider()
    let value = NSTextField(labelWithString: "")
    var onBegin: ((UInt32) -> Bool)?
    var onChange: ((Double) -> Void)?
    var onEnd: (() -> Void)?
    var onStep: ((UInt32, Double) -> Void)?
    var onPin: ((UInt32, Bool) -> Void)?
    override var isFlipped: Bool { true }

    init(parameter: DeviceParameter) {
        self.parameter = parameter
        super.init(frame: .zero)
        pin.setButtonType(.pushOnPushOff)
        pin.bezelStyle = .texturedRounded
        pin.isBordered = false
        pin.imagePosition = .imageOnly
        pin.target = self
        pin.action = #selector(togglePin)
        name.lineBreakMode = .byTruncatingTail
        slider.isContinuous = true
        slider.target = self
        slider.action = #selector(moved)
        slider.onBegin = { [weak self] in
            guard let self else { return false }
            return self.onBegin?(self.parameter.id) ?? false
        }
        slider.onEnd = { [weak self] in self?.onEnd?() }
        value.alignment = .right
        value.font = .monospacedDigitSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        value.lineBreakMode = .byTruncatingTail
        for view in [pin, name, slider, value] { addSubview(view) }
        configure(parameter, pinned: false, locked: false)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func configure(_ parameter: DeviceParameter, pinned: Bool, locked: Bool) {
        let same = parameter.id == self.parameter.id
        self.parameter = parameter
        if !same || name.stringValue.isEmpty {
            name.stringValue = parameter.name
            name.toolTip = parameter.module.isEmpty ? parameter.name : parameter.module + " / " + parameter.name
            slider.setAccessibilityLabel(parameter.name)
        }
        slider.minValue = parameter.min
        slider.maxValue = parameter.max
        let steps = parameter.max - parameter.min
        // Tick marks are drawn only when they stay readable; values are still
        // rounded for every stepped parameter.
        if parameter.stepped, steps >= 1, steps <= 64 {
            slider.numberOfTickMarks = Int(steps) + 1
            slider.allowsTickMarkValuesOnly = true
        } else {
            slider.numberOfTickMarks = 0
            slider.allowsTickMarkValuesOnly = false
        }
        if !slider.dragging { slider.doubleValue = parameter.value }
        value.stringValue = parameter.text.isEmpty ? String(format: "%.4g", parameter.value) : parameter.text
        slider.setAccessibilityValueDescription(value.stringValue)
        slider.isEnabled = !parameter.readOnly && (!locked || slider.dragging)
        pin.state = pinned ? .on : .off
        let title = text(pinned ? "Unpin Parameter" : "Pin Parameter")
        pin.image = NSImage(systemSymbolName: pinned ? "pin.fill" : "pin", accessibilityDescription: title)
        pin.toolTip = title
        pin.setAccessibilityLabel(title + " " + parameter.name)
    }

    override func layout() {
        super.layout()
        let pinWidth = 22.0
        let labelWidth = min(140, max(0, bounds.width - pinWidth) * 0.35)
        let valueWidth = min(80, bounds.width * 0.22)
        pin.frame = CGRect(x: 0, y: 5, width: pinWidth, height: 20)
        name.frame = CGRect(x: pinWidth + 2, y: 6, width: labelWidth, height: 18)
        value.frame = CGRect(x: max(0, bounds.width - valueWidth), y: 6, width: valueWidth, height: 18)
        let sliderX = pinWidth + labelWidth + 8
        slider.frame = CGRect(x: sliderX, y: 3, width: max(0, bounds.width - sliderX - valueWidth - 8), height: 24)
    }

    @objc private func moved() {
        if slider.dragging { onChange?(slider.doubleValue) } else { onStep?(parameter.id, slider.doubleValue) }
    }

    @objc private func togglePin() { onPin?(parameter.id, pin.state == .on) }
}
