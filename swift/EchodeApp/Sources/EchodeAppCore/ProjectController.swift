import EchodeBridge
import Foundation
import Observation

public struct TrackRow: Identifiable, Sendable {
    public let id: UUID
    public let name: String
}

/// UI state is derived from Rust snapshots. Package IO and shutdown never run
/// on MainActor, and an edit only appears after its journal has been written.
@MainActor @Observable
public final class ProjectController {
    /// The open document's state, replaced whole by `open` and `close`. While
    /// no document is open this is an empty session.
    private(set) var session: DocumentSession
    /// Replacing or closing the document disables workspace input. Ordinary
    /// edits keep their order without changing the controls' enabled state.
    public private(set) var busy = false
    private(set) var pluginNames: [String: String] = [:]
    @ObservationIgnored private var pluginNamesLoading = false
    @ObservationIgnored private var pluginNamesLoaded = false
    public var errorMessage: String?
    public internal(set) var recoveryPrompt: RecoveryPrompt?
    @ObservationIgnored var recoveryReply: CheckedContinuation<Bool, Never>?
    @ObservationIgnored private let sendAudio: @Sendable (Document, AudioCommand) async throws -> AudioState
    /// Opens the output stopped when a project opens, so the first play does not
    /// wait for helpers and media (ADR 0147). The app turns it on; tests and
    /// probes keep their documents off the audio hardware.
    @ObservationIgnored public var preparesOutput = false
    @ObservationIgnored let sendDevice: @Sendable (Document, DeviceCommand) async throws -> DeviceResult
    /// Starts or stops the browser's sample audition. It skips the edit queue.
    @ObservationIgnored let sendAudition: @Sendable (Document, URL?, AuditionOrigin) -> Void
    @ObservationIgnored var sendAuditionAt: @Sendable (Document, URL, UInt64, AuditionOrigin) -> Void = { $0.audition($1, from: $2, origin: $3) }
    @ObservationIgnored var sendAuditionSettings: @Sendable (Document, Bool, Float) -> Void = { _ = $0.configureAudition(visible: $1, gain: $2) }
    /// Ends a replaced document off the main actor. Tests hold it to read the
    /// state views see during shutdown.
    @ObservationIgnored var endDocument: @Sendable (Document) async -> Void = { document in
        await Task.detached { document.shutdown() }.value
    }
    @ObservationIgnored let auditionLog: AuditionLog
    /// One document change at a time. A repeated step waits instead of being dropped.
    @ObservationIgnored let edits = EditTurn()
    @ObservationIgnored var pluginCommand: ((WorkspaceCommand) -> Void)?
    @ObservationIgnored var pluginQuietDelay: Duration = .milliseconds(500)

    /// The open document's state as views and callers read it.
    public var snapshot: Snapshot? { session.snapshot }
    public var tracks: [TrackRow] { session.tracks }
    public var documentID: DocumentID? { session.id }
    public var projectURL: URL? { session.projectURL }
    public var undoCount: Int { session.undoCount }
    public var redoCount: Int { session.redoCount }
    public var selectedTracks: Set<UUID> { session.selectedTracks }
    public var selection: UUID? {
        get { session.selection }
        set { session.selection = newValue }
    }
    public internal(set) var pianoTrack: UUID? {
        get { session.pianoTrack }
        set { session.pianoTrack = newValue }
    }
    var pianoLocation: PianoLocation? {
        get { session.pianoLocation }
        set { session.pianoLocation = newValue }
    }
    var hiddenPianoTracks: Set<UUID> { session.hiddenPianoTracks }
    var noteInput: NoteInputLength {
        get { session.noteInput }
        set { session.noteInput = newValue }
    }
    var noteVelocity: NoteInputVelocity {
        get { session.noteVelocity }
        set { session.noteVelocity = newValue }
    }
    var noteSelection: Set<PianoNote.ID> { session.noteSelection }
    public var noteFocus: PianoNote.ID? {
        get { session.noteFocus }
        set { session.noteFocus = newValue }
    }
    public var clipFocus: UUID? {
        get { session.clipFocus }
        set { session.clipFocus = newValue }
    }
    public var faderPreview: FaderPreview? { session.faderPreview }
    public var audio: AudioState { session.audio }
    public var outputOpening: Bool { session.outputOpening }
    public var playbackStart: Int64 { session.playbackStart }
    public var audioBusy: Bool { session.audioBusy }
    /// The transport bar, both rulers and the transport commands share this.
    /// Only replacing or closing the document locks them.
    public var transportLocked: Bool { snapshot == nil || busy }
    public var loopRange: LoopRange {
        get { session.loopRange }
        set { session.loopRange = newValue }
    }
    public var playback: PlaybackFeed? { session.playback }
    public var transportGeneration: UInt64 { session.transportGeneration }
    public var pendingPlayback: Bool? { session.pendingPlayback }
    public var playbackRequested: Bool { pendingPlayback ?? audio.playing }
    var document: Document? { session.document }
    public var parameterStates: [UUID: DeviceParameterState] { session.parameterStates }
    public var collapsedDevices: Set<UUID> { session.collapsedDevices }
    public var openPluginWindows: Set<UUID> { session.openPluginWindows }
    public var historyBusy: Bool { session.historyBusy }
    var mixerSwitchTail: Task<Bool, Never>? { session.mixerSwitchTail }
    var pluginWindowTask: Task<Void, Never>? { session.pluginWindowTask }

