import AppKit
import EchodeBridge
import Testing

@testable import EchodeAppCore

@MainActor
struct ClipGestureDocumentTests {
    private func mouse(_ type: NSEvent.EventType, _ point: CGPoint, in view: NSView, clicks: Int = 1) throws -> NSEvent
    {
        try #require(
            NSEvent.mouseEvent(
                with: type, location: view.convert(point, to: nil), modifierFlags: [], timestamp: 0,
                windowNumber: view.window?.windowNumber ?? 0, context: nil, eventNumber: 1, clickCount: clicks,
                pressure: 1))
    }

    private func show(
        _ controller: ProjectController, in pane: ArrangePaneView, track: UUID, clip: UUID,
        completion: @escaping (Task<Void, Never>) -> Void
    ) throws -> ClipControlLayout {
        // Match ArrangeCanvas.updateNSView: callbacks change before the document ID and snapshot.
        let document = try #require(controller.documentID)
        pane.onClipEdit = { edit in
            completion(
                Task {
                    switch edit {
                    case .gain(let id, let gain): await controller.setClipGain(id, gain: gain, documentID: document)
                    case .fades(let id, let entering, let leaving):
                        await controller.setClipFades(id, fadeIn: entering, fadeOut: leaving, documentID: document)
                    }
                })
        }
        pane.timeline.audioDocument = document
        pane.setSnapshot(try #require(controller.snapshot), selected: track)
        pane.setControlSelection(clip: clip, preview: nil)
        pane.timeline.setViewport(
            ArrangeCamera(pixelsPerBeat: 200), header: ArrangeHeaderInput(width: 160, palette: ArrangeHeaderPalette()))
        let frame = ArrangeFrame(input: pane.timeline.renderInput)
        pane.timeline.present(frame)
        return try #require(frame.clipControlLayout)
    }

    @Test func contractOldClipControlsCannotEditTheReopenedDocument() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-clip-gesture-document-\(UUID())")
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
        let snapshot = try #require(controller.snapshot)
        let clip = try #require(
            snapshot.visibleElements(
                originTick: 0, widthTicks: 705_600 * 8,
                originTrack: 0, heightTracks: 2, pixelsPerTick: 200 / 705_600, pixelsPerTrack: 64
            )
            .elements.first { $0.role == .clip }?.id.placement)
        #expect(await controller.setClipGain(clip, gain: 2))
        #expect(await controller.setClipFades(clip, fadeIn: 70_560_000, fadeOut: 141_120_000))
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 1200, height: 128))
        pane.layout()
        let timeline = pane.timeline
        timeline.clicks.interval = 60  // Settle explicitly, without a timing assertion.
        var completion: Task<Void, Never>?

        for handle in [ClipHandle.gain, .fadeIn, .fadeOut] {
            for count in [1, 2, 3, 4] {
                let layout = try show(controller, in: pane, track: track, clip: clip) { completion = $0 }
                let rect = handle == .gain ? layout.gain : handle == .fadeIn ? layout.entering : layout.leaving
                let start = CGPoint(x: rect.midX, y: rect.midY)
                let end = CGPoint(x: start.x + 12, y: start.y - 8)
                timeline.mouseDown(with: try mouse(.leftMouseDown, start, in: timeline, clicks: count == 4 ? 1 : count))
                if count == 2 { timeline.mouseUp(with: try mouse(.leftMouseUp, start, in: timeline, clicks: count)) }
                if count == 4 {
                    timeline.mouseDragged(with: try mouse(.leftMouseDragged, end, in: timeline))
                    #expect(timeline.renderInput.clipDraft != nil)
                }
                let entry = timeline.entry
                if count == 3 {
                    #expect(entry != nil)
                    entry?.stringValue = handle == .gain ? "0 dB" : "250 ms"
                } else {
                    #expect(timeline.clicks.active)
                }
                await controller.close()
                await controller.open(url)
                _ = try show(controller, in: pane, track: track, clip: clip) { completion = $0 }
                #expect(!timeline.clicks.active, "document replacement must discard old clip presses and reset waits")
                #expect(
                    timeline.entry == nil && (entry == nil || entry?.closed == true),
                    "document replacement must close the old clip entry without submitting it")
                timeline.mouseDragged(with: try mouse(.leftMouseDragged, end, in: timeline))
                timeline.mouseUp(with: try mouse(.leftMouseUp, end, in: timeline))
                timeline.clicks.settle()
                entry?.submit()
                await entry?.pending?.value
                await completion?.value
                #expect(controller.undoCount == 0, "old clip input must not consume history in the reopened document")
                #expect(controller.snapshot?.clipMix(clip)?.gain == 2)
                #expect(controller.snapshot?.clipMix(clip)?.fadeIn == 70_560_000)
                #expect(controller.snapshot?.clipMix(clip)?.fadeOut == 141_120_000)
                await controller.close()
                await controller.open(url)
                #expect(
                    controller.snapshot?.clipMix(clip)?.gain == 2
                        && controller.snapshot?.clipMix(clip)?.fadeIn == 70_560_000
                        && controller.snapshot?.clipMix(clip)?.fadeOut == 141_120_000,
                    "old clip controls must not persist edits into the reopened project")
            }
        }
        // A submitted field may still be queued on MainActor when the view detaches its document.
        let layout = try show(controller, in: pane, track: track, clip: clip) { completion = $0 }
        let point = CGPoint(x: layout.gain.midX, y: layout.gain.midY)
        timeline.mouseDown(with: try mouse(.leftMouseDown, point, in: timeline, clicks: 3))
        let pendingEntry = try #require(timeline.entry)
        pendingEntry.stringValue = "0 dB"
        pendingEntry.submit()
        timeline.audioDocument = nil
        await pendingEntry.pending?.value
        await completion?.value
        #expect(
            controller.undoCount == 0 && controller.snapshot?.clipMix(clip)?.gain == 2,
            "a queued clip entry must recheck its document before invoking its edit callback")
        #expect(pendingEntry.closed && timeline.entry == nil)
        await controller.close()
    }

    @Test func contractClipGestureKeepsItsOriginalEditCallback() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-clip-callback-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("clip.wav")
        try wavMono(rate: 48_000, frames: 48_000).write(to: wav)
        let controller = ProjectController(sendAudio: { _, _ in AudioState() })
        let url = root.appendingPathComponent("Project.echode")
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Audio")
        #expect(await controller.importAudio(wav))
        let track = try #require(controller.selection)
        let snapshot = try #require(controller.snapshot)
        let clip = try #require(
            snapshot.visibleElements(
                originTick: 0, widthTicks: 705_600 * 8,
                originTrack: 0, heightTracks: 2, pixelsPerTick: 200 / 705_600, pixelsPerTrack: 64
            )
            .elements.first { $0.role == .clip }?.id.placement)
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 1200, height: 128))
        pane.layout()
        var completion: Task<Void, Never>?
        var borrowed = 0
        let before = controller.undoCount
        let timeline = pane.timeline
        timeline.clicks.interval = 60
        for count in [1, 2, 3] {
            let layout = try show(controller, in: pane, track: track, clip: clip) { completion = $0 }
            let point = CGPoint(x: layout.gain.midX, y: layout.gain.midY)
            timeline.mouseDown(with: try mouse(.leftMouseDown, point, in: timeline, clicks: count))
            pane.onClipEdit = { _ in borrowed += 1 }
            if count == 1 {
                let end = CGPoint(x: point.x, y: point.y - 8)
                timeline.mouseDragged(with: try mouse(.leftMouseDragged, end, in: timeline))
                timeline.mouseUp(with: try mouse(.leftMouseUp, end, in: timeline))
            } else if count == 2 {
                timeline.mouseUp(with: try mouse(.leftMouseUp, point, in: timeline, clicks: count))
                timeline.clicks.settle()
            } else {
                let entry = try #require(timeline.entry)
                entry.stringValue = "6 dB"
                entry.submit()
                await entry.pending?.value
            }
            await completion?.value
            #expect(borrowed == 0, "a clip gesture must retain the edit callback captured on its press")
            #expect(controller.undoCount == before + count)
        }
        let gain = controller.snapshot?.clipMix(clip)?.gain
        #expect(gain == Float(pow(10, 6.0 / 20)))
        await controller.undo()
        #expect(controller.snapshot?.clipMix(clip)?.gain == 1)
        await controller.redo()
        await controller.close()
        await controller.open(url)
        #expect(controller.snapshot?.clipMix(clip)?.gain == gain)
        await controller.close()
    }
}
