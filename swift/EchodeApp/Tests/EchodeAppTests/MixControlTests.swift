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
        // A click away from the knob position does not jump there (ADR 0153).
        let down = CGPoint(x: layout.gain.minX + 1, y: layout.gain.midY)
        header.mouseDown(with: try event(.leftMouseDown, down))
        header.mouseUp(with: try event(.leftMouseUp, down))
        await completion?.value
        #expect(controller.faderPreview == nil)
        #expect(controller.mixer(track)?.gain.value == 1)
        #expect(controller.undoCount == before)
        header.mouseDown(with: try event(.leftMouseDown, down))
        for step in 1...20 {
            header.mouseDragged(with: try event(.leftMouseDragged, CGPoint(x: down.x + layout.gain.width * 0.2 * Double(step) / 25, y: down.y)))
        }
        #expect(controller.mixer(track)?.gain.value == 1)
        #expect(controller.undoCount == before)
        let up = CGPoint(x: down.x + layout.gain.width * 0.2, y: down.y)
        header.mouseUp(with: try event(.leftMouseUp, up))
        await completion?.value
        #expect(controller.errorMessage == nil)
        let expected = MixScale.dragged(.gain, from: 1, by: 0.2)
        #expect(expected > 1)
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
        #expect(controller.mixer(track)?.pan.value == 0)
        header.mouseDown(with: try event(.leftMouseDown, pan))
        header.mouseDragged(with: try event(.leftMouseDragged, CGPoint(x: pan.x + layout.pan.width * 0.25, y: pan.y)))
        header.mouseUp(with: try event(.leftMouseUp, CGPoint(x: pan.x + layout.pan.width * 0.25, y: pan.y)))
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

    @Test func headerControlsSpeakTheInspectorUnits() {
        #expect(MixScale.spokenValue(.gain, 1) == "0.0 dB")
        #expect(MixScale.spokenValue(.gain, 0) == "−∞ dB")
        #expect(MixScale.spokenValue(.pan, 0) == "0.0%")
        #expect(MixScale.spokenValue(.pan, -1) == "-100.0%")
        #expect(MixScale.spokenValue(.mute, 1) == nil)
        #expect(MixScale.spokenValue(.solo, 0) == nil)
        let first = MixScale.adjusted(.gain, value: 0, direction: 1)
        #expect(abs(MixScale.decibels(first) - (-59)) < 0.01)
        #expect(MixScale.position(first) > 0.01)
        #expect(MixScale.position(0.001) < 1e-6)
        #expect(MixScale.adjusted(.gain, value: 0, direction: -1) == 0)
        #expect(MixScale.adjusted(.gain, value: 0.001, direction: -1) == 0)
        #expect(MixScale.adjusted(.gain, value: first, direction: -1) == 0)
        #expect(abs(MixScale.decibels(MixScale.adjusted(.gain, value: 1, direction: -1)) - (-1)) < 0.01)
        #expect(MixScale.adjusted(.gain, value: 4, direction: 1) == 4)
        #expect(MixScale.adjusted(.pan, value: 0, direction: 1) == 0.05)
        #expect(MixScale.adjusted(.pan, value: 1, direction: 1) == 1)
        #expect(MixScale.adjusted(.mute, value: 0, direction: -1) == 1)
        #expect(MixScale.adjusted(.mute, value: 1, direction: 1) == 0)
        var step = MixStep(basis: 1)
        let raised = step.advance(.gain, direction: 1)
        let raisedAgain = step.advance(.gain, direction: 1)
        #expect(raised != nil && raisedAgain != nil && raised != raisedAgain)
        #expect(step.adopt(1) == raisedAgain)
        #expect(step.pending)
        #expect(step.adopt(raisedAgain ?? 0) == raisedAgain)
        #expect(!step.pending)
    }

    @Test func repeatedMixerStepsLandWhileTheFirstEditIsStillBusy() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-steps-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Steps")
        let track = try #require(controller.selection)
        let before = controller.undoCount
        let step1 = MixScale.adjusted(.gain, value: 1, direction: 1)
        let step2 = MixScale.adjusted(.gain, value: step1, direction: 1)
        let first = Task { await controller.setMixer(track, field: .gain, value: step1) }
        var overlapped = false
        for _ in 0..<1000 {
            if controller.edits.busy { overlapped = true; break }
            await Task.yield()
        }
        let second = Task { await controller.setMixer(track, field: .gain, value: step2) }
        #expect(await first.value)
        #expect(await second.value)
        #expect(abs(try #require(controller.mixer(track)).gain.value - step2) < 0.0001)
        #expect(controller.undoCount == before + 2)
        #expect(overlapped)
        await controller.close()
    }

    @Test func editTurnWaitsForTheBodyAlreadyRunning() async {
        let turn = EditTurn()
        let gate = TurnGate()
        var secondStarted = false
        let first = Task { await turn.run { await gate.wait(); return 1 } }
        await gate.entered()
        let second = Task { await turn.run { secondStarted = true; return 2 } }
        for _ in 0..<20 { await Task.yield() }
        #expect(!secondStarted)
        #expect(turn.busy)
        gate.resume()
        #expect(await first.value == 1)
        #expect(await second.value == 2)
        #expect(!turn.busy)
    }

    @Test func onlyDocumentReplacementLocksInteractionWhileEditsStayOrdered() async {
        let turn = EditTurn()
        let gate = TurnGate()
        var locks: [Bool] = []
        turn.onExclusive = { locks.append($0) }
        let first = Task { await turn.run { await gate.wait(); return 1 } }
        await gate.entered()
        #expect(turn.busy && locks.isEmpty)
        let replacement = Task { await turn.run(exclusive: true) { return 2 } }
        for _ in 0..<20 { await Task.yield() }
        #expect(locks == [true])
        let last = Task { await turn.run { #expect(locks == [true, false]); return 3 } }
        gate.resume()
        #expect(await first.value == 1)
        #expect(await replacement.value == 2)
        #expect(await last.value == 3)
        #expect(!turn.busy && locks == [true, false])
    }

    @Test func queuedSwitchesBackToTheDisplayedValueAreNotDropped() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-switches-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "switches")
        let document = try #require(controller.documentID)
        let track = try #require(controller.selection)
        #expect(await controller.insertPlugin("org.example.fx", on: track, documentID: document))
        let device = try #require(controller.deviceRows(on: track).first?.id)

        func roundTrip(_ on: @escaping @MainActor () async -> Bool, _ off: @escaping @MainActor () async -> Bool) async {
            let gate = TurnGate()
            let held = Task { await controller.edits.run { await gate.wait() } }
            await gate.entered()
            let before = controller.undoCount
            let first = Task { await on() }
            for _ in 0..<20 { await Task.yield() }
            let second = Task { await off() }
            for _ in 0..<20 { await Task.yield() }
            // Both requests arrive while the snapshot still shows the old
            // value. The second must be compared after the first publishes.
            gate.resume()
            await held.value
            #expect(await first.value)
            #expect(await second.value)
            #expect(controller.undoCount == before + 2)
        }
        await roundTrip({ await controller.editDeviceChain(.bypass(device: device, bypassed: true), documentID: document) },
                        { await controller.editDeviceChain(.bypass(device: device, bypassed: false), documentID: document) })
        await roundTrip({ await controller.editDeviceChain(.offline(device: device, offline: true), documentID: document) },
                        { await controller.editDeviceChain(.offline(device: device, offline: false), documentID: document) })
        await roundTrip({ await controller.setPinned(1, device: device, pinned: true, documentID: document) },
                        { await controller.setPinned(1, device: device, pinned: false, documentID: document) })
        await roundTrip({ await controller.setMixer(track, field: .gain, value: 0.5, documentID: document) },
                        { await controller.setMixer(track, field: .gain, value: 1, documentID: document) })
        let row = try #require(controller.deviceRows(on: track).first)
        #expect(!row.bypassed && !row.offline && row.pinned.isEmpty)
        #expect(controller.mixer(track)?.gain.value == 1)
        await controller.undo()
        #expect(controller.mixer(track)?.gain.value == 0.5)
        await controller.close()
    }

    @Test func startingAFaderDoesNotDropAnEditAlreadyQueued() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-queued-fader-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController(sendAudio: { _, _ in AudioState() })
        await controller.open(url, creating: true)
        await controller.addTrack(name: "queued fader")
        let track = try #require(controller.selection)
        let document = try #require(controller.documentID)
        let gate = TurnGate()
        let held = Task { await controller.edits.run { await gate.wait() } }
        await gate.entered()
        let edit = Task { await controller.setMixer(track, field: .gain, value: 0.5, documentID: document) }
        for _ in 0..<20 { await Task.yield() }
        #expect(controller.beginFader(track, field: .gain))
        // A drag that ends back on its displayed starting value still has to
        // restore that value if the earlier edit changed it in the meantime.
        controller.changeFader(1)
        gate.resume()
        await held.value
        #expect(await edit.value)
        #expect(controller.mixer(track)?.gain.value == 0.5)
        await controller.finishFader(commit: true)
        #expect(controller.mixer(track)?.gain.value == 1)
        await controller.undo()
        #expect(controller.mixer(track)?.gain.value == 0.5)
        await controller.close()
    }

    @Test func inspectorNumbersAcceptTheShownUnitAndSilence() {
        #expect(MixEntry.number("−∞", unit: "dB", silence: true) == -.infinity)
        #expect(MixEntry.number("-inf dB", unit: "dB", silence: true) == -.infinity)
        #expect(MixEntry.number("−6 dB", unit: "dB", silence: true) == -6)
        #expect(MixEntry.number("0,5", unit: "dB", silence: false) == 0.5)
        #expect(MixEntry.number("50%", unit: "%", silence: false) == 50)
        #expect(MixEntry.number("5 ms", unit: "ms", silence: false) == 5)
        #expect(MixEntry.number("inf", unit: "dB", silence: true) == nil)
        #expect(MixEntry.number("", unit: "dB", silence: true) == nil)
        #expect(MixEntry.number("12 dB extra", unit: "dB", silence: false) == nil)
    }

    @Test func inspectorMeterFollowsPlaybackAndStopsAtTheFloor() {
        #expect(MeterFollow.running(transportActive: true, playing: false, peak: 0))
        #expect(MeterFollow.running(transportActive: false, playing: true, peak: 0))
        #expect(MeterFollow.running(transportActive: false, playing: false, peak: 0.2))
        #expect(!MeterFollow.running(transportActive: false, playing: false, peak: 0))
        #expect(!MeterFollow.running(transportActive: false, playing: false, peak: MixScale.meterSilence))
        var hold = MeterLinkHold()
        hold.note(playing: true, now: 10)
        hold.note(playing: false, now: 10)
        #expect(hold.holding(now: 10.1))
        #expect(!hold.holding(now: 10 + MeterLinkHold.duration))
        hold.note(playing: false, now: 12)
        #expect(!hold.holding(now: 12))
    }
}

@MainActor
private final class TurnGate {
    private var started: CheckedContinuation<Void, Never>?
    private var hold: CheckedContinuation<Void, Never>?
    private var waiting = false
    private var released = false
    func wait() async {
        waiting = true
        let notify = started
        started = nil
        notify?.resume()
        if released { return }
        await withCheckedContinuation { hold = $0 }
    }
    func entered() async {
        if waiting { return }
        await withCheckedContinuation { started = $0 }
    }
    func resume() {
        released = true
        hold?.resume()
        hold = nil
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
