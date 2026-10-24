import AppKit
import EchodeBridge
import SwiftUI

/// The selected file's overview; a late status for another file stays hidden.
struct SamplePreview: NSViewRepresentable {
    let url: URL
    let status: AuditionStatus?
    var onSeek: (UInt64, AuditionOrigin) -> Void = { _, _ in }

    func makeNSView(context: Context) -> SamplePreviewView { SamplePreviewView() }
    func updateNSView(_ view: SamplePreviewView, context: Context) {
        view.onSeek = onSeek
        view.show(url: url, status: status)
    }
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: SamplePreviewView, context: Context) -> CGSize? {
        CGSize(width: proposal.width ?? 200, height: 86)
    }
}

@MainActor
final class SamplePreviewView: NSView {
    private(set) var waveform: AuditionWaveform?
    private let label = NSTextField(labelWithString: "")
    private let detail = NSTextField(labelWithString: "")
    var onSeek: (UInt64, AuditionOrigin) -> Void = { _, _ in }
    private var startFrame: UInt64 = 0
    private var plot: NSRect { NSRect(x: 0, y: 20, width: bounds.width, height: 44) }

    override var isFlipped: Bool { true }
    override var intrinsicContentSize: NSSize { NSSize(width: NSView.noIntrinsicMetric, height: 86) }

    init() {
        super.init(frame: .zero)
        label.font = .preferredFont(forTextStyle: .caption1)
        label.lineBreakMode = .byTruncatingMiddle
        detail.font = .monospacedDigitSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        detail.textColor = .secondaryLabelColor
        addSubview(label)
        addSubview(detail)
        setAccessibilityElement(true)
        setAccessibilityRole(.button)
        setAccessibilityLabel(text("Sample waveform"))
        setAccessibilityHelp(text("Click waveform to audition from here"))
    }

    required init?(coder: NSCoder) { nil }

    func show(url: URL, status: AuditionStatus?) {
        let current = status?.path == url.path ? status : nil
        waveform = current?.waveform
        startFrame = current?.history.last?.startFrame ?? 0
        label.stringValue = url.lastPathComponent
        if let error = current?.error {
            detail.stringValue = text("Sample preview unavailable")
            toolTip = error
        } else if let waveform, waveform.sampleRate > 0 {
            let seconds = Double(waveform.frames) / Double(waveform.sampleRate)
            let duration = String(format: "%.2f s", seconds)
            detail.stringValue = !waveform.complete ? "\(duration) · \(text("Loading sample"))"
                : (waveform.firstFrame ?? 0) > 0 ? "\(duration) · \(text("Partial waveform"))" : duration
            toolTip = nil
        } else {
            detail.stringValue = text(current?.phase == .loading ? "Loading sample" : "Preview not loaded")
            toolTip = nil
        }
        setAccessibilityValue("\(label.stringValue), \(detail.stringValue)")
        needsLayout = true
        needsDisplay = true
    }

    override func layout() {
        super.layout()
        label.frame = NSRect(x: 0, y: 0, width: bounds.width, height: 16)
        detail.frame = NSRect(x: 0, y: 68, width: bounds.width, height: 16)
    }

    /// The same geometry is drawn below and inspected without a window in tests.
    func bars(in plot: NSRect) -> [NSRect] {
        guard let waveform, waveform.frames > 0, waveform.framesPerPeak > 0 else { return [] }
        let scale = plot.width / Double(waveform.frames)
        return waveform.peaks.enumerated().compactMap { index, peak in
            guard peak.count == 2, peak[0].isFinite, peak[1].isFinite else { return nil }
            let x = plot.minX + Double(index) * Double(waveform.framesPerPeak) * scale
            let endFrame = (UInt64(index) + 1) * waveform.framesPerPeak
            guard (x < plot.maxX), (endFrame > (waveform.firstFrame ?? 0)) else { return nil }
            let low = Double(max(-1, min(1, peak[0])))
            let high = Double(max(-1, min(1, peak[1])))
            return NSRect(x: x, y: plot.midY - high * plot.height / 2,
                          width: min(plot.maxX - x, max(1, Double(waveform.framesPerPeak) * scale)),
                          height: max(1, (high - low) * plot.height / 2))
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.controlBackgroundColor.setFill()
        plot.fill()
        NSColor.separatorColor.setFill()
        NSRect(x: plot.minX, y: plot.midY, width: plot.width, height: 1).fill()
        NSColor.controlAccentColor.setFill()
        for bar in bars(in: plot) { bar.fill() }
        if let waveform, waveform.frames > 0 {
            NSColor.labelColor.setFill()
            let x = plot.minX + Double(startFrame) / Double(waveform.frames) * plot.width
            NSRect(x: min(plot.maxX - 1, x), y: plot.minY, width: 1, height: plot.height).fill()
        }
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        guard plot.contains(point), plot.width > 0, let waveform, waveform.frames > 0 else { return }
        let fraction = max(0, min(1, (point.x - plot.minX) / plot.width))
        let frame = min(waveform.frames - 1, UInt64((fraction * Double(waveform.frames)).rounded(.down)))
        onSeek(frame, AuditionOrigin(input: .click, delayMicroseconds: FileListTable.delay(event)))
    }

    override func accessibilityPerformPress() -> Bool {
        guard waveform != nil else { return false }
        onSeek(0, .other)
        return true
    }
}
