import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct DeviceChainTests {
    @Test func contractDeviceChainGesturesPersistOnceAndUndo() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-chain-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "chords")
        let first = try #require(controller.selection)
        #expect(await controller.insertPlugin("org.example.a", on: first, documentID: document))
        #expect(await controller.insertPlugin("org.example.b", on: first, documentID: document))
        let original = try #require(controller.snapshot).devices(on: first)
        #expect(original.count == 2)
        let catalog = try JSONDecoder().decode([PluginListing].self, from: Data("""
            [{"id":"org.example.a","name":"Example EQ","vendor":"Example","version":"1","features":[],"fresh":false}]
            """.utf8))
        controller.rememberPluginNames(catalog + catalog)
        #expect(controller.deviceRows(on: first).map(\.title) == ["Example EQ", "org.example.b"])
        await controller.addTrack(name: "melody")
        let second = try #require(controller.selection)
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 160, height: 200))
        header.setSnapshot(try #require(controller.snapshot), selected: second)
        header.onSelect = { controller.selection = $0 }
        header.onOpenFX = { session.openPlugins(on: $0) }
        let rect = TrackFXLayout(row: header.camera.rows(trackCount: 2, height: 200).rect(at: 0, width: 160)).rect
        header.mouseDown(with: try #require(NSEvent.mouseEvent(with: .leftMouseDown, location: header.convert(CGPoint(x: rect.midX, y: rect.midY), to: nil),
            modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1)))
        #expect(controller.selection == first && state.pluginBrowserDocument == document && state.deviceEditor)
        #expect(session.enabled(.undo))

        let list = DeviceChainListView(frame: CGRect(x: 0, y: 0, width: 520, height: 160))
        var pending: Task<Bool, Never>?
        list.onEdit = { edit in pending = Task { await controller.editDeviceChain(edit, documentID: document) } }
        list.configure(devices: controller.deviceRows(on: first), track: first, document: document, busy: false)
        list.layoutSubtreeIfNeeded()
        let namedRow = try #require(list.subviews.first as? DeviceChainRow)
        let label = try #require(namedRow.subviews.compactMap { $0 as? NSTextField }.first)
        #expect(label.stringValue == "Example EQ" && label.toolTip == original[0].plugin)
        let row = try #require(list.subviews[1] as? DeviceChainRow)
        let before = controller.undoCount
        row.up.performClick(nil)
        #expect(await pending?.value == true)
        #expect(controller.undoCount == before + 1)
        #expect(controller.snapshot?.devices(on: first).map(\.id) == [original[1].id, original[0].id])
        await controller.undo()
        #expect(controller.snapshot?.devices(on: first) == original)
        await controller.redo()
        #expect(controller.snapshot?.devices(on: first).first?.id == original[1].id)

        let pasteboard = NSPasteboard.withUniqueName()
        defer { pasteboard.releaseGlobally() }
        let drag = FXDrag(document: document, track: first, device: original[0].id, plugin: nil)
        pasteboard.setData(try JSONEncoder().encode(drag), forType: FXDrag.deviceType)
        header.documentID = document
        header.setSnapshot(try #require(controller.snapshot), selected: first)
        header.onDropFX = { target, drop in pending = Task { await controller.dropFX(drop, on: target, documentID: document) } }
        pending = nil
        let destination = header.camera.rows(trackCount: 2, height: 200).rect(at: 1, width: 160)
        #expect(header.acceptFX(pasteboard, at: CGPoint(x: destination.midX, y: destination.midY), copying: false))
        #expect(await pending?.value == true)
        let copied = try #require(controller.snapshot).devices(on: second)
        #expect(copied.count == 1 && copied[0].plugin == original[0].plugin && copied[0].id != original[0].id)
        #expect(controller.snapshot?.devices(on: first).count == 2)
        await controller.undo()
        #expect(controller.snapshot?.devices(on: second).isEmpty == true)
        await controller.redo()
        #expect(controller.snapshot?.devices(on: second) == copied)
        let unchanged = controller.undoCount
        #expect(!(await controller.editDeviceChain(.reorder(device: original[0].id, before: original[0].id), documentID: document)))
        #expect(controller.undoCount == unchanged)

        list.configure(devices: try #require(controller.snapshot).devices(on: first), track: first, document: document, busy: false)
        pending = nil
        #expect(list.accept(pasteboard, at: CGPoint(x: 5, y: 150), copying: true))
        #expect(await pending?.value == true)
        let duplicated = try #require(controller.snapshot).devices(on: first)
        #expect(duplicated.count == 3 && duplicated.last?.plugin == original[0].plugin)
        #expect(duplicated.last?.id != original[0].id)
        let savedOrder = duplicated.map(\.id)
        await controller.close(); await controller.open(url)
        #expect(controller.snapshot?.devices(on: first).map(\.id) == savedOrder)
        #expect(controller.snapshot?.devices(on: second) == copied)
        #expect(!(await controller.editDeviceChain(.remove(original[0].id), documentID: document)))
        #expect(controller.snapshot?.devices(on: first).map(\.id) == savedOrder)
        await controller.close()
    }

    @Test func staleMalformedAndBusyDropsDoNotEdit() async throws {
        let list = DeviceChainListView(frame: CGRect(x: 0, y: 0, width: 520, height: 100))
        let track = UUID(), document = UUID(), device = UUID()
        var edits = [DeviceChainEdit]()
        list.onEdit = { edits.append($0) }
        list.configure(devices: [TrackDevice(id: device, plugin: "org.example.a")], track: track, document: document, busy: false)
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        board.setData(try JSONEncoder().encode(FXDrag(document: UUID(), track: track, device: device, plugin: nil)), forType: FXDrag.deviceType)
        #expect(!list.accept(board, at: .zero, copying: false))
        board.setData(Data("{}".utf8), forType: FXDrag.deviceType)
        #expect(!list.accept(board, at: .zero, copying: false))
        board.setData(try JSONEncoder().encode(FXDrag(document: document, track: track, device: UUID(), plugin: nil)), forType: FXDrag.deviceType)
        #expect(!list.accept(board, at: .zero, copying: false))
        board.setData(try JSONEncoder().encode(FXDrag(document: document, track: track, device: device, plugin: nil)), forType: FXDrag.deviceType)
        #expect(!list.accept(board, at: CGPoint(x: 0, y: CGFloat.infinity), copying: false))
        #expect(!list.accept(board, at: CGPoint(x: CGFloat.infinity, y: 0), copying: false))
        list.configure(devices: [], track: track, document: document, busy: true)
        #expect(!list.accept(board, at: .zero, copying: true))
        #expect(edits.isEmpty)
    }

    @Test func emptyChainKeepsTheWholeViewportAvailableForDrops() throws {
        let scroll = DeviceChainScrollView(frame: CGRect(x: 0, y: 0, width: 520, height: 220))
        let list = DeviceChainListView(frame: .zero)
        scroll.documentView = list
        let track = UUID(), document = UUID()
        list.configure(devices: [], track: track, document: document, busy: false)
        scroll.layout()
        #expect(list.frame.height == scroll.contentSize.height)
        scroll.setFrameSize(CGSize(width: 640, height: 420))
        scroll.layout()
        #expect(list.frame.size == scroll.contentSize)

        var inserted: String?
        list.onInsert = { inserted = $0 }
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        board.setData(try JSONEncoder().encode(FXDrag(document: document, track: nil, device: nil, plugin: "org.example.a")), forType: FXDrag.pluginType)
        let point = CGPoint(x: 10, y: scroll.contentSize.height - 10)
        #expect(list.bounds.contains(point))
        #expect(list.accept(board, at: point, copying: false))
        #expect(inserted == "org.example.a")
    }
}
