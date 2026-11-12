import AppKit
import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

@MainActor
struct PluginWindowStatusTests {
    @Test func contractPluginWindowStatusIsVisibleWithoutChangingTheProject() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-window-status-\(UUID()).echode")
        defer { removeTestFile(url) }
        let plugin = FakeParameterPlugin()
        let controller = ProjectController(sendDevice: { try await plugin.send($0, $1) })
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "Pad")
        let track = try #require(controller.selection)
        for _ in 0..<2 {
            #expect(await controller.insertPlugin("dev.echode.test-params", on: track, documentID: document))
        }
        let devices = controller.deviceRows(on: track)
        let device = try #require(devices.first).id
        await controller.addTrack(name: "Other")
        let other = try #require(controller.selection)
        let strip = DeviceStripView(frame: CGRect(x: 0, y: 0, width: 640, height: 300))
        func refresh(collapsed: Bool = false) {
            strip.configure(
                devices: controller.deviceRows(on: track), track: track, document: document, busy: false,
                collapsed: collapsed ? [device] : [])
            strip.layoutSubtreeIfNeeded()
            for column in strip.columns { column.layoutSubtreeIfNeeded() }
        }
        refresh()
        let column = try #require(strip.columns.first)
        let indicator = try #require(
            column.subviews.compactMap { $0 as? NSImageView }.first {
                $0.identifier?.rawValue == "plugin-window-open"
            })
        #expect(indicator.isHidden)
        let before = (controller.undoCount, controller.redoCount)
        let snapshot = controller.snapshot
        let report = try #require(await controller.openPluginWindow(device, documentID: document))
        #expect(report.created && !report.key)
        // A device reply is not ordered with activity on MainActor. Only the
        // ordered lifetime notification drives the indicator, never a late reply.
        #expect(controller.openPluginWindows.isEmpty)
        controller.receivePluginActivity([PluginActivity(device: device, windowState: true)], documentID: document)
        refresh()
        #expect(!indicator.isHidden && indicator.image != nil)
        #expect(indicator.accessibilityLabel() == text("Plugin window open") + " " + column.device.title)
        #expect(indicator.toolTip == text("Plugin window open"))
        #expect(column.title.frame.maxX <= indicator.frame.minX)
        #expect(indicator.frame.maxX <= column.offline.frame.minX)
        #expect(strip.columns[1].windowIndicator.isHidden, "instances of the same plugin are independent")
        #expect(controller.selection == other, "a window's lifetime is not the selected track")

        // A hidden window is still open. An edit's historical window flag,
        // or an older close merged before a reopen, is not its latest lifetime.
        controller.receivePluginActivity([PluginActivity(device: device, windowOpen: false)], documentID: document)
        refresh(collapsed: true)
        #expect(!indicator.isHidden && column.isCollapsed)
        #expect(column.title.frame.maxX <= indicator.frame.minX && indicator.frame.maxX < column.bounds.maxX)
        controller.receivePluginActivity(
            [PluginActivity(device: device, closed: true, windowState: true)], documentID: document)
        #expect(controller.openPluginWindows == [device])
        // The actual name action still raises this device without another edit.
        var opening: Task<Void, Never>?
        strip.onOpenWindow = { id in opening = Task { _ = await controller.openPluginWindow(id, documentID: document) }
        }
        let action = try #require(column.title.accessibilityCustomActions()?.first)
        #expect(action.handler?() == true)
        await opening?.value
        #expect(await plugin.windowRequests() == [device, device])
        #expect(controller.snapshot === snapshot)
        #expect(controller.undoCount == before.0 && controller.redoCount == before.1)

        controller.receivePluginActivity(
            [PluginActivity(device: device, closed: true, windowState: false)], documentID: document)
        refresh()
        #expect(indicator.isHidden)
        controller.receivePluginActivity([PluginActivity(device: device, windowOpen: true)], documentID: document)
        #expect(controller.openPluginWindows.isEmpty, "an earlier edit must not reopen the indicator")
        controller.receivePluginActivity([PluginActivity(device: device, windowState: true)], documentID: document)

        // The same cached column and open state survive switching tracks.
        strip.configure(devices: [], track: other, document: document, busy: false)
        controller.selection = track
        refresh()
        #expect(strip.columns.first === column && !indicator.isHidden)
        #expect(
            await controller.editDeviceChain(.copy(source: device, track: track, before: nil), documentID: document))
        refresh()
        #expect(strip.columns.last?.windowIndicator.isHidden == true)
        #expect(controller.openPluginWindows == [device], "copies do not copy window lifetimes")

        // Offline and deleted devices have no window, even before any list was
        // requested. Undo restores the device, not its presentation state.
        #expect(controller.parameterStates.isEmpty)
        #expect(await controller.editDeviceChain(.offline(device: device, offline: true), documentID: document))
        controller.receivePluginActivity([PluginActivity(device: device, windowState: true)], documentID: document)
        refresh()
        #expect(indicator.isHidden && controller.openPluginWindows.isEmpty)
        await controller.undo()
        refresh()
        #expect(indicator.isHidden)
        let redo = controller.redoCount
        controller.receivePluginActivity([PluginActivity(device: device, windowState: true)], documentID: document)
        #expect(controller.redoCount == redo)
        #expect(await controller.editDeviceChain(.remove(device), documentID: document))
        controller.receivePluginActivity([PluginActivity(device: device, windowState: true)], documentID: document)
        #expect(controller.openPluginWindows.isEmpty)
        await controller.undo()
        refresh()
        #expect(indicator.isHidden)

        controller.receivePluginActivity([PluginActivity(device: device, windowState: true)], documentID: document)
        await controller.close()
        #expect(controller.openPluginWindows.isEmpty)
        await controller.open(url)
        controller.receivePluginActivity([PluginActivity(device: device, windowState: true)], documentID: document)
        #expect(controller.openPluginWindows.isEmpty, "old documents cannot update reopened copies")
        #expect(controller.deviceRows(on: track).allSatisfy { !$0.windowOpen }, "window state is not saved")
        await controller.close()
    }

    @Test func aLateShowReplyCannotReopenAClosedWindowIndicator() async throws {
        let started = AsyncStream<Void>.makeStream()
        let release = AsyncStream<Void>.makeStream()
        let plugin = FakeParameterPlugin()
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-window-late-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController(sendDevice: { document, command in
            let result = try await plugin.send(document, command)
            if case .showWindow = command {
                started.continuation.yield(())
                var replies = release.stream.makeAsyncIterator()
                _ = await replies.next()
            }
            return result
        })
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "Pad")
        let track = try #require(controller.selection)
        #expect(await controller.insertPlugin("dev.echode.test-params", on: track, documentID: document))
        let device = try #require(controller.deviceRows(on: track).first).id
        let opening = Task { await controller.openPluginWindow(device, documentID: document) }
        var requests = started.stream.makeAsyncIterator()
        _ = await requests.next()
        controller.receivePluginActivity(
            [
                PluginActivity(device: device, windowState: true),
                PluginActivity(device: device, closed: true, windowState: false),
            ], documentID: document)
        #expect(controller.openPluginWindows.isEmpty)
        release.continuation.yield(())
        #expect(await opening.value != nil)
        #expect(controller.openPluginWindows.isEmpty)
        started.continuation.finish()
        release.continuation.finish()
        await controller.close()
    }
}
