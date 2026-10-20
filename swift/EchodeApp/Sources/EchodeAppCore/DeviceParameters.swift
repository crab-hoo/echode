import EchodeBridge
import Foundation

/// A device command's report and the snapshot that completed it.
struct DeviceResult: Sendable {
    let report: DeviceReport
    let snapshot: Snapshot

    @Sendable static func send(_ document: Document, _ command: DeviceCommand) async throws -> DeviceResult {
        let snapshot = try await document.device(command)
        return DeviceResult(report: try snapshot.deviceReport(), snapshot: snapshot)
    }
}

/// The parameters of one device in one open document, as its plugin reported them.
public struct ParameterPanel: Equatable {
    public let document: UUID
    public let device: UUID
    /// Parameters the plugin does not hide, in the plugin's order.
    public internal(set) var parameters: [DeviceParameter] = []
    public internal(set) var loading = true
    public internal(set) var error: String?
}

/// One drag. At most one value is in flight and only the newest one waits.
struct ParameterGesture {
    let document: UUID
    let device: UUID
    let parameter: UInt32
    let original: Double
    var latest: Double
    var pending: Double?
    /// What the plugin reported for the last value it applied.
    var applied: DeviceParameterValue?
}

extension ProjectController {
    /// Shows a device's parameters. Reading them starts its plugin, and the
    /// output engine stopped if playback has not opened it yet.
    public func showParameters(_ device: UUID) async {
        guard let documentID, let row = deviceRow(device), !row.offline else { return }
        if parameterGesture != nil { await finishParameter(commit: true) }
        guard self.documentID == documentID else { return }
        parameterPanel = ParameterPanel(document: documentID, device: device)
        await reloadParameters()
    }

    public func closeParameters() {
        parameterPanel = nil
        parameterGesture = nil
    }

    /// Reads values again, for example after Undo loaded earlier plugin bytes.
    /// Runs outside the edit turn, so an edit body may await it.
    func reloadParameters() async {
        guard let panel = parameterPanel, let document, panel.document == documentID else { return }
        let current = { self.parameterPanel?.document == panel.document && self.parameterPanel?.device == panel.device }
        do {
            let result = try await sendDevice(document, .parameters(device: panel.device))
            guard current() else { return }
            parameterPanel?.parameters = (result.report.parameters ?? []).filter { !$0.hidden }
            parameterPanel?.error = nil
        } catch {
            guard current() else { return }
            parameterPanel?.parameters = []
            parameterPanel?.error = (error as? BridgeError)?.message ?? error.localizedDescription
        }
        parameterPanel?.loading = false
    }

    /// Starts a drag on a parameter the plugin lets the host change.
    @discardableResult
    public func beginParameter(_ id: UInt32) -> Bool {
        guard parameterGesture == nil, !busy, let panel = parameterPanel, panel.document == documentID,
              let parameter = panel.parameters.first(where: { $0.id == id }), !parameter.readOnly else { return false }
        parameterGesture = ParameterGesture(document: panel.document, device: panel.device, parameter: id,
                                            original: parameter.value, latest: parameter.value)
        return true
    }

    /// Sends the newest value of the drag to the running plugin. The project
    /// does not change until the drag ends.
    public func changeParameter(_ value: Double) {
        guard var gesture = parameterGesture, value.isFinite, let document,
              let parameter = parameterPanel?.parameters.first(where: { $0.id == gesture.parameter }) else { return }
        let clamped = Swift.min(Swift.max(value, parameter.min), parameter.max)
        let target = parameter.stepped ? clamped.rounded() : clamped
        guard target != gesture.latest else { return }
        gesture.latest = target
        gesture.pending = target
        parameterGesture = gesture
        updateParameter(gesture.parameter, value: target, text: nil)
        guard parameterTask == nil else { return }
        parameterTask = Task { [weak self] in
            guard let self else { return }
            while let gesture = self.parameterGesture, let value = gesture.pending {
                self.parameterGesture?.pending = nil
                await self.send(gesture, value: value, document: document)
            }
            self.parameterTask = nil
        }
    }

    /// Ends the drag. A commit records the plugin's bytes as one Undo step
    /// when they changed. A cancel puts the original value back, then also
    /// commits: equal bytes record nothing, and the edit no longer defers
    /// the plugin's own background saves.
    public func finishParameter(commit: Bool) async {
        guard parameterGesture != nil else { return }
        if let task = parameterTask { await task.value }
        guard let gesture = parameterGesture else { return }
        parameterGesture = nil
        if let applied = gesture.applied { updateParameter(applied.id, value: applied.value, text: applied.text) }
        guard let document, gesture.document == documentID, gesture.latest != gesture.original else { return }
        if !commit {
            updateParameter(gesture.parameter, value: gesture.original, text: nil)
            await send(gesture, value: gesture.original, document: document)
        }
        await record(device: gesture.device, document: document, documentID: gesture.document)
    }

    /// A keyboard or VoiceOver step, applied and recorded at once.
    @discardableResult
    public func setParameter(_ id: UInt32, value: Double) async -> Bool {
        guard beginParameter(id) else { return false }
        let before = undoCount
        changeParameter(value)
        await finishParameter(commit: true)
        return undoCount == before + 1
    }

    private func send(_ gesture: ParameterGesture, value: Double, document: Document) async {
        do {
            let result = try await sendDevice(document, .set(device: gesture.device, changes: [DeviceParameterChange(id: gesture.parameter, value: value)]))
            guard parameterPanel?.device == gesture.device,
                  let applied = result.report.values?.first(where: { $0.id == gesture.parameter }) else { return }
            if let active = parameterGesture, active.device == gesture.device, active.parameter == gesture.parameter {
                // The slider already shows a newer drag value; keep only the text in step.
                parameterGesture?.applied = applied
                updateParameter(applied.id, value: active.latest, text: applied.text)
            } else {
                updateParameter(applied.id, value: applied.value, text: applied.text)
            }
        } catch {
            if gesture.document == documentID { report(error) }
        }
    }

    @discardableResult
    private func record(device: UUID, document: Document, documentID: UUID) async -> Bool {
        await edits.run {
            guard self.document === document, self.documentID == documentID else { return false }
            do {
                let result = try await self.sendDevice(document, .commit(device: device))
                guard result.report.changed == true else { return false }
                self.publish(result.snapshot)
                self.undoCount += 1
                self.redoCount = 0
                return true
            } catch {
                self.report(error)
                return false
            }
        }
    }

    private func updateParameter(_ id: UInt32, value: Double, text: String?) {
        guard let index = parameterPanel?.parameters.firstIndex(where: { $0.id == id }) else { return }
        parameterPanel?.parameters[index].value = value
        if let text { parameterPanel?.parameters[index].text = text }
    }

    private func deviceRow(_ device: UUID) -> TrackDevice? {
        guard let snapshot else { return nil }
        return tracks.lazy.flatMap { snapshot.devices(on: $0.id) }.first { $0.id == device }
    }
}
