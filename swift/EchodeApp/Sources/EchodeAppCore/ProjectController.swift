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
    public private(set) var projectURL: URL?
    public private(set) var busy = false
    public private(set) var undoCount = 0
    public private(set) var redoCount = 0
    public var selection: UUID?
    public var noteFocus: PianoNote.ID?
    public var errorMessage: String?
    public private(set) var audio = AudioState()
    public private(set) var audioBusy = false
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
            playback = opened.playbackFeed
            audio = initialAudio
            reportedAudioError = nil
            projectURL = url
            undoCount = 0
            redoCount = 0
            selection = nil
            noteFocus = nil
            publish(initial)
            if let previous { await Task.detached { previous.shutdown() }.value }
        } catch { report(error) }
    }

    public func addTrack(name: String) async {
        guard !busy, document != nil else { return }
        await edit(["type": "track.add", "name": name])
        if let last = tracks.last { selection = last.id }
    }

    public func insertPlugin(_ plugin: String) async {
        guard let selection, !plugin.isEmpty else { return }
        await edit(["type": "device.add", "track": selection.uuidString, "plugin": plugin])
    }

    public func removeDevice(_ id: UUID) async {
        await edit(["type": "device.remove", "device": id.uuidString])
    }

    public func insertNote(pitch: Int, beat: Double, length: Double) async {
        guard let selection, pitch >= 0, pitch <= 127,
              beat.isFinite, length.isFinite, beat >= 0, length > 0,
              beat < Double(Int64.max) / 705_600, length < Double(Int64.max) / 705_600 else { return }
        await edit([
            "type": "note.insert", "track": selection.uuidString,
            "pitch": pitch, "velocity": 100,
            "start": Int64((beat * 705_600).rounded()),
            "length": Int64((length * 705_600).rounded()),
        ])
    }

    public func editNote(_ id: PianoNote.ID, geometry: NoteGeometry) async {
        guard let before = snapshot?.pianoNote(id), geometry.isValid,
              before.start != geometry.start || before.length != geometry.length || before.pitch != geometry.pitch else { return }
        await edit(["type": "note.edit", "track": id.track.uuidString, "clip": before.id.placement.uuidString,
                    "note": id.note.uuidString, "pitch": geometry.pitch, "start": geometry.start, "length": geometry.length])
    }

    public func deleteNote(_ id: PianoNote.ID) async {
        await edit(["type": "note.delete", "track": id.track.uuidString, "note": id.note.uuidString])
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

    public func close() async {
        guard !busy else { return }
        busy = true
        let previous = document
        document = nil
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
                        audio = state
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

    private func edit(_ fields: [String: Any]) async {
        guard !busy, let document else { return }
        busy = true
        defer { busy = false }
        do {
            let bytes = try JSONSerialization.data(withJSONObject: fields, options: [.sortedKeys])
            publish(try await document.perform(String(decoding: bytes, as: UTF8.self)))
            undoCount += 1
            redoCount = 0
        } catch { report(error) }
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
