import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

/// Answers device commands like a running CLAP, without an output device.
/// A commit still records a real `device.set_state` edit in the document.
actor FakeParameterPlugin {
    private var gain = 1.0
    private var mode = 0.0
    private var recorded: String?
    private(set) var sentGains: [Double] = []
    private(set) var reads = 0

    private func table() -> [DeviceParameter] {
        [DeviceParameter(id: 1, flags: 1 << 5, name: "Gain", module: "Output", min: 0, max: 2, defaultValue: 1,
                         value: gain, text: String(format: "%.2fx", gain)),
         DeviceParameter(id: 2, flags: 1 | 1 << 16, name: "Mode", min: 0, max: 2, defaultValue: 0,
                         value: mode, text: ["Clean", "Half", "Mute"][Int(mode)]),
         DeviceParameter(id: 3, flags: 1 << 2, name: "Internal", min: 0, max: 1, defaultValue: 0, value: 0, text: "0.00"),
         DeviceParameter(id: 4, flags: 1 << 3, name: "Meter", min: 0, max: 1, defaultValue: 0, value: 0, text: "0.00")]
    }

    func send(_ document: Document, _ command: DeviceCommand) async throws -> DeviceResult {
        switch command {
        case .parameters:
            reads += 1
            return DeviceResult(report: DeviceReport(parameters: table()), snapshot: try await document.snapshot())
        case .set(_, let changes):
            for change in changes {
                if change.id == 1 { gain = change.value; sentGains.append(change.value) }
                if change.id == 2 { mode = change.value }
            }
            let values = table().filter { parameter in changes.contains { $0.id == parameter.id } }
                .map { DeviceParameterValue(id: $0.id, value: $0.value, text: $0.text) }
            return DeviceResult(report: DeviceReport(values: values), snapshot: try await document.snapshot())
        case .commit(let device):
            let hash = String(format: "%064llx", UInt64(gain * 1000) << 8 | UInt64(mode))
            guard hash != recorded else {
                return DeviceResult(report: DeviceReport(changed: false), snapshot: try await document.snapshot())
            }
            recorded = hash
            let snapshot = try await document.perform(#"{"type":"device.set_state","device":"\#(device.uuidString)","state":"\#(hash)"}"#)
            return DeviceResult(report: DeviceReport(changed: true), snapshot: snapshot)
        }
    }
}

@MainActor
struct DeviceParameterTests {
    private func list(for controller: ProjectController, pending: @escaping (Task<Void, Never>) -> Void) -> DeviceParameterListView {
        let list = DeviceParameterListView(frame: CGRect(x: 0, y: 0, width: 520, height: 200))
        list.onBegin = { controller.beginParameter($0) }
        list.onChange = { controller.changeParameter($0) }
        list.onEnd = { pending(Task { await controller.finishParameter(commit: true) }) }
        list.onStep = { id, value in pending(Task { _ = await controller.setParameter(id, value: value) }) }
        list.configure(parameters: controller.parameterPanel?.parameters ?? [], busy: controller.busy)
        list.layoutSubtreeIfNeeded()
        return list
    }

    @Test func contractParameterDragSoundsLiveAndRecordsOneUndoStep() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-parameters-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let plugin = FakeParameterPlugin()
        let controller = ProjectController(sendDevice: { try await plugin.send($0, $1) })
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "fx")
        let track = try #require(controller.selection)
        #expect(await controller.insertPlugin("dev.echode.test-params", on: track, documentID: document))
        let device = try #require(controller.deviceRows(on: track).first)

        // The FX row's toggle opens the panel for that device.
        let chain = DeviceChainListView(frame: CGRect(x: 0, y: 0, width: 520, height: 100))
        var opened: Task<Void, Never>?
        chain.onParameters = { id in opened = Task { await controller.showParameters(id) } }
        chain.configure(devices: controller.deviceRows(on: track), track: track, document: document, busy: false)
        let row = try #require(chain.subviews.first as? DeviceChainRow)
        row.parameters.performClick(nil)
        await opened?.value
        let panel = try #require(controller.parameterPanel)
        #expect(panel.device == device.id && !panel.loading && panel.error == nil)
        // Hidden parameters are not offered; read-only ones are shown but locked.
        #expect(panel.parameters.map(\.id) == [1, 2, 4])

        var pending: Task<Void, Never>?
        let parameters = list(for: controller) { pending = $0 }
        let gain = try #require(parameters.rows.first)
        let meter = try #require(parameters.rows.last)
        #expect(!meter.slider.isEnabled && !controller.beginParameter(4))

        let before = controller.undoCount
        gain.slider.beginDrag()
        for value in [1.2, 1.4, 1.6] {
            gain.slider.doubleValue = value
            gain.slider.sendAction(gain.slider.action, to: gain.slider.target)
        }
        // Values reach the plugin during the drag, but the project waits.
        #expect(controller.undoCount == before)
        #expect(controller.parameterPanel?.parameters.first?.value == 1.6)
        gain.slider.endDrag()
        await pending?.value
        #expect(await plugin.sentGains.last == 1.6)
        #expect(controller.undoCount == before + 1)
        #expect(controller.parameterPanel?.parameters.first?.text == "1.60x")

        // A keyboard step on the stepped mode is one more Undo step, rounded.
        let mode = parameters.rows[1]
        mode.slider.doubleValue = 1.2
        mode.slider.sendAction(mode.slider.action, to: mode.slider.target)
        await pending?.value
        #expect(controller.undoCount == before + 2)
        #expect(controller.parameterPanel?.parameters[1].text == "Half")

        // A cancelled drag puts the value back and records nothing.
        #expect(controller.beginParameter(1))
        controller.changeParameter(0.5)
        await controller.finishParameter(commit: false)
        #expect(await plugin.sentGains.suffix(2) == [0.5, 1.6])
        #expect(controller.undoCount == before + 2)
        // An unchanged value sends nothing and adds no step.
        #expect(!(await controller.setParameter(1, value: 1.6)))

        // Undo reads the restored values back from the plugin.
        let reads = await plugin.reads
        await controller.undo()
        #expect(controller.undoCount == before + 1)
        #expect(await plugin.reads == reads + 1)
        await controller.redo()
        #expect(await plugin.reads == reads + 2)

        // Taking the device offline closes the panel; it has no running plugin.
        #expect(await controller.editDeviceChain(.offline(device: device.id, offline: true), documentID: document))
        #expect(controller.parameterPanel == nil)
        await controller.showParameters(device.id)
        #expect(controller.parameterPanel == nil)
        await controller.close()
        #expect(controller.parameterPanel == nil)
    }
}
