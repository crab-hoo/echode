import AppKit
import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

/// The UI fixture writes a real document edit; the Rust device-lane contract
/// checks the CLAP lifecycle, actual captured bytes and state restoration.
actor ConfigPlugin {
    private(set) var reads = 0
    private(set) var selections: [UInt32] = []
    private(set) var applications: [[DeviceAudioPortChange]] = []
    private var fail = false
    private var held = false
    private var waiting: CheckedContinuation<Void, Never>?
    private var current: UInt32 = 100

    func failNext() { fail = true }
    func hold() { held = true }
    func release() {
        held = false
        waiting?.resume()
        waiting = nil
    }
    func isWaiting() -> Bool { waiting != nil }

    func send(_ document: Document, _ command: DeviceCommand) async throws -> DeviceResult {
        switch command {
        case .parameters:
            return DeviceResult(report: DeviceReport(parameters: []), snapshot: try await document.snapshot())
        case .audioConfigs:
            reads += 1
            if held { await withCheckedContinuation { waiting = $0 } }
            if fail {
                fail = false
                throw CancellationError()
            }
            let json = """
                {"current_id":\(current),"configs":[
                {"id":100,"name":"Same","input_ports":1,"output_ports":1,"main_input_channels":2,"main_output_channels":2},
                {"id":206,"name":"Same","input_ports":1,"output_ports":1,"main_input_channels":2,"main_output_channels":1}]}
                """
            let list = try JSONDecoder().decode(DeviceAudioConfigs.self, from: Data(json.utf8))
            return DeviceResult(report: DeviceReport(audioConfigs: list), snapshot: try await document.snapshot())
        case .selectAudioConfig(let device, let id):
            selections.append(id)
            return try await capture(document, device: device, id: id)
        case .applyAudioPorts(let device, let requests):
            applications.append(requests)
            if held { await withCheckedContinuation { waiting = $0 } }
            let mono = requests.contains { !$0.input && $0.index == 0 && $0.channels == 1 }
            return try await capture(document, device: device, id: mono ? 206 : 100)
        default:
            return try await DeviceResult.send(document, command)
        }
    }
    private func capture(_ document: Document, device: UUID, id: UInt32) async throws -> DeviceResult {
        if fail {
            fail = false
            throw CancellationError()
        }
        current = id
        let hash = String(repeating: id == 206 ? "b" : "a", count: 64)
        let snapshot = try await document.perform(
            #"{"type":"device.set_state","device":"\#(device.uuidString)","state":"\#(hash)"}"#)
        return DeviceResult(report: DeviceReport(changed: true), snapshot: snapshot)
    }
}

@MainActor
struct DeviceAudioConfigTests {
    func project(_ plugin: ConfigPlugin) async throws -> (ProjectController, DocumentID, UUID, UUID, URL) {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-audio-config-\(UUID()).echode")
        let controller = ProjectController(sendDevice: { try await plugin.send($0, $1) })
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "fx")
        let track = try #require(controller.selection)
        #expect(await controller.insertPlugin("dev.echode.test-gain", on: track, documentID: document))
        let device = try #require(controller.deviceRows(on: track).first?.id)
        await controller.showDevices(on: track).value
        return (controller, document, track, device, url)
    }

    @Test func contractAudioConfigControlsReadExplicitlyAndSelectIDsAsOneEdit() async throws {
        let plugin = ConfigPlugin()
        let (controller, document, track, device, url) = try await project(plugin)
        defer { removeTestFile(url) }
        let parameters = try #require(controller.parameterState(device))
        let state = parameters.audioConfiguration
        let strip = DeviceStripView(frame: CGRect(x: 0, y: 0, width: 640, height: 420))
        var pending: Task<Void, Never>?
        strip.onReadAudioConfigs = { id in pending = controller.loadAudioConfigs(id, documentID: document) }
        strip.onSelectAudioConfig = { id, config in
            pending = Task { await controller.selectAudioConfig(config, device: id, documentID: document) }
        }
        func refresh(collapsed: Bool = false) {
            strip.configure(
                devices: controller.deviceRows(on: track), track: track, document: document,
                busy: controller.busy, states: [device: parameters], collapsed: collapsed ? [device] : [])
            strip.layoutSubtreeIfNeeded()
        }
        refresh()
        let view = try #require(strip.columns.first?.audioConfigs)
        #expect(await plugin.reads == 0, "showing devices must not call the plugin to enumerate presets")
        let before = controller.undoCount
        view.read.performClick(nil)
        await pending?.value
        refresh()
        #expect(state.list?.configs.map(\.id) == [100, 206])
        #expect(view.presets.numberOfItems == 3 && view.presets.indexOfSelectedItem == 1)
        #expect(view.presets.itemTitle(at: 1) != view.presets.itemTitle(at: 2), "duplicate names need distinct ids")
        #expect(view.read.toolTip == text("Audio configuration interruption help"))
        #expect(controller.undoCount == before)
        #expect(await plugin.reads == 1)
        view.presets.selectItem(at: 2)
        view.presets.sendAction(view.presets.action, to: view.presets.target)
        await pending?.value
        refresh()
        #expect(await plugin.selections == [206], "send the plugin id, never a menu index")
        #expect(controller.undoCount == before + 1)
        #expect(state.list == nil && !state.busy && !controller.busy)
        await controller.undo()
        #expect(controller.undoCount == before && state.list == nil)
        await controller.redo()
        #expect(controller.undoCount == before + 1)
        #expect(await plugin.reads == 1, "Undo and Redo must not enumerate presets")
        refresh(collapsed: true)
        #expect(view.isHidden)
        await controller.close()
        // This UI-only fixture names synthetic state without a stored object.
        // Real state persistence/reopening is covered by the Rust lane contract.
        let next = url.deletingLastPathComponent().appendingPathComponent("echode-next-\(UUID()).echode")
        defer { removeTestFile(next) }
        await controller.open(next, creating: true)
        #expect(controller.documentID != document)
        #expect(!(await controller.selectAudioConfig(100, device: device, documentID: document)))
        await controller.close()
    }

    @Test func failedRequestsRetryAndOldRepliesNeverRepublishAList() async throws {
        let plugin = ConfigPlugin()
        let (controller, document, track, device, url) = try await project(plugin)
        defer { removeTestFile(url) }
        let state = try #require(controller.parameterState(device)?.audioConfiguration)
        await plugin.failNext()
        await controller.loadAudioConfigs(device, documentID: document).value
        #expect(state.error != nil && !state.busy && state.list == nil)
        await controller.loadAudioConfigs(device, documentID: document).value
        #expect(state.error == nil && state.list?.configs.count == 2)
        let before = controller.undoCount
        await plugin.failNext()
        #expect(!(await controller.selectAudioConfig(206, device: device, documentID: document)))
        #expect(state.error != nil && !state.busy && controller.undoCount == before)
        await controller.loadAudioConfigs(device, documentID: document).value
        #expect(!(await controller.selectAudioConfig(999, device: device, documentID: document)))
        #expect(!(await controller.selectAudioConfig(100, device: device, documentID: document)))
        #expect(await plugin.selections == [206])
        await plugin.hold()
        let read = controller.loadAudioConfigs(device, documentID: document)
        var turns = 0
        while !(await plugin.isWaiting()), turns < 10_000 {
            await Task.yield()
            turns += 1
        }
        #expect(await plugin.isWaiting(), "the explicit read reached the fixture")
        let reads = await plugin.reads
        await controller.loadAudioConfigs(device, documentID: document).value
        #expect(await plugin.reads == reads, "coalesce repeated clicks")
        // An unrelated edit completes without waiting for the read. Its
        // publication invalidates the menu conservatively, including this reply.
        await controller.addTrack(name: "while reading")
        #expect(controller.tracks.count == 2 && !controller.busy && state.busy)
        await plugin.release()
        await read.value
        #expect(!state.busy && state.list == nil)
        controller.selectTrack(track)
        await controller.loadAudioConfigs(device, documentID: document).value
        #expect(state.list != nil)
        #expect(await controller.editDeviceChain(.offline(device: device, offline: true), documentID: document))
        #expect(controller.parameterState(device) == nil)
        let afterOffline = await plugin.reads
        await controller.loadAudioConfigs(device, documentID: document).value
        #expect(await plugin.reads == afterOffline)
        await controller.close()
    }
}
