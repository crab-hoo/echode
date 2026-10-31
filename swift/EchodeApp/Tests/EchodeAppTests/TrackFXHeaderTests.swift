import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct TrackFXHeaderTests {
    @Test func contractHeaderNamesOpenDevicesAndKeepCandidatesSeparate() async throws {
        _ = NSApplication.shared
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-header-fx-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "Track")
        let track = try #require(controller.selection)
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 240, height: 200))
        header.documentID = document
        var opened: [UUID] = []
        var candidates: [UUID] = []
        header.onOpenPlugin = { target, device in
            #expect(target == track)
            opened.append(device)
        }
        header.onOpenFX = { candidates.append($0) }
        func refresh() throws { header.setSnapshot(try #require(controller.snapshot), selected: track) }
        func click(_ rect: CGRect) throws {
            header.mouseDown(with: try #require(NSEvent.mouseEvent(with: .leftMouseDown,
                location: header.convert(CGPoint(x: rect.midX, y: rect.midY), to: nil), modifierFlags: [],
                timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1)))
        }
        try refresh()
        let row = header.camera.rows(trackCount: 1, height: 200).rect(at: 0, width: 240)
        try click(TrackFXLayout(row: row).rect)
        #expect(candidates == [track] && opened.isEmpty)
        #expect(await controller.insertPlugin("example.a", on: track, documentID: document))
        #expect(await controller.insertPlugin("example.b", on: track, documentID: document))
        try refresh()
        let devices = header.headerDevices(on: track)
        let before = controller.undoCount
        let window = NSWindow(contentRect: header.bounds, styleMask: [.borderless], backing: .buffered, defer: true)
        window.contentView = header
        let children = try #require(header.accessibilityChildren()?.first as? NSAccessibilityElement)
        let nameButton = try #require(children.accessibilityChildren()?.compactMap { $0 as? NSAccessibilityElement }
            .first(where: { $0.accessibilityIdentifier() == "\(devices[0].id)/window" }))
        #expect(nameButton.accessibilityLabel() == "example.a Track")
        #expect(nameButton.accessibilityRole() == .button)
        let title = TrackFolderLayout(row: row, depth: 0, folder: false,
                                      titleEnd: TrackFXNamesLayout(row: row, count: devices.count).titleEnd)
        #expect(title.titleX + title.titleWidth < TrackFXNamesLayout(row: row, count: devices.count).name.minX)
        let layout = TrackFXNamesLayout(row: row, count: devices.count)
        try click(layout.name)
        #expect(opened == [devices[0].id], "the inserted name opens its own GUI, not the candidates")
        #expect(candidates == [track])
        try click(TrackFXLayout(row: row).rect)
        #expect(candidates == [track, track])
        header.pluginNames = ["example.b": "Second Effect"]
        let menu = header.deviceMenu(Array(devices.dropFirst()), track: track)
        #expect(menu.items.map(\.title) == ["Second Effect"])
        menu.performActionForItem(at: 0)
        #expect(opened == devices.map(\.id))
        #expect(controller.undoCount == before)
        #expect(nameButton.accessibilityPerformPress())
        await Task.yield()
        #expect(opened == [devices[0].id, devices[1].id, devices[0].id])
        opened = Array(opened.prefix(2))
        #expect(await controller.editDeviceChain(.offline(device: devices[0].id, offline: true), documentID: document))
        try refresh()
        try click(layout.name)
        #expect(opened.count == 2, "offline names do not start a helper")
        let offlineMenu = header.deviceMenu(header.headerDevices(on: track), track: track)
        #expect(!offlineMenu.items[0].isEnabled && offlineMenu.items[1].isEnabled)
        #expect(await controller.editDeviceChain(.remove(devices[1].id), documentID: document))
        try refresh()
        menu.performActionForItem(at: 0)
        #expect(opened.count == 2, "a menu retained across deletion cannot open the removed device")
        // A menu retained across document replacement cannot open a new target.
        let retained = header.deviceMenu([devices[1]], track: track)
        await controller.undo()
        try refresh()
        header.documentID = DocumentID()
        retained.performActionForItem(at: 0)
        #expect(opened.count == 2)
        await controller.close()
    }

    @Test func compactNamesNeverOverlapTheAddButtonOrEscapeTheHeader() {
        for width in [80.0, 100, 160, 240, 500] {
            for count in [0, 1, 2, 2000] {
                let row = CGRect(x: 0, y: -10, width: width, height: 64)
                let layout = TrackFXNamesLayout(row: row, count: count)
                if !layout.name.isEmpty {
                    #expect(layout.name.minX >= 0 && layout.name.maxX < TrackFXLayout(row: row).rect.minX)
                    #expect(layout.overflow.isEmpty || layout.name.maxX <= layout.overflow.minX)
                }
                let deep = TrackFXNamesLayout(row: row, count: count, depth: 100)
                if !deep.name.isEmpty {
                    #expect(deep.name.minX > TrackFolderLayout(row: row, depth: 100, folder: true).disclosure.maxX)
                }
            }
        }
    }
}
