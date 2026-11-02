import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct MixerDragBatchTests {
    @Test func contractHeaderDragAndResetKeepCapturedGroupBalanceAndAtomicHistory() async throws {
        let fixture = try await MixerDragFixture()
        defer { removeTestFile(fixture.url) }
        let controller = fixture.controller, tracks = fixture.tracks
        let undo = controller.undoCount
        let gain = fixture.point(.gain)
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain))
        fixture.header.mouseDragged(with: try fixture.mouse(.leftMouseDragged, gain, dx: 10))
        controller.selection = tracks[2]
        fixture.header.mouseUp(with: try fixture.mouse(.leftMouseUp, gain, dx: 20))
        await fixture.completion?.value
        let raised = try #require(controller.mixer(tracks[0])?.gain.value)
        #expect(raised > 0.5)
        #expect(controller.mixer(tracks[1])?.gain.value == raised / 2,
                "header drag must apply the same dB difference to every captured selected track")
        #expect(controller.mixer(tracks[2])?.gain.value == 1)
        #expect(controller.undoCount == undo + 1 && controller.selection == tracks[2])
        await controller.undo()
        #expect(controller.mixer(tracks[0])?.gain.value == 0.5 && controller.mixer(tracks[1])?.gain.value == 0.25)
        await controller.redo()
        #expect(controller.mixer(tracks[1])?.gain.value == raised / 2)

        controller.selection = tracks[0]
        controller.selectTrack(tracks[1], change: .toggle)
        try fixture.refresh()
        let pan = fixture.point(.pan)
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, pan, clicks: 2))
        fixture.header.mouseUp(with: try fixture.mouse(.leftMouseUp, pan, clicks: 2))
        controller.selection = tracks[2]
        fixture.header.clicks.settle()
        await fixture.completion?.value
        #expect(controller.mixer(tracks[0])?.pan.value == 0)
        #expect(controller.mixer(tracks[1])?.pan.value == 0.75,
                "header reset must preserve the captured group's pan differences")
        #expect(controller.undoCount == undo + 2)
        await controller.undo()
        #expect(controller.mixer(tracks[0])?.pan.value == -0.5 && controller.mixer(tracks[1])?.pan.value == 0.25)
        await controller.redo()
        let recorded = tracks.map { controller.mixer($0) }
        await controller.close(); await controller.open(fixture.url)
        #expect(tracks.map { controller.mixer($0) } == recorded)
        await controller.close()
    }

    @Test func contractGroupDragClampsTogetherAndCancellationPreservesRedo() async throws {
        let fixture = try await MixerDragFixture()
        defer { removeTestFile(fixture.url) }
        let controller = fixture.controller, tracks = fixture.tracks
        #expect(await controller.setMixer(tracks[1], field: .gain, value: 2))
        #expect(await controller.setMixer(tracks[2], field: .pan, value: 0.25))
        await controller.undo()
        let undo = controller.undoCount
        try fixture.refresh()
        let gain = fixture.point(.gain)
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain))
        fixture.header.mouseDragged(with: try fixture.mouse(.leftMouseDragged, gain, dx: 200))
        #expect(controller.faderPreview?.gain == 1,
                "group preview must stop when another member reaches maximum gain")
        fixture.header.clicks.cancel()
        await fixture.completion?.value
        #expect(controller.mixer(tracks[0])?.gain.value == 0.5 && controller.mixer(tracks[1])?.gain.value == 2)
        #expect(controller.undoCount == undo && controller.redoCount == 1 && controller.faderPreview == nil)

        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain))
        fixture.header.mouseDragged(with: try fixture.mouse(.leftMouseDragged, gain, dx: 200))
        fixture.header.mouseUp(with: try fixture.mouse(.leftMouseUp, gain))
        await fixture.completion?.value
        #expect(controller.undoCount == undo && controller.redoCount == 1,
                "returning a group drag to its origin must leave Redo untouched")
        fixture.header.mouseDown(with: try fixture.mouse(.leftMouseDown, gain, clicks: 2))
        fixture.header.mouseUp(with: try fixture.mouse(.leftMouseUp, gain, clicks: 2))
        fixture.header.clicks.settle()
        await fixture.completion?.value
        #expect(controller.mixer(tracks[0])?.gain.value == 1 && controller.mixer(tracks[1])?.gain.value == 4)
        #expect(controller.undoCount == undo + 1 && controller.redoCount == 0)
        await controller.close()
    }
}

@MainActor
final class MixerDragFixture {
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-drag-batch-\(UUID()).echode")
    let controller: ProjectController
    let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 220, height: 192))
    var tracks: [UUID] = []
    var completion: Task<Void, Never>?

    init(sendAudio: @escaping @Sendable (Document, AudioCommand) async throws -> AudioState = { _, _ in AudioState() }) async throws {
        controller = ProjectController(sendAudio: sendAudio)
        await controller.open(url, creating: true)
        for name in ["anchor", "member", "outside"] {
            await controller.addTrack(name: name)
            tracks.append(try #require(controller.selection))
        }
        #expect(await controller.setMixer(tracks[0], field: .gain, value: 0.5))
        #expect(await controller.setMixer(tracks[1], field: .gain, value: 0.25))
        #expect(await controller.setMixer(tracks[0], field: .pan, value: -0.5))
        #expect(await controller.setMixer(tracks[1], field: .pan, value: 0.25))
        controller.selection = tracks[0]
        controller.selectTrack(tracks[1], change: .toggle)
        header.clicks.interval = 60 // Settle explicitly; elapsed time is not a test oracle.
        header.documentID = controller.documentID
        header.onSelectTrack = { [controller] in controller.selectTrack($0, change: $1) }
        var actions = controller.arrangeMixActions(document: controller.documentID)
        actions.finish = { [weak self] commit in
            guard let self else { return }
            completion = Task { await self.controller.finishFader(commit: commit) }
        }
        header.mixActions = actions
        try refresh()
    }

    func refresh() throws {
        header.setSnapshot(try #require(controller.snapshot), selected: controller.selection, selectedTracks: controller.selectedTracks)
    }
    func point(_ field: MixerField) -> CGPoint {
        let rect = TrackControlLayout(row: CGRect(x: 0, y: 0, width: 220, height: 64)).rect(field)
        return CGPoint(x: rect.midX, y: rect.midY)
    }
    func mouse(_ type: NSEvent.EventType, _ point: CGPoint, dx: Double = 0, clicks: Int = 1) throws -> NSEvent {
        try #require(NSEvent.mouseEvent(with: type, location: header.convert(CGPoint(x: point.x + dx, y: point.y), to: nil),
            modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: clicks, pressure: 1))
    }
}
