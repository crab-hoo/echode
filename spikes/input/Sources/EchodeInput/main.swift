import AppKit
import EchodeInputCore

@MainActor
final class InputCanvas: NSView {
    private var zoom = Zoom(x: 1, y: 1)
    private var origin = CGPoint(x: 80, y: 80)
    private var pinch: PinchGesture?
    private var pinchMode: PinchMode = .uniform
    private var pinchFingerIDs: [NSObject] = []
    private var keyboard = ComputerKeyboard()
    private var pressure = 0.0
    private var lastStage = 0
    private var log: [String] = []

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        allowedTouchTypes = [.indirect]
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    override var acceptsFirstResponder: Bool { true }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        allowedTouchTypes = [.indirect]
        window?.makeFirstResponder(self)
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor(calibratedWhite: 0.12, alpha: 1).setFill()
        dirtyRect.fill()

        let spacing: CGFloat = 40
        NSColor(calibratedWhite: 0.22, alpha: 1).setStroke()
        let grid = NSBezierPath()
        grid.lineWidth = 1
        var x = bounds.minX
        while x <= bounds.maxX {
            grid.move(to: NSPoint(x: x, y: bounds.minY))
            grid.line(to: NSPoint(x: x, y: bounds.maxY))
            x += spacing
        }
        var y = bounds.minY
        while y <= bounds.maxY {
            grid.move(to: NSPoint(x: bounds.minX, y: y))
            grid.line(to: NSPoint(x: bounds.maxX, y: y))
            y += spacing
        }
        grid.stroke()

        let box = NSRect(
            x: origin.x,
            y: origin.y,
            width: 220 * zoom.x,
            height: 140 * zoom.y
        )
        NSColor(calibratedRed: 0.35, green: 0.55, blue: 0.95, alpha: 0.9).setFill()
        NSBezierPath(rect: box).fill()