    /// The one comparison with the open document. A stale or missing ID finds
    /// nothing, so every document operation rejects it the same way.
    func session(for id: DocumentID?) -> DocumentSession? {
        guard let id, session.id == id else { return nil }
        return session
    }

    /// Input also stops while the document is being replaced or closed.
    func inputSession(for id: DocumentID?) -> DocumentSession? { busy ? nil : session(for: id) }
    func isCurrent(_ id: DocumentID?) -> Bool { session(for: id) != nil }
    func acceptsInput(_ id: DocumentID?) -> Bool { inputSession(for: id) != nil }

    func selectNotes(_ ids: Set<PianoNote.ID>) { session.selectNotes(ids) }

    public init() {
        let log = AuditionLog()
        auditionLog = log
        session = DocumentSession(auditionLog: log)
        sendAudio = { try await $0.audio($1) }
        sendDevice = DeviceResult.send
        sendAudition = { $0.audition($1, origin: $2) }
        edits.onExclusive = { [weak self] value in self?.busy = value }
    }

    init(sendAudio: @escaping @Sendable (Document, AudioCommand) async throws -> AudioState = { try await $0.audio($1) },
         sendDevice: @escaping @Sendable (Document, DeviceCommand) async throws -> DeviceResult = DeviceResult.send,
         sendAudition: @escaping @Sendable (Document, URL?, AuditionOrigin) -> Void = { $0.audition($1, origin: $2) }) {
        let log = AuditionLog()
        auditionLog = log
        session = DocumentSession(auditionLog: log)
        self.sendAudio = sendAudio
        self.sendDevice = sendDevice
        self.sendAudition = sendAudition
        edits.onExclusive = { [weak self] value in self?.busy = value }
    }

    public func open(_ url: URL, creating: Bool = false) async {
        let request = RecoveryOpenRequest()
        await withTaskCancellationHandler {
            await open(url, creating: creating, request: request)
        } onCancel: {
            request.cancel()
            Task { @MainActor [weak self] in
                guard let self, self.recoveryPrompt?.id == request.id else { return }
                self.respondToRecovery(accept: false)
            }
        }
    }

    private func open(_ url: URL, creating: Bool, request: RecoveryOpenRequest) async {
        if faderPreview != nil { await finishFader(commit: false, in: session) }
        await edits.run(exclusive: true) {
            guard !request.isCancelled else { return }
            if !creating, self.projectURL?.resolvingSymlinksInPath() == url.resolvingSymlinksInPath() { return }
            do {
                let opened = try await Task.detached {
                    try creating ? Document(creating: url.path) : Document(opening: url.path)
                }.value
                guard !request.isCancelled else {
                    await Task.detached { opened.shutdown() }.value
                    return
                }
                var initial = try await opened.snapshot()
                if let notice = try initial.historyReport().recovery {
                    let accepted = await self.confirmRecovery(RecoveryPrompt(url: url, id: request.id, notice: notice), request: request)
                    guard accepted, !request.isCancelled else {
                        await Task.detached { opened.shutdown() }.value
                        return
                    }
                    do { initial = try await opened.history(.acceptRecovery(notice)) }
                    catch {
                        await Task.detached { opened.shutdown() }.value
                        throw error
                    }
                }
                let next = DocumentSession(opening: opened, at: url, audio: try initial.audioState(), auditionLog: self.auditionLog)
                next.publish(initial)
                let previous = self.replaceSession(with: next)
                self.startPluginActivity(next)
                await self.shutDown(previous)
                if self.preparesOutput { self.prepareOutput(opened) }
            } catch { self.report(error) }
        }
    }

