import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct MixerSwitchBatchTests {
    private func mouse(_ point: CGPoint, in view: NSView) throws -> NSEvent {
        try #require(NSEvent.mouseEvent(with: .leftMouseDown, location: view.convert(point, to: nil), modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
    }

    @Test func contractHeaderSwitchesSetTheCapturedSelectionAtomicallyAndPreserveOtherSolos() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-switch-batch-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        var tracks: [UUID] = []
        for name in ["one", "two", "three", "outside"] {
            await controller.addTrack(name: name)
            tracks.append(try #require(controller.selection))
        }
        let document = try #require(controller.documentID)
        #expect(await controller.setMixer(tracks[1], field: .mute, value: 1, documentID: document))
        #expect(await controller.setMixer(tracks[1], field: .solo, value: 1, documentID: document))
        #expect(await controller.setMixer(tracks[3], field: .solo, value: 1, documentID: document))
        controller.selection = tracks[0]
        for track in tracks[1...2] { controller.selectTrack(track, change: .toggle) }
        let selected = Set(tracks.prefix(3)), undo = controller.undoCount
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 220, height: 256))
        header.onSelectTrack = { controller.selectTrack($0, change: $1) }
        header.setSnapshot(try #require(controller.snapshot), selected: controller.selection, selectedTracks: selected)
        header.mixActions = controller.arrangeMixActions(document: document)
        let controls = TrackControlLayout(row: CGRect(x: 0, y: 0, width: 220, height: 64))
        func press(_ field: MixerField) throws {
            let rect = controls.rect(field)
            header.mouseDown(with: try mouse(CGPoint(x: rect.midX, y: rect.midY), in: header))
        }
        try press(.mute)
        #expect(controller.selectedTracks == selected)
        // The selected IDs belong to the input event, not the later asynchronous task.
        controller.selection = tracks[3]
        #expect(await (try #require(controller.mixerSwitchTail)).value)
        #expect(tracks.prefix(3).allSatisfy { controller.mixer($0)?.mute.value == 1 }, "a header switch must set every captured selected track")
        #expect(controller.mixer(tracks[3])?.mute.value == 0)
        #expect(controller.undoCount == undo + 1 && controller.selection == tracks[3])
        await controller.undo()
        #expect(tracks.map { controller.mixer($0)?.mute.value } == [0, 1, 0, 0])
        await controller.redo()
        #expect(tracks.map { controller.mixer($0)?.mute.value } == [1, 1, 1, 0])
        controller.selection = tracks[0]
        for track in tracks[1...2] { controller.selectTrack(track, change: .toggle) }
        header.setSnapshot(try #require(controller.snapshot), selected: tracks[0], selectedTracks: selected)
        try press(.solo)
        #expect(await (try #require(controller.mixerSwitchTail)).value)
        #expect(tracks.allSatisfy { controller.mixer($0)?.solo.value == 1 }, "adding a solo group must not clear unrelated solos")
        #expect(controller.undoCount == undo + 2)
        header.setSnapshot(try #require(controller.snapshot), selected: tracks[0], selectedTracks: selected)
        try press(.solo)
        #expect(await (try #require(controller.mixerSwitchTail)).value)
        #expect(tracks.map { controller.mixer($0)?.solo.value } == [0, 0, 0, 1])
        #expect(controller.undoCount == undo + 3 && controller.selectedTracks == selected)
        let recorded = tracks.map { controller.mixer($0) }
        await controller.close(); await controller.open(url)
        #expect(tracks.map { controller.mixer($0) } == recorded)
        await controller.close()
    }
}
