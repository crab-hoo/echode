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
    public private(set) var snapshot: Snapshot?
    public private(set) var tracks: [TrackRow] = []
    public private(set) var documentID: UUID?
    public private(set) var projectURL: URL?
    public private(set) var busy = false
    public private(set) var undoCount = 0
    public private(set) var redoCount = 0
    private(set) var pluginNames: [String: String] = [:]
    @ObservationIgnored private var pluginNamesLoading = false
    @ObservationIgnored private var pluginNamesLoaded = false
    public var selection: UUID? {
        didSet {
            if let selection, !tracks.contains(where: { $0.id == selection }) { self.selection = nil }
            if selection != oldValue { noteFocus = nil; clipFocus = nil }
        }
    }
    public var noteFocus: PianoNote.ID? {
        didSet {
            if let noteFocus, noteFocus.track != selection || snapshot?.pianoNote(noteFocus) == nil { self.noteFocus = nil }
        }
    }
    public var errorMessage: String?
    public var clipFocus: UUID? {
        didSet { if let clipFocus, snapshot?.clipMix(clipFocus)?.track != selection { self.clipFocus = nil } }
    }
    public private(set) var faderPreview: FaderPreview?
    @ObservationIgnored private var previewPending: FaderPreview?
    @ObservationIgnored private var previewTask: Task<Void, Never>?
    @ObservationIgnored private var previewFinishing = false
    @ObservationIgnored private var previewFinishTask: Task<Void, Never>?
    public private(set) var audio = AudioState()
    public private(set) var audioBusy = false
    /// Loop bounds in beats. Transport state, not part of the project.
    public var loopStartBeat = 0.0
    public var loopEndBeat = 4.0
    public private(set) var playback: PlaybackFeed?
    /// Bumps on play, pause, stop, and locate. Position polling does not.
    public private(set) var transportGeneration: UInt64 = 0
    public private(set) var pendingPlayback: Bool?
    public var playbackRequested: Bool { pendingPlayback ?? audio.playing }
    @ObservationIgnored private var audioRequests = 0
    @ObservationIgnored private var audioRevision: UInt64 = 0
    @ObservationIgnored private var document: Document?
    @ObservationIgnored private var reportedAudioError: String?
    @ObservationIgnored private let sendAudio: @Sendable (Document, AudioCommand) async throws -> AudioState
    /// One document change at a time. A repeated step waits instead of being dropped.
    @ObservationIgnored private let edits = EditTurn()

    public init() {
        sendAudio = { try await $0.audio($1) }
        edits.onBusy = { [weak self] value in self?.busy = value }
    }

    init(sendAudio: @escaping @Sendable (Document, AudioCommand) async throws -> AudioState) {
        self.sendAudio = sendAudio
        edits.onBusy = { [weak self] value in self?.busy = value }
    }

    public func open(_ url: URL, creating: Bool = false) async {
        if faderPreview != nil { await finishFader(commit: false) }
        await edits.run {
            if !creating, self.projectURL?.resolvingSymlinksInPath() == url.resolvingSymlinksInPath() { return }
            do {
                let opened = try await Task.detached {
                    try creating ? Document(creating: url.path) : Document(opening: url.path)
                }.value
                let initial = try await opened.snapshot()
                let initialAudio = try initial.audioState()
                let previous = self.document
                self.document = opened
                self.audioRevision &+= 1
                self.pendingPlayback = nil
                self.documentID = UUID()
                self.playback = opened.playbackFeed
                self.transportGeneration = 0
                self.audio = initialAudio
                self.reportedAudioError = nil
                self.loopStartBeat = 0
                self.loopEndBeat = 4
                self.projectURL = url
                self.undoCount = 0
                self.redoCount = 0
                self.selection = nil
                self.noteFocus = nil
                self.publish(initial)
                if let previous { await Task.detached { previous.shutdown() }.value }
            } catch { self.report(error) }
        }
    }

    @discardableResult
    public func importAudio(_ url: URL, on track: UUID? = nil) async -> Bool {
        guard let target = track ?? selection, tracks.contains(where: { $0.id == target }) else { return false }
        guard audio.tick >= 0 else { return false }
        return await edit([
            "type": "audio.import",
            "track": target.uuidString,
            "start": audio.tick,
            "path": url.path,
        ])
    }

    public func addTrack(name: String) async {
        guard document != nil else { return }
        guard await edit(["type": "track.add", "name": name]) else { return }
        if let last = tracks.last { selection = last.id }
    }

    func deviceRows(on track: UUID) -> [TrackDevice] {
        (snapshot?.devices(on: track) ?? []).map { TrackDevice(id: $0.id, plugin: $0.plugin, name: pluginNames[$0.plugin], bypassed: $0.bypassed) }
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
    public func insertPlugin(_ plugin: String, on track: UUID? = nil, documentID: UUID? = nil) async -> Bool {
        guard let target = track ?? selection, tracks.contains(where: { $0.id == target }), !plugin.isEmpty else { return false }
        return await edit(["type": "device.add", "track": target.uuidString, "plugin": plugin], documentID: documentID)
    }

    public func removeDevice(_ id: UUID, documentID: UUID? = nil) async {
        await edit(["type": "device.remove", "device": id.uuidString], documentID: documentID)
    }

    @discardableResult
    func editDeviceChain(_ change: DeviceChainEdit, documentID: UUID) async -> Bool {
        guard self.documentID == documentID else { return false }
        var command: [String: Any]
        switch change {
        case .bypass(let device, let bypassed):
            guard let old = tracks.flatMap({ snapshot?.devices(on: $0.id) ?? [] }).first(where: { $0.id == device }), old.bypassed != bypassed else { return false }
            command = ["type": "device.bypass", "device": device.uuidString, "bypassed": bypassed]
        case .reorder(let device, let before):
            guard let snapshot, let owner = tracks.first(where: { snapshot.devices(on: $0.id).contains(where: { $0.id == device }) }) else { return false }
            let ids = snapshot.devices(on: owner.id).map(\.id)
            guard let index = ids.firstIndex(of: device) else { return false }
            if before == device || (before == nil && index == ids.count - 1) || (index + 1 < ids.count && before == ids[index + 1]) { return false }
            command = ["type": "device.reorder", "device": device.uuidString]
            if let before { command["before"] = before.uuidString }
        case .copy(let source, let track, let before):
            command = ["type": "device.copy", "source": source.uuidString, "track": track.uuidString]
            if let before { command["before"] = before.uuidString }
        case .remove(let device):
            command = ["type": "device.remove", "device": device.uuidString]
        }
        return await edit(command, documentID: documentID)
    }

    @discardableResult
    func dropFX(_ drop: TrackFXDrop, on track: UUID, documentID: UUID) async -> Bool {
        guard self.documentID == documentID, tracks.contains(where: { $0.id == track }) else { return false }
        switch drop {
        case .plugin(let plugin): return await insertPlugin(plugin, on: track, documentID: documentID)
        case .copyDevice(let device): return await editDeviceChain(.copy(source: device, track: track, before: nil), documentID: documentID)
        case .moveToEnd(let device): return await editDeviceChain(.reorder(device: device, before: nil), documentID: documentID)
        }
    }

    @discardableResult
    public func insertNote(pitch: Int, beat: Double, length: Double, on track: UUID? = nil, documentID: UUID? = nil) async -> Bool {
        guard let target = track ?? selection, tracks.contains(where: { $0.id == target }),
              let start = PianoCamera.ticks(beat), let duration = PianoCamera.ticks(length),
              NoteGeometry(start: start, length: duration, pitch: pitch).isValid else { return false }
        return await edit([
            "type": "note.insert", "track": target.uuidString,
            "pitch": pitch, "velocity": 100, "start": start, "length": duration,
        ], documentID: documentID)
    }

    @discardableResult
    public func editNote(_ id: PianoNote.ID, geometry: NoteGeometry, documentID: UUID? = nil) async -> Bool {
        guard let before = snapshot?.pianoNote(id), geometry.isValid,
              before.start != geometry.start || before.length != geometry.length || before.pitch != geometry.pitch else { return false }
        return await edit(["type": "note.edit", "track": id.track.uuidString, "clip": before.id.placement.uuidString,
                          "note": id.note.uuidString, "pitch": geometry.pitch, "start": geometry.start, "length": geometry.length], documentID: documentID)
    }

    @discardableResult
    public func deleteNote(_ id: PianoNote.ID, documentID: UUID? = nil) async -> Bool {
        guard snapshot?.pianoNote(id) != nil else { return false }
        return await edit(["type": "note.delete", "track": id.track.uuidString, "note": id.note.uuidString], documentID: documentID)
    }

    public func undo() async {
        let target = documentID
        if faderPreview != nil { await finishFader(commit: false) }
        await edits.run {
            guard self.documentID == target, self.undoCount > 0, let document = self.document else { return }
            do { self.publish(try await document.undo()); self.undoCount -= 1; self.redoCount += 1 }
            catch { self.report(error) }
        }
    }

    public func redo() async {
        let target = documentID
        if faderPreview != nil { await finishFader(commit: false) }
        await edits.run {
            guard self.documentID == target, self.redoCount > 0, let document = self.document else { return }
            do { self.publish(try await document.redo()); self.redoCount -= 1; self.undoCount += 1 }
            catch { self.report(error) }
        }
    }

    /// Newest autosave first. An empty list means there is no commit yet, or the request failed.
    public func savedRevisions() async -> [SavedRevision] {
        await history(.list)?.revisions ?? []
    }

    /// Restores one autosave as a new version. `changed` is false when that revision is already current.
    public func restoreRevision(_ id: String) async -> HistoryReport? {
        await history(.restore(revision: id))
    }

    public func close() async {
        let target = documentID
        if faderPreview != nil { await finishFader(commit: false) }
        await edits.run {
            guard self.documentID == target else { return }
            let previous = self.document
            self.document = nil
            self.audioRevision &+= 1
            self.pendingPlayback = nil
            self.documentID = nil
            self.playback = nil
            self.transportGeneration = 0
            if let previous { await Task.detached { previous.shutdown() }.value }
            self.snapshot = nil
            self.tracks = []
            self.projectURL = nil
            self.selection = nil
            self.noteFocus = nil
            self.clipFocus = nil
            self.undoCount = 0
            self.redoCount = 0
            self.audio = AudioState()
            self.reportedAudioError = nil
            self.loopStartBeat = 0
            self.loopEndBeat = 4
        }
    }

    public func togglePlayback() async {
        await audioCommand(playbackRequested ? .pause : .play)
    }

    public func stopPlayback() async { await audioCommand(.stop) }

    public func locate(beat: Double) async {
        guard let tick = PianoCamera.ticks(beat), tick >= 0 else { return }
        await audioCommand(.locate(tick: tick))
    }

    public func toggleLoop() async {
        await setLoop(enabled: audio.loopStart == nil)
    }

    public func setLoop(enabled: Bool) async {
        guard enabled else {
            await audioCommand(.loop(start: nil, end: nil))
            return
        }
        guard let start = PianoCamera.ticks(loopStartBeat), let end = PianoCamera.ticks(loopEndBeat),
              start >= 0, end > start else { return }
        await audioCommand(.loop(start: start, end: end))
    }

    public func toggleMetronome() async {
        await audioCommand(.metronome(enabled: !audio.metronome))
    }

    public func selectOutput(uid: String?) async { await audioCommand(.output(uid: uid)) }
    public func refreshOutputs() async { await audioCommand(.refresh) }

    /// Runs only while the workspace is visible. The renderer reads the live
    /// feed independently; this loop updates native controls and error text.
    public func observeAudio() async {
        while !Task.isCancelled {
            if let document, !busy, !audioBusy {
                let revision = audioRevision
                do {
                    let state = try await sendAudio(document, .status)
                    if self.document === document, revision == audioRevision, !audioBusy {
                        if audio != state { audio = state }
                        if let error = state.error, error != reportedAudioError { errorMessage = error }
                        reportedAudioError = state.error
                    }
                } catch { if !Task.isCancelled, self.document === document { report(error) } }
            }
            do { try await Task.sleep(for: .milliseconds(50)) }
            catch { return }
        }
    }

    private func audioCommand(_ command: AudioCommand) async {
        guard !busy, !audioBusy || command.playbackIntent != nil, let document else { return }
        audioRevision &+= 1
        let revision = audioRevision
        if let intent = command.playbackIntent { pendingPlayback = intent }
        audioRequests += 1
        audioBusy = true
        defer {
            audioRequests -= 1
            audioBusy = audioRequests > 0
            if revision == audioRevision { pendingPlayback = nil }
        }
        do {
            let state = try await sendAudio(document, command)
            if self.document === document, revision == audioRevision {
                audio = state
                if command.requestsPlayheadFrame { transportGeneration &+= 1 }
            }
        } catch { if self.document === document, revision == audioRevision { report(error) } }
    }

    @discardableResult
    private func edit(_ fields: [String: Any], documentID: UUID? = nil, finishingPreview: Bool = false) async -> Bool {
        await edits.run {
            guard self.faderPreview == nil || finishingPreview, let document = self.document,
                  documentID == nil || documentID == self.documentID else { return false }
            do {
                let bytes = try JSONSerialization.data(withJSONObject: fields, options: [.sortedKeys])
                self.publish(try await document.perform(String(decoding: bytes, as: UTF8.self)))
                self.undoCount += 1
                self.redoCount = 0
                return true
            } catch { self.report(error); return false }
        }
    }

    private func history(_ command: HistoryCommand) async -> HistoryReport? {
        await edits.run {
            guard let document = self.document else { return nil }
            do {
                let snapshot = try await document.history(command)
                let report = try snapshot.historyReport()
                if report.changed {
                    self.publish(snapshot)
                    self.undoCount += 1
                    self.redoCount = 0
                }
                return report
            } catch {
                self.report(error)
                return nil
            }
        }
    }

    private func publish(_ snapshot: Snapshot) {
        self.snapshot = snapshot
        tracks = (0..<snapshot.trackCount).compactMap { index in
            guard let id = snapshot.trackID(at: index), let name = snapshot.trackName(at: index) else { return nil }
            return TrackRow(id: id, name: name)
        }
        if !tracks.contains(where: { $0.id == selection }) { selection = tracks.first?.id }
        if let noteFocus { self.noteFocus = snapshot.pianoNote(noteFocus)?.id }
        if let clipFocus, snapshot.clipMix(clipFocus) == nil { self.clipFocus = nil }
    }

    private func report(_ error: any Error) {
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
        case .play, .pause, .stop, .locate: true
        case .output, .refresh, .status, .loop, .metronome, .previewFader, .clearFaderPreview: false
        }
    }
}

