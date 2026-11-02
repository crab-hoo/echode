import AppKit
import EchodeBridge
import Foundation
import Observation
import Synchronization
import Testing
@testable import EchodeAppCore

/// Answers device commands like a running CLAP, without an output device.
/// A commit still records a real `device.set_state` edit in the document.
actor FakeParameterPlugin {
    private var gain = 1.0
    private var mode = 0.0
    private var levels: [UInt32: Double] = [:]
    /// The fixture's defaults count as recorded. Its state is gain and mode
    /// only, so a commit that changed neither records nothing.
    private var recorded: String? = FakeParameterPlugin.hash(gain: 1, mode: 0)
    /// Continuous parameters after the fixed four, to exercise long lists.
    /// The count describes this fixture only; the host has no such limit.
    private let extra: Int
    private(set) var sentGains: [Double] = []
    private(set) var sent: [UInt32: Double] = [:]
    private(set) var reads = 0
    /// Window requests in order, with the title each one carried.
    private(set) var windows: [(device: UUID, title: String)] = []
    private var openWindows: Set<UUID> = []
    private var started: [UUID: Int] = [:]
    private var holds: Set<UUID> = []
    private var waiting: [UUID: [CheckedContinuation<Void, Never>]] = [:]
    private var failures: Set<UUID> = []
    /// False acts like a plugin without `text_to_value`.
    private let converts: Bool
    private(set) var parsed: [String] = []
    private(set) var commits = 0

    init(extra: Int = 0, converts: Bool = true) { self.extra = extra; self.converts = converts }

    static func level(_ index: Int) -> UInt32 { UInt32(100 + index) }
    static func hash(gain: Double, mode: Double) -> String {
        String(format: "%064llx", UInt64(gain * 1000) << 8 | UInt64(mode))
    }

    private func table() -> [DeviceParameter] {
        [DeviceParameter(id: 1, flags: 1 << 5, name: "Gain", module: "Output", min: 0, max: 2, defaultValue: 1,
                         value: gain, text: String(format: "%.2fx", gain)),
         DeviceParameter(id: 2, flags: 1 | 1 << 16, name: "Mode", min: 0, max: 2, defaultValue: 0,
                         value: mode, text: ["Clean", "Half", "Mute"][Int(mode)]),
         DeviceParameter(id: 3, flags: 1 << 2, name: "Internal", min: 0, max: 1, defaultValue: 0, value: 0, text: "0.00"),
         DeviceParameter(id: 4, flags: 1 << 3, name: "Meter", min: 0, max: 1, defaultValue: 0, value: 0, text: "0.00")]
            + (0..<extra).map { index in
                let id = Self.level(index)
                let value = levels[id] ?? 0
                return DeviceParameter(id: id, name: "Level \(index)", module: "Bank \(index / 100)", min: 0, max: 1,
                                       defaultValue: 0, value: value, text: String(format: "%.2f", value))
            }
    }

    func failNextListing(_ device: UUID) { failures.insert(device) }

    /// The next listing of `device` waits until `release`.
    func hold(_ device: UUID) { holds.insert(device) }
    func release(_ device: UUID) {
        for waiter in waiting.removeValue(forKey: device) ?? [] { waiter.resume() }
    }
    func listings(_ device: UUID) -> Int { started[device] ?? 0 }
    func setGain(_ value: Double) { gain = value }
    func windowRequests() -> [UUID] { windows.map(\.device) }
    func windowTitles() -> [String] { windows.map(\.title) }

    func send(_ document: Document, _ command: DeviceCommand) async throws -> DeviceResult {
        switch command {
        case .parameters(let device):
            reads += 1
            started[device, default: 0] += 1
            if failures.remove(device) != nil { throw CancellationError() }
            // The reply describes the plugin when the request arrived.
            let listed = table()
            if holds.remove(device) != nil {
                await withCheckedContinuation { waiting[device, default: []].append($0) }
            }
            return DeviceResult(report: DeviceReport(parameters: listed), snapshot: try await document.snapshot())
        case .set(_, let changes):
            for change in changes {
                sent[change.id] = change.value
                if change.id == 1 { gain = change.value; sentGains.append(change.value) }
                if change.id == 2 { mode = change.value }
                if change.id >= 100 { levels[change.id] = change.value }
            }
            let values = table().filter { parameter in changes.contains { $0.id == parameter.id } }
                .map { DeviceParameterValue(id: $0.id, value: $0.value, text: $0.text) }
            return DeviceResult(report: DeviceReport(values: values), snapshot: try await document.snapshot())
        case .showWindow(let device, let title):
            windows.append((device, title))
            let created = openWindows.insert(device).inserted
            return DeviceResult(report: DeviceReport(window: DeviceWindowReport(created: created, visible: true, key: false)),
                                snapshot: try await document.snapshot())
        case .parse(_, let id, let text):
            parsed.append(text)
            // The fixture's own units: "1.25x" and the mode names. Reading text changes nothing.
            let value: Double? = id == 2 ? ["Clean", "Half", "Mute"].firstIndex(of: text).map(Double.init)
                : Double(text.hasSuffix("x") ? String(text.dropLast()) : text)
            return DeviceResult(report: DeviceReport(parsed: DeviceParameterParse(id: id, value: converts ? value : nil, converts: converts)),
                                snapshot: try await document.snapshot())
        case .closeWindow(let device):
            openWindows.remove(device)
            return DeviceResult(report: DeviceReport(closed: true), snapshot: try await document.snapshot())
        case .commit(let device):
            commits += 1
            let hash = Self.hash(gain: gain, mode: mode)
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
    /// The strip with the same wiring as the workspace, in an offscreen window
    /// so the table lays out its rows as on screen.
    @MainActor private final class Harness {
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 640, height: 420), styleMask: [.borderless],
                              backing: .buffered, defer: true)
        let strip = DeviceStripView(frame: CGRect(x: 0, y: 0, width: 640, height: 420))
        var pending: Task<Void, Never>?
        let controller: ProjectController
        let track: UUID
        let document: DocumentID

        init(_ controller: ProjectController, track: UUID, document: DocumentID) {
            self.controller = controller; self.track = track; self.document = document
            window.contentView = strip
            strip.onEdit = { [unowned self] edit in self.pending = Task { _ = await controller.editDeviceChain(edit, documentID: document) } }
            strip.onBegin = { controller.beginParameter($0, device: $1, documentID: document) }
            strip.onChange = { controller.changeParameter($0, documentID: document) }
            strip.onEnd = { [unowned self] commit in self.pending = Task { await controller.finishParameter(commit: commit, documentID: document) } }
            strip.onEnter = { device, id, text in await controller.enterParameter(text, id: id, device: device, documentID: document) }
            strip.onStep = { [unowned self] device, id, value in
                self.pending = Task { _ = await controller.setParameter(id, device: device, value: value, documentID: document) }
            }
            strip.onPin = { [unowned self] device, id, pinned in
                self.pending = Task { _ = await controller.setPinned(id, device: device, pinned: pinned, documentID: document) }
            }
            refresh()
        }

        func refresh() {
            let devices = controller.deviceRows(on: track)
            let states = Dictionary(uniqueKeysWithValues: devices.compactMap { device in
                controller.parameterState(device.id).map { (device.id, $0) }
            })
            strip.configure(devices: devices, track: track, document: document, busy: controller.busy,
                            locked: controller.historyBusy, states: states)
            strip.layoutSubtreeIfNeeded()
            for column in strip.columns { column.list.table.layoutSubtreeIfNeeded() }
        }

        func settle() async {
            await pending?.value
            pending = nil
            refresh()
        }
    }

    private func project(_ name: String, plugin: FakeParameterPlugin, devices: Int = 1) async throws
        -> (ProjectController, DocumentID, UUID, URL) {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-\(name)-\(UUID()).echode")
        let controller = ProjectController(sendDevice: { try await plugin.send($0, $1) })
        await controller.open(url, creating: true)
        let document = try #require(controller.documentID)
        await controller.addTrack(name: "fx")
        let track = try #require(controller.selection)
        for _ in 0..<devices { #expect(await controller.insertPlugin("dev.echode.test-params", on: track, documentID: document)) }
        return (controller, document, track, url)
    }

    @Test func contractParameterDragSoundsLiveAndRecordsOneUndoStep() async throws {
        let plugin = FakeParameterPlugin()
        let (controller, document, track, url) = try await project("parameters", plugin: plugin)
        defer { removeTestFile(url) }
        let device = try #require(controller.deviceRows(on: track).first)

        // Showing the track's devices lists each running plugin's parameters.
        await controller.showDevices(on: track).value
        let state = try #require(controller.parameterState(device.id))
        #expect(!state.loading && state.error == nil)
        // Hidden parameters are not offered; read-only ones are shown but locked.
        #expect(state.parameters.map(\.id) == [1, 2, 4])

        let harness = Harness(controller, track: track, document: document)
        let list = try #require(harness.strip.columns.first?.list)
        let rows = list.visibleRows
        #expect(rows.map(\.parameter.id) == [1, 2, 4])
        let gain = try #require(rows.first)
        let meter = try #require(rows.last)
        #expect(!meter.slider.isEnabled && !controller.beginParameter(4, device: device.id))

        let before = controller.undoCount
        gain.slider.beginDrag()
        for value in [1.2, 1.4, 1.6] {
            gain.slider.doubleValue = value
            gain.slider.sendAction(gain.slider.action, to: gain.slider.target)
        }
        // Values reach the plugin during the drag, but the project waits.
        #expect(controller.undoCount == before)
        #expect(state.parameters.first?.value == 1.6)
        gain.slider.endDrag()
        await harness.settle()
        #expect(await plugin.sentGains.last == 1.6)
        #expect(controller.undoCount == before + 1)
        #expect(state.parameters.first?.text == "1.60x")
        #expect(list.visibleRows.first?.value.stringValue == "1.60x")

        // A keyboard step on the stepped mode is one more Undo step, rounded.
        let mode = list.visibleRows[1]
        mode.slider.doubleValue = 1.2
        mode.slider.sendAction(mode.slider.action, to: mode.slider.target)
        await harness.settle()
        #expect(controller.undoCount == before + 2)
        #expect(state.parameters[1].text == "Half")

        // A cancelled drag puts the value back and records nothing.
        #expect(controller.beginParameter(1, device: device.id))
        controller.changeParameter(0.5)
        await controller.finishParameter(commit: false)
        #expect(await plugin.sentGains.suffix(2) == [0.5, 1.6])
        #expect(controller.undoCount == before + 2)
        // An unchanged value sends nothing and adds no step.
        #expect(!(await controller.setParameter(1, device: device.id, value: 1.6)))

        // Undo reads the restored values back from the plugin, after the edit turn.
        let reads = await plugin.reads
        await controller.undo()
        #expect(controller.undoCount == before + 1 && !controller.busy)
        #expect(await plugin.reads == reads + 1)
        await controller.redo()
        #expect(await plugin.reads == reads + 2)

        // Taking the device offline drops its list; it has no running plugin.
        #expect(await controller.editDeviceChain(.offline(device: device.id, offline: true), documentID: document))
        #expect(controller.parameterState(device.id) == nil)
        await controller.showDevices(on: track).value
        #expect(controller.parameterState(device.id) == nil)
        harness.refresh()
        let column = try #require(harness.strip.columns.first)
        #expect(column.list.isHidden && !column.message.isHidden && !column.search.isEnabled)
        await controller.close()
        #expect(controller.parameterStates.isEmpty)
    }

    /// DOC-A1: a drag that came back to where it started still commits, so
    /// the plugin's own saves stop waiting for an edit that has ended. Equal
    /// bytes add no step. A press without a move sends nothing.
    @Test func parameterDragBackToStartStillCommits() async throws {
        let plugin = FakeParameterPlugin()
        let (controller, document, track, url) = try await project("parameter-back", plugin: plugin)
        defer { removeTestFile(url) }
        let device = try #require(controller.deviceRows(on: track).first)
        await controller.showDevices(on: track).value
        let before = controller.undoCount
        let commits = await plugin.commits
        #expect(controller.beginParameter(1, device: device.id, documentID: document))
        await controller.finishParameter(commit: true, documentID: document)
        #expect(await plugin.commits == commits, "a press without a move has nothing to record")
        for commit in [true, false] {
            #expect(controller.beginParameter(1, device: device.id, documentID: document))
            controller.changeParameter(1.5, documentID: document)
            controller.changeParameter(1, documentID: document)
            await controller.finishParameter(commit: commit, documentID: document)
            #expect(await plugin.sentGains.last == 1)
        }
        #expect(await plugin.commits == commits + 2, "each drag that moved ends its edit")
        #expect(controller.undoCount == before)
    }

    private func mouse(_ type: NSEvent.EventType, _ point: CGPoint, in view: NSView, clicks: Int = 1) throws -> NSEvent {
        try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: [], timestamp: 0,
            windowNumber: view.window?.windowNumber ?? 0, context: nil, eventNumber: 1, clickCount: clicks, pressure: 1))
    }

    private func key(_ characters: String, code: UInt16, in window: NSWindow) throws -> NSEvent {
        try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: window.windowNumber, context: nil, characters: characters,
            charactersIgnoringModifiers: characters, isARepeat: false, keyCode: code))
    }

    /// Clicks one to `count` times; the third press opens the field and is not released.
    private func click(_ view: NSView, at point: CGPoint, count: Int) throws {
        for clicks in 1...count {
            view.mouseDown(with: try mouse(.leftMouseDown, point, in: view, clicks: clicks))
            if clicks < 3 { view.mouseUp(with: try mouse(.leftMouseUp, point, in: view, clicks: clicks)) }
        }
    }

    /// Replaces the field's text and presses Return through its key routing.
    private func type(_ text: String, into field: ValueEntryField) async throws {
        let window = try #require(field.window)
        let editor = try #require(field.currentEditor() as? NSTextView)
        editor.selectAll(nil)
        editor.insertText(text, replacementRange: NSRange(location: NSNotFound, length: 0))
        #expect(field.performKeyEquivalent(with: try key("\r", code: 36, in: window)))
        await field.pending?.value
    }

    /// The shared value-control clicks on a real parameter row (ADR 0153). A
    /// plugin with `text_to_value` reads its own units; one without reads numbers.
    @Test func contractParameterClicksResetTypeInPluginUnitsAndRecordOnce() async throws {
        for converts in [true, false] {
            let plugin = FakeParameterPlugin(converts: converts)
            let (controller, document, track, url) = try await project("value-clicks", plugin: plugin)
            defer { removeTestFile(url) }
            let device = try #require(controller.deviceRows(on: track).first)
            await controller.showDevices(on: track).value
            let state = try #require(controller.parameterState(device.id))
            let harness = Harness(controller, track: track, document: document)
            let list = try #require(harness.strip.columns.first?.list)
            let gain = try #require(list.visibleRows.first)
            let slider = gain.slider
            slider.clicks.interval = 0.01
            let before = controller.undoCount
            let travel = Double(slider.bounds.width) - slider.knobThickness
            let near = CGPoint(x: slider.bounds.maxX - 4, y: slider.bounds.midY)

            // A click away from the knob does not jump there.
            try click(slider, at: near, count: 1)
            await harness.settle()
            #expect(await plugin.sentGains.isEmpty)
            #expect(state.parameter(1)?.value == 1 && controller.undoCount == before)

            // A drag moves from the pressed value: a quarter of the travel is a quarter of 0...2.
            slider.mouseDown(with: try mouse(.leftMouseDown, near, in: slider))
            let left = CGPoint(x: near.x - travel / 4, y: near.y)
            slider.mouseDragged(with: try mouse(.leftMouseDragged, left, in: slider))
            slider.mouseUp(with: try mouse(.leftMouseUp, left, in: slider))
            await harness.settle()
            #expect(abs(try #require(state.parameter(1)).value - 0.5) < 0.0001)
            #expect(controller.undoCount == before + 1)

            // The third click puts the value back and opens the plugin's text.
            try click(slider, at: near, count: 3)
            await harness.settle()
            #expect(await plugin.sentGains.suffix(2) == [1, try #require(state.parameter(1)).value])
            #expect(controller.undoCount == before + 1)
            let field = try #require(gain.entry)
            #expect(field.stringValue == "0.50x")
            try await type(converts ? "0.25x" : "0.25", into: field)
            #expect(field.closed && gain.entry == nil)
            #expect(await plugin.parsed.last == (converts ? "0.25x" : "0.25"))
            #expect(state.parameter(1)?.value == 0.25)
            #expect(controller.undoCount == before + 2)

            // A double click shows the plugin's default, then records it once.
            try click(slider, at: near, count: 2)
            #expect(state.parameter(1)?.value == 1 && controller.undoCount == before + 2)
            try await Task.sleep(for: .milliseconds(100))
            await harness.settle()
            #expect(await plugin.sentGains.last == 1)
            #expect(controller.undoCount == before + 3)

            let mode = try #require(list.visibleRows.dropFirst().first)
            try click(mode.slider, at: CGPoint(x: mode.slider.bounds.midX, y: mode.slider.bounds.midY), count: 3)
            await harness.settle()
            let modeField = try #require(mode.entry)
            #expect(modeField.stringValue == "Clean")
            if converts {
                try await type("Mute", into: modeField)
                #expect(state.parameter(2)?.text == "Mute" && controller.undoCount == before + 4)
            } else {
                // Without a converter the plugin's words are not numbers, and 5 is out of range.
                for refused in ["Mute", "5"] {
                    try await type(refused, into: modeField)
                    #expect(modeField.invalid && !modeField.closed)
                }
                #expect(modeField.performKeyEquivalent(with: try key("\u{1b}", code: 53, in: harness.window)))
                #expect(modeField.closed && state.parameter(2)?.value == 0 && controller.undoCount == before + 3)
            }
            await controller.undo()
            #expect(controller.undoCount == before + (converts ? 3 : 2))
            await controller.close()
        }
    }

    /// Device edits keep the workspace enabled for their entire edit turn,
    /// including the state capture at the end of a parameter gesture.
    @Test func contractDeviceEditsNeverDisableTheWorkspace() async throws {
        let plugin = FakeParameterPlugin()
        let (controller, document, track, url) = try await project("enabled", plugin: plugin)
        defer { removeTestFile(url) }
        await controller.showDevices(on: track).value
        let harness = Harness(controller, track: track, document: document)
        let column = try #require(harness.strip.columns.first)
        let gain = try #require(column.list.visibleRows.first)

        // Observe every transition, so even a lock that clears before the
        // request returns fails. A final-state assertion misses the flash.
        func staysEnabled(_ operation: @MainActor () async -> Void) async {
            let changes = Mutex(0)
            withObservationTracking { _ = controller.busy } onChange: {
                changes.withLock { $0 += 1 }
            }
            await operation()
            #expect(changes.withLock { $0 } == 0, "a device edit disabled the whole workspace")
        }
        let before = controller.undoCount
        await staysEnabled {
            gain.slider.beginDrag()
            gain.slider.doubleValue = 1.5
            gain.slider.sendAction(gain.slider.action, to: gain.slider.target)
            gain.slider.endDrag()
            await harness.settle()
        }
        await staysEnabled { column.bypass.performClick(nil); await harness.settle() }
        await staysEnabled { column.offline.performClick(nil); await harness.settle() }
        #expect(controller.undoCount == before + 3)
        #expect(controller.deviceRows(on: track).first?.bypassed == true)
        #expect(controller.deviceRows(on: track).first?.offline == true)
        await controller.close()
    }

    /// A plugin that is slow to list its parameters delays only its own list.
    /// Selection, unrelated edits, transport and other devices go on, and a
    /// late reply never overwrites a newer list or revives a removed device.
    @Test func contractSlowPluginListingDoesNotHoldSelectionOrEdits() async throws {
        let plugin = FakeParameterPlugin()
        let (controller, document, first, url) = try await project("slow", plugin: plugin)
        defer { removeTestFile(url) }
        await controller.addTrack(name: "other")
        let second = try #require(controller.selection)
        #expect(await controller.insertPlugin("dev.echode.test-params", on: second, documentID: document))
        let slow = try #require(controller.deviceRows(on: first).first?.id)
        let other = try #require(controller.deviceRows(on: second).first?.id)

        await plugin.hold(slow)
        controller.selectTrack(first)
        let held = controller.showDevices(on: first)
        while await plugin.listings(slow) == 0 { await Task.yield() }
        #expect(controller.parameterState(slow)?.loading == true)
        #expect(!controller.busy)

        // Each of these finishes while the reply is still held.
        controller.selectTrack(second)
        #expect(controller.selection == second)
        let undoBefore = controller.undoCount
        #expect(await controller.setMixer(second, field: .gain, value: 0.5, documentID: document))
        #expect(controller.undoCount == undoBefore + 1)
        await controller.stopPlayback()
        #expect(controller.errorMessage == nil)
        await controller.showDevices(on: second).value
        #expect(controller.parameterState(other)?.parameters.map(\.id) == [1, 2, 4])
        // Undo also reads every shown list again; the held one is not waited for twice.
        await plugin.setGain(1.5)
        await controller.undo()
        #expect(controller.undoCount == undoBefore)
        #expect(controller.parameterState(slow)?.parameters.first?.value == 1.5)
        #expect(controller.parameterState(slow)?.loading == false)

        // The older reply lands last and is dropped.
        await plugin.release(slow)
        await held.value
        #expect(controller.parameterState(slow)?.parameters.first?.value == 1.5)

        // A reply for a device removed meanwhile does not bring its list back.
        await plugin.hold(other)
        let removed = controller.showDevices(on: second)
        while await plugin.listings(other) < 3 { await Task.yield() }
        #expect(await controller.editDeviceChain(.remove(other), documentID: document))
        #expect(controller.parameterState(other) == nil)
        await plugin.release(other)
        await removed.value
        #expect(controller.parameterState(other) == nil)

        // Nor does a reply for a document that closed.
        await plugin.hold(slow)
        let closing = controller.showDevices(on: first)
        while await plugin.listings(slow) < 3 { await Task.yield() }
        await controller.close()
        await plugin.release(slow)
        await closing.value
        #expect(controller.parameterStates.isEmpty && controller.documentID == nil)
    }

    /// Thousands of parameters build only the rows on screen. A parameter
    /// reached by scrolling, search or a pin is edited by its CLAP id, and each
    /// instance of the same plugin keeps its own pins through Undo and reopen.
    @Test func contractLongListsBuildVisibleRowsAndPinsStayPerDevice() async throws {
        let count = 3000
        let plugin = FakeParameterPlugin(extra: count)
        let (controller, document, track, url) = try await project("long", plugin: plugin, devices: 2)
        defer { removeTestFile(url) }
        let devices = controller.deviceRows(on: track)
        #expect(devices.count == 2)
        await controller.showDevices(on: track).value
        let harness = Harness(controller, track: track, document: document)
        let column = try #require(harness.strip.columns.first)
        let list = column.list
        #expect(list.table.numberOfRows == count + 3)
        let built = list.visibleRows.count
        #expect(built > 0 && built <= Int(list.bounds.height / DeviceParameterListView.rowHeight) + 2)
        // Rows scrolled away are not views; nothing like one row per parameter exists.
        var made = 0
        for row in 0..<list.table.numberOfRows where list.table.view(atColumn: 0, row: row, makeIfNecessary: false) != nil { made += 1 }
        #expect(made <= built + 4)

        // An offscreen parameter, scrolled to and dragged, reaches its own id.
        let target = FakeParameterPlugin.level(count - 1)
        let far = try #require(list.reveal(target))
        #expect(far.parameter.id == target && far.name.stringValue == "Level \(count - 1)")
        far.slider.beginDrag()
        far.slider.doubleValue = 0.75
        far.slider.sendAction(far.slider.action, to: far.slider.target)
        far.slider.endDrag()
        await harness.settle()
        #expect(await plugin.sent[target] == 0.75)
        #expect(await plugin.sent.count == 1)

        // Search narrows by every word in the module or name.
        column.search.stringValue = "bank 29 level 2999"
        column.search.sendAction(column.search.action, to: column.search.target)
        #expect(list.order.map { controller.parameterState(devices[0].id)!.parameters[$0].id } == [target])
        // A pinned row stays at the top whatever the query.
        let before = controller.undoCount
        #expect(await controller.setPinned(1, device: devices[0].id, pinned: true, documentID: document))
        #expect(controller.undoCount == before + 1)
        harness.refresh()
        #expect(list.visibleRows.map(\.parameter.id) == [1, target])
        #expect(list.visibleRows.first?.pin.state == .on)
        column.search.stringValue = ""
        column.search.sendAction(column.search.action, to: column.search.target)
        list.table.scrollRowToVisible(0)
        harness.refresh()
        #expect(list.visibleRows.prefix(3).map(\.parameter.id) == [1, 2, 4])
        // The pin button records a second pin; the plugin's order holds among pins.
        let pinButton = try #require(list.reveal(target)?.pin)
        pinButton.performClick(nil)
        await harness.settle()
        #expect(controller.deviceRows(on: track)[0].pinned == [1, target])
        list.table.scrollRowToVisible(0)
        harness.refresh()
        #expect(list.visibleRows.prefix(3).map(\.parameter.id) == [1, target, 2])
        // The other instance keeps its own list and pins.
        #expect(controller.deviceRows(on: track)[1].pinned.isEmpty)
        #expect(harness.strip.columns[1].list.visibleRows.first?.parameter.id == 1)
        #expect(harness.strip.columns[1].list.visibleRows.first?.pin.state == .off)

        await controller.undo()
        #expect(controller.deviceRows(on: track)[0].pinned == [1])
        await controller.redo()
        #expect(controller.deviceRows(on: track)[0].pinned == [1, target])
        // A copy inherits the pins under a new identity.
        #expect(await controller.editDeviceChain(.copy(source: devices[0].id, track: track, before: nil), documentID: document))
        let copied = controller.deviceRows(on: track)
        #expect(copied.count == 3 && copied[2].pinned == [1, target] && copied[2].id != devices[0].id)
        await controller.close(); await controller.open(url)
        #expect(controller.deviceRows(on: track).map(\.pinned) == [[1, target], [], [1, target]])
        let current = try #require(controller.documentID)
        #expect(!(await controller.setPinned(1, device: devices[0].id, pinned: true, documentID: current)))
        #expect(!(await controller.setPinned(2, device: devices[0].id, pinned: true, documentID: document)))
        await controller.close()
    }
}
