import AppKit
import EchodeBridge
import Foundation
import Observation
import Synchronization
import Testing
@testable import EchodeAppCore

/// Audio status replies only; this fixture never opens HAL or a helper.
private actor PreparingAudio {
    private var opening = true
    private var revision = 1

    func advance() { revision += 1 }
    func prepared() { opening = false; revision += 1 }

    func status() throws -> AudioState {
        try JSONDecoder().decode(AudioState.self, from: Data("""
            {"playing":false,"running":false,"tick":\(revision),"devices":[],"metronome":false,"opening":\(opening)}
            """.utf8))
    }
}

@MainActor
struct DevicePreparationTests {
    @Test func contractDeviceColumnsDistinguishOutputPreparationFromParameterReplies() async throws {
        let audio = PreparingAudio()
        let plugin = FakeParameterPlugin()
        let controller = ProjectController(sendAudio: { _, _ in try await audio.status() },
                                           sendDevice: { try await plugin.send($0, $1) })
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-device-preparation-\(UUID()).echode")
        defer { removeTestFile(url) }
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "fx")
        let track = try #require(controller.selection)
        for _ in 0..<2 { #expect(await controller.insertPlugin("dev.echode.test-params", on: track, documentID: document)) }
        let devices = controller.deviceRows(on: track)
        let device = devices[0].id
        await plugin.hold(device)
        let listing = controller.showDevices(on: track)
        try await wait("the unheld device parameter reply") { controller.parameterState(devices[1].id)?.loading == false }
        let state = try #require(controller.parameterState(device))
        state.query = "gain"
        let observing = Task { await controller.observeAudio() }
        defer { observing.cancel() }
        try await wait("the initial audio status") { controller.position == 1 }

        let strip = DeviceStripView(frame: CGRect(x: 0, y: 0, width: 900, height: 400))
        func refresh() {
            strip.configure(devices: controller.deviceRows(on: track), track: track, document: document, busy: controller.busy,
                            states: controller.parameterStates, outputOpening: controller.outputOpening)
            strip.layoutSubtreeIfNeeded()
        }
        refresh()
        let column = try #require(strip.columns.first)
        #expect(column.message.stringValue == text("Preparing audio output"))
        #expect(!column.message.isHidden && column.list.isHidden && column.retry.isHidden)
        #expect(column.remove.isEnabled && column.offline.isEnabled && strip.addButton.isEnabled)
        #expect(!strip.columns[1].list.isHidden, "A cached list remains usable while the output prepares")
        let before = controller.undoCount
        await controller.stopPlayback()
        #expect(!controller.busy && controller.undoCount == before)
        await controller.addTrack(name: "unrelated")
        #expect(!controller.busy && controller.errorMessage == nil)
        let edited = controller.undoCount
        let selected = controller.selection

        // Output status changes independently of the still-held parameter reply.
        await audio.prepared()
        try await wait("the prepared audio status") { controller.position == 2 }
        refresh()
        #expect(column.message.stringValue == text("Loading parameters"))
        #expect(state.loading && state.query == "gain")
        #expect(controller.undoCount == edited && controller.selection == selected)
        await plugin.release(device)
        await listing.value
        refresh()
        #expect(!state.loading && state.error == nil && !column.list.isHidden && column.message.isHidden)
        #expect(state.query == "gain" && controller.undoCount == edited && controller.selection == selected)
        observing.cancel()
        await observing.value
        await controller.close()
    }

    @Test func outputPreparationObservationIgnoresPositionUpdates() async throws {
        let audio = PreparingAudio()
        let controller = ProjectController(sendAudio: { _, _ in try await audio.status() })
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-preparation-observation-\(UUID()).echode")
        defer { removeTestFile(url) }
        await controller.open(url, creating: true)
        await controller.refreshOutputs()
        #expect(controller.outputOpening)
        let invalidated = Mutex(false)
        withObservationTracking { _ = controller.outputOpening } onChange: { invalidated.withLock { $0 = true } }
        await audio.advance()
        await controller.refreshOutputs()
        #expect(controller.position == 2 && controller.outputOpening)
        #expect(!invalidated.withLock { $0 }, "Position polling must not invalidate the device strip")
        await audio.prepared()
        await controller.refreshOutputs()
        #expect(!controller.outputOpening && invalidated.withLock { $0 })
        await controller.close()
        #expect(!controller.outputOpening)
    }

    @Test func preparationMessagesRespectDeviceStateAndDisclosure() throws {
        #expect(!AudioState().opening)
        let document = DocumentID(), track = UUID(), device = UUID()
        let row = TrackDevice(id: device, plugin: "dev.echode.test-params")
        let state = DeviceParameterState(document: document, device: device)
        let column = DeviceColumnView(device: row, payload: FXDrag(document: document, track: track, device: device, plugin: nil))
        func refresh(_ state: DeviceParameterState?, opening: Bool = true, offline: Bool = false) {
            column.configure(device: TrackDevice(id: device, plugin: row.plugin, offline: offline), first: true, last: true,
                             busy: false, locked: false, outputOpening: opening, state: state)
        }
        refresh(nil)
        #expect(column.message.stringValue == text("Preparing audio output"))
        #expect(column.message.accessibilityLabel()?.contains(row.title) == true)
        refresh(state)
        column.disclosure.clickWithoutAnimation()
        #expect(column.isCollapsed && column.message.isHidden)
        column.disclosure.clickWithoutAnimation()
        #expect(!column.isCollapsed && !column.message.isHidden)
        #expect(column.message.stringValue == text("Preparing audio output"), "Local disclosure must keep the output status")
        refresh(state, opening: false)
        #expect(column.message.stringValue == text("Loading parameters"))
        state.fail("Preparation failed")
        refresh(state)
        #expect(column.message.stringValue == "Preparation failed" && !column.retry.isHidden)
        refresh(state, offline: true)
        #expect(column.message.stringValue == text("Offline device parameters") && column.retry.isHidden)
        state.list([], keeping: nil)
        refresh(state)
        #expect(column.message.stringValue == text("No editable parameters"))
        let parameter = DeviceParameter(id: 1, name: "Gain", min: 0, max: 2, defaultValue: 1, value: 1, text: "1")
        state.list([parameter], keeping: nil)
        refresh(state)
        #expect(column.message.isHidden && !column.list.isHidden)
        state.beginLoading()
        refresh(state)
        #expect(!column.list.isHidden, "Refreshing a cached list must not hide its controls")
    }

    private func wait(_ phase: String, _ ready: () -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(5)
        var polls = 0
        while !ready() {
            try #require(ContinuousClock.now < deadline, "Status did not arrive: \(phase), \(polls) yielded polls")
            try await Task.sleep(for: .milliseconds(5))
            polls += 1
        }
    }
}
