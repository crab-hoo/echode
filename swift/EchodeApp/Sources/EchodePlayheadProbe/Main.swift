import AppKit
import EchodePlayheadChecks

@main
@MainActor
struct EchodePlayheadProbe {
    static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        let delegate = ProbeDelegate()
        app.delegate = delegate
        withExtendedLifetime(delegate) { app.run() }
    }
}

@MainActor
private final class ProbeDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        Task { @MainActor in
            let checks = PlayheadChecks()
            let scenarios: [(String, @MainActor () async throws -> Void)] = [
                ("locate", checks.stoppedLocateDrawsArrangeAndPianoAfterTheLinkPauses),
                ("play", checks.playDrawsTheLiveTickAndPauseFreezesIt),
                ("closed", checks.closedViewDoesNotDrawAndAReopenedOneDoes),
                ("document", checks.planSwapAndDocumentSwitchKeepTheLiveView),
            ]
            var failed = false
            for (name, run) in scenarios {
                do { try await run() }
                catch { failed = true; print("spike-playhead: \(name) failed: \(error)") }
            }
            print("spike-playhead: \(failed ? "failed" : "all four scenarios completed")")
            exit(failed ? 1 : 0)
        }
    }
}