    /// The whole document changes at once, so no view sees one document's ID
    /// with another's contents. Returns the old session to shut down.
    private func replaceSession(with next: DocumentSession) -> DocumentSession {
        let previous = session
        previous.retire()
        session = next
        auditionLog.record(next.audio.audition)
        return previous
    }

    private func shutDown(_ session: DocumentSession) async {
        if let document = session.document { await endDocument(document) }
    }

    /// Outside the edit turn: the worker answers at once and prepares on its own
    /// thread. A failure appears in the polled audio state, as for a play.
    private func prepareOutput(_ document: Document) {
        let send = sendAudio
        Task { _ = try? await send(document, .prepare) }
    }

    @discardableResult
    public func importAudio(_ url: URL, on track: UUID? = nil) async -> Bool {
        guard let request = captureAudioImport(url, on: track) else { return false }
        return await importAudio(request)
    }

    public func addTrack(name: String) async {
        guard let documentID else { return }
        await insertTrack(TrackInsertionRequest(document: documentID, after: nil, name: name))
    }

    @discardableResult
    func insertTrack(_ request: TrackInsertionRequest) async -> Bool {
        let action: EditAction
        if let after = request.after {
            action = .insertTrack(after: after.uuidString, intoFolder: request.intoFolder, name: request.name)
        } else {
            action = .addTrack(name: request.name)
        }
        return await performActions({ [action] }, documentID: request.document, selectingAddedTrack: true)
    }

    /// One undo step. Each copy lands right after its source subtree.
    @discardableResult
    func duplicateTracks(_ request: TrackDuplicationRequest) async -> Bool {
        guard isCurrent(request.document), !request.sources.isEmpty,
              Set(request.sources).count == request.sources.count,
              Set(request.sources).isSubset(of: Set(tracks.map(\.id))) else { return false }
        let actions = request.sources.map { EditAction.duplicateTrack(source: $0.uuidString) }
        return await performActions({ actions }, documentID: request.document, selectingAddedTrack: true)
    }

    public func selectTrack(_ id: UUID, change: TrackSelectionChange = .replace, visibleOrder: [UUID]? = nil) {
        session.selectTrack(id, change: change, visibleOrder: visibleOrder)
    }

    func deviceRows(on track: UUID) -> [TrackDevice] {
        (snapshot?.devices(on: track) ?? []).map {
            TrackDevice(id: $0.id, plugin: $0.plugin, name: pluginNames[$0.plugin], bypassed: $0.bypassed,
                        offline: $0.offline, pinned: $0.pinned, windowOpen: openPluginWindows.contains($0.id))
        }
    }

    func rememberPluginNames(_ listings: [PluginListing]) {
        for plugin in listings where !plugin.name.isEmpty { pluginNames[plugin.id] = plugin.name }
        pluginNamesLoaded = true
    }

    func loadPluginNames() async {
        guard !pluginNamesLoaded && !pluginNamesLoading else { return }
        pluginNamesLoading = true
        defer { pluginNamesLoading = false }
        do {
            let listings = try await Task.detached {
                try PluginCatalog.startWatch()
                return try PluginCatalog.listings()
            }.value
            rememberPluginNames(listings)
        } catch {
            // Keep the recorded id visible if metadata is unavailable. The
            // candidate browser reports scan failures without editing a project.
        }
    }

    @discardableResult
    public func insertPlugin(_ plugin: String, on track: UUID? = nil, documentID: DocumentID) async -> Bool {
        guard let target = track ?? selection, tracks.contains(where: { $0.id == target }), !plugin.isEmpty else { return false }
        return await performActions({ [.addDevice(track: target.uuidString, plugin: plugin)] }, documentID: documentID)
    }

    @discardableResult
    func insertPlugin(_ plugin: String, on targets: [UUID], documentID: DocumentID) async -> Bool {
        guard isCurrent(documentID), !plugin.isEmpty, !targets.isEmpty,
              Set(targets).count == targets.count,
              Set(targets).isSubset(of: Set(tracks.map(\.id))) else { return false }
        let actions = targets.map { EditAction.addDevice(track: $0.uuidString, plugin: plugin) }
        return await performActions({ actions }, documentID: documentID)
    }

    public func removeDevice(_ id: UUID, documentID: DocumentID) async {
        _ = await performActions({ [.removeDevice(device: id.uuidString)] }, documentID: documentID)
    }

