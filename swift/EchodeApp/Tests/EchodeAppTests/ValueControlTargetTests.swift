import AppKit
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct ValueControlTargetTests {
    private func mouse(_ type: NSEvent.EventType, _ point: CGPoint, in view: NSView, clicks: Int = 1) throws -> NSEvent {
        try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: [], timestamp: 0,
            windowNumber: view.window?.windowNumber ?? 0, context: nil, eventNumber: 1, clickCount: clicks, pressure: 1))
    }

    @Test func contractHeaderResetWaitDoesNotBorrowTheNextControlsPress() async throws {
        // Exercise both parts of target identity: the field and the track.
        for otherTrack in [false, true] {
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-reset-target-\(UUID()).echode")
            defer { removeTestFile(url) }
            let controller = ProjectController(sendAudio: { _, _ in AudioState() })
            await controller.open(url, creating: true)
            await controller.addTrack(name: "First")
            let first = try #require(controller.selection)
            await controller.addTrack(name: "Second")
            let second = try #require(controller.selection)
            let target = otherTrack ? second : first
            let field: MixerField = otherTrack ? .gain : .pan
            #expect(await controller.setMixer(first, field: .gain, value: 0.5))
            #expect(await controller.setMixer(target, field: field, value: 0.25))
            let before = controller.undoCount
            let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 220, height: 128))
            header.documentID = controller.documentID
            header.setSnapshot(try #require(controller.snapshot), selected: first)
            header.clicks.interval = 60 // Settle explicitly, without an elapsed-time assertion.
            var completion: Task<Void, Never>?
            var actions = controller.arrangeMixActions(document: controller.documentID)
            actions.finish = { commit in completion = Task { await controller.finishFader(commit: commit) } }
            header.mixActions = actions
            let resetRect = TrackControlLayout(row: CGRect(x: 0, y: 0, width: 220, height: 64)).gain
            let reset = CGPoint(x: resetRect.midX, y: resetRect.midY)
            header.mouseDown(with: try mouse(.leftMouseDown, reset, in: header, clicks: 2))
            header.mouseUp(with: try mouse(.leftMouseUp, reset, in: header, clicks: 2))
            #expect(controller.faderPreview?.gain == 1 && header.clicks.active)

            let rect = TrackControlLayout(row: CGRect(x: 0, y: otherTrack ? 64 : 0, width: 220, height: 64)).rect(field)
            let start = CGPoint(x: rect.midX, y: rect.midY)
            let end = CGPoint(x: start.x + rect.width / 10, y: start.y)
            header.mouseDown(with: try mouse(.leftMouseDown, start, in: header))
            await completion?.value // The previous reset finishes before the next drag event.
            #expect(controller.mixer(first)?.gain.value == 1 && controller.undoCount == before + 1)
            header.mouseDragged(with: try mouse(.leftMouseDragged, end, in: header))
            #expect(controller.faderPreview?.track == target && controller.faderPreview?.field == field,
                "a different control pressed during reset waiting must start its own drag")
            let expected = MixScale.dragged(field, from: 0.25, by: (end.x - start.x) / rect.width)
            #expect(controller.faderPreview?.value == expected)
            header.mouseUp(with: try mouse(.leftMouseUp, end, in: header))
            await completion?.value
            #expect(controller.mixer(target)?[field].value == expected && controller.undoCount == before + 2)
            await controller.undo()
            #expect(controller.mixer(target)?[field].value == 0.25 && controller.mixer(first)?.gain.value == 1)
            await controller.undo()
            #expect(controller.mixer(first)?.gain.value == 0.5 && controller.undoCount == before)
            await controller.redo(); await controller.redo()
            await controller.close(); await controller.open(url)
            #expect(controller.mixer(first)?.gain.value == 1 && controller.mixer(target)?[field].value == expected)
            await controller.close()
        }
    }

    @Test func contractClipResetWaitKeepsOnlyTheMatchingHandlesPress() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-clip-reset-target-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("clip.wav")
        try wavMono(rate: 48_000, frames: 48_000).write(to: wav)
        let url = root.appendingPathComponent("Project.echode")
        let controller = ProjectController(sendAudio: { _, _ in AudioState() })
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Audio")
        #expect(await controller.importAudio(wav))
        let track = try #require(controller.selection)
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 1200, height: 128))
        pane.layout()
        let timeline = pane.timeline
        timeline.clicks.interval = 60
        let camera = ArrangeCamera(pixelsPerBeat: 200)
        let initial = try #require(controller.snapshot)
        let id = try #require(initial.visibleElements(originTick: 0, widthTicks: 705_600 * 8,
            originTrack: 0, heightTracks: 2, pixelsPerTick: 200 / 705_600, pixelsPerTrack: 64)
            .elements.first { $0.role == .clip }?.id.placement)
        #expect(await controller.setClipGain(id, gain: 2))
        #expect(await controller.setClipFades(id, fadeIn: 70_560_000, fadeOut: 0))
        let before = controller.undoCount
        var completion: Task<Void, Never>?
        pane.onClipEdit = { edit in completion = Task {
            switch edit {
            case .gain(let id, let gain): await controller.setClipGain(id, gain: gain)
            case .fades(let id, let entering, let leaving): await controller.setClipFades(id, fadeIn: entering, fadeOut: leaving)
            }
        } }
        pane.setSnapshot(try #require(controller.snapshot), selected: track)
        pane.setControlSelection(clip: id, preview: nil)
        timeline.setViewport(camera, header: ArrangeHeaderInput(width: 160, palette: ArrangeHeaderPalette()))
        let frame = ArrangeFrame(input: timeline.renderInput)
        timeline.present(frame)
        let layout = try #require(frame.clipControlLayout)
        let gain = CGPoint(x: layout.gain.midX, y: layout.gain.midY)
        let fade = CGPoint(x: layout.entering.midX, y: layout.entering.midY)
        timeline.mouseDown(with: try mouse(.leftMouseDown, gain, in: timeline, clicks: 2))
        timeline.mouseUp(with: try mouse(.leftMouseUp, gain, in: timeline, clicks: 2))
        #expect(timeline.renderInput.clipDraft?.gain == 1)
        // A third-count press on another target must settle the old reset, not reuse its press.
        timeline.mouseDown(with: try mouse(.leftMouseDown, fade, in: timeline, clicks: 3))
        await completion?.value
        let entry = try #require(timeline.entry, "a different clip handle must open its own numeric entry after a pending reset")
        #expect(entry.stringValue == "100.0" && timeline.renderInput.clipDraft == nil)
        entry.stringValue = "250 ms"
        entry.submit()
        await entry.pending?.value
        await completion?.value
        #expect(controller.snapshot?.clipMix(id)?.gain == 1 && controller.undoCount == before + 2)
        #expect(controller.snapshot?.clipMix(id)?.fadeIn == 176_400_000)
        await controller.undo()
        #expect(controller.snapshot?.clipMix(id)?.gain == 1 && controller.snapshot?.clipMix(id)?.fadeIn == 70_560_000)
        await controller.redo()
        await controller.close(); await controller.open(url)
        #expect(controller.snapshot?.clipMix(id)?.gain == 1 && controller.snapshot?.clipMix(id)?.fadeIn == 176_400_000)
        await controller.close()
    }
}
