import AppKit
import EchodeBridge

/// Note values, in quarter-note ticks. Grid choice is viewport state,
/// not a quantization of existing notes or of the remembered input length.
enum PianoGrid: Sendable, Equatable {
    case automatic
    case fixed(Int64)

    static let divisions: [Int64] = (0..<8).map { (4 * Tick.perQuarter) / (1 << $0) }
    static let triplets = divisions.map { $0 * 2 / 3 }
    static let dotted = divisions.map { $0 * 3 / 2 }
    static let choices: [PianoGrid] = [.automatic] + (divisions + triplets + dotted).map { .fixed($0) }

    func ticks(pixelsPerBeat: Double) -> Int64 {
        switch self {
        case .fixed(let ticks): return ticks
        case .automatic:
            guard pixelsPerBeat.isFinite, pixelsPerBeat > 0 else { return Tick.perQuarter / 4 }
            // Choose the finest straight division with at least 18pt between lines.
            return Self.divisions.reversed().first { Double($0) / Tick.perBeat * pixelsPerBeat >= 18 }
                ?? Self.divisions[0]
        }
    }

    var title: String {
        switch self {
        case .automatic: text("Automatic Grid")
        case .fixed(let ticks): Self.noteValue(ticks)
        }
    }

    static func noteValue(_ ticks: Int64) -> String {
        for (values, suffix) in [(divisions, ""), (triplets, "Triplet"), (dotted, "Dotted")] {
            if let index = values.firstIndex(of: ticks) {
                let base = "1/\(1 << index)"
                return suffix.isEmpty ? base : "\(base) \(text(suffix))"
            }
        }
        return "\(ticks) ticks"
    }
}

/// Native keyboard/VoiceOver-accessible controls, outside the Metal surface.
@MainActor
final class PianoGridBar: NSView {
    static let height = 28.0
    let picker = NSPopUpButton(frame: .zero, pullsDown: false)
    private let value = NSTextField(labelWithString: "")
    var onChange: ((PianoGrid) -> Void)?
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        clipsToBounds = true
        picker.addItems(withTitles: PianoGrid.choices.map(\.title))
        picker.controlSize = .small
        picker.identifier = NSUserInterfaceItemIdentifier("piano-grid")
        picker.setAccessibilityLabel(text("Note Grid"))
        picker.setAccessibilityHelp(text("Note grid help"))
        picker.target = self
        picker.action = #selector(changed)
        value.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        value.textColor = .secondaryLabelColor
        addSubview(picker)
        addSubview(value)
        show(PianoCamera())
    }
    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func layout() {
        super.layout()
        picker.frame = NSRect(x: 6, y: 3, width: min(170, max(0, bounds.width - 12)), height: 22)
        value.frame = NSRect(x: 184, y: 6, width: max(0, bounds.width - 190), height: 18)
    }

    func show(_ camera: PianoCamera) {
        if let index = PianoGrid.choices.firstIndex(of: camera.gridMode) { picker.selectItem(at: index) }
        value.stringValue = PianoGrid.noteValue(camera.grid)
    }

    @objc private func changed() {
        let index = picker.indexOfSelectedItem
        guard PianoGrid.choices.indices.contains(index) else { return }
        onChange?(PianoGrid.choices[index])
    }
}