    @discardableResult
    func editDeviceChain(_ change: DeviceChainEdit, documentID: DocumentID) async -> Bool {
        await performActions({ [self] in
            let command: EditAction
            switch change {
            case .offline(let device, let offline):
                guard let old = tracks.flatMap({ snapshot?.devices(on: $0.id) ?? [] }).first(where: { $0.id == device }), old.offline != offline else { return nil }
                command = .offlineDevice(device: device.uuidString, offline: offline)
            case .bypass(let device, let bypassed):
                guard let old = tracks.flatMap({ snapshot?.devices(on: $0.id) ?? [] }).first(where: { $0.id == device }), old.bypassed != bypassed else { return nil }
                command = .bypassDevice(device: device.uuidString, bypassed: bypassed)
            case .reorder(let device, let before):
                guard let snapshot, let owner = tracks.first(where: { snapshot.devices(on: $0.id).contains(where: { $0.id == device }) }) else { return nil }
                let ids = snapshot.devices(on: owner.id).map(\.id)
                guard let index = ids.firstIndex(of: device) else { return nil }
                if before == device || (before == nil && index == ids.count - 1) || (index + 1 < ids.count && before == ids[index + 1]) { return nil }
                command = .reorderDevice(device: device.uuidString, before: before?.uuidString)
            case .copy(let source, let track, let before):
                command = .copyDevice(source: source.uuidString, track: track.uuidString, before: before?.uuidString)
            case .remove(let device):
                command = .removeDevice(device: device.uuidString)
            }
            return [command]
        }, documentID: documentID)
    }

    @discardableResult
    func dropFX(_ drop: TrackFXDrop, on track: UUID, documentID: DocumentID) async -> Bool {
        guard isCurrent(documentID), tracks.contains(where: { $0.id == track }) else { return false }
        switch drop {
        case .plugin(let plugin): return await addPluginOpeningWindow(plugin, on: track, documentID: documentID)
        case .copyDevice(let device): return await editDeviceChain(.copy(source: device, track: track, before: nil), documentID: documentID)
        case .moveToEnd(let device): return await editDeviceChain(.reorder(device: device, before: nil), documentID: documentID)
        }
    }

    @discardableResult
    public func insertNote(pitch: Int, beat: Double, length: Double, on track: UUID? = nil, documentID: DocumentID) async -> Bool {
        guard let target = track ?? selection, tracks.contains(where: { $0.id == target }),
              let start = Tick.ticks(beat), let duration = Tick.ticks(length),
              NoteGeometry(start: start, length: duration, pitch: pitch).isValid else { return false }
        let velocity = noteVelocity.value
        return await performActions({ [
            .insertNote(track: target.uuidString, pitch: UInt8(pitch), velocity: UInt8(velocity), start: start, length: duration),
        ] }, documentID: documentID, rememberingLength: { duration })
    }

    @discardableResult
    public func setNoteVelocity(_ id: PianoNote.ID, velocity: Int, documentID: DocumentID) async -> Bool {
        guard ModelLimits.midi7.contains(velocity) else { return false }
        return await performActions({ [self] in
            guard let before = snapshot?.pianoNote(id), Int(before.velocity >> 9) != velocity else { return nil }
            return [.setNoteVelocity(track: id.track.uuidString, clip: id.placement.uuidString,
                                     note: id.note.uuidString, velocity: UInt8(velocity))]
        }, documentID: documentID, rememberingVelocity: { velocity })
    }

    @discardableResult
    func setNoteVelocities(_ change: PianoVelocityChange, documentID: DocumentID) async -> Bool {
        await performActions({ [self] in
            change.actions(in: snapshot)
        }, documentID: documentID, rememberingVelocity: { change.anchorValue })
    }

    @discardableResult
    func resizeNotes(_ change: PianoResizeChange, documentID: DocumentID) async -> Bool {
        await performActions({ [self] in change.actions(in: snapshot) }, documentID: documentID)
    }

    @discardableResult
    func moveNotes(_ change: PianoMoveChange, documentID: DocumentID) async -> Bool {
        await performActions({ [self] in change.actions(in: snapshot) }, documentID: documentID,
            selectingNotes: change.copies.isEmpty ? nil : Set(change.originals.keys.compactMap { id in
                change.copies[id.note].map { PianoNote.ID(track: id.track, placement: id.placement, note: $0) }
            }))
    }

    /// Capture native event order synchronously, before the new tasks can run.
    func enqueuePianoStep(_ step: PianoStep, documentID: DocumentID) -> Task<Bool, Never> {
        let session = self.session
        let previous = session.pianoStepTail
        let task = Task { [weak self] in
            _ = await previous?.value
            return await self?.stepNotes(step, documentID: documentID) ?? false
        }
        session.pianoStepTail = task
        return task
    }

