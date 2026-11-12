import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

@MainActor
struct ModelUnitsTests {
    @Test func contractModelUnitsReachNotesTransportAndMixControls() async throws {
        // Independent protocol values catch a unit error shared by the UI consumers.
        #expect(Tick.perQuarter == 705_600 && Tick.ticks(1.25) == 882_000)
        #expect(PianoGrid.divisions == [2_822_400, 1_411_200, 705_600, 352_800, 176_400, 88_200, 44_100, 22_050])
        #expect(MixScale.gain(1) == 4 && MixScale.clipGainMax == 1_048_576)
        #expect(MixScale.dragged(.pan, from: 0, by: 2) == 1)
        #expect(MixEntry.value(.pan, "-100%") == -1 && MixEntry.value(.pan, "101%") == nil)
        #expect(ClipEntry.flicksPerMillisecond == 705_600)
        #expect(ClipEntry.maximumFadeMilliseconds == Double(UInt32.max) / 768_000 * 1000)
        #expect(!NoteGeometry(start: 0, length: 1, pitch: 128).isValid)
        #expect(ModelLimits.midi7 == 0...127)

        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-units-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "units")
        let track = try #require(controller.selection)
        #expect(await controller.insertNote(pitch: 127, beat: 1.25, length: 1.0 / 3))
        let snapshot = try #require(controller.snapshot)
        let note = try #require(
            snapshot.pianoNotes(
                track: track, originTick: 0, widthTicks: 2_822_400,
                topPitch: 127, heightPitches: 128, pixelsPerTick: 1, pixelsPerPitch: 1
            ).first)
        #expect(note.start == 882_000 && note.length == 235_200 && note.pitch == 127)
        #expect(snapshot.flicks(at: 705_600) == 352_800_000)
        let undo = controller.undoCount
        await controller.setPlaybackStart(beat: 1.25)
        await controller.locate(beat: 1.5)
        #expect(controller.playbackStart == 882_000 && controller.position == 1_058_400)
        #expect(controller.undoCount == undo)
        await controller.setPlaybackStart(beat: .infinity)
        #expect(controller.playbackStart == 882_000)
        await controller.setMixer(track, field: .gain, value: ModelLimits.gain.upperBound)
        #expect(controller.mixer(track)?.gain.value == 4)
        await controller.setMixer(track, field: .gain, value: 4.01)
        #expect(controller.mixer(track)?.gain.value == 4)
        await controller.close()
        await controller.open(url)
        let saved = try #require(controller.snapshot?.pianoNote(note.id))
        #expect(saved.start == 882_000 && saved.length == 235_200)
        #expect(controller.playbackStart == 882_000 && controller.mixer(track)?.gain.value == 4)
        await controller.close()
    }
}
