import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct MixerDragAudioTests {
    @Test func completeGroupsCoalesceBeforeCancellationAndUseTheProductWireShape() async throws {
        let gate = MixerGroupPreviewGate()
        let fixture = try await MixerDragFixture(sendAudio: { _, command in
            if case .previewFaders(let values) = command { await gate.receive(values) }
            return AudioState()
        })
        defer { removeTestFile(fixture.url) }
        let controller = fixture.controller, gain = fixture.point(.gain)
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain))
        fixture.header.mouseDragged(with: try fixture.mouse(.leftMouseDragged, gain, dx: 1))
        await gate.entered()
        for dx in 2...20 { fixture.header.mouseDragged(with: try fixture.mouse(.leftMouseDragged, gain, dx: Double(dx))) }
        let preview = try #require(controller.faderPreview)
        #expect(await gate.groups.count == 1)
        fixture.header.clicks.cancel()
        let finishing = try #require(fixture.completion)
        await gate.release()
        await finishing.value
        let groups = await gate.groups
        #expect(groups.count == 2)
        let last = try #require(groups.last)
        #expect(last.count == 2)
        for track in fixture.tracks.prefix(2) {
            #expect(last.first { $0.track == track } == preview.values(on: track))
        }
        let json = try #require(JSONSerialization.jsonObject(with: JSONEncoder().encode(AudioCommand.previewFaders(values: last))) as? [String: Any])
        #expect(json["type"] as? String == "audio.preview_faders")
        let values = try #require(json["values"] as? [[String: Any]])
        #expect(values.count == 2 && values.allSatisfy { Set($0.keys) == ["track", "gain", "pan"] })
        #expect(controller.faderPreview == nil && controller.mixer(fixture.tracks[0])?.gain.value == 0.5)
        #expect(controller.mixer(fixture.tracks[1])?.gain.value == 0.25)
        await controller.close()
    }

    @Test func changedCapturedMemberRejectsTheWholeCommit() async throws {
        let fixture = try await MixerDragFixture()
        defer { removeTestFile(fixture.url) }
        let controller = fixture.controller, gain = fixture.point(.gain)
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain))
        fixture.header.mouseDragged(with: try fixture.mouse(.leftMouseDragged, gain, dx: 10))
        let preview = try #require(controller.faderPreview)
        #expect(controller.faderActions(preview)?.count == 2)
        await controller.finishFader(commit: false)
        #expect(await controller.setMixer(fixture.tracks[1], field: .pan, value: 0.5))
        #expect(controller.faderActions(preview) == nil)
        await controller.close()
    }
}

private actor MixerGroupPreviewGate {
    var groups: [[FaderValue]] = []
    private var started: CheckedContinuation<Void, Never>?
    private var held: CheckedContinuation<Void, Never>?
    func receive(_ values: [FaderValue]) async {
        groups.append(values)
        if groups.count == 1 {
            started?.resume(); started = nil
            await withCheckedContinuation { held = $0 }
        }
    }
    func entered() async {
        if groups.isEmpty { await withCheckedContinuation { started = $0 } }
    }
    func release() { held?.resume(); held = nil }
}
