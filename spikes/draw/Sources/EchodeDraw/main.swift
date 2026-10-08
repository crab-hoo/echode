import AppKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?
    private var canvas: MetalCanvas?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let arguments = CommandLine.arguments
        let bench = arguments.contains("--bench")
        let reportURL = argumentValue("--report", in: arguments).map { URL(fileURLWithPath: $0) }
        let frame = NSRect(x: 0, y: 0, width: 1280, height: 800)
        let canvas = MetalCanvas(frame: frame, bench: bench, reportURL: reportURL)
        let window = NSWindow(
            contentRect: frame,
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Echode draw"
        window.contentView = canvas
        window.center()
        window.makeKeyAndOrderFront(nil)
        self.window = window
        self.canvas = canvas
        NSApp.activate()
        canvas.start()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}

@MainActor
final class MetalCanvas: NSView {
    private let renderer: Renderer

    init(frame: NSRect, bench: Bool, reportURL: URL?) {
        guard let renderer = Renderer(bench: bench, reportURL: reportURL) else {
            fputs("EchodeDraw: failed to create a Metal device\n", stderr)
            exit(1)
        }
        self.renderer = renderer
        super.init(frame: frame)
        allowedTouchTypes = [.indirect]
        addSubview(renderer.hostView)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    func start() {
        renderer.start()
    }

    override func layout() {
        super.layout()
        renderer.hostView.frame = bounds
    }

    override func scrollWheel(with event: NSEvent) {
        renderer.scroll(by: CGSize(width: event.scrollingDeltaX, height: event.scrollingDeltaY))
    }

    override func magnify(with event: NSEvent) {
        renderer.magnify(by: event.magnification)
    }

    override func touchesBegan(with event: NSEvent) {
        renderer.beginTouches(event)
    }

    override func touchesMoved(with event: NSEvent) {
        renderer.moveTouches(event)
    }

    override func touchesEnded(with event: NSEvent) {
        renderer.endTouches()
    }

    override func touchesCancelled(with event: NSEvent) {
        renderer.endTouches()
    }
}

private let appDelegate = AppDelegate()

private func argumentValue(_ name: String, in arguments: [String]) -> String? {
    guard let index = arguments.firstIndex(of: name), arguments.indices.contains(index + 1) else {
        return nil
    }
    return arguments[index + 1]
}

NSApplication.shared.delegate = appDelegate
NSApplication.shared.setActivationPolicy(.regular)
NSApplication.shared.run()
