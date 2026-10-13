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
    public var selection: UUID? {
        didSet {
            if let selection, !tracks.contains(where: { $0.id == selection }) { self.selection = nil }
            if selection != oldValue { noteFocus = nil }
        }
    }
    public var noteFocus: PianoNote.ID? {
        didSet {
            if let noteFocus, noteFocus.track != selection || snapshot?.pianoNote(noteFocus) == nil { self.noteFocus = nil }
        }
    }
    public var errorMessage: String?
    public private(set) var audio = AudioState()
    public private(set) var audioBusy = false
    /// Loop bounds in beats. Transport state, not part of the project.
    public var loopStartBeat = 0.0
    public var loopEndBeat = 4.0
    public private(set) var playback: PlaybackFeed?
    @ObservationIgnored private var document: Document?
    @ObservationIgnored private var reportedAudioError: String?

    public init() {}

    public func open(_ url: URL, creating: Bool = false) async {
        guard !busy else { return }
        if !creating, projectURL?.resolvingSymlinksInPath() == url.resolvingSymlinksInPath() { return }
        busy = true
        defer { busy = false }
        do {
            let opened = try await Task.detached {
                try creating ? Document(creating: url.path) : Document(opening: url.path)
            }.value
            let initial = try await opened.snapshot()
            let initialAudio = try initial.audioState()
            let previous = document
            document = opened
            documentID = UUID()
            playback = opened.playbackFeed
            audio = initialAudio
            reportedAudioError = nil
            loopStartBeat = 0
            loopEndBeat = 4
            projectURL = url
            undoCount = 0
            redoCount = 0
            selection = nil
            noteFocus = nil
            publish(initial)
            if let previous { await Task.detached { previous.shutdown() }.value }
        } catch { report(error) }
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
        guard !busy, document != nil else { return }
        guard await edit(["type": "track.add", "name": name]) else { return }
        if let last = tracks.last { selection = last.id }
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
        guard !busy, undoCount > 0, let document else { return }
        busy = true
        defer { busy = false }
        do { publish(try await document.undo()); undoCount -= 1; redoCount += 1 }
        catch { report(error) }
    }

    public func redo() async {
        guard !busy, redoCount > 0, let document else { return }
        busy = true
        defer { busy = false }
        do { publish(try await document.redo()); redoCount -= 1; undoCount += 1 }
        catch { report(error) }
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
        guard !busy else { return }
        busy = true
        let previous = document
        document = nil
        documentID = nil
        playback = nil
        if let previous { await Task.detached { previous.shutdown() }.value }
        snapshot = nil
        tracks = []
        projectURL = nil
        selection = nil
        noteFocus = nil
        undoCount = 0
        redoCount = 0
        audio = AudioState()
        reportedAudioError = nil
        loopStartBeat = 0
        loopEndBeat = 4
        busy = false
    }

    public func togglePlayback() async {
        await audioCommand(audio.playing ? .pause : .play)
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
                do {
                    let state = try await document.audio(.status)
                    if self.document === document {
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
        guard !busy, !audioBusy, let document else { return }
        audioBusy = true
        defer { audioBusy = false }
        do {
            let state = try await document.audio(command)
            if self.document === document { audio = state }
        } catch { if self.document === document { report(error) } }
    }

    @discardableResult
    private func edit(_ fields: [String: Any], documentID: UUID? = nil) async -> Bool {
        guard !busy, let document, documentID == nil || documentID == self.documentID else { return false }
        busy = true
        defer { busy = false }
        do {
            let bytes = try JSONSerialization.data(withJSONObject: fields, options: [.sortedKeys])
            publish(try await document.perform(String(decoding: bytes, as: UTF8.self)))
            undoCount += 1
            redoCount = 0
            return true
        } catch { report(error); return false }
    }

    private func history(_ command: HistoryCommand) async -> HistoryReport? {
        guard !busy, let document else { return nil }
        busy = true
        defer { busy = false }
        do {
            let snapshot = try await document.history(command)
            let report = try snapshot.historyReport()
            if report.changed {
                publish(snapshot)
                undoCount += 1
                redoCount = 0
            }
            return report
        } catch {
            report(error)
            return nil
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
    }

    private func report(_ error: any Error) {
        errorMessage = (error as? BridgeError)?.message ?? error.localizedDescription
    }
}