extension ProjectController {
    public func mixer(_ track: UUID) -> TrackMix? {
        guard let index = tracks.firstIndex(where: { $0.id == track }) else { return nil }
        return snapshot?.mixer(onTrack: index)
    }

    @discardableResult
    public func setMixer(_ track: UUID, field: MixerField, value: Float, documentID: UUID? = nil) async -> Bool {
        guard let mix = mixer(track), value.isFinite, mix[field].value != value,
              field == .gain ? (0...4).contains(value) : field == .pan ? (-1...1).contains(value) : (value == 0 || value == 1) else { return false }
        return await edit(["type": "param.set", "parameter": mix[field].id.uuidString, "value": value], documentID: documentID)
    }

    @discardableResult
    public func beginFader(_ track: UUID, field: MixerField) -> Bool {
        guard !busy, faderPreview == nil, field == .gain || field == .pan,
              let documentID, let mix = mixer(track) else { return false }
        faderPreview = FaderPreview(document: documentID, track: track, field: field,
                                    parameter: mix[field].id, original: mix[field].value,
                                    gain: mix.gain.value, pan: mix.pan.value)
        return true
    }

    public func changeFader(_ value: Float) {
        guard var preview = faderPreview, !previewFinishing, value.isFinite,
              preview.field == .gain ? (0...4).contains(value) : (-1...1).contains(value),
              let document else { return }
        if preview.field == .gain { preview.gain = value } else { preview.pan = value }
        faderPreview = preview
        previewPending = preview
        guard previewTask == nil else { return }
        // At most one request is in flight and one latest value is retained.
        // Mouse event frequency never creates an unbounded document queue.
        previewTask = Task { [weak self] in
            guard let self else { return }
            while let pending = self.previewPending {
                self.previewPending = nil
                do { _ = try await self.sendAudio(document, .previewFader(track: pending.track, gain: pending.gain, pan: pending.pan)) }
                catch { if pending.document == self.documentID { self.report(error) } }
            }
            self.previewTask = nil
        }
    }