    @discardableResult
    func stepNotes(_ step: PianoStep, documentID: DocumentID) async -> Bool {
        await performActions({ [self] in step.actions(in: snapshot) }, documentID: documentID,
            rememberingLength: { [self] in
                guard step.resize, step.ids.count == 1 else { return nil }
                return step.geometries(in: snapshot)?.values.first?.length
            })
    }

    @discardableResult
    public func editNote(_ id: PianoNote.ID, geometry: NoteGeometry, documentID: DocumentID) async -> Bool {
        await performActions({ [self] in
            guard let before = snapshot?.pianoNote(id), geometry.isValid,
                  before.start != geometry.start || before.length != geometry.length || before.pitch != geometry.pitch else { return nil }
            return [.editNote(track: id.track.uuidString, clip: before.id.placement.uuidString,
                              note: id.note.uuidString, pitch: UInt8(geometry.pitch), start: geometry.start, length: geometry.length)]
        }, documentID: documentID, rememberingLength: { [self] in
            guard let before = snapshot?.pianoNote(id), before.length != geometry.length else { return nil }
            return geometry.length
        })
    }

    @discardableResult
    public func deleteNote(_ id: PianoNote.ID, documentID: DocumentID) async -> Bool {
        await deleteNotes([id], documentID: documentID)
    }

    @discardableResult
    func deleteNotes(_ ids: Set<PianoNote.ID>, documentID: DocumentID) async -> Bool {
        await performActions({ [self] in
            guard !ids.isEmpty, ids.allSatisfy({ snapshot?.pianoNote($0) != nil }) else { return nil }
            // Linked placements can expose the same content note more than once.
            // note.delete addresses the content note, not its placement window.
            var seen: Set<UUID> = []
            return ids.sorted { $0.descriptionKey < $1.descriptionKey }.compactMap { id -> EditAction? in
                guard seen.insert(id.note).inserted else { return nil }
                return .deleteNote(track: id.track.uuidString, note: id.note.uuidString)
            }
        }, documentID: documentID)
    }

    public func undo() async {
        let session = self.session
        if session.faderPreview != nil { await finishFader(commit: false, in: session) }
        if session.parameterGesture != nil { await finishParameter(commit: true, in: session) }
        let changed = await edits.run {
            guard self.session === session, session.undoCount > 0, let document = session.document else { return false }
            session.historyBusy = true
            defer { session.historyBusy = false }
            do {
                session.publish(try await document.undo())
                session.noteInput.undo()
                session.undoCount -= 1; session.redoCount += 1
                return true
            }
            catch { self.report(error); return false }
        }
        // Values are read after the turn, so a slow plugin does not hold later edits.
        if changed { await reloadParameters(in: session) }
    }

    public func redo() async {
        let session = self.session
        if session.faderPreview != nil { await finishFader(commit: false, in: session) }
        if session.parameterGesture != nil { await finishParameter(commit: true, in: session) }
        let changed = await edits.run {
            guard self.session === session, session.redoCount > 0, let document = session.document else { return false }
            session.historyBusy = true
            defer { session.historyBusy = false }
            do {
                session.publish(try await document.redo())
                session.noteInput.redo()
                session.redoCount -= 1; session.undoCount += 1
                return true
            }
            catch { self.report(error); return false }
        }
        if changed { await reloadParameters(in: session) }
    }

    /// Newest autosave first. An empty list means there is no commit yet, or the request failed.
    public func savedRevisions() async -> [SavedRevision] {
        await history(.list, in: session)?.revisions ?? []
    }

    /// Restores one autosave as a new version. `changed` is false when that revision is already current.
    public func restoreRevision(_ id: String, documentID: DocumentID) async -> HistoryReport? {
        guard let session = session(for: documentID) else { return nil }
        return await history(.restore(revision: id), in: session)
    }

    public func close() async {
        let target = session
        if target.faderPreview != nil { await finishFader(commit: false, in: target) }
        await edits.run(exclusive: true) {
            guard self.session === target else { return }
            await self.shutDown(self.replaceSession(with: DocumentSession(auditionLog: self.auditionLog)))
        }
    }

    /// Space: playing stops at the remembered start; otherwise plays from
    /// the playhead, which is the start after a stop and the held position
    /// after a pause (ADR 0117, ADR 0155).
    public func togglePlayback() async {
        await audioCommand(playbackRequested ? .stop : .play)
    }

    /// The play button and Return: pause in place, or resume.
    public func togglePause() async {
        await audioCommand(playbackRequested ? .pause : .play)
    }

    public func stopPlayback() async { await audioCommand(.stop) }

    /// Moves the playhead without changing the remembered start.
    public func locate(beat: Double) async {
        guard let tick = Tick.ticks(beat), tick >= 0 else { return }
        await orderedAudioCommand { .locate(tick: tick) }
    }

