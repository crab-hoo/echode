import EchodeBridge
import Foundation
import Observation

/// A transient, explicitly requested list. Reading it restarts plugin processing,
/// so track selection, activity notifications and Undo must never fetch it.
@MainActor @Observable
public final class DeviceAudioConfigState {
    public private(set) var list: DeviceAudioConfigs?
    public private(set) var busy = false
    public private(set) var error: String?
    @ObservationIgnored private var request = 0

    func begin() -> Int? {
        guard !busy else { return nil }
        busy = true
        error = nil
        list = nil
        request &+= 1
        return request
    }

    func finish(_ request: Int, list: DeviceAudioConfigs? = nil, error: String? = nil) {
        busy = false
        guard self.request == request else { return }
        self.list = list
        self.error = error
    }

    func invalidate() {
        request &+= 1
        list = nil
        error = nil
    }
}

extension ProjectController {
    /// Only an explicit click reads presets. Repeated clicks coalesce, and a
    /// reply for a removed/offline device or another document is discarded.
    @discardableResult
    public func loadAudioConfigs(_ device: UUID, documentID: DocumentID) -> Task<Void, Never> {
        guard let session = inputSession(for: documentID), let document = session.document,
            !session.historyBusy,
            let parameters = session.parameterStates[device], let request = parameters.audioConfiguration.begin()
        else { return Task {} }
        let state = parameters.audioConfiguration
        return Task { [weak self] in
            guard let self else { return }
            do {
                let result = try await self.sendDevice(document, .audioConfigs(device: device))
                guard self.session === session, session.parameterStates[device] === parameters else { return }
                state.finish(request, list: result.report.audioConfigs)
            } catch {
                guard self.session === session, session.parameterStates[device] === parameters else { return }
                state.finish(request, error: (error as? BridgeError)?.message ?? error.localizedDescription)
            }
        }
    }

    /// The document lane selects and captures before returning to its worker,
    /// where the ordinary plugin-state edit records exactly one Undo step.
    @discardableResult
    public func selectAudioConfig(_ id: UInt32, device: UUID, documentID: DocumentID) async -> Bool {
        guard let session = inputSession(for: documentID), let parameters = session.parameterStates[device],
            !session.historyBusy, let list = parameters.audioConfiguration.list,
            list.currentID != id, list.configs.contains(where: { $0.id == id })
        else { return false }
        let state = parameters.audioConfiguration
        guard let request = state.begin() else { return false }
        await finishParameter(commit: true, in: session)
        let changed = await edits.run {
            guard self.session === session, let document = session.document,
                session.parameterStates[device] === parameters
            else { return false }
            do {
                let result = try await self.sendDevice(document, .selectAudioConfig(device: device, id: id))
                session.publish(result.snapshot)
                if result.report.changed == true { session.recordEdit() }
                state.finish(request)
                return result.report.changed == true
            } catch {
                state.finish(request, error: (error as? BridgeError)?.message ?? error.localizedDescription)
                return false
            }
        }
        // Descriptions and latency can change with the configuration. This is
        // a parameter read, not another interrupting preset enumeration.
        if self.session === session { await listParameters(parameters, in: session) }
        return changed
    }
}