        let held = keyboard.heldNotes.map(noteName).joined(separator: " ")
        let expression = chordPressure(notes: keyboard.notes, pressure: pressure)
        let status = """
            横 X \(String(format: "%.2f", zoom.x))    縦 Y \(String(format: "%.2f", zoom.y))    \(pinchModeLabel)
            通常のピンチは X と Y が同じ倍率。キー 2 ではっきり横か縦のときだけその軸
            ドラッグで格子をまたぐと、トラックパッドが鳴ります
            和音 \(held.isEmpty ? "（A S D など）" : held)    プレッシャー \(String(format: "%.2f", expression?.pressure ?? 0))
            \(log.suffix(4).joined(separator: "\n"))
            """
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = 4
        status.draw(
            in: NSRect(x: 24, y: bounds.maxY - 150, width: bounds.width - 48, height: 130),
            withAttributes: [
                .font: NSFont.monospacedSystemFont(ofSize: 13, weight: .regular),
                .foregroundColor: NSColor.white,
                .paragraphStyle: paragraph,
            ]
        )
    }

    override func touchesBegan(with event: NSEvent) {
        guard pinch == nil else { return }
        pinchFingerIDs = []
        guard let span = fingerSpan(from: event) else { return }
        pinch = PinchGesture(span: span, zoom: zoom, mode: pinchMode)
    }

    override func touchesMoved(with event: NSEvent) {
        guard let next = fingerSpan(from: event) else { return }
        if pinch == nil {
            pinch = PinchGesture(span: next, zoom: zoom, mode: pinchMode)
            return
        }
        guard var gesture = pinch else { return }
        let updated = gesture.update(span: next)
        pinch = gesture
        if updated != zoom {
            record(
                String(
                    format: "pinch %@  X %.2f → %.2f    Y %.2f → %.2f",
                    axisLabel(gesture.axis),
                    zoom.x,
                    updated.x,
                    zoom.y,
                    updated.y
                )
            )
            zoom = updated
            needsDisplay = true
        }
    }

    override func touchesEnded(with event: NSEvent) {
        let stillTouching = event.touches(matching: .touching, in: nil)
        if stillTouching.count < 2 {
            pinch = nil
            pinchFingerIDs = []
        }
    }

    override func touchesCancelled(with event: NSEvent) {
        pinch = nil
        pinchFingerIDs = []
    }

    private func axisLabel(_ axis: PinchAxis) -> String {
        switch axis {
        case .uniform: "uniform"
        case .horizontal: "horizontal"
        case .vertical: "vertical"
        }
    }

    private var pinchModeLabel: String {
        pinchMode == .uniform ? "モード 1 通常" : "モード 2 軸固定"
    }

    private func fingerSpan(from event: NSEvent) -> FingerSpan? {
        let live = event.touches(matching: .touching, in: nil)
        let pair = trackedPair(in: live)
        guard pair.count == 2 else { return nil }
        return EchodeInputCore.fingerSpan(
            first: trackpadTouch(pair[0]),
            second: trackpadTouch(pair[1])
        )
    }

    /// Prefer the fingers that started the gesture. If their identities no
    /// longer match, keep the pinch alive with the two contacts still down.
    private func trackedPair(in live: Set<NSTouch>) -> [NSTouch] {
        if pinchFingerIDs.count == 2 {
            let matched = live.filter { touch in
                pinchFingerIDs.contains { $0.isEqual(touch.identity) }
            }
            if matched.count == 2 { return Array(matched) }
        }
        let moving = live.filter { !$0.isResting }
        let pool = moving.count >= 2 ? Array(moving) : Array(live)
        let chosen = Array(pool.prefix(2))
        guard chosen.count == 2 else { return [] }
        pinchFingerIDs = chosen.compactMap { $0.identity as? NSObject }
        return chosen
    }

    private func trackpadTouch(_ touch: NSTouch) -> TrackpadTouch {
        TrackpadTouch(
            x: touch.normalizedPosition.x,
            y: touch.normalizedPosition.y,
            deviceWidth: touch.deviceSize.width,
            deviceHeight: touch.deviceSize.height
        )
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
    }

    override func mouseDragged(with event: NSEvent) {
        let previous = origin
        origin.x += event.deltaX
        origin.y -= event.deltaY
        if crossedGridLine(from: previous.x, to: origin.x, spacing: 40)
            || crossedGridLine(from: previous.y, to: origin.y, spacing: 40)
        {
            playHaptic(.alignment)
            record("haptic alignment")
        }
        needsDisplay = true
    }

    override func keyDown(with event: NSEvent) {
        guard let character = event.charactersIgnoringModifiers?.first, !event.isARepeat else {
            return
        }
        if character == "1" {
            pinchMode = .uniform
            record("mode uniform")
            needsDisplay = true
            return
        }
        if character == "2" {
            pinchMode = .directional
            record("mode directional")
            needsDisplay = true
            return
        }
        keyboard.press(character)
        if midiNote(for: character) != nil {
            record("note on \(keyboard.heldNotes.map(noteName).joined(separator: " "))")
        }
        needsDisplay = true
    }

    override func keyUp(with event: NSEvent) {
        guard let character = event.charactersIgnoringModifiers?.first else { return }
        keyboard.release(character)
        if keyboard.heldNotes.isEmpty {
            pressure = 0
        }
        needsDisplay = true
    }

    override func pressureChange(with event: NSEvent) {
        if event.stage != lastStage, chordPressure(notes: keyboard.notes, pressure: 1) != nil {
            playHaptic(.levelChange)
            record("haptic level \(event.stage)")
            lastStage = event.stage
        }
        pressure = Double(event.pressure)
        if let expression = chordPressure(notes: keyboard.notes, pressure: pressure) {
            let names = expression.notes.map(noteName).joined(separator: " ")
            record(String(format: "pressure %@  %.2f", names, expression.pressure))
        }
        needsDisplay = true
    }

    private func record(_ line: String) {
        log.append(line)
        if log.count > 12 {
            log.removeFirst(log.count - 12)
        }
        print(line)
    }
}

private func playHaptic(_ pattern: NSHapticFeedbackManager.FeedbackPattern) {
    NSHapticFeedbackManager.defaultPerformer.perform(pattern, performanceTime: .now)
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let frame = NSRect(x: 0, y: 0, width: 760, height: 520)
        let canvas = InputCanvas(frame: frame)
        let window = NSWindow(
            contentRect: frame,
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Echode input"
        window.contentView = canvas
        window.center()
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(canvas)
        self.window = window
        NSApp.activate()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}

private let appDelegate = AppDelegate()

NSApplication.shared.delegate = appDelegate
NSApplication.shared.setActivationPolicy(.regular)
NSApplication.shared.run()