    /// Rulers and the transport field. Playing, it seeks and keeps playing.
    public func setPlaybackStart(tick: Int64) async {
        guard tick >= 0 else { return }
        await orderedAudioCommand { .setStart(tick: tick) }
    }

    public func setPlaybackStart(beat: Double) async {
        guard let tick = Tick.ticks(beat) else { return }
        await setPlaybackStart(tick: tick)
    }

    /// Reads the loop state on its turn, so a quick second toggle undoes the first.
    public func toggleLoop() async {
        await setLoop { $0.loopStart == nil }
    }

    public func setLoop(enabled: Bool) async {
        await setLoop { _ in enabled }
    }

    /// Sends the current bounds when the loop is on at this command's turn.
    /// A draft never turns an inactive loop on.
    func updateLoopBounds() async {
        await setLoop { $0.loopStart != nil ? true : nil }
    }

    /// `enabled` runs on the command's turn, after earlier commands answered:
    /// true sends the bounds then current, false turns the loop off, nil sends nothing.
    private func setLoop(_ enabled: @escaping @MainActor (AudioState) -> Bool?) async {
        await orderedAudioCommand { [self] in
            guard let on = enabled(audio) else { return nil }
            guard on else { return .loop(start: nil, end: nil) }
            return .loop(start: loopRange.start, end: loopRange.end)
        }
    }

    public func toggleMetronome() async {
        await orderedAudioCommand { [self] in .metronome(enabled: !audio.metronome) }
    }

    public func selectOutput(uid: String?) async { await orderedAudioCommand { .output(uid: uid) } }
    public func refreshOutputs() async { await orderedAudioCommand { .refresh } }

    /// Runs only while the workspace is visible. The renderer reads the live
    /// feed independently; this loop updates native controls and error text.
    public func observeAudio() async {
        while !Task.isCancelled {
            let session = self.session
            if let document = session.document, !busy, !session.audioBusy {
                let revision = session.audioRevision
                do {
                    let state = try await sendAudio(document, .status)
                    if revision == session.audioRevision, !session.audioBusy {
                        if session.audio != state { session.audio = state }
                        if let error = state.error, error != session.reportedAudioError { errorMessage = error }
                        session.reportedAudioError = state.error
                    }
                } catch { if !Task.isCancelled, self.session === session { report(error) } }
            }
            do { try await Task.sleep(for: .milliseconds(50)) }
            catch { return }
        }
    }

    /// Play, pause and stop go at once: the bridge cancels a loading play
    /// without waiting behind it (ADR 0097). Later ordered commands still
    /// start after them.
    private func audioCommand(_ command: AudioCommand) async {
        let session = self.session
        guard !busy, session.document != nil else { return }
        let previous = session.audioTail
        let sent = Task { @MainActor in await self.send(command, in: session) }
        session.audioTail = Task { @MainActor in
            await previous?.value
            await sent.value
        }
        await sent.value
    }

    /// Every other command waits for the one before it instead of disabling
    /// the controls (ADR 0160). `make` runs on the command's turn, so a toggle
    /// reads the state the earlier command left; `nil` sends nothing.
    private func orderedAudioCommand(_ make: @escaping @MainActor () -> AudioCommand?) async {
        let session = self.session
        guard !busy, session.document != nil else { return }
        let previous = session.audioTail
        let turn = Task { @MainActor in
            await previous?.value
            guard self.session === session, !self.busy, let command = make() else { return }
            await self.send(command, in: session)
        }
        session.audioTail = turn
        await turn.value
    }

    private func send(_ command: AudioCommand, in session: DocumentSession) async {
        guard let document = session.document else { return }
        session.audioRevision &+= 1
        let revision = session.audioRevision
        if let intent = command.playbackIntent { session.pendingPlayback = intent }
        session.audioRequests += 1
        session.audioBusy = true
        defer {
            session.audioRequests -= 1
            session.audioBusy = session.audioRequests > 0
            if revision == session.audioRevision { session.pendingPlayback = nil }
        }
        do {
            let state = try await sendAudio(document, command)
            if revision == session.audioRevision {
                session.audio = state
                if command.requestsPlayheadFrame { session.transportGeneration &+= 1 }
            }
        } catch { if revision == session.audioRevision { report(error) } }
    }

