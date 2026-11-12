import AppKit
import Testing

@testable import EchodeAppCore

@MainActor
struct CanvasBoundaryTests {
    @Test func contractShrinkingPanesClipThePreviousSurfaceBeforeChildLayout() {
        let root = NSView(frame: NSRect(x: 0, y: 0, width: 900, height: 700))
        let arrange = ArrangePaneView(frame: root.bounds)
        let piano = PianoRollView(frame: root.bounds)
        for (pane, surface) in [
            (arrange as NSView, arrange.timeline as NSView), (piano as NSView, piano.grid as NSView),
        ] {
            root.addSubview(pane)
            pane.layoutSubtreeIfNeeded()
            surface.frame = root.bounds
            // SwiftUI can shrink its host before AppKit lays out the child.
            // Keep the old surface for this interleaving, rather than testing
            // only the settled frame after both layouts have completed.
            pane.frame.size = NSSize(width: 380, height: 180)
            #expect(surface.visibleRect.width > 0 && surface.visibleRect.height > 0)
            #expect(surface.visibleRect.maxX <= pane.bounds.maxX)
            #expect(surface.visibleRect.maxY <= pane.bounds.maxY)
            pane.removeFromSuperview()
        }
    }
}
