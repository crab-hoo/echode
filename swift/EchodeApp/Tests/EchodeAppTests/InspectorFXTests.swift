import AppKit
import SwiftUI
import Testing
@testable import EchodeAppCore

@MainActor
struct InspectorFXTests {
    @Test func contractInspectorOpensCandidatesForEmptyAndPopulatedChains() async throws {
        _ = NSApplication.shared
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-inspector-fx-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "First")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "Second")
        let second = try #require(controller.selection)
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        let host = NSHostingView(rootView: WorkspaceInspectorView(controller: controller, add: { session.openPlugins(on: $0) }))
        host.frame = CGRect(x: 0, y: 0, width: 260, height: 900)
        let window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: true)
        window.contentView = host
        func findButton(_ view: NSView) -> NSButton? {
            if let button = view as? NSButton, button.accessibilityIdentifier() == "inspector-add-fx" { return button }
            return view.subviews.lazy.compactMap { findButton($0) }.first
        }
        host.layoutSubtreeIfNeeded()
        let button = try #require(findButton(host), "the inspector exposes Add FX even for an empty chain")
        #expect(button.title == text("Add FX"))
        #expect(button.isAccessibilityEnabled())
        let before = controller.undoCount
        button.performClick(nil)
        await Task.yield()
        #expect(state.pluginBrowserDocument == document && controller.selection == second && state.showsDevices)
        #expect(controller.undoCount == before)
        #expect(await controller.insertPlugin("example.effect", on: first, documentID: document))
        controller.selectTrack(first, change: .focus)
        host.rootView = WorkspaceInspectorView(controller: controller, add: { session.openPlugins(on: $0) })
        host.layoutSubtreeIfNeeded()
        let populated = try #require(findButton(host))
        let populatedUndo = controller.undoCount
        controller.selectTrack(second, change: .focus)
        state.pluginBrowserDocument = nil
        populated.performClick(nil)
        await Task.yield()
        #expect(state.pluginBrowserDocument == document && controller.selection == first)
        #expect(controller.undoCount == populatedUndo)
        let retained = try #require(populated as? InspectorAddFXButton).add
        await controller.close()
        state.pluginBrowserDocument = nil
        retained()
        #expect(state.pluginBrowserDocument == nil)
        var calls = 0
        let disabledHost = NSHostingView(rootView: InspectorAddFX(enabled: false, add: { calls += 1 }))
        disabledHost.frame = CGRect(x: 0, y: 0, width: 200, height: 40)
        disabledHost.layoutSubtreeIfNeeded()
        let native = try #require(findButton(disabledHost))
        native.performClick(nil)
        #expect(!native.isEnabled && calls == 0)
    }
}
