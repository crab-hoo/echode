import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

private actor DisclosureRequests {
    var count = 0
    func send() throws -> DeviceResult { count += 1; throw CancellationError() }
}

@MainActor
struct DeviceDisclosureTests {
    @Test func contractDeviceDisclosureKeepsIdentityAndDropTargets() throws {
        let document = DocumentID(), track = UUID()
        let devices = (0..<3).map { TrackDevice(id: UUID(), plugin: "org.example.\($0)") }
        let strip = DeviceStripView(frame: CGRect(x: 0, y: 0, width: 1200, height: 300))
        let state = DeviceParameterState(document: document, device: devices[0].id)
        state.list([DeviceParameter(id: 7, name: "Gain", min: 0, max: 1, defaultValue: 0, value: 0.4, text: "0.4")], keeping: nil)
        strip.configure(devices: devices, track: track, document: document, busy: false, states: [devices[0].id: state])
        layout(strip)
        let column = try #require(strip.columns.first)
        let disclosure = try #require(column.subviews.compactMap { $0 as? NSButton }.first {
            $0.identifier?.rawValue == "device-disclosure"
        })
        column.search.stringValue = "gain"
        column.search.sendAction(column.search.action, to: column.search.target)
        let width = strip.contentWidth
        var opened: UUID?
        strip.onOpenWindow = { opened = $0 }
        var edits: [DeviceChainEdit] = []
        strip.onEdit = { edits.append($0) }
        disclosure.clickWithoutAnimation()
        layout(strip)
        #expect(strip.contentWidth < width)
        #expect(column.isCollapsed && !strip.columns[1].isCollapsed)
        #expect(column.list.isHidden && column.search.isHidden && column.message.isHidden)
        #expect(!column.title.isHidden && !column.bypass.isHidden && !column.offline.isHidden)
        #expect(disclosure.state == .off && disclosure.accessibilityLabel()?.contains(devices[0].title) == true)
        #expect(column.list.state === state && state.query == "gain")
        #expect(edits.isEmpty && opened == nil)
        #expect(Double(strip.columns[1].frame.minX) == column.preferredWidth)
        #expect(strip.addButton.frame.minX > strip.columns.last!.frame.maxX)
        #expect(column.title.frame.minX >= disclosure.frame.maxX)
        #expect(!column.bypass.frame.intersects(column.offline.frame))

        // A compact name still opens the same GUI, without expanding the column.
        column.layoutSubtreeIfNeeded()
        let point = column.convert(CGPoint(x: column.title.frame.midX, y: column.title.frame.midY), to: nil)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            let event = try #require(NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: 0,
                                                      windowNumber: 0, context: nil, eventNumber: 0, clickCount: 1, pressure: 1))
            if type == .leftMouseDown { column.mouseDown(with: event) } else { column.mouseUp(with: event) }
        }
        #expect(opened == devices[0].id && column.isCollapsed)
        column.bypass.performClick(nil)
        #expect(edits == [.bypass(device: devices[0].id, bypassed: true)])
        column.copy.performClick(nil)
        #expect(edits.last == .copy(source: devices[0].id, track: track, before: devices[1].id))

        // Every boundary uses the displayed width, not a fixed column index.
        strip.columns[2].disclosure.clickWithoutAnimation()
        layout(strip)
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        board.setData(try JSONEncoder().encode(FXDrag(document: document, track: track, device: devices[2].id, plugin: nil)),
                      forType: FXDrag.deviceType)
        var start = 0.0
        for (index, target) in strip.columns.enumerated() {
            let middle = start + target.preferredWidth / 2
            for (x, expected) in [(middle - 1, Optional(devices[index].id)),
                                  (middle + 1, index + 1 < devices.count ? devices[index + 1].id : nil)] {
                #expect(strip.accept(board, at: CGPoint(x: x, y: 8), copying: false))
                #expect(edits.last == .reorder(device: devices[2].id, before: expected))
                #expect(strip.accept(board, at: CGPoint(x: x, y: 8), copying: true))
                #expect(edits.last == .copy(source: devices[2].id, track: track, before: expected))
            }
            start += target.preferredWidth
        }
        board.setData(try JSONEncoder().encode(FXDrag(document: document, track: UUID(), device: devices[0].id, plugin: nil)),
                      forType: FXDrag.deviceType)
        #expect(strip.accept(board, at: .zero, copying: false))
        #expect(edits.last == .copy(source: devices[0].id, track: track, before: devices[0].id))

        // A reply arriving while closed changes the state, not the visible body.
        state.update(7, value: 0.8, text: "0.8")
        disclosure.clickWithoutAnimation()
        layout(strip)
        #expect(!column.isCollapsed && !column.list.isHidden && !column.search.isHidden)
        #expect(column.list.state === state && column.search.stringValue == "gain")
        #expect(column.list.state?.parameter(7)?.value == 0.8)
        #expect(disclosure.state == .on)
    }

    @Test func disclosureIsDocumentLocalAndNeverEditsOrContactsThePlugin() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-disclosure-\(UUID()).echode")
        defer { removeTestFile(url) }
        let requests = DisclosureRequests()
        let controller = ProjectController(sendDevice: { _, _ in try await requests.send() })
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        #expect(await controller.insertPlugin("org.example.fx", on: first, documentID: document))
        let device = try #require(controller.deviceRows(on: first).first)
        await controller.addTrack(name: "second")
        let second = try #require(controller.selection)
        let strip = DeviceStripView(frame: CGRect(x: 0, y: 0, width: 800, height: 300))
        strip.onCollapse = { id, collapsed in controller.setDeviceCollapsed(id, collapsed: collapsed, documentID: document) }
        func refresh(_ track: UUID) {
            strip.configure(devices: controller.deviceRows(on: track), track: track, document: document,
                            busy: false, collapsed: controller.collapsedDevices)
        }
        let before = controller.undoCount
        refresh(first)
        let original = try #require(strip.columns.first)
        original.disclosure.clickWithoutAnimation()
        #expect(controller.collapsedDevices == [device.id])
        #expect(controller.undoCount == before && controller.redoCount == 0)
        #expect(controller.deviceRows(on: first) == [device])
        refresh(second); refresh(first)
        #expect(strip.columns.first === original && original.isCollapsed)
        // Rebuilding the editor also preserves disclosure for this document.
        let rebuilt = DeviceStripView(frame: .zero)
        rebuilt.configure(devices: [device], track: first, document: document, busy: false, collapsed: controller.collapsedDevices)
        #expect(rebuilt.columns.first?.isCollapsed == true)
        // Closing a column preserves it through remove/Undo; a copy starts expanded.
        #expect(await controller.editDeviceChain(.copy(source: device.id, track: first, before: nil), documentID: document))
        refresh(first)
        #expect(strip.columns.map(\.isCollapsed) == [true, false])
        #expect(await controller.editDeviceChain(.reorder(device: device.id, before: nil), documentID: document))
        refresh(first)
        #expect(strip.columns.last === original && original.isCollapsed)
        await controller.undo()
        refresh(first)
        #expect(strip.columns.first === original && original.isCollapsed)
        #expect(await controller.editDeviceChain(.remove(device.id), documentID: document))
        refresh(first)
        #expect(!controller.setDeviceCollapsed(device.id, collapsed: false, documentID: document))
        await controller.undo()
        refresh(first)
        #expect(strip.columns.first === original && original.isCollapsed)
        #expect(controller.redoCount == 1)
        original.disclosure.clickWithoutAnimation()
        #expect(controller.redoCount == 1)
        #expect(!controller.setDeviceCollapsed(UUID(), collapsed: true, documentID: document))
        #expect(!controller.setDeviceCollapsed(device.id, collapsed: true, documentID: DocumentID()))
        original.disclosure.clickWithoutAnimation()
        #expect(controller.collapsedDevices == [device.id])
        #expect(await requests.count == 0)
        await controller.close()
        #expect(controller.collapsedDevices.isEmpty)
        await controller.open(url)
        let reopened = try #require(controller.documentID)
        #expect(reopened != document && controller.collapsedDevices.isEmpty)
        #expect(controller.deviceRows(on: first).count == 2)
        #expect(!controller.setDeviceCollapsed(device.id, collapsed: true, documentID: document))
        await controller.close()
    }

    @Test func offlineLoadingAndFailedColumnsCanBeFoldedAndScrollExtentShrinks() throws {
        let document = DocumentID(), track = UUID()
        var devices = (0..<3).map { TrackDevice(id: UUID(), plugin: "org.example.\($0)") }
        devices[0].offline = true
        let failed = DeviceParameterState(document: document, device: devices[2].id)
        failed.fail("Fixture failed")
        let scroll = DeviceStripScrollView(frame: CGRect(x: 0, y: 0, width: 520, height: 220))
        let strip = DeviceStripView(frame: .zero)
        scroll.documentView = strip
        strip.configure(devices: devices, track: track, document: document, busy: false, states: [devices[2].id: failed])
        scroll.layout(); layout(strip)
        #expect(strip.columns.allSatisfy { !$0.message.isHidden })
        scroll.contentView.scroll(to: CGPoint(x: strip.contentWidth - scroll.contentSize.width, y: 0))
        for column in strip.columns { column.disclosure.clickWithoutAnimation() }
        scroll.layout(); layout(strip)
        #expect(Double(strip.frame.width) == strip.contentWidth)
        #expect(scroll.contentView.bounds.maxX <= strip.frame.maxX)
        #expect(strip.columns.allSatisfy { $0.isCollapsed && $0.message.isHidden && $0.list.isHidden })
        for column in strip.columns { column.disclosure.clickWithoutAnimation() }
        #expect(strip.columns.allSatisfy { !$0.isCollapsed && !$0.message.isHidden })
        strip.configure(devices: devices, track: track, document: document, busy: true)
        strip.columns[0].disclosure.clickWithoutAnimation()
        #expect(!strip.columns[0].isCollapsed && !strip.columns[0].disclosure.isEnabled)
    }

    @Test func foldingReleasesSearchFocusInsteadOfHidingTheActiveEditor() throws {
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 800, height: 300),
                              styleMask: [.borderless], backing: .buffered, defer: true)
        let strip = DeviceStripView(frame: .zero)
        window.contentView = strip
        let device = TrackDevice(id: UUID(), plugin: "org.example.fx")
        strip.configure(devices: [device], track: UUID(), document: DocumentID(), busy: false)
        layout(strip)
        let column = try #require(strip.columns.first)
        #expect(window.makeFirstResponder(column.search))
        let editor = try #require(column.search.currentEditor())
        column.disclosure.clickWithoutAnimation()
        #expect(column.isCollapsed && column.search.isHidden)
        #expect(window.firstResponder !== editor && window.firstResponder !== column.search)
    }

    private func layout(_ strip: DeviceStripView) {
        strip.layout()
        for column in strip.columns { column.layout() }
    }
}
