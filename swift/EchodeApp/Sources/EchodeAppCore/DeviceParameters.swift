import EchodeBridge
import Foundation
import Observation

/// A device command's report and the snapshot that completed it.
struct DeviceResult: Sendable {
    let report: DeviceReport
    let snapshot: Snapshot

    @Sendable static func send(_ document: Document, _ command: DeviceCommand) async throws -> DeviceResult {
        let snapshot = try await document.device(command)
        return DeviceResult(report: try snapshot.deviceReport(), snapshot: snapshot)
    }
}

/// The parameters one device reported in one open document. A list stays
/// while the document is open, so returning to a track shows it at once.
/// Each device has its own object: a value change redraws only that list.
@MainActor @Observable
public final class DeviceParameterState {
    public let document: UUID
    public let device: UUID
    /// Parameters the plugin does not hide, in the plugin's order.
    public private(set) var parameters: [DeviceParameter] = []
    /// True until the first listing arrives or fails.
    public private(set) var loading = true
    public private(set) var error: String?
    /// Bumps when the listed ids change, which is the only time rows are rebuilt.
    public private(set) var layout = 0
    /// Bumps when a value or its text changes. Rows on screen are refreshed.
    public private(set) var revision = 0
    /// Search text of this device's list, kept while the app runs and not saved.
    public var query = ""
    /// The newest listing request. An older reply is dropped.
    @ObservationIgnored var request = 0
    @ObservationIgnored private var positions: [UInt32: Int] = [:]

    init(document: UUID, device: UUID) {
        self.document = document
        self.device = device
    }

    func parameter(_ id: UInt32) -> DeviceParameter? {
        positions[id].map { parameters[$0] }
    }

    /// Takes a listing. The same ids keep their rows and only values change.
    /// `keeping` is a parameter under the pointer: its slider is not moved.
    func list(_ listed: [DeviceParameter], keeping: UInt32?) {
        let shown = listed.filter { !$0.hidden }
        if !loading, error == nil, shown.map(\.id) == parameters.map(\.id) {
            for (index, parameter) in shown.enumerated() where parameter.id != keeping {
                parameters[index] = parameter
            }
            revision &+= 1
        } else {
            parameters = shown
            positions = Dictionary(shown.enumerated().map { ($1.id, $0) }, uniquingKeysWith: { first, _ in first })
            layout &+= 1
        }
        loading = false
        error = nil
    }

    func fail(_ message: String) {
        parameters = []
        positions = [:]
        error = message
        loading = false
        layout &+= 1
    }

    func update(_ id: UInt32, value: Double, text: String?) {
        guard let index = positions[id] else { return }
        parameters[index].value = value
        if let text { parameters[index].text = text }
        revision &+= 1
    }

    /// Indices into `parameters` in display order. Pinned parameters come
    /// first and stay through any query; the rest follow if they match. Both
    /// keep the plugin's order.
    func rows(pinned: Set<UInt32>) -> [Int] {
        let words = query.split(whereSeparator: \.isWhitespace).map(String.init)
        var top: [Int] = []
        var rest: [Int] = []
        for (index, parameter) in parameters.enumerated() {
            if pinned.contains(parameter.id) {
                top.append(index)
            } else if Self.matches(parameter, words) {
                rest.append(index)
            }
        }
        return top + rest
    }

    /// Every word appears in the name or the module, ignoring case and accents.
    static func matches(_ parameter: DeviceParameter, _ words: [String]) -> Bool {
        let haystack = parameter.module.isEmpty ? parameter.name : parameter.module + " " + parameter.name
        return words.allSatisfy { haystack.localizedStandardContains($0) }
    }
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
    /// Lists the parameters of every running device on `track`. A list read
    /// before keeps its rows while its values are read again. Each device is
    /// read on its own, so a slow plugin delays only its list. Selection and
    /// other edits never wait for this.
    @discardableResult
    public func showDevices(on track: UUID) -> Task<Void, Never> {
        guard let documentID, tracks.contains(where: { $0.id == track }) else { return Task {} }
        var reads: [DeviceParameterState] = []
        for device in deviceRows(on: track) where !device.offline {
            let state = parameterStates[device.id] ?? DeviceParameterState(document: documentID, device: device.id)
            if parameterStates[device.id] !== state { parameterStates[device.id] = state }
            reads.append(state)
        }
        return Task { [weak self] in
            await withTaskGroup(of: Void.self) { group in
                for state in reads { group.addTask { await self?.listParameters(state) } }
            }
        }
    }

    public func parameterState(_ device: UUID) -> DeviceParameterState? {
        guard let state = parameterStates[device], state.document == documentID else { return nil }
        return state
    }

    /// Drops every list, for example when the document closes.
    func forgetParameters() {
        parameterStates = [:]
        parameterGesture = nil
    }

