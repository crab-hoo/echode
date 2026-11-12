import AppKit
import EchodeBridge
import Testing

@testable import EchodeAppCore

@MainActor
struct DeviceOfflineTests {
    @Test func contractOfflineFXPreservesSettingsAndUndoAcrossReopen() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-offline-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "track")
        let track = try #require(controller.selection)
        #expect(await controller.insertPlugin("org.example.fx", on: track, documentID: document))
        let device = try #require(controller.deviceRows(on: track).first)
        #expect(await controller.editDeviceChain(.bypass(device: device.id, bypassed: true), documentID: document))
        let list = DeviceStripView(frame: CGRect(x: 0, y: 0, width: 620, height: 300))
        var pending: Task<Bool, Never>?
        list.onEdit = { change in pending = Task { await controller.editDeviceChain(change, documentID: document) } }
        list.configure(devices: controller.deviceRows(on: track), track: track, document: document, busy: false)
        list.layoutSubtreeIfNeeded()
        let row = try #require(list.columns.first)
        #expect(row.offline.state == .off && row.bypass.state == .on)
        for width in [240.0, DeviceStripView.columnWidth - 1] {
            row.frame.size.width = width
            row.needsLayout = true
            row.layoutSubtreeIfNeeded()
            #expect(!row.offline.frame.intersects(row.bypass.frame))
            #expect(!row.bypass.frame.intersects(row.left.frame))
            #expect(!row.title.frame.intersects(row.offline.frame))
            #expect(!row.search.frame.intersects(row.bypass.frame))
        }
        let before = controller.undoCount
        row.offline.performClick(nil)
        #expect(await pending?.value == true)
        #expect(controller.undoCount == before + 1)
        #expect(controller.deviceRows(on: track).first?.offline == true)
        #expect(controller.deviceRows(on: track).first?.bypassed == true)
        #expect(!(await controller.editDeviceChain(.offline(device: device.id, offline: true), documentID: document)))
        #expect(controller.undoCount == before + 1)
        await controller.undo()
        #expect(controller.deviceRows(on: track).first?.offline == false)
        await controller.redo()
        #expect(controller.deviceRows(on: track).first?.offline == true)
        #expect(
            await controller.editDeviceChain(.copy(source: device.id, track: track, before: nil), documentID: document))
        #expect(controller.deviceRows(on: track).allSatisfy { $0.offline && $0.bypassed })
        await controller.close()
        await controller.open(url)
        #expect(controller.deviceRows(on: track).count == 2)
        #expect(controller.deviceRows(on: track).allSatisfy { $0.offline && $0.bypassed })
        #expect(!(await controller.editDeviceChain(.offline(device: device.id, offline: false), documentID: document)))
        let current = try #require(controller.documentID)
        #expect(await controller.editDeviceChain(.offline(device: device.id, offline: false), documentID: current))
        #expect(controller.deviceRows(on: track).first?.offline == false)
        #expect(controller.deviceRows(on: track).last?.offline == true)
        let snapshot = try #require(controller.snapshot)
        #expect(snapshot.deviceOffline(onTrack: -1, at: 0) == nil)
        #expect(snapshot.deviceOffline(onTrack: 0, at: 999) == nil)
        await controller.close()
    }
}
