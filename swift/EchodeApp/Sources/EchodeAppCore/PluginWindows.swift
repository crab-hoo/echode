import EchodeBridge
import Foundation

/// Reads the document's plugin activity on its own thread and hands each page
/// to the main thread in order. It never waits for the document thread, and
/// stops when asked or when the document closes.
final class PluginActivityReader: @unchecked Sendable {
    private let lock = NSLock()
    private var stopped = false

    init(document: Document, deliver: @escaping @Sendable ([PluginActivity]) -> Void) {
        let thread = Thread { [self] in
            var next: UInt64 = 0
            while !isStopped {
                // A short wait, so a stop request is noticed soon.
                guard let page = try? document.pluginActivity(after: next, timeout: 0.25) else { return }
                next = page.next
                if !page.entries.isEmpty && !isStopped { deliver(page.entries) }
                if page.closed { return }
            }
        }
        thread.name = "echode-plugin-activity"
        thread.start()
    }

    private var isStopped: Bool { lock.withLock { stopped } }

    func stop() { lock.withLock { stopped = true } }
}

/// Plugin windows and the changes a plugin makes in its own window (ADR 0132).
extension ProjectController {
    /// Opens the plugin's own window in its helper, or raises it. The window
    /// never takes keyboard focus. Selection and edits never wait for this.
    @discardableResult
    public func openPluginWindow(_ device: UUID, documentID: UUID) async -> DeviceWindowReport? {
        guard self.documentID == documentID, let document,
              let track = tracks.first(where: { track in deviceRows(on: track.id).contains { $0.id == device } }),
              let row = deviceRows(on: track.id).first(where: { $0.id == device }), !row.offline else { return nil }
        do {
            let result = try await sendDevice(document, .showWindow(device: device, title: "\(track.name) — \(row.title)"))
            guard self.documentID == documentID else { return nil }
            return result.report.window
        } catch {
            if self.documentID == documentID { report(error) }
            return nil
        }
    }

    /// Adds one plugin the user picked as a single device, then opens its
    /// window without focus. Batch adds, copies and track duplication call
    /// the plain insertion and leave windows closed.
    @discardableResult
    func addPluginOpeningWindow(_ plugin: String, on track: UUID, documentID: UUID) async -> Bool {
        let before = Set(deviceRows(on: track).map(\.id))
        guard await insertPlugin(plugin, on: track, documentID: documentID) else { return false }
        if let added = deviceRows(on: track).first(where: { !before.contains($0.id) }) {
            let previous = pluginWindowTask
            pluginWindowTask = Task { [weak self] in
                await previous?.value
                await self?.openPluginWindow(added.id, documentID: documentID)
            }
        }
        return true
    }

    func startPluginActivity(_ document: Document, documentID: UUID) {
        pluginActivity?.stop()
        pluginActivity = PluginActivityReader(document: document) { [weak self] entries in
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.receivePluginActivity(entries, documentID: documentID) }
            }
        }
    }

    func stopPluginActivity() {
        pluginActivity?.stop()
        pluginActivity = nil
        for task in pluginQuiet.values { task.cancel() }
        pluginQuiet = [:]
        pluginGestures = [:]
    }

    /// Applies what plugins did on their own. Values reach each device's list
    /// at once. A finished gesture is recorded as one Undo step; a change
    /// without one, while the plugin's window is open, is recorded once the
    /// plugin has been quiet for `pluginQuietDelay`. Recording only adds a
    /// step when the plugin's bytes changed.
    func receivePluginActivity(_ entries: [PluginActivity], documentID: UUID) {
        guard self.documentID == documentID else { return }
        for entry in entries {
            let device = entry.device
            guard tracks.contains(where: { track in deviceRows(on: track.id).contains { $0.id == device } }) else { continue }
            if entry.windowOpen {
                for name in entry.commands {
                    if let command = WorkspaceCommand(rawValue: name) { pluginCommand?(command) }
                }
            }
            // The parameter under the user's pointer in the device view keeps its slider.
            let dragging = parameterGesture.flatMap { $0.device == device ? $0.parameter : nil }
            for value in entry.values where value.id != dragging {
                parameterState(device)?.update(value.id, value: value.value, text: value.text)
            }
            if entry.rescan || entry.lost, let state = parameterState(device) {
                Task { await self.relistParameters(state) }
            }
            var open = pluginGestures[device] ?? []
            var ended = false
            var plain = false
            for event in entry.events {
                switch event.kind {
                case .begin: open.insert(event.id)
                case .end:
                    open.remove(event.id)
                    ended = true
                case .value: if !open.contains(event.id) { plain = true }
                }
            }
            pluginGestures[device] = open.isEmpty ? nil : open
            // A drag in the device view records this device itself when it ends.
            guard parameterGesture?.device != device else { continue }
            if ended && open.isEmpty {
                pluginQuiet.removeValue(forKey: device)?.cancel()
                recordPluginEdit(device, documentID: documentID)
            } else if open.isEmpty && entry.windowOpen && (plain || entry.dirty || entry.lost) {
                waitForQuiet(device, documentID: documentID)
            }
        }
    }

    /// Waits for every plugin-made edit already scheduled, including quiet
    /// waits that are still running, so tests do not depend on timing.
    func settlePluginEdits() async {
        await pluginWindowTask?.value
        while true {
            if let (device, quiet) = pluginQuiet.first {
                await quiet.value
                if pluginQuiet[device] == quiet { pluginQuiet[device] = nil }
                continue
            }
            guard let task = pluginRecordTask else { return }
            await task.value
            if pluginRecordTask == task { pluginRecordTask = nil }
        }
    }

    private func waitForQuiet(_ device: UUID, documentID: UUID) {
        pluginQuiet.removeValue(forKey: device)?.cancel()
        let delay = pluginQuietDelay
        pluginQuiet[device] = Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard !Task.isCancelled, let self else { return }
            self.pluginQuiet[device] = nil
            guard self.pluginGestures[device] == nil, self.parameterGesture?.device != device else { return }
            self.recordPluginEdit(device, documentID: documentID)
        }
    }

    private func recordPluginEdit(_ device: UUID, documentID: UUID) {
        guard let document, self.documentID == documentID else { return }
        let previous = pluginRecordTask
        pluginRecordTask = Task { [weak self] in
            await previous?.value
            guard let self else { return }
            _ = await self.record(device: device, document: document, documentID: documentID)
        }
    }
}