    /// Reads every shown list again, for example after Undo loaded earlier
    /// plugin bytes. Runs after the edit turn, so later edits do not wait.
    func reloadParameters() async {
        let states = Array(parameterStates.values)
        await withTaskGroup(of: Void.self) { group in
            for state in states { group.addTask { await self.listParameters(state) } }
        }
    }

    /// Reads one list again, for example after a plugin rescanned it.
    func relistParameters(_ state: DeviceParameterState) async {
        await listParameters(state)
    }

    private func listParameters(_ state: DeviceParameterState) async {
        guard let document, state.document == documentID else { return }
        state.request &+= 1
        let request = state.request
        // A reply for an older request, a closed document or a removed device is dropped.
        let current = { self.parameterStates[state.device] === state && state.document == self.documentID && state.request == request }
        do {
            let result = try await sendDevice(document, .parameters(device: state.device))
            guard current() else { return }
            let dragging = parameterGesture.flatMap { $0.device == state.device ? $0.parameter : nil }
            state.list(result.report.parameters ?? [], keeping: dragging)
        } catch {
            guard current() else { return }
            state.fail((error as? BridgeError)?.message ?? error.localizedDescription)
        }
    }

    /// Starts a drag on a parameter the plugin lets the host change.
    @discardableResult
    public func beginParameter(_ id: UInt32, device: UUID) -> Bool {
        guard parameterGesture == nil, !historyBusy, let state = parameterState(device),
              let parameter = state.parameter(id), !parameter.readOnly else { return false }
        parameterGesture = ParameterGesture(document: state.document, device: device, parameter: id,
                                            original: parameter.value, latest: parameter.value)
        return true
    }

    /// Sends the newest value of the drag to the running plugin. The project
    /// does not change until the drag ends.
    public func changeParameter(_ value: Double) {
        guard var gesture = parameterGesture, value.isFinite, let document,
              let parameter = parameterState(gesture.device)?.parameter(gesture.parameter) else { return }
        let clamped = Swift.min(Swift.max(value, parameter.min), parameter.max)
        let target = parameter.stepped ? clamped.rounded() : clamped
        guard target != gesture.latest else { return }
        gesture.latest = target
        gesture.pending = target
        parameterGesture = gesture
        updateParameter(gesture.device, gesture.parameter, value: target, text: nil)
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
        if let applied = gesture.applied { updateParameter(gesture.device, applied.id, value: applied.value, text: applied.text) }
        guard let document, gesture.document == documentID, gesture.latest != gesture.original else { return }
        if !commit {
            updateParameter(gesture.device, gesture.parameter, value: gesture.original, text: nil)
            await send(gesture, value: gesture.original, document: document)
        }
        await record(device: gesture.device, document: document, documentID: gesture.document)
    }

    /// A keyboard or VoiceOver step, applied and recorded at once.
    @discardableResult
    public func setParameter(_ id: UInt32, device: UUID, value: Double) async -> Bool {
        guard beginParameter(id, device: device) else { return false }
        let before = undoCount
        changeParameter(value)
        await finishParameter(commit: true)
        return undoCount == before + 1
    }

    /// Pins a parameter to the top of its device's list, or releases it, as
    /// one Undo step. The id is a CLAP parameter id, not a row number.
    @discardableResult
    public func setPinned(_ parameter: UInt32, device: UUID, pinned: Bool, documentID: UUID) async -> Bool {
        guard self.documentID == documentID,
              let row = tracks.lazy.flatMap({ self.deviceRows(on: $0.id) }).first(where: { $0.id == device }),
              row.pinned.contains(parameter) != pinned else { return false }
        return await edit(["type": "device.pin_parameter", "device": device.uuidString,
                           "parameter": parameter, "pinned": pinned], documentID: documentID)
    }

    private func send(_ gesture: ParameterGesture, value: Double, document: Document) async {
        do {
            let result = try await sendDevice(document, .set(device: gesture.device, changes: [DeviceParameterChange(id: gesture.parameter, value: value)]))
            guard parameterState(gesture.device)?.document == gesture.document,
                  let applied = result.report.values?.first(where: { $0.id == gesture.parameter }) else { return }
            if let active = parameterGesture, active.device == gesture.device, active.parameter == gesture.parameter {
                // The slider already shows a newer drag value; keep only the text in step.
                parameterGesture?.applied = applied
                updateParameter(gesture.device, applied.id, value: active.latest, text: applied.text)
            } else {
                updateParameter(gesture.device, applied.id, value: applied.value, text: applied.text)
            }
        } catch {
            if gesture.document == documentID { report(error) }
        }
    }

    /// Records the plugin's bytes as one Undo step when they changed.
    @discardableResult
    func record(device: UUID, document: Document, documentID: UUID) async -> Bool {
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

    private func updateParameter(_ device: UUID, _ id: UInt32, value: Double, text: String?) {
        parameterState(device)?.update(id, value: value, text: text)
    }
}
