import EchodeBridge
import Foundation
import Testing

@testable import EchodeAppCore

@Suite @MainActor
struct TypedMixerEditTests {
    @Test func contractMixerEditsPreserveExactValuesPreviewHistoryAndPersistence() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-typed-mixer-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("clip.wav")
        let url = root.appendingPathComponent("Project.echode")
        // This one-second fixture only supplies a clip for mix edits, not a product buffer limit.
        try wavMono(rate: 48_000, frames: 48_000).write(to: wav)
        let controller = ProjectController(sendAudio: { _, _ in AudioState() })
        await controller.open(url, creating: true)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        #expect(await controller.importAudio(wav))
        let clip = try #require(
            controller.snapshot?.visibleElements(
                originTick: 0, widthTicks: 705_600 * 8,
                originTrack: 0, heightTracks: 2, pixelsPerTick: 200 / 705_600, pixelsPerTrack: 64
            )
            .elements.first { $0.role == .clip }?.id.placement)
        await controller.addTrack(name: "second")
        let second = try #require(controller.selection)
        let document = try #require(controller.documentID)
        #expect(await controller.setMixer(first, field: .gain, value: 0.5, documentID: document))
        #expect(await controller.setMixer(second, field: .gain, value: 0.25, documentID: document))
        controller.selectTrack(first, change: .toggle)
        let value = try #require(controller.captureMixerValue(first, field: .gain, value: 1, document: document))
        let before = controller.undoCount
        #expect(await controller.applyMixerValue(value))
        #expect(controller.mixer(first)?.gain.value == 1 && controller.mixer(second)?.gain.value == 0.5)
        #expect(controller.undoCount == before + 1)
        await controller.undo()
        #expect(controller.mixer(first)?.gain.value == 0.5 && controller.mixer(second)?.gain.value == 0.25)
        await controller.redo()
        #expect(controller.mixer(first)?.gain.value == 1 && controller.mixer(second)?.gain.value == 0.5)

        // A discrete group edit commits a pending single-track preview before resolving its ratio.
        #expect(controller.beginFader(first, field: .gain))
        controller.changeFader(0.75)
        let next = try #require(controller.captureMixerValue(first, field: .gain, value: 1.5, document: document))
        let pending = try #require(controller.enqueueMixerValue(next))
        controller.selection = second
        #expect(await pending.value)
        #expect(controller.faderPreview == nil && controller.undoCount == before + 3)
        #expect(controller.mixer(first)?.gain.value == 1.5 && controller.mixer(second)?.gain.value == 1)
        await controller.undo()
        #expect(controller.mixer(first)?.gain.value == 0.75 && controller.mixer(second)?.gain.value == 0.5)
        await controller.undo()
        #expect(controller.mixer(first)?.gain.value == 1 && controller.mixer(second)?.gain.value == 0.5)
        await controller.redo()
        await controller.redo()
        let previewUndo = controller.undoCount
        #expect(controller.beginFader(second, field: .pan))
        controller.changeFader(-0.75)
        await controller.finishFader(commit: false)
        #expect(controller.mixer(second)?.pan.value == 0 && controller.undoCount == previewUndo)

        controller.selectTrack(first, change: .toggle)
        let mute = try #require(controller.captureMixerSwitch(first, field: .mute, document: document))
        let switchUndo = controller.undoCount
        #expect(await controller.applyMixerSwitch(mute))
        #expect(controller.mixer(first)?.mute.value == 1 && controller.mixer(second)?.mute.value == 1)
        #expect(controller.undoCount == switchUndo + 1)
        await controller.undo()
        #expect(controller.mixer(first)?.mute.value == 0 && controller.mixer(second)?.mute.value == 0)
        await controller.redo()
        let piano = PianoTrackTarget(document: document, track: first)
        #expect(await controller.togglePianoMixer(piano, field: .mute))
        #expect(controller.mixer(first)?.mute.value == 0 && controller.mixer(second)?.mute.value == 1)

        // Values bypass dB/Double conversions on the request path, including subnormal Float bits.
        #expect(await controller.setMixer(first, field: .gain, value: .leastNonzeroMagnitude, documentID: document))
        #expect(controller.mixer(first)?.gain.value.bitPattern == Float.leastNonzeroMagnitude.bitPattern)
        #expect(await controller.setMixer(first, field: .pan, value: -0.12345679, documentID: document))
        #expect(await controller.setClipGain(clip, gain: .leastNonzeroMagnitude, documentID: document))
        let entering: Int64 = 3_900_000_000_001
        let leaving: Int64 = 705_600_007
        let fadeUndo = controller.undoCount
        #expect(await controller.setClipFades(clip, fadeIn: entering, fadeOut: leaving, documentID: document))
        #expect(
            controller.snapshot?.clipMix(clip)?.fadeIn == entering
                && controller.snapshot?.clipMix(clip)?.fadeOut == leaving)
        #expect(controller.undoCount == fadeUndo + 1)
        await controller.undo()
        #expect(controller.snapshot?.clipMix(clip)?.fadeIn == 0 && controller.snapshot?.clipMix(clip)?.fadeOut == 0)
        await controller.redo()
        let savedClip = try #require(controller.snapshot?.clipMix(clip))
        let savedFirst = try #require(controller.mixer(first))
        let savedSecond = try #require(controller.mixer(second))

        #expect(await controller.setMixer(first, field: .solo, value: 1, documentID: document))
        await controller.undo()
        let unchanged = controller.undoCount
        #expect(!(await controller.setMixer(first, field: .gain, value: .leastNonzeroMagnitude, documentID: document)))
        #expect(!(await controller.setClipGain(clip, gain: .leastNonzeroMagnitude, documentID: document)))
        #expect(!(await controller.setClipFades(clip, fadeIn: entering, fadeOut: leaving, documentID: document)))
        for invalid: Float in [.nan, .infinity, -1, ModelLimits.clipGain.upperBound.nextUp] {
            #expect(!(await controller.setClipGain(clip, gain: invalid, documentID: document)))
        }
        #expect(!(await controller.setMixer(first, field: .gain, value: .nan, documentID: document)))
        #expect(!(await controller.setMixer(first, field: .pan, value: 2, documentID: document)))
        #expect(!(await controller.setClipFades(clip, fadeIn: -1, fadeOut: leaving, documentID: document)))
        #expect(
            !(await controller.setClipFades(clip, fadeIn: 9_007_199_254_740_993, fadeOut: leaving, documentID: document))
        )
        #expect(!(await controller.setMixer(UUID(), field: .solo, value: 1, documentID: document)))
        let missing = MixerSwitchBatch(
            document: document, tracks: [first, UUID()], anchor: first, field: .solo, value: 1)
        #expect(!(await controller.applyMixerSwitch(missing)))
        #expect(controller.undoCount == unchanged && controller.redoCount == 1)
        #expect(controller.snapshot?.clipMix(clip) == savedClip)
        #expect(controller.mixer(first) == savedFirst && controller.mixer(second) == savedSecond)
        controller.errorMessage = nil
        await controller.redo()
        #expect(controller.mixer(first)?.solo.value == 1)
        await controller.undo()

        await controller.close()
        await controller.open(url)
        #expect(controller.errorMessage == nil && controller.snapshot?.clipMix(clip) == savedClip)
        #expect(controller.mixer(first) == savedFirst && controller.mixer(second) == savedSecond)
        #expect(!(await controller.setMixer(first, field: .pan, value: 0, documentID: document)))
        #expect(!(await controller.setClipGain(clip, gain: 1, documentID: document)))
        #expect(!(await controller.setClipFades(clip, fadeIn: 0, fadeOut: 0, documentID: document)))
        #expect(!(await controller.applyMixerValue(value)))
        #expect(!(await controller.applyMixerSwitch(mute)))
        #expect(!(await controller.togglePianoMixer(piano, field: .mute)))
        #expect(controller.undoCount == 0 && controller.redoCount == 0)
        #expect(controller.mixer(first) == savedFirst && controller.mixer(second) == savedSecond)
        #expect(controller.snapshot?.clipMix(clip) == savedClip)
        await controller.close()
    }
}
