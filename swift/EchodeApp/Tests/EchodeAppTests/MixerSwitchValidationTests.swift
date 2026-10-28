import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct MixerSwitchValidationTests {
    @Test func missingOrMalformedTargetsCannotPartiallyChangeTracksOrDiscardRedo() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-switch-invalid-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "second")
        let second = try #require(controller.selection), document = try #require(controller.documentID)
        controller.selectTrack(first, change: .toggle)
        let request = try #require(controller.captureMixerSwitch(first, field: .mute, value: 1, document: document))
        #expect(request.tracks == [first, second])
        #expect(await controller.applyMixerSwitch(request))
        await controller.undo()
        let undo = controller.undoCount, redo = controller.redoCount
        let missing = UUID()
        for invalid in [
            MixerSwitchBatch(document: document, tracks: [], anchor: first, field: .mute, value: 1),
            MixerSwitchBatch(document: document, tracks: [first, first], anchor: first, field: .mute, value: 1),
            MixerSwitchBatch(document: document, tracks: [first, missing], anchor: first, field: .mute, value: 1),
            MixerSwitchBatch(document: document, tracks: [first, second], anchor: missing, field: .mute, value: 1),
            MixerSwitchBatch(document: document, tracks: [first, second], anchor: first, field: .pan, value: 1),
            MixerSwitchBatch(document: document, tracks: [first, second], anchor: first, field: .mute, value: .nan),
            MixerSwitchBatch(document: document, tracks: [first, second], anchor: first, field: .mute, value: 0.5),
            MixerSwitchBatch(document: UUID(), tracks: [first, second], anchor: first, field: .mute, value: 1),
        ] {
            #expect(!(await controller.applyMixerSwitch(invalid)))
            #expect(controller.mixer(first)?.mute.value == 0 && controller.mixer(second)?.mute.value == 0)
            #expect(controller.undoCount == undo && controller.redoCount == redo)
        }
        #expect(controller.captureMixerSwitch(first, field: .gain, document: document) == nil)
        #expect(controller.captureMixerSwitch(first, field: .solo, value: .infinity, document: document) == nil)
        #expect(controller.captureMixerSwitch(first, field: .solo, document: nil) == nil)
        await controller.undo()
        #expect(controller.mixer(second) == nil)
        let afterRemoval = controller.undoCount
        #expect(!(await controller.applyMixerSwitch(request)))
        #expect(controller.mixer(first)?.mute.value == 0 && controller.undoCount == afterRemoval)
        await controller.redo()
        controller.selection = first
        let single = try #require(controller.captureMixerSwitch(second, field: .solo, value: 1, document: document))
        #expect(single.tracks == [second], "an unselected target must not borrow another track's selection")
        #expect(await controller.applyMixerSwitch(single))
        #expect(controller.mixer(first)?.solo.value == 0 && controller.mixer(second)?.solo.value == 1)
        await controller.close()
    }

    @Test func aSwitchCommitsTheSingleTrackFaderPreviewBeforeTheAtomicGroupEdit() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-switch-preview-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "first")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "second")
        let second = try #require(controller.selection), document = try #require(controller.documentID)
        controller.selectTrack(first, change: .toggle)
        let undo = controller.undoCount
        let actions = controller.arrangeMixActions(document: document)
        #expect(actions.begin(first, .gain))
        actions.change(0.5)
        let request = try #require(controller.captureMixerSwitch(first, field: .mute, document: document))
        #expect(await (try #require(controller.enqueueMixerSwitch(request))).value)
        #expect(controller.faderPreview == nil && controller.undoCount == undo + 2)
        #expect(controller.mixer(first)?.gain.value == 0.5 && controller.mixer(second)?.gain.value == 1)
        #expect(controller.mixer(first)?.mute.value == 1 && controller.mixer(second)?.mute.value == 1)
        await controller.undo()
        #expect(controller.mixer(first)?.mute.value == 0 && controller.mixer(second)?.mute.value == 0)
        #expect(controller.mixer(first)?.gain.value == 0.5)
        await controller.undo()
        #expect(controller.mixer(first)?.gain.value == 1)
        await controller.close()
    }
}