    public func finishFader(commit: Bool) async {
        if let task = previewFinishTask { await task.value; return }
        guard let preview = faderPreview, !previewFinishing, let document else { return }
        previewFinishing = true
        let task = Task { [self] in
            await completeFader(preview, document: document, commit: commit)
            previewFinishTask = nil
            previewFinishing = false
        }
        previewFinishTask = task
        await task.value
    }

    private func completeFader(_ preview: FaderPreview, document: Document, commit: Bool) async {
        if let task = previewTask { await task.value }
        var applied = false
        if commit, preview.document == documentID, preview.value != preview.original {
            applied = await edit(["type": "param.set", "parameter": preview.parameter.uuidString, "value": preview.value],
                                 documentID: preview.document, finishingPreview: true)
        }
        if !applied {
            do { _ = try await sendAudio(document, .clearFaderPreview) }
            catch { if preview.document == documentID { report(error) } }
        }
        faderPreview = nil; previewPending = nil
    }

    @discardableResult
    public func setClipGain(_ id: UUID, gain: Float, documentID: UUID? = nil) async -> Bool {
        guard let clip = snapshot?.clipMix(id), clip.isAudio, gain.isFinite, gain >= 0, clip.gain != gain else { return false }
        return await edit(["type": "clip.set_gain", "clip": id.uuidString, "gain": gain], documentID: documentID)
    }

