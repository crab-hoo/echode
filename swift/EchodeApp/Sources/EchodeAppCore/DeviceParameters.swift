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
    public let document: DocumentID
    public let device: UUID
    /// Parameters the plugin does not hide, in the plugin's order.
    public private(set) var parameters: [DeviceParameter] = []
    /// True while an empty or failed list is waiting for its first result.
    public private(set) var loading = true
    public private(set) var error: String?
    /// Unknown until a live helper reports it. This is not delay compensation.
    public private(set) var latencySamples: UInt32?
    /// Bumps when the listed ids change, which is the only time rows are rebuilt.
    public private(set) var layout = 0
    /// Bumps when a value or its text changes. Rows on screen are refreshed.
    public private(set) var revision = 0
    /// Search text of this device's list, kept while the app runs and not saved.
    public var query = ""
    /// The newest listing request. An older reply is dropped.
    @ObservationIgnored var request = 0
    @ObservationIgnored private var positions: [UInt32: Int] = [:]
    @ObservationIgnored private(set) var latencyRevision = 0

    init(document: DocumentID, device: UUID) {
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

    /// A failed list can be read again without touching the plugin's state.
    func beginLoading() {
        loading = parameters.isEmpty
        error = nil
    }

    func fail(_ message: String) {
        parameters = []
        setLatency(nil)
        positions = [:]
        error = message
        loading = false
        layout &+= 1
    }

    func setLatency(_ samples: UInt32?) {
        guard latencySamples != samples else { return }
        latencySamples = samples
        latencyRevision &+= 1
        revision &+= 1
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
    let device: UUID
    let parameter: UInt32
    let original: Double
    var latest: Double
    var pending: Double?
    /// What the plugin reported for the last value it applied.
    var applied: DeviceParameterValue?
    /// A value was sent, even if the drag came back to `original`.
    var moved = false
}

extension ProjectController {
    /// Lists the parameters of every running device on `track`. A list read
    /// before keeps its rows while its values are read again. Each device is
    /// read on its own, so a slow plugin delays only its list. Selection and
    /// other edits never wait for this.
    @discardableResult
    public func showDevices(on track: UUID) -> Task<Void, Never> {
        let session = self.session
        guard let documentID = session.id, session.tracks.contains(where: { $0.id == track }) else { return Task {} }
        var reads: [DeviceParameterState] = []
        for device in deviceRows(on: track) where !device.offline {
            let state = session.parameterStates[device.id] ?? DeviceParameterState(document: documentID, device: device.id)
            if session.parameterStates[device.id] !== state { session.parameterStates[device.id] = state }
            reads.append(state)
        }
        return Task { [weak self] in
            await withTaskGroup(of: Void.self) { group in
                for state in reads { group.addTask { await self?.listParameters(state, in: session) } }
            }
        }
    }

    public func parameterState(_ device: UUID) -> DeviceParameterState? { session.parameterStates[device] }

    /// Retries only this failed list. Capture the document and device at the
    /// click, and mark it loading before yielding so repeated clicks coalesce.
    @discardableResult
    public func retryParameters(_ device: UUID, documentID: DocumentID) -> Task<Void, Never> {
        guard let session = inputSession(for: documentID), let state = session.parameterStates[device],
              state.error != nil, !state.loading else { return Task {} }
        state.beginLoading()
        return Task { [weak self] in await self?.listParameters(state, in: session) }
    }

    /// Reads every shown list again, for example after Undo loaded earlier
    /// plugin bytes. Runs after the edit turn, so later edits do not wait.
    func reloadParameters(in session: DocumentSession) async {
        let states = Array(session.parameterStates.values)
        await withTaskGroup(of: Void.self) { group in
            for state in states { group.addTask { await self.listParameters(state, in: session) } }
        }
    }

    /// Reads one list again, for example after a plugin rescanned it.
    func listParameters(_ state: DeviceParameterState, in session: DocumentSession) async {
        guard self.session === session, let document = session.document, session.parameterStates[state.device] === state else { return }
        state.beginLoading()
        state.request &+= 1
        let request = state.request
        let latencyRevision = state.latencyRevision
        // A reply for an older request, a closed document or a removed device is dropped.
        let current = { self.session === session && session.parameterStates[state.device] === state && state.request == request }
        do {
            let result = try await sendDevice(document, .parameters(device: state.device))
            guard current() else { return }
            let dragging = session.parameterGesture.flatMap { $0.device == state.device ? $0.parameter : nil }
            state.list(result.report.parameters ?? [], keeping: dragging)
            // An activity delivered while the list waited is newer than its metadata.
            if state.latencyRevision == latencyRevision { state.setLatency(result.report.latencySamples) }
        } catch {
            guard current() else { return }
            state.fail((error as? BridgeError)?.message ?? error.localizedDescription)
        }
    }

    /// Starts a drag on a parameter the plugin lets the host change.
    @discardableResult
    public func beginParameter(_ id: UInt32, device: UUID, documentID: DocumentID) -> Bool {
        guard let session = session(for: documentID), session.parameterGesture == nil, !session.historyBusy,
              let parameter = session.parameterStates[device]?.parameter(id), !parameter.readOnly else { return false }
        session.parameterGesture = ParameterGesture(device: device, parameter: id, original: parameter.value, latest: parameter.value)
        return true
    }

    /// Sends the newest value of the drag to the running plugin. The project
    /// does not change until the drag ends.
    public func changeParameter(_ value: Double, documentID: DocumentID) {
        guard let session = session(for: documentID), var gesture = session.parameterGesture, value.isFinite, let document = session.document,
              let parameter = session.parameterStates[gesture.device]?.parameter(gesture.parameter) else { return }
        let clamped = Swift.min(Swift.max(value, parameter.min), parameter.max)
        let target = parameter.stepped ? clamped.rounded() : clamped
        guard target != gesture.latest else { return }
        gesture.latest = target
        gesture.pending = target
        gesture.moved = true
        session.parameterGesture = gesture
        session.parameterStates[gesture.device]?.update(gesture.parameter, value: target, text: nil)
        guard session.parameterTask == nil else { return }
        session.parameterTask = Task { [weak self] in
            guard let self else { return }
            while let gesture = session.parameterGesture, let value = gesture.pending {
                session.parameterGesture?.pending = nil
                await self.send(gesture, value: value, document: document, in: session)
            }
            session.parameterTask = nil
        }
    }

    public func finishParameter(commit: Bool, documentID: DocumentID) async {
        guard let session = session(for: documentID) else { return }
        await finishParameter(commit: commit, in: session)
    }

    /// Ends the drag. A commit records the plugin's bytes as one Undo step
    /// when they changed. A cancel puts the original value back, then also
    /// commits: equal bytes record nothing, and the edit no longer defers
    /// the plugin's own background saves. A drag that came back to where it
    /// started commits for the same reason (DOC-A1).
    func finishParameter(commit: Bool, in session: DocumentSession) async {
        guard session.parameterGesture != nil else { return }
        if let task = session.parameterTask { await task.value }
        guard let gesture = session.parameterGesture else { return }
        session.parameterGesture = nil
        let state = session.parameterStates[gesture.device]
        if let applied = gesture.applied { state?.update(applied.id, value: applied.value, text: applied.text) }
        guard self.session === session, let document = session.document, gesture.moved else { return }
        if !commit && gesture.latest != gesture.original {
            state?.update(gesture.parameter, value: gesture.original, text: nil)
            await send(gesture, value: gesture.original, document: document, in: session)
        }
        await record(device: gesture.device, in: session)
    }

    /// A keyboard or VoiceOver step, applied and recorded at once.
    @discardableResult
    public func setParameter(_ id: UInt32, device: UUID, value: Double, documentID: DocumentID) async -> Bool {
        guard beginParameter(id, device: device, documentID: documentID) else { return false }
        let before = undoCount
        changeParameter(value, documentID: documentID)
        await finishParameter(commit: true, documentID: documentID)
        return undoCount == before + 1
    }

    /// Typed text for one parameter, applied and recorded as one Undo step
    /// (ADR 0153). The plugin reads its own units with `text_to_value`; a
    /// plugin without that reads a plain number. False means the text is not
    /// a value in range, so the field stays open. An unchanged value is
    /// accepted and records nothing.
    public func enterParameter(_ text: String, id: UInt32, device: UUID, documentID: DocumentID) async -> Bool {
        guard let session = session(for: documentID), let document = session.document,
              let parameter = session.parameterStates[device]?.parameter(id), !parameter.readOnly else { return false }
        let value: Double?
        do {
            let result = try await sendDevice(document, .parse(device: device, id: id, text: text))
            guard let parsed = result.report.parsed, parsed.id == id else { return false }
            value = parsed.converts ? parsed.value : MixEntry.number(text, unit: "", silence: false)
        } catch {
            if isCurrent(documentID) { report(error) }
            return false
        }
        guard isCurrent(documentID), let value, let current = session.parameterStates[device]?.parameter(id),
              value >= current.min, value <= current.max else { return false }
        // A default shown by the double click before is put back first.
        if session.parameterGesture != nil { await finishParameter(commit: false, in: session) }
        await setParameter(id, device: device, value: value, documentID: documentID)
        return true
    }

    /// Pins a parameter to the top of its device's list, or releases it, as
    /// one Undo step. The id is a CLAP parameter id, not a row number.
    @discardableResult
    public func setPinned(_ parameter: UInt32, device: UUID, pinned: Bool, documentID: DocumentID) async -> Bool {
        await performActions({ [self] in
            guard let row = tracks.lazy.flatMap({ self.deviceRows(on: $0.id) }).first(where: { $0.id == device }),
                  row.pinned.contains(parameter) != pinned else { return nil }
            return [.pinParameter(device: device.uuidString, parameter: parameter, pinned: pinned)]
        }, documentID: documentID)
    }

    private func send(_ gesture: ParameterGesture, value: Double, document: Document, in session: DocumentSession) async {
        do {
            let result = try await sendDevice(document, .set(device: gesture.device, changes: [DeviceParameterChange(id: gesture.parameter, value: value)]))
            guard let state = session.parameterStates[gesture.device],
                  let applied = result.report.values?.first(where: { $0.id == gesture.parameter }) else { return }
            if let active = session.parameterGesture, active.device == gesture.device, active.parameter == gesture.parameter {
                // The slider already shows a newer drag value; keep only the text in step.
                session.parameterGesture?.applied = applied
                state.update(applied.id, value: active.latest, text: applied.text)
            } else {
                state.update(applied.id, value: applied.value, text: applied.text)
            }
        } catch {
            if self.session === session { report(error) }
        }
    }

    /// Records the plugin's bytes as one Undo step when they changed.
    @discardableResult
    func record(device: UUID, in session: DocumentSession) async -> Bool {
        await edits.run {
            guard self.session === session, let document = session.document else { return false }
            do {
                let result = try await self.sendDevice(document, .commit(device: device))
                guard result.report.changed == true else { return false }
                session.publish(result.snapshot)
                session.recordEdit()
                return true
            } catch {
                self.report(error)
                return false
            }
        }
    }
}
