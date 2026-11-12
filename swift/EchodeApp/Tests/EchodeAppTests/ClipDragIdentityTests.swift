import AppKit
import EchodeBridge
import Testing

@testable import EchodeAppCore

@MainActor
struct ClipDragIdentityTests {
    private func mouse(_ type: NSEvent.EventType, _ point: CGPoint, in view: NSView) throws -> NSEvent {
        try #require(
            NSEvent.mouseEvent(
                with: type, location: view.convert(point, to: nil), modifierFlags: [], timestamp: 0,
                windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
    }

    @Test func contractOrthogonalClipDragsPreserveExactValuesAndRedo() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-clip-drag-identity-\(UUID())")
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
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 1200, height: 128))
        pane.layout()
        let timeline = pane.timeline
        var completion: Task<Void, Never>?
        pane.onClipEdit = { edit in
            completion = Task {
                switch edit {
                case .gain(let id, let gain): await controller.setClipGain(id, gain: gain)
                case .fades(let id, let entering, let leaving):
                    await controller.setClipFades(id, fadeIn: entering, fadeOut: leaving)
                }
            }
        }
        func present() throws -> ClipControlLayout {
            timeline.audioDocument = controller.documentID
            pane.setSnapshot(try #require(controller.snapshot), selected: track)
            pane.setControlSelection(clip: clip, preview: nil)
            timeline.setViewport(
                ArrangeCamera(pixelsPerBeat: 200),
                header: ArrangeHeaderInput(width: 160, palette: ArrangeHeaderPalette()))
            let frame = ArrangeFrame(input: timeline.renderInput)
            timeline.present(frame)
            return try #require(frame.clipControlLayout)
        }
        // Quiet/subnormal and large gains must not round-trip through dB. Fades
        // include sub-tick flicks and values longer than this one-second fixture.
        let cases: [(Float, Int64, Int64)] = [
            (0, 0, 0), (.leastNonzeroMagnitude, 70_560_001, 141_120_007),
            (0.000000000001, 70_560_001, 141_120_007),
            (0.00000001, 70_560_001, 141_120_007), (0.001, 70_560_000, 141_120_000),
            (0.12345679, 705_600_000 * 4, 705_600_000 * 5), (MixScale.clipGainMax, 70_560_001, 141_120_007),
        ]
        for (gain, entering, leaving) in cases {
            if controller.snapshot?.clipMix(clip)?.gain != gain {
                #expect(await controller.setClipGain(clip, gain: gain))
            }
            let current = try #require(controller.snapshot?.clipMix(clip))
            if current.fadeIn != entering || current.fadeOut != leaving {
                #expect(await controller.setClipFades(clip, fadeIn: entering, fadeOut: leaving))
            }
            #expect(await controller.setMixer(track, field: .pan, value: 0.25))
            await controller.undo()
            let before = controller.undoCount
            let original = try #require(controller.snapshot?.clipMix(clip))
            #expect(controller.redoCount == 1)
            for handle in [ClipHandle.gain, .fadeIn, .fadeOut] {
                for returning in [false, true] {
                    let layout = try present()
                    let rect = handle == .gain ? layout.gain : handle == .fadeIn ? layout.entering : layout.leaving
                    let start = CGPoint(x: rect.midX, y: rect.midY)
                    let orthogonal = CGPoint(
                        x: start.x + (handle == .gain ? 8 : 0), y: start.y + (handle == .gain ? 0 : 8))
                    let displaced =
                        handle == .gain
                        ? CGPoint(x: start.x, y: start.y + (gain == MixScale.clipGainMax ? 8 : -24))
                        : CGPoint(x: start.x + (handle == .fadeIn ? 8 : -8), y: start.y)
                    completion = nil
                    timeline.mouseDown(with: try mouse(.leftMouseDown, start, in: timeline))
                    timeline.mouseDragged(
                        with: try mouse(.leftMouseDragged, returning ? displaced : orthogonal, in: timeline))
                    let draft = try #require(timeline.renderInput.clipDraft)
                    #expect(draft.handle == handle)
                    if returning {
                        #expect(
                            draft.gain != original.gain || draft.fadeIn != original.fadeIn
                                || draft.fadeOut != original.fadeOut,
                            "the return path must first preview a real change")
                    } else {
                        #expect(
                            draft.gain == original.gain && draft.fadeIn == original.fadeIn
                                && draft.fadeOut == original.fadeOut,
                            "orthogonal clip drags must preserve exact gains and flick durations")
                        #expect(
                            draft.fadeInTicks == original.fadeInTicks && draft.fadeOutTicks == original.fadeOutTicks)
                    }
                    // Release on the original value axis without a final drag event.
                    timeline.mouseUp(with: try mouse(.leftMouseUp, orthogonal, in: timeline))
                    let task = try #require(completion)
                    await task.value
                    #expect(
                        controller.snapshot?.clipMix(clip) == original,
                        "returning to the press axis must restore the original clip mix on release")
                    #expect(
                        controller.undoCount == before && controller.redoCount == 1,
                        "zero value-axis travel must not record an edit or discard Redo")
                    #expect(
                        timeline.renderInput.clipDraft == nil && !timeline.clicks.active
                            && controller.errorMessage == nil)
                }
            }
            await controller.redo()
            #expect(controller.mixer(track)?.pan.value == 0.25 && controller.snapshot?.clipMix(clip) == original)
            await controller.undo()
            await controller.close()
            await controller.open(url)
            #expect(
                controller.snapshot?.clipMix(clip) == original,
                "identity drags must preserve clip values through save and reopen: \(String(describing: controller.snapshot?.clipMix(clip))) != \(original)"
            )
            #expect(controller.mixer(track)?.pan.value == 0)
        }
        await controller.close()
    }
}
