import AppKit
import EchodeBridge
import Testing

@testable import EchodeAppCore

@MainActor
struct MixerGestureDocumentTests {
    private func mouse(_ type: NSEvent.EventType, _ point: CGPoint, in header: ArrangeHeaderView, clicks: Int = 1)
        throws -> NSEvent
    {
        try #require(
            NSEvent.mouseEvent(
                with: type, location: header.convert(point, to: nil), modifierFlags: [], timestamp: 0,
                windowNumber: header.window?.windowNumber ?? 0, context: nil, eventNumber: 1, clickCount: clicks,
                pressure: 1))
    }

    private func show(_ controller: ProjectController, in header: ArrangeHeaderView, track: UUID) throws {
        // Match ArrangeCanvas.updateNSView: replace callbacks before publishing the document ID.
        header.mixActions = controller.arrangeMixActions(document: controller.documentID)
        header.documentID = controller.documentID
        header.setSnapshot(try #require(controller.snapshot), selected: track)
    }

    @Test func contractOldHeaderPressCannotStartADragInTheReopenedDocument() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-mixer-press-document-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "saved")
        let track = try #require(controller.selection)
        #expect(await controller.setMixer(track, field: .gain, value: 0.5))
        let window = NSWindow(
            contentRect: CGRect(x: 0, y: 0, width: 220, height: 64), styleMask: [.borderless], backing: .buffered,
            defer: true)
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 220, height: 64))
        window.contentView = header
        try show(controller, in: header, track: track)
        let rect = TrackControlLayout(row: header.bounds).gain
        let start = CGPoint(x: rect.midX, y: rect.midY)
        let end = CGPoint(x: rect.midX + 8, y: rect.midY)
        header.mouseDown(with: try mouse(.leftMouseDown, start, in: header))
        #expect(header.clicks.active && controller.faderPreview == nil)
        await controller.close()
        await controller.open(url)
        try show(controller, in: header, track: track)
        #expect(!header.clicks.active, "a document change must discard the old header press")
        header.mouseDragged(with: try mouse(.leftMouseDragged, end, in: header))
        #expect(controller.faderPreview == nil, "an old press must not start a fader preview in the reopened document")
        header.mouseUp(with: try mouse(.leftMouseUp, end, in: header))
        // Complete any accidentally started preview so that late persistence is also visible.
        await controller.finishFader(commit: true)
        #expect(controller.undoCount == 0 && controller.mixer(track)?.gain.value == 0.5)
        await controller.close()
        await controller.open(url)
        #expect(controller.mixer(track)?.gain.value == 0.5, "an old drag must not persist into the reopened project")
        await controller.close()
    }

    @Test func contractOldResetAndEntryCannotFinishANewDocumentsPreview() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-mixer-reset-document-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "saved")
        let track = try #require(controller.selection)
        #expect(await controller.setMixer(track, field: .gain, value: 0.5))
        let window = NSWindow(
            contentRect: CGRect(x: 0, y: 0, width: 220, height: 64), styleMask: [.borderless], backing: .buffered,
            defer: true)
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 220, height: 64))
        window.contentView = header
        header.clicks.interval = 60  // Keep settlement deterministic; no elapsed-time assertion.
        try show(controller, in: header, track: track)
        let rect = TrackControlLayout(row: header.bounds).gain
        let point = CGPoint(x: rect.midX, y: rect.midY)
        header.mouseDown(with: try mouse(.leftMouseDown, point, in: header, clicks: 2))
        header.mouseUp(with: try mouse(.leftMouseUp, point, in: header, clicks: 2))
        #expect(controller.faderPreview?.gain == 1 && header.clicks.active)
        await controller.close()
        await controller.open(url)
        #expect(controller.beginFader(track, field: .pan))
        controller.changeFader(0.25)
        // An update may arrive after a new document's preview has already started.
        var currentActions = controller.arrangeMixActions(document: controller.documentID)
        let finish = currentActions.finish
        var newFinishes = 0
        currentActions.finish = {
            newFinishes += 1
            finish($0)
        }
        header.mixActions = currentActions
        header.documentID = controller.documentID
        header.setSnapshot(try #require(controller.snapshot), selected: track)
        header.clicks.settle()
        #expect(newFinishes == 0, "an old reset must retain its original finish callback during document replacement")
        #expect(controller.faderPreview?.field == .pan && controller.faderPreview?.pan == 0.25)
        await controller.finishFader(commit: false)
        #expect(
            controller.undoCount == 0 && controller.mixer(track)?.pan.value == 0,
            "an old reset must not commit a new document's preview")
        #expect(!header.clicks.active && controller.mixer(track)?.gain.value == 0.5)
        header.mouseDown(with: try mouse(.leftMouseDown, point, in: header, clicks: 3))
        let entry = try #require(header.entry)
        entry.stringValue = "0 dB"
        await controller.close()
        await controller.open(url)
        try show(controller, in: header, track: track)
        #expect(
            entry.closed && header.entry == nil,
            "document replacement must close the old numeric entry without submission")
        entry.submit()
        await entry.pending?.value
        #expect(
            controller.undoCount == 0 && controller.mixerSwitchTail == nil && controller.mixer(track)?.gain.value == 0.5
        )
        await controller.close()
    }

    @Test func sameDocumentUpdatesKeepThePressCallbacksAndFreshGesturesStillWork() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "echode-mixer-gesture-update-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "saved")
        let track = try #require(controller.selection)
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 220, height: 64))
        try show(controller, in: header, track: track)
        let rect = TrackControlLayout(row: header.bounds).pan
        let start = CGPoint(x: rect.midX, y: rect.midY)
        let end = CGPoint(x: rect.midX + 4, y: rect.midY)
        header.mouseDown(with: try mouse(.leftMouseDown, start, in: header))
        var borrowed = 0
        let replacement = MixActions(
            begin: { _, _ in
                borrowed += 1
                return false
            }, change: { _ in borrowed += 1 },
            finish: { _ in borrowed += 1 }, set: { _, _, _ in borrowed += 1 })
        header.mixActions = replacement
        header.documentID = controller.documentID
        header.mouseDragged(with: try mouse(.leftMouseDragged, end, in: header))
        #expect(controller.faderPreview?.pan == MixScale.dragged(.pan, from: 0, by: 4 / rect.width))
        header.mouseUp(with: try mouse(.leftMouseUp, end, in: header))
        await controller.finishFader(commit: true)
        #expect(borrowed == 0, "a gesture must use the callbacks captured on its press, not a later view update")
        #expect(controller.undoCount == 2 && controller.mixer(track)?.pan.value != 0)
        await controller.undo()
        #expect(controller.mixer(track)?.pan.value == 0)
        await controller.redo()
        let pan = controller.mixer(track)?.pan.value
        await controller.close()
        await controller.open(url)
        #expect(controller.mixer(track)?.pan.value == pan)
        await controller.close()
    }
}
