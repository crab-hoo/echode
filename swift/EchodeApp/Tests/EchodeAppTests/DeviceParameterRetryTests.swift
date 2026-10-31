import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct DeviceParameterRetryTests {
    @Test func failedListOffersAnAccessibleRetryInItsColumn() throws {
        let document = DocumentID(), track = UUID(), device = UUID()
        let row = TrackDevice(id: device, plugin: "dev.echode.test-params")
        let state = DeviceParameterState(document: document, device: device)
        state.fail("The plugin did not answer")
        let column = DeviceColumnView(device: row, payload: FXDrag(document: document, track: track, device: device, plugin: nil))
        column.configure(device: row, first: true, last: true, busy: false, locked: false, state: state)
        let retry = try #require(column.subviews.compactMap { $0 as? NSButton }.first {
            $0.identifier?.rawValue == "retry-parameters"
        })
        #expect(!retry.isHidden && retry.isEnabled)
        #expect(retry.accessibilityLabel()?.contains(row.title) == true)
        #expect(column.message.stringValue == "The plugin did not answer")
        #expect(column.list.isHidden)
        var retried = 0
        column.onRetry = { retried += 1 }
        column.configure(device: row, first: true, last: true, busy: true, locked: false, state: state)
        #expect(!retry.isHidden && !retry.isEnabled)
        retry.performClick(nil)
        #expect(retried == 0)
        column.configure(device: row, first: true, last: true, busy: false, locked: false, collapsed: true, state: state)
        #expect(retry.isHidden)
        let offline = TrackDevice(id: device, plugin: row.plugin, offline: true)
        column.configure(device: offline, first: true, last: true, busy: false, locked: false, state: state)
        #expect(retry.isHidden && column.message.stringValue == text("Offline device parameters"))
        state.beginLoading()
        column.configure(device: row, first: true, last: true, busy: false, locked: false, state: state)
        #expect(retry.isHidden && column.message.stringValue == text("Loading parameters"))
        state.list([], keeping: nil)
        column.configure(device: row, first: true, last: true, busy: false, locked: false, state: state)
        #expect(retry.isHidden && column.message.stringValue == text("No editable parameters"))
    }

    @Test func contractFailedListRetriesOnlyItsDeviceWithoutHoldingEdits() async throws {
        let plugin = FakeParameterPlugin()
        let (controller, document, track, url) = try await project(plugin)
        defer { try? FileManager.default.removeItem(at: url) }
        let devices = controller.deviceRows(on: track)
        let failed = devices[0].id, other = devices[1].id
        await plugin.failNextListing(failed)
        await controller.showDevices(on: track).value
        let state = try #require(controller.parameterState(failed))
        let otherState = try #require(controller.parameterState(other))
        #expect(state.error != nil && !state.loading)
        state.query = "gain"
        let strip = DeviceStripView(frame: CGRect(x: 0, y: 0, width: 900, height: 400))
        var pending: Task<Void, Never>?
        strip.onRetry = { pending = controller.retryParameters($0, documentID: document) }
        func refresh() {
            strip.configure(devices: controller.deviceRows(on: track), track: track, document: document, busy: controller.busy,
                            states: [failed: state, other: otherState])
            strip.layoutSubtreeIfNeeded()
        }
        refresh()
        let column = try #require(strip.columns.first)
        let before = controller.undoCount
        let otherReads = await plugin.listings(other)
        let otherLayout = otherState.layout
        #expect(!column.retry.isHidden && column.retry.isEnabled)
        #expect(!strip.columns[1].list.isHidden)
        #expect(!column.message.frame.intersects(column.retry.frame))
        #expect(column.bounds.contains(column.retry.frame))

        await plugin.hold(failed)
        column.retry.performClick(nil)
        let retry = try #require(pending)
        // Loading starts at the click, before the task gets a turn. Even the
        // still-visible native button cannot enqueue a second request.
        #expect(state.loading && state.error == nil && !controller.busy)
        column.retry.performClick(nil)
        await pending?.value
        while await plugin.listings(failed) < 2 { await Task.yield() }
        refresh()
        #expect(column.retry.isHidden && !column.retry.isEnabled)
        #expect(column.message.stringValue == text("Loading parameters"))
        #expect(state.query == "gain" && strip.addButton.isEnabled && column.remove.isEnabled)
        #expect(controller.undoCount == before)

        // The held listing neither changes selection nor holds edits or Stop.
        await controller.addTrack(name: "unrelated")
        let selected = controller.selection
        #expect(await controller.setMixer(track, field: .gain, value: 0.5, documentID: document))
        await controller.stopPlayback()
        let edited = controller.undoCount
        #expect(!controller.busy && controller.errorMessage == nil)
        await plugin.release(failed)
        await retry.value
        refresh()
        #expect(state.error == nil && !state.loading && state.parameters.map(\.id) == [1, 2, 4])
        #expect(state.query == "gain" && !column.list.isHidden && column.retry.isHidden)
        #expect(controller.undoCount == edited && controller.selection == selected)
        #expect(controller.parameterState(other) === otherState && otherState.layout == otherLayout)
        #expect(await plugin.listings(other) == otherReads)
        #expect(await plugin.listings(failed) == 2)

        // A second failure returns to the same local recovery action.
        await plugin.failNextListing(failed)
        await controller.showDevices(on: track).value
        refresh()
        #expect(!column.retry.isHidden && state.error != nil)
        column.retry.performClick(nil)
        await pending?.value
        refresh()
        #expect(state.error == nil && !column.list.isHidden && column.retry.isHidden)
        #expect(controller.undoCount == edited && controller.errorMessage == nil)
        await controller.close()
    }

    @Test func retryRejectsOfflineRemovedAndOldDocumentTargets() async throws {
        let plugin = FakeParameterPlugin()
        let (controller, document, track, url) = try await project(plugin)
        defer { try? FileManager.default.removeItem(at: url) }
        let device = try #require(controller.deviceRows(on: track).first?.id)
        await plugin.failNextListing(device)
        await controller.showDevices(on: track).value
        let reads = await plugin.listings(device)
        await controller.retryParameters(device, documentID: DocumentID()).value
        #expect(await plugin.listings(device) == reads)
        #expect(await controller.editDeviceChain(.offline(device: device, offline: true), documentID: document))
        await controller.retryParameters(device, documentID: document).value
        #expect(await plugin.listings(device) == reads)
        #expect(await controller.editDeviceChain(.remove(device), documentID: document))
        await controller.retryParameters(device, documentID: document).value
        #expect(await plugin.listings(device) == reads)
        await controller.close()
        await controller.open(url, creating: false)
        let remaining = try #require(controller.deviceRows(on: track).first?.id)
        await plugin.failNextListing(remaining)
        await controller.showDevices(on: track).value
        let reopenedReads = await plugin.listings(remaining)
        #expect(controller.parameterState(remaining)?.error != nil)
        await controller.retryParameters(remaining, documentID: document).value
        #expect(await plugin.listings(remaining) == reopenedReads)
        #expect(controller.errorMessage == nil)
        await controller.close()
    }

    @Test func lateRetryCannotOverwriteANewerListOrRestoreAClosedDocument() async throws {
        let plugin = FakeParameterPlugin()
        let (controller, document, track, url) = try await project(plugin)
        defer { try? FileManager.default.removeItem(at: url) }
        let device = try #require(controller.deviceRows(on: track).first?.id)
        await plugin.failNextListing(device)
        await controller.showDevices(on: track).value
        await plugin.hold(device)
        let retry = controller.retryParameters(device, documentID: document)
        while await plugin.listings(device) < 2 { await Task.yield() }
        await plugin.setGain(1.5)
        await controller.showDevices(on: track).value
        await plugin.release(device)
        await retry.value
        #expect(controller.parameterState(device)?.parameters.first?.value == 1.5)

        await plugin.failNextListing(device)
        await controller.showDevices(on: track).value
        let reads = await plugin.listings(device)
        await plugin.hold(device)
        let closing = controller.retryParameters(device, documentID: document)
        while await plugin.listings(device) == reads { await Task.yield() }
        await controller.close()
        await plugin.release(device)
        await closing.value
        #expect(controller.documentID == nil && controller.parameterStates.isEmpty)
    }

    private func project(_ plugin: FakeParameterPlugin) async throws -> (ProjectController, DocumentID, UUID, URL) {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-parameter-retry-\(UUID()).echode")
        let controller = ProjectController(sendDevice: { try await plugin.send($0, $1) })
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "fx")
        let track = try #require(controller.selection)
        for _ in 0..<2 { #expect(await controller.insertPlugin("dev.echode.test-params", on: track, documentID: document)) }
        return (controller, document, track, url)
    }
}
