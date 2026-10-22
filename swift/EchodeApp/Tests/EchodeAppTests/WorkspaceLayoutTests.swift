import AppKit
import Foundation
import SwiftUI
import Testing
@testable import EchodeAppCore

@MainActor
struct WorkspaceLayoutTests {
    @Test func contractEditorAndDevicesToggleIndependentlyWithoutEditingTheProject() async throws {
        let suite = "echode-layout-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-layout-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "notes")
        await controller.insertNote(pitch: 60, beat: 0, length: 1)
        let state = WorkspaceState(defaults: defaults)
        let session = WorkspaceSession(controller: controller, state: state)
        state.layout.editorHeight = 360
        let snapshot = try #require(controller.snapshot)
        let undo = controller.undoCount, track = try #require(controller.selection)
        let note = try #require(snapshot.pianoNeighbor(track: track, current: nil, backwards: false))
        controller.noteFocus = note.id
        session.perform(.devices)
        #expect(state.showsEditor && state.showsDevices, "opening devices must keep the piano roll open")
        #expect(session.isSelected(.devices) && session.isSelected(.editor))
        session.perform(.devices)
        #expect(state.showsEditor && !state.showsDevices, "the device command must also close devices")
        session.perform(.devices)
        session.perform(.editor)
        #expect(!state.showsEditor && state.showsDevices)
        session.perform(.maximizeEditors)
        #expect(state.layout.editorsMaximized && !state.showsEditor && state.showsDevices)
        session.perform(.editor)
        #expect(state.layout.editorsMaximized && state.showsEditor && state.showsDevices)
        session.perform(.maximizeEditors)
        #expect(!state.layout.editorsMaximized && state.showsEditor && state.showsDevices)
        #expect(state.layout.editorHeight == 360)
        state.toggle(.editor); state.toggle(.devices)
        session.openPlugins()
        #expect(!state.showsEditor && state.showsDevices, "FX candidates reveal devices, not the piano roll")
        state.pluginBrowserDocument = nil
        session.perform(.maximizeEditors)
        let reopened = WorkspaceState(defaults: defaults)
        #expect(reopened.layout == state.layout)
        #expect(controller.snapshot === snapshot && controller.undoCount == undo && controller.redoCount == 0)
        #expect(controller.selection == track && controller.noteFocus == note.id)
        // Layout controls remain usable without a selected track.
        controller.selection = nil
        for command in [WorkspaceCommand.editor, .devices, .maximizeEditors] { #expect(session.enabled(command)) }
        await controller.close()
        #expect(!session.enabled(.devices) && !session.enabled(.editor) && !session.enabled(.maximizeEditors))
    }

    @Test func contractCenterStackKeepsFixedDevicesBelowThePianoRoll() throws {
        for mask in 0..<8 {
            var layout = WorkspaceLayout()
            layout.editor = mask & 1 != 0; layout.devices = mask & 2 != 0; layout.editorsMaximized = mask & 4 != 0
            layout.editorHeight = 360
            let arrange = NSView(), piano = NSView(), devices = NSView()
            let size = CGSize(width: 600, height: 900)
            let sizes = layout.dimensions(in: size)
            let stack = WorkspaceCenterStack(layout: layout, sizes: sizes, resize: { _ in }, finish: {},
                arrange: { LayoutProbe(view: arrange) }, editor: { LayoutProbe(view: piano) }, devices: { LayoutProbe(view: devices) })
            let host = NSHostingView(rootView: stack)
            host.frame = CGRect(origin: .zero, size: size)
            host.layout()
            host.layoutSubtreeIfNeeded()
            #expect(host.window == nil)
            #expect((arrange.superview != nil) == !layout.editorsMaximized)
            #expect((piano.superview != nil) == layout.editor)
            #expect((devices.superview != nil) == layout.devices)
            if !layout.editorsMaximized {
                #expect(abs(arrange.frame.height - sizes.arrange) < 0.01)
            }
            if layout.editor {
                let pianoFrame = try #require(piano.superview).convert(piano.frame, to: host)
                #expect(abs(pianoFrame.height - sizes.editor) < 0.01)
                if !layout.editorsMaximized {
                    let arrangeFrame = try #require(arrange.superview).convert(arrange.frame, to: host)
                    #expect(arrangeFrame.maxY <= pianoFrame.minY)
                }
                if layout.devices {
                    let deviceFrame = try #require(devices.superview).convert(devices.frame, to: host)
                    #expect(pianoFrame.maxY <= deviceFrame.minY)
                }
            }
            if layout.devices {
                let deviceFrame = try #require(devices.superview).convert(devices.frame, to: host)
                #expect(abs(deviceFrame.height - WorkspaceLayout.deviceHeight) < 0.01)
                #expect(abs(deviceFrame.maxY - host.bounds.maxY) < 0.01)
            }
        }
    }

    @Test func allVisibilityCombinationsFitTheMinimumWindow() {
        for mask in 0..<32 {
            var layout = WorkspaceLayout()
            layout.inspector = mask & 1 != 0; layout.browser = mask & 2 != 0
            layout.editor = mask & 4 != 0; layout.devices = mask & 8 != 0; layout.editorsMaximized = mask & 16 != 0
            let size = CGSize(width: layout.minimumWidth, height: layout.minimumHeight - WorkspaceLayout.windowChrome)
            let dims = layout.dimensions(in: size)
            let count = (layout.editorsMaximized ? 0 : 1) + (layout.editor ? 1 : 0) + (layout.devices ? 1 : 0)
            let total = dims.arrange + dims.editor + dims.devices + dims.spacer + Double(max(0, count - 1)) * dims.separator
            #expect(abs(total - size.height) < 0.01)
            #expect(size.width - dims.inspector - dims.browser - (layout.inspector ? 5 : 0) - (layout.browser ? 5 : 0) >= 520)
            #expect(dims.arrange >= (layout.editorsMaximized ? 0 : WorkspaceLayout.minimumArrangeHeight))
            #expect(dims.editor >= (layout.editor ? WorkspaceLayout.minimumEditorHeight : 0))
            #expect(dims.devices == (layout.devices ? WorkspaceLayout.deviceHeight : 0))
        }
    }

    @Test func constraintsAndMaximizingDoNotOverwritePreferredHeights() {
        var layout = WorkspaceLayout()
        layout.editorHeight = 360
        let roomy = CGSize(width: 1200, height: 930), cramped = CGSize(width: 1200, height: 830)
        #expect(layout.dimensions(in: cramped).editor == 360)
        layout.devices = true
        #expect(layout.dimensions(in: cramped).editor == 300)
        #expect(layout.dimensions(in: cramped).devices == 300)
        #expect(layout.dimensions(in: roomy).editor == 360)
        layout.editorsMaximized = true
        #expect(layout.dimensions(in: cramped).editor == 525)
        layout.resize(.editor, to: 500, in: cramped)
        layout.resize(.devices, to: 200, in: cramped)
        #expect(layout.editorHeight == 360)
        layout.editorsMaximized = false
        layout.resize(.editor, to: 500, in: cramped)
        #expect(layout.editorHeight == 300)
        layout.devices = false
        #expect(layout.dimensions(in: roomy).editor == 300)
        for size in [CGSize.zero, CGSize(width: 1, height: 1), CGSize(width: 500, height: 280)] {
            layout.devices = true
            let dims = layout.dimensions(in: size)
            #expect(dims.arrange >= 0 && dims.editor >= 0 && dims.devices >= 0 && dims.separator >= 0)
            #expect(dims.arrange + dims.editor + dims.devices + dims.separator * 2 <= size.height)
        }
        let saved = layout
        layout.resize(.editor, to: .nan, in: roomy)
        layout.resize(.editor, to: 400, in: CGSize(width: CGFloat.infinity, height: 900))
        #expect(layout == saved)
    }

    @Test func migratesV1OnceAndKeepsV2VisibilityAndDimensions() throws {
        let suite = "echode-layout-migration-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let old = Data("""
            {"inspector":false,"browser":true,"editor":false,"inspectorWidth":250,"browserWidth":260,"editorHeight":360}
            """.utf8)
        defaults.set(old, forKey: "workspace.layout.v1")
        let state = WorkspaceState(defaults: defaults)
        #expect(!state.layout.inspector && state.layout.browser && !state.showsEditor && !state.showsDevices)
        #expect(!state.layout.editorsMaximized && state.layout.editorHeight == 360)
        #expect(state.layout.inspectorWidth == 250 && state.layout.browserWidth == 260)
        #expect(defaults.data(forKey: "workspace.layout.v2") != nil)
        state.toggle(.editor); state.toggle(.devices); state.toggleEditorsMaximized()
        #expect(WorkspaceState(defaults: defaults).layout == state.layout)
        // A broken v2 must not silently resurrect a stale v1 layout.
        defaults.set(Data("invalid".utf8), forKey: "workspace.layout.v2")
        #expect(WorkspaceState(defaults: defaults).layout == WorkspaceLayout())
        var invalid = WorkspaceLayout()
        invalid.editorHeight = .infinity
        let encoder = JSONEncoder()
        encoder.nonConformingFloatEncodingStrategy = .convertToString(positiveInfinity: "inf", negativeInfinity: "-inf", nan: "nan")
        defaults.set(try encoder.encode(invalid), forKey: "workspace.layout.v2")
        #expect(WorkspaceState(defaults: defaults).layout == WorkspaceLayout())
    }

    @Test func layoutShortcutsAndToggleRolesAreDistinct() {
        for (command, key) in [(WorkspaceCommand.editor, "3"), (.devices, "4"), (.maximizeEditors, "5")] {
            #expect(command.isLayoutToggle && command.shortcut?.key == KeyEquivalent(Character(key)))
            #expect(command.shortcut?.modifiers == [.command, .option])
            #expect(command.shortcutLabel == "⌥⌘\(key)")
        }
        #expect(!WorkspaceCommand.plugins.isLayoutToggle)
    }
}

private struct LayoutProbe: NSViewRepresentable {
    let view: NSView
    func makeNSView(context: Context) -> NSView { view }
    func updateNSView(_ nsView: NSView, context: Context) {}
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSView, context: Context) -> CGSize? {
        CGSize(width: proposal.width ?? 0, height: proposal.height ?? 0)
    }
}
