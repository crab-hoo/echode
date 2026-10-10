import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoTests {
    @Test func pitchRowsSnapAndSolfegeFollowTheDisplayKey() {
        let camera = PianoCamera(topPitch: 72)
        #expect(camera.pitch(at: 0) == 72)
        #expect(camera.pitch(at: 19) == 72)
        #expect(camera.pitch(at: 20) == 71)
        #expect(camera.pitch(at: 1_000_000) == 0)
        #expect(camera.tick(at: 10) == 176_400)
        #expect(PianoCamera.ticks(.infinity) == nil)
        #expect(PianoCamera.ticks(Double(Int64.max)) == nil)
        #expect(DisplayKey().syllable(60) == "Do")
        let minor = DisplayKey(tonic: 9, minor: true)
        #expect(minor.syllable(69) == "Do")
        #expect(minor.syllable(72) == "♭Mi")
        #expect(minor.inScale(72))
        #expect(!minor.inScale(73))
        #expect(PianoGesture.mode(at: 1, start: 0, end: 2) == .move)
        #expect(PianoGesture.mode(at: 0, start: 0, end: 2) == .leftEdge)
        #expect(PianoGesture.mode(at: 2, start: 0, end: 2) == .rightEdge)
        #expect((PianoCamera().topPitch - 60) * PianoCamera().pixelsPerPitch < 96)
    }

    @Test func aDragIsOneDurableEditAndPreservesIdentityAndVelocity() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        await controller.insertNote(pitch: 60, beat: 0, length: 1)
        let track = try #require(controller.selection)
        let before = try #require(controller.snapshot)
        let note = try #require(before.pianoNeighbor(track: track, current: nil))
        controller.noteFocus = note.id
        let count = controller.undoCount
        let camera = PianoCamera()
        let drag = PianoGesture(id: note.id, original: NoteGeometry(note), mode: .move, x: 0, y: 40)
        let intermediate = try #require(drag.preview(x: 18, y: 20, camera: camera))
        let final = try #require(drag.preview(x: 144, y: 0, camera: camera))
        #expect(intermediate.start == 176_400)
        #expect(controller.undoCount == count)
        await controller.editNote(note.id, geometry: final)
        #expect(controller.errorMessage == nil)
        #expect(controller.undoCount == count + 1)
        let edited = try #require(controller.snapshot?.pianoNote(note.id))
        #expect(edited.id.note == note.id.note)
        #expect(edited.velocity == note.velocity)
        #expect(edited.start == 1_411_200 && edited.pitch == 62)
        #expect(before.pianoNote(note.id)?.start == 0)
        await controller.undo()
        #expect(controller.snapshot?.pianoNote(note.id)?.start == 0)
        // An abandoned preview leaves Redo and durable storage untouched.
        _ = drag.preview(x: 300, y: 0, camera: camera)
        #expect(controller.redoCount == 1)
        await controller.redo()
        await controller.close()
        await controller.open(url)
        let reopened = try #require(controller.snapshot?.pianoNote(note.id))
        #expect(reopened.start == edited.start && reopened.pitch == edited.pitch)
        await controller.deleteNote(reopened.id)
        #expect(controller.snapshot?.pianoNote(note.id) == nil)
        await controller.undo()
        #expect(controller.snapshot?.pianoNote(note.id)?.id.note == note.id.note)
        await controller.close()
    }

    @Test func resizeMergesAutoClipsAndNavigationRevealsOffscreenNotes() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "keys")
        let track = try #require(controller.selection)
        await controller.insertNote(pitch: 60, beat: 0, length: 1)
        await controller.insertNote(pitch: 110, beat: 1000, length: 1)
        let first = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil))
        let far = try #require(controller.snapshot?.pianoNeighbor(track: track, current: first.id))
        var camera = PianoCamera()
        camera.reveal(far, width: 360, height: 200)
        let shown = try #require(controller.snapshot).pianoNotes(track: track,
            originTick: try #require(PianoCamera.ticks(camera.originBeat)), widthTicks: 705_600 * 5,
            topPitch: camera.topPitch, heightPitches: 10, pixelsPerTick: 72 / 705_600, pixelsPerPitch: 20)
        #expect(shown.contains { $0.id == far.id })
        await controller.editNote(far.id, geometry: NoteGeometry(start: 705_600 * 8, length: 705_600, pitch: 64))
        let edge = PianoGesture(id: first.id, original: NoteGeometry(first), mode: .rightEdge, x: 0, y: 0)
        let lengthened = try #require(edge.preview(x: 72 * 8, y: 0, camera: PianoCamera()))
        await controller.editNote(first.id, geometry: lengthened)
        let resolved = try #require(controller.snapshot?.pianoNote(far.id))
        #expect(resolved.id.placement == first.id.placement)
        await controller.editNote(first.id, geometry: NoteGeometry(start: .max, length: 1, pitch: 60))
        #expect(controller.errorMessage == nil)
        await controller.close()
    }
}
