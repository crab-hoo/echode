import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

/// Plugin windows (ADR 0132) without a helper or a window on screen. The
/// fake plugin answers device commands; activity entries stand in for what
/// the helper relays. Rust's `plugin_gui` contracts cover the real helper.
@MainActor
struct PluginWindowTests {
    @Test func contractPluginShortcutsUseTheWorkspaceCommands() async throws {
        let plugin = FakeParameterPlugin()
        let sent = AsyncStream<AudioCommand>.makeStream()
        let (controller, document, tracks, url) = try await project("window-keys", plugin: plugin, sendAudio: { _, command in
            sent.continuation.yield(command)
            let playing = if case .play = command { true } else { false }
            return try JSONDecoder().decode(AudioState.self, from: Data("""
                {"playing":\(playing),"running":true,"opening":false,"tick":0,"devices":[],"metronome":false}
                """.utf8))
        })
        defer { removeTestFile(url) }
        #expect(await controller.insertPlugin("dev.echode.test-params", on: tracks[0], documentID: document))
        let device = try #require(controller.deviceRows(on: tracks[0]).first).id
        let suite = "echode-plugin-keys-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let state = WorkspaceState(defaults: defaults)
        let session = WorkspaceSession(controller: controller, state: state)
        var received: [WorkspaceCommand] = []
        controller.pluginCommand = { command in received.append(command); session.perform(command) }
        defer { controller.pluginCommand = nil }
        let before = state.layout.inspector
        controller.receivePluginActivity([PluginActivity(device: device, windowOpen: true, commands: ["Toggle Inspector"])], documentID: document)
        try #require(received == [.inspector])
        #expect(state.layout.inspector != before)
        controller.receivePluginActivity([PluginActivity(device: device, windowOpen: true,
            commands: ["Devices", "Maximize Editors"])], documentID: document)
        #expect(state.showsDevices && state.layout.editorsMaximized)
        #expect(received.suffix(2) == [.devices, .maximizeEditors])
        received.removeLast(2)
        // Modal capability checks still apply to forwarded shortcuts.
        state.presentation = .palette
        controller.receivePluginActivity([PluginActivity(device: device, windowOpen: true, commands: ["Toggle Inspector"])], documentID: document)
        #expect(state.layout.inspector != before)
        state.presentation = nil
        for (id, target, open) in [(document, device, false), (DocumentID(), device, true), (document, UUID(), true)] {
            controller.receivePluginActivity([PluginActivity(device: target, windowOpen: open, commands: ["Toggle Inspector"])], documentID: id)
        }
        #expect(received == [.inspector, .inspector], "closed windows and stale targets cannot issue commands")
        controller.receivePluginActivity([PluginActivity(device: device, windowOpen: true, commands: ["unknown"])], documentID: document)
        #expect(received.count == 2)
        var commands = sent.stream.makeAsyncIterator()
        // Space plays and stops at the remembered start, as in the workspace (ADR 0155).
        for (name, expected) in [("Play / Stop", "transport.play"), ("Play / Stop", "transport.stop"), ("Stop", "transport.stop"),
                                 ("Pause", "transport.play"), ("Pause", "transport.pause"),
                                 ("Pause", "transport.play")] {
            // An unknown name sends nothing; fail here instead of waiting forever below.
            try #require(WorkspaceCommand(rawValue: name) != nil, "the helper relays \(name), which no command handles")
            controller.receivePluginActivity([PluginActivity(device: device, windowOpen: true, commands: [name])], documentID: document)
            let command = try #require(await commands.next())
            let encoded = try JSONSerialization.jsonObject(with: JSONEncoder().encode(command)) as? [String: Any]
            #expect(encoded?["type"] as? String == expected)
        }
    }
    @Test func contractAddFXFollowsTheLastDevice() throws {
        let strip = DeviceStripView(frame: CGRect(x: 0, y: 0, width: 800, height: 320))
        let track = UUID(), document = DocumentID()
        let rows = (0..<2).map { index in
            TrackDevice(id: UUID(), plugin: "FX \(index)")
        }
        var additions = 0
        strip.onAdd = { additions += 1 }
        for count in [0, 2, 1, 0] {
            strip.configure(devices: Array(rows.prefix(count)), track: track, document: document, busy: false)
            strip.layoutSubtreeIfNeeded()
            let button = try #require(strip.subviews.compactMap { $0 as? NSButton }.first { $0.title == text("Add FX") })
            #expect(button.frame.minX >= Double(count) * DeviceStripView.columnWidth)
            #expect(button.frame.maxX <= strip.contentWidth)
            #expect(button.isEnabled)
            // The actual native button is the keyboard and VoiceOver entry too.
            button.performClick(nil)
        }
        #expect(additions == 4)
        strip.configure(devices: rows, track: track, document: document, busy: true)
        let button = try #require(strip.subviews.compactMap { $0 as? NSButton }.first { $0.title == text("Add FX") })
        #expect(!button.isEnabled)
        button.performClick(nil)
        #expect(additions == 4)
    }
    private func project(_ name: String, plugin: FakeParameterPlugin, tracks: Int = 1,
                         sendAudio: @escaping @Sendable (Document, AudioCommand) async throws -> AudioState = { try await $0.audio($1) })
        async throws -> (ProjectController, DocumentID, [UUID], URL) {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-\(name)-\(UUID()).echode")
        let controller = ProjectController(sendAudio: sendAudio, sendDevice: { try await plugin.send($0, $1) })
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        var ids: [UUID] = []
        for index in 0..<tracks {
            await controller.addTrack(name: "fx \(index)")
            ids.append(try #require(controller.selection))
        }
        return (controller, document, ids, url)
    }

    private func value(_ id: UInt32, _ value: Double) -> DeviceParameterValue {
        DeviceParameterValue(id: id, value: value, text: String(format: "%.2fx", value))
    }

    /// A gesture in the plugin's window becomes one Undo step when it ends,
    /// however many values it sent. Values reach the device's list at once.
    /// Without a gesture, a change is recorded once the plugin is quiet, but
    /// only while its window is open.
    @Test func contractPluginWindowGestureRecordsOneUndoStep() async throws {
        let plugin = FakeParameterPlugin()
        let (controller, document, tracks, url) = try await project("window-undo", plugin: plugin)
        defer { removeTestFile(url) }
        #expect(await controller.insertPlugin("dev.echode.test-params", on: tracks[0], documentID: document))
        let device = try #require(controller.deviceRows(on: tracks[0]).first).id
        await controller.showDevices(on: tracks[0]).value
        let state = try #require(controller.parameterState(device))
        // Long enough that only a finished gesture can record during this part.
        controller.pluginQuietDelay = .seconds(60)
        let before = controller.undoCount

        controller.receivePluginActivity([PluginActivity(device: device, events: [.init(id: 1, kind: .begin)], windowOpen: true)],
                                         documentID: document)
        for gain in [1.3, 1.6] {
            await plugin.setGain(gain)
            controller.receivePluginActivity([PluginActivity(device: device, events: [.init(id: 1, kind: .value)],
                                                             values: [value(1, gain)], windowOpen: true)], documentID: document)
            #expect(state.parameter(1)?.value == gain)
        }
        await controller.settlePluginEdits()
        #expect(controller.undoCount == before, "nothing is recorded during the gesture")
        controller.receivePluginActivity([PluginActivity(device: device, events: [.init(id: 1, kind: .end)], windowOpen: true)],
                                         documentID: document)
        await controller.settlePluginEdits()
        #expect(controller.undoCount == before + 1)
        #expect(state.parameter(1)?.text == "1.60x")

        // A preset-like change without a gesture, while the window is open.
        // Settling waits for the quiet timer itself, so no test timing is measured.
        controller.pluginQuietDelay = .milliseconds(20)
        await plugin.setGain(0.5)
        for _ in 0..<3 {
            controller.receivePluginActivity([PluginActivity(device: device, events: [.init(id: 1, kind: .value)],
                                                             values: [value(1, 0.5)], windowOpen: true)], documentID: document)
        }
        await controller.settlePluginEdits()
        #expect(controller.undoCount == before + 2, "one step once the plugin was quiet")

        // A state change without parameters, while the window is open.
        await plugin.setGain(0.75)
        controller.receivePluginActivity([PluginActivity(device: device, dirty: true, windowOpen: true)], documentID: document)
        await controller.settlePluginEdits()
        #expect(controller.undoCount == before + 3)

        // With the window closed, the plugin's own change stays a background save.
        await plugin.setGain(0.25)
        controller.receivePluginActivity([PluginActivity(device: device, events: [.init(id: 1, kind: .value)],
                                                         values: [value(1, 0.25)], lost: true)], documentID: document)
        await controller.settlePluginEdits()
        #expect(controller.undoCount == before + 3)
        #expect(state.parameter(1)?.value == 0.25, "the list still shows the plugin's value")

        // Undo takes back one plugin-made step.
        await controller.undo()
        #expect(controller.undoCount == before + 2)
        // Activity for another document is not applied.
        controller.receivePluginActivity([PluginActivity(device: device, events: [.init(id: 1, kind: .end)])],
                                         documentID: DocumentID())
        await controller.settlePluginEdits()
        #expect(controller.undoCount == before + 2)
    }

    /// The name of an inserted plugin, in the device view and the inspector,
    /// opens its own window. Only a single add from the candidates opens one
    /// by itself; adding to every selected track, copying a device and
    /// duplicating a track do not, even with a single target.
    @Test func contractNameClickOpensWindowAndOnlySingleAddsOpenIt() async throws {
        let plugin = FakeParameterPlugin()
        let (controller, document, tracks, url) = try await project("window-open", plugin: plugin, tracks: 2)
        defer { removeTestFile(url) }
        let first = tracks[0]
        #expect(await controller.insertPlugin("dev.echode.test-params", on: first, documentID: document))
        let device = try #require(controller.deviceRows(on: first).first)
        #expect(await plugin.windowRequests().isEmpty, "a plain insert opens nothing")

        // The device view: a click on the name, without a drag.
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 640, height: 420), styleMask: [.borderless],
                              backing: .buffered, defer: true)
        let strip = DeviceStripView(frame: CGRect(x: 0, y: 0, width: 640, height: 420))
        window.contentView = strip
        var opening: Task<Void, Never>?
        strip.onOpenWindow = { id in opening = Task { _ = await controller.openPluginWindow(id, documentID: document) } }
        strip.configure(devices: controller.deviceRows(on: first), track: first, document: document, busy: false,
                        locked: false, states: [:])
        strip.layoutSubtreeIfNeeded()
        let column = try #require(strip.columns.first)
        column.layoutSubtreeIfNeeded()
        let point = column.convert(CGPoint(x: column.title.frame.midX, y: column.title.frame.midY), to: nil)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            let event = try #require(NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: 0,
                                                        windowNumber: window.windowNumber, context: nil, eventNumber: 0,
                                                        clickCount: 1, pressure: 1))
            if type == .leftMouseDown { column.mouseDown(with: event) } else { column.mouseUp(with: event) }
        }
        await opening?.value
        #expect(await plugin.windowRequests() == [device.id])
        #expect(await plugin.windowTitles() == ["fx 0 — \(device.title)"])

        // The inspector's name and a second click raise the same window.
        let raised = await controller.openPluginWindow(device.id, documentID: document)
        #expect(raised?.created == false && raised?.key == false)

        // A single add from the candidates opens the new device's window.
        controller.selection = first
        let single = try #require(controller.capturePluginInsertion("dev.echode.test-params", allSelected: false, document: document))
        #expect(await controller.insertPlugin(single))
        await controller.settlePluginEdits()
        let added = try #require(controller.deviceRows(on: first).last)
        #expect(await plugin.windowRequests() == [device.id, device.id, added.id])

        // Dropped from the candidates onto a track: also a single add.
        #expect(await controller.dropFX(.plugin("dev.echode.test-params"), on: tracks[1], documentID: document))
        await controller.settlePluginEdits()
        let dropped = try #require(controller.deviceRows(on: tracks[1]).last)
        #expect(await plugin.windowRequests().last == dropped.id)
        let requested = await plugin.windowRequests().count

        // Adding to every selected track never opens, even with one track selected.
        controller.selection = first
        let batch = try #require(controller.capturePluginInsertion("dev.echode.test-params", allSelected: true, document: document))
        #expect(batch.tracks == [first])
        #expect(await controller.insertPlugin(batch))
        // Copies and duplicated tracks keep their windows closed.
        #expect(await controller.editDeviceChain(.copy(source: device.id, track: tracks[1], before: nil), documentID: document))
        #expect(await controller.duplicateTracks(TrackDuplicationRequest(document: document, sources: [first])))
        await controller.settlePluginEdits()
        #expect(await plugin.windowRequests().count == requested)

        // An offline device has no running plugin, so its name opens nothing.
        #expect(await controller.editDeviceChain(.offline(device: device.id, offline: true), documentID: document))
        #expect(await controller.openPluginWindow(device.id, documentID: document) == nil)
        #expect(await plugin.windowRequests().count == requested)
    }
}