    /// Resolve targets and no-op checks after earlier edits publish, retaining typed payloads.
    func performActions(_ actions: @escaping @MainActor () -> [EditAction]?, documentID: DocumentID, finishingPreview: Bool = false, selectingAddedTrack: Bool = false,
                        rememberingLength: @escaping @MainActor () -> Int64? = { nil },
                        rememberingVelocity: @escaping @MainActor () -> Int? = { nil }, selectingNotes: Set<PianoNote.ID>? = nil) async -> Bool {
        await applyEdit(actions, documentID: documentID, finishingPreview: finishingPreview, selectingAddedTrack: selectingAddedTrack, rememberingLength: rememberingLength,
                        rememberingVelocity: rememberingVelocity, selectingNotes: selectingNotes) { document, actions in
            try await document.perform(actions)
        }
    }

    /// Applies one transaction whose later actions name earlier results, such as a new track's ID.
    func performActions(referencing actions: [ReferencingEditAction], documentID: DocumentID, selectingAddedTrack: Bool = false) async -> Bool {
        await applyEdit({ actions }, documentID: documentID, selectingAddedTrack: selectingAddedTrack) { document, actions in
            try await document.perform(referencing: actions)
        }
    }

    private func applyEdit<Fields>(_ fields: @escaping @MainActor () -> Fields?, documentID: DocumentID, finishingPreview: Bool = false,
                                   selectingAddedTrack: Bool = false, rememberingLength: @escaping @MainActor () -> Int64? = { nil },
                                   rememberingVelocity: @escaping @MainActor () -> Int? = { nil }, selectingNotes: Set<PianoNote.ID>? = nil,
                                   submit: @escaping @MainActor (Document, Fields) async throws -> Snapshot) async -> Bool {
        guard faderPreview == nil || finishingPreview else { return false }
        return await edits.run {
            guard let session = self.session(for: documentID), let document = session.document, let fields = fields() else { return false }
            do {
                let inputLength = session.noteInput.change(to: rememberingLength())
                let velocity = rememberingVelocity(), velocityGeneration = session.noteVelocity.generation
                let previousTracks = selectingAddedTrack ? Set(session.tracks.map(\.id)) : []
                let edited = try await submit(document, fields)
                session.publish(edited)
                if selectingAddedTrack { session.selectAddedRoots(previous: previousTracks) }
                if let selectingNotes { session.selectEdited(selectingNotes, in: edited) }
                if try edited.historyReport().changed {
                    session.recordEdit(inputLength: inputLength)
                }
                session.noteVelocity.finish(velocity, generation: velocityGeneration)
                return true
            } catch { self.report(error); return false }
        }
    }

    private func history(_ command: HistoryCommand, in session: DocumentSession) async -> HistoryReport? {
        if session.parameterGesture != nil { await finishParameter(commit: true, in: session) }
        let report: HistoryReport? = await edits.run {
            guard self.session === session, let document = session.document else { return nil }
            session.historyBusy = true
            defer { session.historyBusy = false }
            do {
                let snapshot = try await document.history(command)
                let report = try snapshot.historyReport()
                if report.changed {
                    session.publish(snapshot)
                    session.recordEdit()
                }
                return report
            } catch {
                self.report(error)
                return nil
            }
        }
        if report?.changed == true { await reloadParameters(in: session) }
        return report
    }

    func report(_ error: any Error) {
        errorMessage = (error as? BridgeError)?.message ?? error.localizedDescription
    }
}

extension AudioCommand {
    var playbackIntent: Bool? {
        switch self {
        case .play: true
        case .pause, .stop: false
        default: nil
        }
    }
    /// Playhead frames are requested from the control side. Status polling is not.
    var requestsPlayheadFrame: Bool {
        switch self {
        case .play, .pause, .stop, .locate, .setStart: true
        case .output, .refresh, .status, .prepare, .loop, .metronome, .previewFader, .clearFaderPreview: false
        }
    }
}

extension ProjectController {
    public func mixer(_ track: UUID) -> TrackMix? {
        guard let index = tracks.firstIndex(where: { $0.id == track }) else { return nil }
        return snapshot?.mixer(onTrack: index)
    }

    @discardableResult
    public func setMixer(_ track: UUID, field: MixerField, value: Float, documentID: DocumentID) async -> Bool {
        await performActions({ [self] in
            guard let mix = mixer(track), value.isFinite, mix[field].value != value,
                  field == .gain ? ModelLimits.gain.contains(value) : field == .pan ? ModelLimits.pan.contains(value) : (value == 0 || value == 1) else { return nil }
            return [.setParameter(parameter: mix[field].id.uuidString, value: value)]
        }, documentID: documentID)
    }

