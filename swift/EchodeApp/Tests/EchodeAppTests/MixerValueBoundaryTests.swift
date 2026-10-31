import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct MixerValueBoundaryTests {
    @Test func contractNumericEntriesClampTheCommonDifferenceAndRejectStaleDocuments() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-value-bounds-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "anchor")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "louder")
        let second = try #require(controller.selection), document = try #require(controller.documentID)
        #expect(await controller.setMixer(first, field: .gain, value: 0.5))
        #expect(await controller.setMixer(second, field: .gain, value: 2))
        #expect(await controller.setMixer(first, field: .pan, value: -0.5))
        #expect(await controller.setMixer(second, field: .pan, value: 0.5))
        controller.selectTrack(first, change: .toggle)
        let actions = controller.arrangeMixActions(document: document)
        let enter = try #require(actions.enter)
        let undo = controller.undoCount
        enter(first, .gain, 4)
        #expect(await (try #require(controller.mixerSwitchTail)).value)
        #expect(controller.mixer(first)?.gain.value == 1 && controller.mixer(second)?.gain.value == 4,
            "the common gain factor must stop at the loudest member's limit")
        enter(first, .pan, 1)
        #expect(await (try #require(controller.mixerSwitchTail)).value)
        #expect(controller.mixer(first)?.pan.value == 0 && controller.mixer(second)?.pan.value == 1,
            "the common pan difference must stop at the rightmost member's limit")
        #expect(controller.undoCount == undo + 2)
        // A blocked group movement must not consume history or discard a redo branch.
        await controller.undo()
        let redo = controller.redoCount, before = controller.undoCount
        enter(first, .gain, 4)
        #expect(!(await (try #require(controller.mixerSwitchTail)).value))
        #expect(controller.undoCount == before && controller.redoCount == redo)
        await controller.redo()
        enter(second, .pan, -1)
        #expect(await (try #require(controller.mixerSwitchTail)).value)
        #expect(controller.mixer(first)?.pan.value == -1 && controller.mixer(second)?.pan.value == 0)
        let stale = try #require(controller.captureMixerValue(first, field: .gain, value: 0.5, document: document))
        await controller.close(); await controller.open(url)
        let reopened = [controller.mixer(first), controller.mixer(second)]
        #expect(controller.enqueueMixerValue(stale) == nil)
        #expect(!(await controller.applyMixerValue(stale)))
        enter(first, .gain, 0.5)
        #expect(controller.mixerSwitchTail == nil)
        #expect([controller.mixer(first), controller.mixer(second)] == reopened && controller.undoCount == 0)
        await controller.close()
    }

    @Test func silenceAndSingleTrackEntriesHaveExplicitSemantics() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-value-silence-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "silent")
        let first = try #require(controller.selection)
        await controller.addTrack(name: "audible")
        let second = try #require(controller.selection), document = try #require(controller.documentID)
        #expect(await controller.setMixer(first, field: .gain, value: 0))
        controller.selectTrack(first, change: .toggle)
        func apply(_ anchor: UUID, _ value: Float) async throws -> Bool {
            await controller.applyMixerValue(try #require(controller.captureMixerValue(anchor, field: .gain, value: value, document: document)))
        }
        let undo = controller.undoCount
        #expect(!(try await apply(first, 1)))
        #expect(controller.errorMessage != nil && controller.undoCount == undo)
        #expect(controller.mixer(first)?.gain.value == 0 && controller.mixer(second)?.gain.value == 1)
        #expect(try await apply(second, 2))
        #expect(controller.mixer(first)?.gain.value == 0 && controller.mixer(second)?.gain.value == 2)
        #expect(try await apply(second, 0))
        #expect(controller.mixer(first)?.gain.value == 0 && controller.mixer(second)?.gain.value == 0)
        await controller.undo()
        #expect(controller.mixer(second)?.gain.value == 2)
        controller.selection = second
        #expect(try await apply(first, 0.75), "an unselected track must be edited alone, including from silence")
        #expect(controller.mixer(first)?.gain.value == 0.75 && controller.mixer(second)?.gain.value == 2)
        await controller.close()
    }

    @Test func malformedTargetsAndExtremeValuesCannotProducePartialEdits() async throws {
        let document = DocumentID(), first = UUID(), second = UUID()
        func values(_ field: MixerField, _ target: Float, _ original: [Float]) -> [Float]? {
            MixerValueBatch(document: document, tracks: [first, second], anchor: first, field: field, value: target).values(from: original)
        }
        #expect(values(.pan, -1, [-0.5, 0.5]) == [-1, 0])
        #expect(values(.gain, 1, [0.5, 0.125]) == [1, 0.25])
        #expect(values(.gain, 1, [0.5, 0]) == [1, 0])
        #expect(values(.gain, .leastNonzeroMagnitude, [1, .leastNonzeroMagnitude]) == [1, .leastNonzeroMagnitude])
        #expect(values(.gain, .nan, [1, 1]) == nil && values(.pan, .infinity, [0, 0]) == nil)
        #expect(values(.gain, 1, [1, -.infinity]) == nil && values(.pan, 0, [0, 2]) == nil)
        #expect(values(.mute, 1, [0, 0]) == nil && values(.gain, 5, [1, 1]) == nil)
        #expect(values(.gain, 1, [1]) == nil)
        for targets in [[], [first, first], [second]] {
            let request = MixerValueBatch(document: document, tracks: targets, anchor: first, field: .gain, value: 1)
            #expect(request.values(from: targets.map { _ in 0.5 }) == nil)
        }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-value-invalid-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "remaining")
        let track = try #require(controller.selection), session = try #require(controller.documentID), undo = controller.undoCount
        let request = MixerValueBatch(document: session, tracks: [track, UUID()], anchor: track, field: .gain, value: 0.5)
        #expect(!(await controller.applyMixerValue(request)))
        #expect(controller.mixer(track)?.gain.value == 1 && controller.undoCount == undo)
        await controller.close()
    }
}
