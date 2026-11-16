import AppKit
import EchodeBridge
import Foundation
import Observation
import Synchronization
import Testing

@testable import EchodeAppCore

@MainActor
struct DeviceAudioPortTests {
    @Test func publicationInvalidatesObservableDraftsEvenWithoutAPresetList() {
        let state = DeviceAudioConfigState()
        let fired = Mutex(false)
        withObservationTracking {
            _ = state.revision
        } onChange: {
            fired.withLock { $0 = true }
        }
        state.invalidate()
        #expect(state.list == nil && !state.busy)
        #expect(fired.withLock { $0 }, "empty-state invalidation must notify the draft's revision observer")
    }

    @Test func contractAudioPortFormAppliesOneBatchAsOneEditWithoutEnumeration() async throws {
        let plugin = ConfigPlugin()
        let (controller, document, track, device, url) = try await DeviceAudioConfigTests().project(plugin)
        defer { removeTestFile(url) }
        let state = try #require(controller.parameterState(device))
        let strip = DeviceStripView(frame: CGRect(x: 0, y: 0, width: 640, height: 420))
        var pending: Task<Void, Never>?
        strip.onApplyAudioPorts = { id, requests in
            pending = Task { await controller.applyAudioPorts(requests, device: id, documentID: document) }
        }
        func refresh(collapsed: Bool = false) {
            strip.configure(
                devices: controller.deviceRows(on: track), track: track, document: document,
                busy: controller.busy, states: [device: state], collapsed: collapsed ? [device] : [])
            strip.layoutSubtreeIfNeeded()
        }
        refresh()
        let view = try #require(strip.columns.first?.audioConfigs)
        view.change.performClick(nil)
        let editor = try #require(view.portEditor)
        #expect(await plugin.reads == 0, "drafting must not enumerate presets")
        #expect(await plugin.applications.isEmpty, "drafting must not interrupt audio")
        editor.add.performClick(nil)
        let output = try #require(editor.rows.last)
        output.direction.selectItem(at: 1)
        output.channels.stringValue = "1"
        output.controlTextDidChange(Notification(name: NSText.didChangeNotification))
        let expected = [
            DeviceAudioPortChange(input: true, index: 0, channels: 2),
            DeviceAudioPortChange(input: false, index: 0, channels: 1),
        ]
        #expect(editor.requests == expected)
        let before = controller.undoCount
        editor.apply.performClick(nil)
        // A second activation of the old form must not submit again.
        editor.apply.performClick(nil)
        await pending?.value
        #expect(await plugin.applications == [expected], "apply the whole port list exactly once")
        #expect(controller.undoCount == before + 1 && !state.audioConfiguration.busy)
        #expect(view.portEditor == nil)
        await controller.undo()
        #expect(controller.undoCount == before)
        await controller.redo()
        #expect(controller.undoCount == before + 1)
        #expect(await plugin.reads == 0, "applying, Undo and Redo must not enumerate presets")
        refresh()
        view.change.performClick(nil)
        let stale = try #require(view.portEditor)
        await controller.addTrack(name: "invalidate draft")
        stale.apply.performClick(nil)
        #expect(await plugin.applications == [expected], "publication invalidates even an unrefreshed form")
        refresh(collapsed: true)
        #expect(view.portEditor == nil && !view.change.isEnabled)
        await controller.close()
        #expect(!(await controller.applyAudioPorts(expected, device: device, documentID: document)))
    }

    @Test func portFormValidatesWholeUnsignedNumbersAndCancelNeverApplies() throws {
        let state = DeviceAudioConfigState()
        let view = DeviceAudioConfigView(frame: CGRect(x: 0, y: 0, width: 300, height: 78))
        view.configure(state, title: "fx", enabled: true)
        var applications = 0
        view.onApplyPorts = { _ in applications += 1 }
        view.change.performClick(nil)
        let editor = try #require(view.portEditor)
        let row = try #require(editor.rows.first)
        for value in ["", "-1", "+1", "1.5", "4294967296", "٢"] {
            row.channels.stringValue = value
            row.controlTextDidChange(Notification(name: NSText.didChangeNotification))
            #expect(editor.requests == nil && !editor.apply.isEnabled)
            editor.apply.performClick(nil)
        }
        row.channels.stringValue = "4294967295"
        row.index.stringValue = "-1"
        row.controlTextDidChange(Notification(name: NSText.didChangeNotification))
        #expect(editor.requests == nil && !editor.apply.isEnabled)
        row.index.stringValue = "0"
        row.controlTextDidChange(Notification(name: NSText.didChangeNotification))
        #expect(editor.requests == [DeviceAudioPortChange(input: true, index: 0, channels: .max)])
        row.channels.stringValue = "0"
        row.index.stringValue = "4294967295"
        row.controlTextDidChange(Notification(name: NSText.didChangeNotification))
        #expect(editor.requests == [DeviceAudioPortChange(input: true, index: .max, channels: 0)])
        #expect(editor.apply.isEnabled)
        row.remove.performClick(nil)
        #expect(editor.rows.isEmpty && !editor.apply.isEnabled)
        editor.add.performClick(nil)
        #expect(editor.apply.isEnabled)
        editor.cancel.performClick(nil)
        editor.apply.performClick(nil)
        #expect(applications == 0 && view.portEditor == nil)
    }

    @Test func failedPortChangesReleaseBusyAndOnlyExplicitRetryResubmits() async throws {
        let plugin = ConfigPlugin()
        let (controller, document, track, device, url) = try await DeviceAudioConfigTests().project(plugin)
        defer { removeTestFile(url) }
        let state = try #require(controller.parameterState(device)?.audioConfiguration)
        let changes = [DeviceAudioPortChange(input: false, index: 0, channels: 1)]
        let before = controller.undoCount
        await plugin.failNext()
        #expect(!(await controller.applyAudioPorts(changes, device: device, documentID: document)))
        #expect(!state.busy && state.error != nil && controller.undoCount == before)
        #expect(await plugin.applications == [changes])
        await plugin.hold()
        let pending = Task { await controller.applyAudioPorts(changes, device: device, documentID: document) }
        var turns = 0
        while !(await plugin.isWaiting()), turns < 10_000 {
            await Task.yield()
            turns += 1
        }
        #expect(await plugin.isWaiting())
        #expect(!(await controller.applyAudioPorts(changes, device: device, documentID: document)))
        await plugin.release()
        #expect(await pending.value)
        #expect(!state.busy && state.error == nil && controller.undoCount == before + 1)
        #expect(await plugin.applications == [changes, changes])
        #expect(!(await controller.applyAudioPorts([], device: device, documentID: document)))
        controller.selectTrack(track)
        #expect(await controller.editDeviceChain(.offline(device: device, offline: true), documentID: document))
        #expect(!(await controller.applyAudioPorts(changes, device: device, documentID: document)))
        #expect(await plugin.applications == [changes, changes])
        await controller.close()
    }
}