    @discardableResult
    public func beginFader(_ track: UUID, field: MixerField, documentID: DocumentID) -> Bool {
        guard let session = inputSession(for: documentID), session.faderPreview == nil,
              field == .gain || field == .pan, let mix = mixer(track) else { return false }
        session.faderPreview = FaderPreview(document: documentID, track: track, field: field,
                                            parameter: mix[field].id, original: mix[field].value,
                                            gain: mix.gain.value, pan: mix.pan.value)
        return true
    }

    public func changeFader(_ value: Float, documentID: DocumentID) {
        guard let session = session(for: documentID), var preview = session.faderPreview, !session.previewFinishing, value.isFinite,
              preview.field == .gain ? ModelLimits.gain.contains(value) : ModelLimits.pan.contains(value),
              let document = session.document else { return }
        if preview.field == .gain { preview.gain = value } else { preview.pan = value }
        session.faderPreview = preview
        session.previewPending = preview
        guard session.previewTask == nil else { return }
        // At most one request is in flight and one latest value is retained.
        // Mouse event frequency never creates an unbounded document queue.
        session.previewTask = Task { [weak self] in
            guard let self else { return }
            while let pending = session.previewPending {
                session.previewPending = nil
                do { _ = try await self.sendAudio(document, .previewFader(track: pending.track, gain: pending.gain, pan: pending.pan)) }
                catch { if self.session === session { self.report(error) } }
            }
            session.previewTask = nil
        }
    }

    public func finishFader(commit: Bool, documentID: DocumentID) async {
        guard let session = session(for: documentID) else { return }
        await finishFader(commit: commit, in: session)
    }

    func finishFader(commit: Bool, in session: DocumentSession) async {
        if let task = session.previewFinishTask { await task.value; return }
        guard let preview = session.faderPreview, !session.previewFinishing, let document = session.document else { return }
        session.previewFinishing = true
        let task = Task { [self] in
            await completeFader(preview, document: document, in: session, commit: commit)
            session.previewFinishTask = nil
            session.previewFinishing = false
        }
        session.previewFinishTask = task
        await task.value
    }

    private func completeFader(_ preview: FaderPreview, document: Document, in session: DocumentSession, commit: Bool) async {
        if let task = session.previewTask { await task.value }
        var applied = false
        if commit, self.session === session,
           let recorded = mixer(preview.track)?[preview.field].value, preview.value != recorded {
            applied = await performActions({ [.setParameter(parameter: preview.parameter.uuidString, value: preview.value)] },
                                           documentID: preview.document, finishingPreview: true)
        }
        if !applied {
            do { _ = try await sendAudio(document, .clearFaderPreview) }
            catch { if self.session === session { report(error) } }
        }
        session.faderPreview = nil; session.previewPending = nil
    }

    @discardableResult
    public func setClipGain(_ id: UUID, gain: Float, documentID: DocumentID) async -> Bool {
        await performActions({ [self] in
            guard let clip = snapshot?.clipMix(id), clip.isAudio, gain.isFinite, gain >= 0, clip.gain != gain else { return nil }
            return [.setClipGain(clip: id.uuidString, gain: gain)]
        }, documentID: documentID)
    }

    @discardableResult
    public func setClipFades(_ id: UUID, fadeIn: Int64, fadeOut: Int64, documentID: DocumentID) async -> Bool {
        await performActions({ [self] in
            guard let clip = snapshot?.clipMix(id), clip.isAudio, fadeIn >= 0, fadeOut >= 0,
                  clip.fadeIn != fadeIn || clip.fadeOut != fadeOut else { return nil }
            return [.setClipFades(clip: id.uuidString, fadeIn: fadeIn, fadeOut: fadeOut)]
        }, documentID: documentID)
    }
}

/// Runs document work one at a time. The body must not call `run` again.
@MainActor
final class EditTurn {
    private var tail: Task<Void, Never>?
    private var latest = 0
    private(set) var busy = false
    private var exclusiveRequests = 0
    var onExclusive: (@MainActor (Bool) -> Void)?

    func run<T: Sendable>(exclusive: Bool = false, _ body: @escaping @MainActor () async -> T) async -> T {
        latest += 1
        let mine = latest
        let previous = tail
        if exclusive {
            exclusiveRequests += 1
            if exclusiveRequests == 1 { onExclusive?(true) }
        }
        let result = Task { @MainActor in
            await previous?.value
            let value = await body()
            if exclusive {
                self.exclusiveRequests -= 1
                if self.exclusiveRequests == 0 { self.onExclusive?(false) }
            }
            if self.latest == mine {
                self.busy = false
                self.tail = nil
            }
            return value
        }
        tail = Task { @MainActor in _ = await result.value }
        busy = true
        return await result.value
    }
}
