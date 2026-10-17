import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct MixControlTests {
    private func mouse(_ type: NSEvent.EventType, _ point: CGPoint, in view: NSView) throws -> NSEvent {
        try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
    }

    @Test func pendingPreviewKeepsOnlyTheLatestValueAndCloseCancelsIt() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-preview-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let gate = PreviewGate()
        let controller = ProjectController(sendAudio: { _, command in
            if case .previewFader(_, let gain, _) = command { await gate.receive(gain) }
            return AudioState()
        })
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Preview")
        let track = try #require(controller.selection)
        let oldDocument = try #require(controller.documentID)
        #expect(controller.beginFader(track, field: .gain))
        controller.changeFader(0.1)
        await gate.entered()
        for step in 1...50 { controller.changeFader(Float(step) / 100) }
        #expect(await gate.values.count == 1)
        let close = Task { await controller.close() }
        await Task.yield()
        #expect(controller.documentID == oldDocument)
        await gate.release()
        await close.value
        #expect(await gate.values == [0.1, 0.5])
        #expect(controller.faderPreview == nil)
        await controller.open(url)
        #expect(controller.mixer(track)?.gain.value == 1)
        #expect(!(await controller.setMixer(track, field: .gain, value: 0, documentID: oldDocument)))
        #expect(controller.mixer(track)?.gain.value == 1)
        await controller.close()
    }

    @Test func contractHeaderMixerGesturesPersistOnceAndUndo() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Mixer")
        let track = try #require(controller.selection)
        let before = controller.undoCount
        let original = try #require(controller.mixer(track))
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 200, height: 64))
        func event(_ type: NSEvent.EventType, _ point: CGPoint) throws -> NSEvent { try mouse(type, point, in: header) }
        var completion: Task<Void, Never>?
        header.setSnapshot(try #require(controller.snapshot), selected: track)
        header.mixActions = MixActions(begin: { controller.beginFader($0, field: $1) }, change: { controller.changeFader($0) },
            finish: { commit in completion = Task { await controller.finishFader(commit: commit) } },
            set: { id, field, value in completion = Task { await controller.setMixer(id, field: field, value: value) } })
        let layout = TrackControlLayout(row: header.bounds)
        #expect(layout.visible)
        let down = CGPoint(x: layout.gain.minX + 1, y: layout.gain.midY)
        header.mouseDown(with: try event(.leftMouseDown, down))
        for step in 1...20 {
            header.mouseDragged(with: try event(.leftMouseDragged, CGPoint(x: layout.gain.minX + layout.gain.width * Double(step) / 25, y: down.y)))
        }
        #expect(controller.mixer(track)?.gain.value == 1)
        #expect(controller.undoCount == before)
        let up = CGPoint(x: layout.gain.minX + layout.gain.width * 0.8, y: down.y)
        header.mouseUp(with: try event(.leftMouseUp, up))
        await completion?.value
        #expect(controller.errorMessage == nil)
        let expected = MixScale.gain(0.8)
        #expect(abs(try #require(controller.mixer(track)).gain.value - expected) < 0.00001)
        #expect(controller.undoCount == before + 1)
        await controller.undo()
        #expect(controller.mixer(track)?.gain.value == 1)
        await controller.redo()
        #expect(abs(try #require(controller.mixer(track)).gain.value - expected) < 0.00001)

        for field in [MixerField.mute, .solo] {
            header.setSnapshot(try #require(controller.snapshot), selected: track)
            let point = CGPoint(x: layout.rect(field).midX, y: layout.rect(field).midY)
            header.mouseDown(with: try event(.leftMouseDown, point))
            await completion?.value
            #expect(controller.mixer(track)?[field].value == 1)
        }
        header.setSnapshot(try #require(controller.snapshot), selected: track)
        let pan = CGPoint(x: layout.pan.minX + layout.pan.width * 0.75, y: layout.pan.midY)
        header.mouseDown(with: try event(.leftMouseDown, pan))
        header.mouseUp(with: try event(.leftMouseUp, pan))
        await completion?.value
        #expect(abs(try #require(controller.mixer(track)).pan.value - 0.5) < 0.0001)

        let saved = try #require(controller.mixer(track))
        let count = controller.undoCount
        #expect(controller.beginFader(track, field: .gain))
        controller.changeFader(0)
        await controller.finishFader(commit: false)
        #expect(controller.mixer(track) == saved)
        #expect(controller.undoCount == count)
        header.setSnapshot(try #require(controller.snapshot), selected: track)
        header.mouseDown(with: try event(.leftMouseDown, down))
        header.mouseCancelled(with: try event(.leftMouseUp, down))
        header.mouseUp(with: try event(.leftMouseUp, down))
        await completion?.value
        #expect(controller.mixer(track) == saved)
        #expect(controller.undoCount == count)
        #expect(!(await controller.setMixer(track, field: .pan, value: .nan)))
        let feed = controller.playback
        await controller.close()
        #expect(feed?.peak(track: track) == 0)
        await controller.open(url)
        #expect(controller.mixer(track) == saved)
        #expect(controller.mixer(track)?.gain.id == original.gain.id)
        await controller.close()
    }

    @Test func contractClipHandlesPersistGainAndFadesInDocumentTime() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-clip-controls-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("clip.wav")
        try wavMono(rate: 48_000, frames: 48_000).write(to: wav)
        let url = root.appendingPathComponent("Project.echode")
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Audio")
        #expect(await controller.importAudio(wav))
        let track = try #require(controller.selection)
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 1200, height: 128))
        pane.layout()
        func event(_ type: NSEvent.EventType, _ point: CGPoint) throws -> NSEvent { try mouse(type, point, in: pane.timeline) }
        var completion: Task<Void, Never>?
        pane.onClipEdit = { edit in completion = Task {
            switch edit {
            case .gain(let id, let gain): await controller.setClipGain(id, gain: gain)
            case .fades(let id, let entering, let leaving): await controller.setClipFades(id, fadeIn: entering, fadeOut: leaving)
            }
        } }
        let camera = ArrangeCamera(pixelsPerBeat: 200)
        let initial = try #require(controller.snapshot)
        let id = try #require(initial.visibleElements(originTick: 0, widthTicks: 705_600 * 8,
            originTrack: 0, heightTracks: 2, pixelsPerTick: 200 / 705_600, pixelsPerTrack: 64)
            .elements.first { $0.role == .clip }?.id.placement)
        func present() throws -> ArrangeFrame {
            let snapshot = try #require(controller.snapshot)
            pane.setSnapshot(snapshot, selected: track)
            pane.setControlSelection(clip: id, preview: nil)
            pane.timeline.setViewport(camera, header: ArrangeHeaderInput(width: 160, palette: ArrangeHeaderPalette()))
            let frame = ArrangeFrame(input: pane.timeline.renderInput)
            pane.timeline.present(frame)
            return frame
        }
        let before = controller.undoCount
        let frame = try present()
        let layout = try #require(frame.clipControlLayout)
        let down = CGPoint(x: layout.gain.midX, y: layout.gain.midY)
        pane.timeline.mouseDown(with: try event(.leftMouseDown, down))
        pane.timeline.mouseDragged(with: try event(.leftMouseDragged, CGPoint(x: down.x, y: down.y - 24)))
        #expect(controller.snapshot?.clipMix(id)?.gain == 1)
        pane.timeline.mouseUp(with: try event(.leftMouseUp, CGPoint(x: down.x, y: down.y - 24)))
        await completion?.value
        #expect(abs(try #require(controller.snapshot?.clipMix(id)).gain - Float(pow(10, 6.0 / 20))) < 0.00001)
        #expect(controller.undoCount == before + 1)

        for entering in [true, false] {
            let frame = try present()
            let layout = try #require(frame.clipControlLayout)
            let handle = entering ? layout.entering : layout.leaving
            let down = CGPoint(x: handle.midX, y: handle.midY)
            let up = CGPoint(x: layout.rect.minX + layout.rect.width * (entering ? 0.2 : 0.7), y: down.y)
            pane.timeline.mouseDown(with: try event(.leftMouseDown, down))
            pane.timeline.mouseDragged(with: try event(.leftMouseDragged, up))
            pane.timeline.mouseUp(with: try event(.leftMouseUp, up))
            await completion?.value
            let clip = try #require(controller.snapshot?.clipMix(id))
            let expected = Int64((entering ? 0.2 : 0.3) * 705_600_000)
            #expect(abs((entering ? clip.fadeIn : clip.fadeOut) - expected) < 1000)
        }
        #expect(controller.undoCount == before + 3)
        let saved = try #require(controller.snapshot?.clipMix(id))
        await controller.undo()
        #expect(controller.snapshot?.clipMix(id)?.fadeOut == 3_528_000)
        await controller.redo()
        #expect(controller.snapshot?.clipMix(id) == saved)
        let cancelLayout = try #require(present().clipControlLayout)
        let cancelDown = CGPoint(x: cancelLayout.gain.midX, y: cancelLayout.gain.midY)
        pane.timeline.mouseDown(with: try event(.leftMouseDown, cancelDown))
        pane.timeline.mouseDragged(with: try event(.leftMouseDragged, CGPoint(x: cancelDown.x, y: cancelDown.y - 20)))
        pane.timeline.mouseCancelled(with: try event(.leftMouseUp, cancelDown))
        pane.timeline.mouseUp(with: try event(.leftMouseUp, cancelDown))
        #expect(controller.snapshot?.clipMix(id) == saved)
        #expect(controller.undoCount == before + 3)
        // Loaded projects and numeric input may exceed the compact handle range.
        // Clicking it without movement must preserve that valid value.
        for gain: Float in [0.00000001, 0.0001, 1000] {
            #expect(await controller.setClipGain(id, gain: gain))
            let count = controller.undoCount
            let layout = try #require(present().clipControlLayout)
            let point = CGPoint(x: layout.gain.midX, y: layout.gain.midY)
            pane.timeline.mouseDown(with: try event(.leftMouseDown, point))
            pane.timeline.mouseUp(with: try event(.leftMouseUp, point))
            await completion?.value
            #expect(controller.snapshot?.clipMix(id)?.gain == gain)
            #expect(controller.undoCount == count)
            pane.timeline.mouseDown(with: try event(.leftMouseDown, point))
            pane.timeline.mouseUp(with: try event(.leftMouseUp, CGPoint(x: point.x + 8, y: point.y)))
            await completion?.value
            #expect(controller.snapshot?.clipMix(id)?.gain == gain)
            #expect(controller.undoCount == count)
            let moved = CGPoint(x: point.x, y: point.y + (gain < 1 ? -4 : 4))
            pane.timeline.mouseDown(with: try event(.leftMouseDown, point))
            pane.timeline.mouseDragged(with: try event(.leftMouseDragged, moved))
            pane.timeline.mouseUp(with: try event(.leftMouseUp, moved))
            await completion?.value
            let ratio = try #require(controller.snapshot?.clipMix(id)?.gain) / gain
            #expect(abs(ratio - Float(pow(10, (gain < 1 ? 1.0 : -1.0) / 20))) < 0.00001)
            #expect(controller.undoCount == count + 1)
            await controller.undo()
            #expect(controller.snapshot?.clipMix(id)?.gain == gain)
        }
        #expect(await controller.setClipGain(id, gain: saved.gain))
        #expect(controller.snapshot?.clipMix(id) == saved)
        let longFadeIn: Int64 = 705_600_000 * 4
        let longFadeOut: Int64 = 705_600_000 * 5
        #expect(await controller.setClipFades(id, fadeIn: longFadeIn, fadeOut: longFadeOut))
        for entering in [true, false] {
            let layout = try #require(present().clipControlLayout)
            let handle = entering ? layout.entering : layout.leaving
            let point = CGPoint(x: handle.midX, y: handle.midY)
            let count = controller.undoCount
            pane.timeline.mouseDown(with: try event(.leftMouseDown, point))
            pane.timeline.mouseUp(with: try event(.leftMouseUp, point))
            await completion?.value
            #expect(controller.snapshot?.clipMix(id)?.fadeIn == longFadeIn)
            #expect(controller.snapshot?.clipMix(id)?.fadeOut == longFadeOut)
            #expect(controller.undoCount == count)
            pane.timeline.mouseDown(with: try event(.leftMouseDown, point))
            pane.timeline.mouseUp(with: try event(.leftMouseUp, CGPoint(x: point.x, y: point.y + 5)))
            await completion?.value
            #expect(controller.snapshot?.clipMix(id)?.fadeIn == longFadeIn)
            #expect(controller.snapshot?.clipMix(id)?.fadeOut == longFadeOut)
            #expect(controller.undoCount == count)
        }
        #expect(await controller.setClipFades(id, fadeIn: saved.fadeIn, fadeOut: saved.fadeOut))
        #expect(controller.snapshot?.clipMix(id) == saved)
        #expect(!(await controller.setClipFades(id, fadeIn: -1, fadeOut: 0)))
        await controller.close()
        #expect(initial.clipMix(id)?.gain == 1)
        await controller.open(url)
        #expect(controller.snapshot?.clipMix(id) == saved)
        await controller.close()
    }

    @Test func inspectorPeakUsesTheHeaderMeterScaleAndStopsAtSilence() {
        #expect(MixScale.meter(1) == 1)
        #expect(MixScale.meterLabel(1) == "0.0 dB")
        let quiet = Float(pow(10.0, -20.0 / 20.0))
        #expect(abs(MixScale.meter(quiet) - 40.0 / 60.0) < 0.001)
        #expect(MixScale.meterLabel(quiet) == "-20.0 dB")
        // The arrange header draws this same fraction of the meter width.
        for peak: Float in [0, 0.0001, MixScale.meterSilence, MixScale.meterSilence / 8, 1e-20] {
            #expect(MixScale.meter(peak) == 0)
            #expect(MixScale.meterLabel(peak) == "−∞ dB")
        }
        #expect(MixScale.meterLabel(1e-6) == MixScale.meterLabel(1e-12))
        let hot = Float(pow(10.0, 6.0 / 20.0))
        #expect(MixScale.meter(hot) == 1)
        #expect(MixScale.meterLabel(hot) == "6.0 dB")
    }
}

private actor PreviewGate {
    private(set) var values: [Float] = []
    private var entry: CheckedContinuation<Void, Never>?
    private var held: CheckedContinuation<Void, Never>?
    func entered() async {
        if !values.isEmpty { return }
        await withCheckedContinuation { entry = $0 }
    }
    func receive(_ gain: Float) async {
        values.append(gain)
        entry?.resume(); entry = nil
        if values.count == 1 { await withCheckedContinuation { held = $0 } }
    }
    func release() { held?.resume(); held = nil }
}
