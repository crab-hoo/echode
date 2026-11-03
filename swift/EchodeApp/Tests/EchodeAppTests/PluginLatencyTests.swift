import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct PluginLatencyTests {
    @Test func latencyIsVisibleAndDocumentScopedWithoutEditingTheProject() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-latency-\(UUID()).echode")
        defer { removeTestFile(url) }
        let plugin = FakeParameterPlugin()
        let controller = ProjectController(sendDevice: { try await plugin.send($0, $1) })
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "Latency")
        let track = try #require(controller.selection)
        #expect(await controller.insertPlugin("dev.echode.test-params", on: track, documentID: document))
        let device = try #require(controller.deviceRows(on: track).first).id
        await controller.showDevices(on: track).value
        let state = try #require(controller.parameterState(device))
        let strip = DeviceStripView(frame: CGRect(x: 0, y: 0, width: 640, height: 300))
        func refresh(collapsed: Bool = false) {
            strip.configure(devices: controller.deviceRows(on: track), track: track, document: document,
                            busy: false, states: [device: state], collapsed: collapsed ? [device] : [])
            strip.layoutSubtreeIfNeeded()
            for column in strip.columns { column.layoutSubtreeIfNeeded() }
        }
        refresh()
        let column = try #require(strip.columns.first)
        #expect(column.latency.isHidden, "unknown latency is not a measured zero")
        let snapshot = controller.snapshot
        let history = (controller.undoCount, controller.redoCount)
        for samples: UInt32 in [512, 0, .max] {
            controller.receivePluginActivity([PluginActivity(device: device, latencySamples: samples)], documentID: document)
            refresh()
            #expect(state.latencySamples == samples)
            #expect(!column.latency.isHidden)
            #expect(column.latency.stringValue.contains(String(samples)))
            #expect(column.latency.accessibilityLabel()?.contains("Latency") == true)
            #expect(column.latency.toolTip == text("Plugin latency uncompensated"))
            #expect(column.latency.frame.maxY <= column.search.frame.minY)
            refresh(collapsed: true)
            #expect(!column.latency.isHidden)
            #expect(column.latency.frame.minY >= column.remove.frame.maxY)
        }
        #expect(controller.snapshot === snapshot)
        #expect(controller.undoCount == history.0 && controller.redoCount == history.1)
        state.fail("unavailable")
        controller.receivePluginActivity([PluginActivity(device: device, latencySamples: 256)], documentID: document)
        refresh()
        #expect(column.latency.isHidden, "a failed list must not expose stale helper metadata")
        #expect(await controller.editDeviceChain(.offline(device: device, offline: true), documentID: document))
        refresh()
        #expect(column.latency.isHidden)
        controller.receivePluginActivity([PluginActivity(device: device, latencySamples: 64)], documentID: document)
        #expect(controller.parameterState(device) == nil)
        await controller.close()
        await controller.open(url)
        controller.receivePluginActivity([PluginActivity(device: device, latencySamples: 128)], documentID: document)
        #expect(controller.parameterStates.isEmpty, "old document activity cannot populate reopened devices")
        await controller.close()
    }

    @Test func aLateListingCannotReplaceNewerLatencyActivity() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-latency-order-\(UUID()).echode")
        defer { removeTestFile(url) }
        let plugin = FakeParameterPlugin()
        let reached = AsyncStream<Void>.makeStream()
        let release = AsyncStream<Void>.makeStream()
        let controller = ProjectController(sendDevice: { document, command in
            let result = try await plugin.send(document, command)
            if case .parameters = command {
                reached.continuation.yield(())
                var gate = release.stream.makeAsyncIterator()
                _ = await gate.next()
                return DeviceResult(report: DeviceReport(parameters: result.report.parameters, latencySamples: 256), snapshot: result.snapshot)
            }
            return result
        })
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "Ordered latency")
        let track = try #require(controller.selection)
        #expect(await controller.insertPlugin("dev.echode.test-params", on: track, documentID: document))
        let device = try #require(controller.deviceRows(on: track).first).id
        let listing = controller.showDevices(on: track)
        var requests = reached.stream.makeAsyncIterator()
        _ = await requests.next()
        controller.receivePluginActivity([PluginActivity(device: device, latencySamples: 512)], documentID: document)
        release.continuation.yield(())
        await listing.value
        #expect(controller.parameterState(device)?.latencySamples == 512)
        await controller.close()
    }
}
