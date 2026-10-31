import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct ClipDuplicationTests {
    private let beat: Int64 = 705_600

    private func key(flags: NSEvent.ModifierFlags = .command, repeated: Bool = false) throws -> NSEvent {
        try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
            windowNumber: 0, context: nil, characters: "d", charactersIgnoringModifiers: "d", isARepeat: repeated, keyCode: 2))
    }

    private func clips(_ snapshot: Snapshot) -> [UUID] {
        // This query covers only the fixtures, not a product size limit.
        snapshot.visibleElements(originTick: 0, widthTicks: 32 * beat, originTrack: 0, heightTracks: 4,
            pixelsPerTick: 0.001, pixelsPerTrack: 64).elements.compactMap { $0.role == .clip ? $0.id.placement : nil }
    }

    @Test func contractCommandDDuplicatesTheCurrentWindowWithIndependentContentAndHistory() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-clip-key-copy-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        let session = WorkspaceSession(controller: controller, state: WorkspaceState(defaults: .init()))
        await controller.open(url, creating: true)
        await controller.addTrack(name: "source")
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 0.25))
        let track = try #require(controller.selection), document = try #require(controller.documentID)
        let note = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        // A manually trimmed, off-grid window must determine the repeat width, not its bar or content length.
        #expect(await controller.edit(.resizeClip(track: track.uuidString, clip: note.id.placement.uuidString, start: beat / 4, length: beat * 3 / 2)))
        let original = try #require(controller.snapshot?.clipMix(note.id.placement))
        await controller.addTrack(name: "unrelated")
        let other = try #require(controller.selection)
        controller.selection = track
        controller.clipFocus = original.id
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 1200, height: 240))
        defer { pane.stop() }
        pane.timeline.audioDocument = document
        pane.setSnapshot(try #require(controller.snapshot), selected: track)
        pane.setControlSelection(clip: original.id, preview: nil)
        var requests: [ClipMove] = [], pending: Task<Bool, Never>?
        pane.onClipMove = { requests.append($0); pending = session.moveClip($0) }
        let undo = controller.undoCount, start = original.start + original.length
        let inputLength = controller.noteInput.length, velocity = controller.noteVelocity.value, piano = controller.pianoTrack
        pane.timeline.keyDown(with: try key())
        try #require(requests.count == 1, "Command-D must send one adjacent clip copy")
        #expect(requests[0] == ClipMove(document: document, clip: original.id, track: track, start: start, copy: true))
        controller.selection = other
        #expect(await (try #require(pending)).value)
        let copy = try #require(controller.clipFocus)
        let mix = try #require(controller.snapshot?.clipMix(copy))
        #expect(copy != original.id && mix.track == track && controller.selection == track)
        #expect(mix.start == start && mix.length == original.length, "Command-D must use the current unrounded clip window")
        #expect(controller.snapshot?.clipMix(original.id) == original)
        #expect(controller.undoCount == undo + 1)
        #expect(controller.noteInput.length == inputLength && controller.noteVelocity.value == velocity)
        #expect(controller.pianoTrack == piano)
        let copiedNote = try #require(controller.snapshot?.pianoNotes(track: track, originTick: 0, widthTicks: 8 * beat,
            topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1).first { $0.id.placement == copy })
        #expect(copiedNote.id.note != note.id.note)
        #expect(copiedNote.pitch == note.pitch && copiedNote.velocity == note.velocity && copiedNote.length == note.length)
        #expect(copiedNote.start == note.start + original.length)
        await controller.undo()
        #expect(controller.snapshot?.clipMix(copy) == nil && controller.snapshot?.clipMix(original.id) == original)
        await controller.redo()
        #expect(controller.snapshot?.clipMix(copy) == mix)
        #expect(await controller.editNote(copiedNote.id,
            geometry: NoteGeometry(start: copiedNote.start, length: copiedNote.length, pitch: 67), documentID: document))
        #expect(controller.snapshot?.pianoNote(note.id)?.pitch == 60, "the repeat must have independent MIDI content")
        await controller.undo()
        // Once the new frame arrives, a second invocation repeats the copy immediately after itself.
        pane.setSnapshot(try #require(controller.snapshot), selected: track)
        pane.setControlSelection(clip: copy, preview: nil)
        pane.timeline.keyDown(with: try key())
        #expect(await (try #require(pending)).value)
        let next = try #require(controller.clipFocus)
        #expect(next != copy && controller.snapshot?.clipMix(next)?.start == start + original.length)
        #expect(controller.undoCount == undo + 2)
        await controller.close(); await controller.open(url)
        #expect(controller.snapshot?.clipMix(original.id) == original && controller.snapshot?.clipMix(copy) == mix)
        #expect(controller.snapshot?.clipMix(next)?.start == start + original.length)
        await controller.close()
    }

    @Test func audioRepeatPreservesTrimMixAndReplaceLandingAndRejectsStaleDocuments() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-audio-key-copy-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("source.wav"), url = root.appendingPathComponent("Project.echode")
        try wavMono(rate: 48_000, frames: 48_000).write(to: wav)
        let controller = ProjectController()
        let session = WorkspaceSession(controller: controller, state: WorkspaceState(defaults: .init()))
        await controller.open(url, creating: true)
        await controller.addTrack(name: "audio")
        let track = try #require(controller.selection), document = try #require(controller.documentID)
        #expect(await controller.importAudio(wav, on: track))
        let imported = try #require(controller.snapshot)
        let source = try #require(clips(imported).first)
        #expect(await controller.edit(.resizeClip(track: track.uuidString, clip: source.uuidString, start: beat / 4, length: beat)))
        #expect(await controller.setClipGain(source, gain: 0.5, documentID: document))
        #expect(await controller.setClipFades(source, fadeIn: 1000, fadeOut: 2000, documentID: document))
        #expect(await controller.edit(.importAudio(track: track.uuidString, start: beat * 5 / 4, path: wav.path)))
        let original = try #require(controller.snapshot?.clipMix(source)), undo = controller.undoCount
        let view = ArrangeView(frame: CGRect(x: 0, y: 0, width: 1000, height: 200))
        defer { view.stop() }
        view.audioDocument = document
        view.setSnapshot(try #require(controller.snapshot), selected: track)
        view.setControlSelection(clip: source, preview: nil)
        var request: ClipMove?, pending: Task<Bool, Never>?
        view.onClipMove = { request = $0; pending = session.moveClip($0) }
        view.keyDown(with: try key())
        #expect(await (try #require(pending)).value)
        let copiedID = try #require(controller.clipFocus)
        let copy = try #require(controller.snapshot?.clipMix(copiedID))
        #expect(copy.start == original.start + original.length && copy.length == original.length && copy.isAudio)
        #expect(copy.gain == original.gain && copy.fadeIn == original.fadeIn && copy.fadeOut == original.fadeOut)
        #expect(controller.snapshot?.clipMix(source) == original && controller.undoCount == undo + 1)
        await controller.undo()
        let undone = try #require(controller.snapshot)
        #expect(undone.clipMix(copy.id) == nil && clips(undone).count == 2)
        await controller.close(); await controller.open(url)
        #expect(!(await controller.moveClip(try #require(request))))
        #expect(controller.redoCount == 0 && controller.undoCount == 0)
        await controller.close()
    }
}