    @discardableResult
    public func setClipFades(_ id: UUID, fadeIn: Int64, fadeOut: Int64, documentID: UUID? = nil) async -> Bool {
        guard let clip = snapshot?.clipMix(id), clip.isAudio, fadeIn >= 0, fadeOut >= 0,
              clip.fadeIn != fadeIn || clip.fadeOut != fadeOut else { return false }
        return await edit(["type": "clip.set_fades", "clip": id.uuidString, "fade_in": fadeIn, "fade_out": fadeOut], documentID: documentID)
    }
}

/// Runs document work one at a time. The body must not call `run` again.
@MainActor
final class EditTurn {
    private var tail: Task<Void, Never>?
    private var latest = 0
    private(set) var busy = false
    var onBusy: (@MainActor (Bool) -> Void)?

    func run<T: Sendable>(_ body: @escaping @MainActor () async -> T) async -> T {
        latest += 1
        let mine = latest
        let previous = tail
        let result = Task { @MainActor in
            await previous?.value
            let value = await body()
            if self.latest == mine {
                self.setBusy(false)
                self.tail = nil
            }
            return value
        }
        tail = Task { @MainActor in _ = await result.value }
        setBusy(true)
        return await result.value
    }

    private func setBusy(_ busy: Bool) {
        guard self.busy != busy else { return }
        self.busy = busy
        onBusy?(busy)
    }
}
