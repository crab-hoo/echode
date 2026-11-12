import AppKit
import SwiftUI
import Testing

@testable import EchodeAppCore

@MainActor
struct WorkspaceChromeTests {
    @Test func contractStatusRowAndPlayButtonKeepTheirSizeWhilePreparing() {
        func size<V: View>(_ view: V, width: CGFloat) -> NSSize {
            let host = NSHostingView(rootView: view)
            host.frame = NSRect(x: 0, y: 0, width: width, height: 200)
            host.layoutSubtreeIfNeeded()
            return host.fittingSize
        }
        let idle = size(
            WorkspaceStatusBar(projectName: "Demo", busy: false, pendingPlayback: nil, hasProject: true), width: 900)
        let applying = size(
            WorkspaceStatusBar(projectName: "Demo", busy: true, pendingPlayback: nil, hasProject: true), width: 900)
        let preparing = size(
            WorkspaceStatusBar(projectName: "Demo", busy: false, pendingPlayback: true, hasProject: true), width: 900)
        #expect(
            idle.height > 0 && idle.height == applying.height && idle.height == preparing.height,
            "status row heights \(idle.height), \(applying.height), \(preparing.height)")
        let play = size(WorkspacePlayButton(paused: false) {}, width: 400)
        let pause = size(WorkspacePlayButton(paused: true) {}, width: 400)
        #expect(play.width > 0 && play.width == pause.width, "play widths \(play.width), \(pause.width)")
        #expect(play.height == pause.height)
    }
}
