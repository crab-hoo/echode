import AppKit
import SwiftUI

/// The candidate list is nonmodal so the user can select a different track
/// while it is open. Its document session stays fixed until it closes.
struct WorkspacePluginPanel: NSViewRepresentable {
    let controller: ProjectController
    let document: DocumentID?
    let close: () -> Void
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeNSView(context: Context) -> NSView { NSView() }
    func updateNSView(_ view: NSView, context: Context) {
        context.coordinator.update(controller: controller, document: document, close: close)
    }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) { coordinator.dismiss() }

    @MainActor
    final class Coordinator: NSObject, NSWindowDelegate {
        private var panel: NSPanel?
        private var document: DocumentID?
        private var close: (() -> Void)?
        func update(controller: ProjectController, document: DocumentID?, close: @escaping () -> Void) {
            self.close = close
            guard let document, controller.isCurrent(document) else { dismiss(); return }
            guard self.document != document || panel == nil else { return }
            dismiss()
            self.document = document
            let panel = NSPanel(contentRect: CGRect(x: 0, y: 0, width: 460, height: 520),
                                styleMask: [.titled, .closable, .utilityWindow], backing: .buffered, defer: false)
            panel.title = text("Plugin Browser")
            panel.isFloatingPanel = true; panel.hidesOnDeactivate = true; panel.isReleasedWhenClosed = false
            panel.delegate = self
            panel.contentViewController = NSHostingController(rootView: WorkspacePluginBrowser(controller: controller, document: document) { [weak self] in
                self?.close?()
            })
            self.panel = panel
            panel.center(); panel.makeKeyAndOrderFront(nil)
        }
        func dismiss() {
            panel?.delegate = nil
            panel?.close()
            panel = nil; document = nil
        }
        func windowWillClose(_ notification: Notification) {
            panel = nil; document = nil
            close?()
        }
    }
}
