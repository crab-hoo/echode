import AppKit
import Foundation

// Most of these tests share the main actor, so what one of them blocks on there
// stops the polling of all the others (docs/debugging.md, `Status did not arrive`).

private let fileRemoval = DispatchQueue(label: "echode-test-file-removal")
private let waitForRemovalAtExit: Void = { atexit { fileRemoval.sync {} } }()

/// Removes a test's files away from the main actor: a removal can wait on the
/// file system while other documents flush. The process waits for it at exit.
func removeTestFile(_ url: URL) {
    _ = waitForRemovalAtExit
    fileRemoval.async { try? FileManager.default.removeItem(at: url) }
}

extension NSButton {
    /// A click for a disclosure button, whose `performClick` holds the main
    /// actor in a nested run loop for the length of its animation.
    @MainActor func clickWithoutAnimation() {
        guard isEnabled else { return }
        setNextState()
        sendAction(action, to: target)
    }
}
