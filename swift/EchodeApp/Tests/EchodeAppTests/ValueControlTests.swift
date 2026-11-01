import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

/// Value controls (ADR 0153): a click keeps the value, a drag is relative, a
/// double click resets to the default, and a third click types instead.
@MainActor
struct ValueControlTests {
    private func mouse(_ type: NSEvent.EventType, _ point: CGPoint, in view: NSView, clicks: Int = 1) throws -> NSEvent {
        try #require(NSEvent.mouseEvent(with: type, location: view.convert(point, to: nil), modifierFlags: [], timestamp: 0,
            windowNumber: view.window?.windowNumber ?? 0, context: nil, eventNumber: 1, clickCount: clicks, pressure: 1))
    }

    private func key(_ characters: String, code: UInt16, in window: NSWindow) throws -> NSEvent {
        try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: window.windowNumber, context: nil, characters: characters,
            charactersIgnoringModifiers: characters, isARepeat: false, keyCode: code))
    }

    /// Clicks one to `count` times at `point`, releasing each press.
    private func click(_ view: NSView, at point: CGPoint, count: Int) throws {
        for clicks in 1...count {
            view.mouseDown(with: try mouse(.leftMouseDown, point, in: view, clicks: clicks))
            if clicks < 3 { view.mouseUp(with: try mouse(.leftMouseUp, point, in: view, clicks: clicks)) }
        }
    }

    /// Types `text` into the open field and presses Return through the field's
    /// key routing, as a keyboard does.
    private func type(_ text: String, into field: ValueEntryField) throws {
        let window = try #require(field.window)
        let editor = try #require(field.currentEditor() as? NSTextView)
        editor.selectAll(nil)
        editor.insertText(text, replacementRange: NSRange(location: NSNotFound, length: 0))
        #expect(field.performKeyEquivalent(with: try key("\r", code: 36, in: window)))
    }

    @Test func clicksKeepTheValueUntilDragsAndTurnAThirdClickIntoTyping() async throws {
        let clicks = ValueClicks<Int>()
        var log: [String] = []
        var refuse = false
        clicks.begin = { target in log.append("begin \(target)"); return !refuse }
        clicks.drag = { target, offset in log.append("drag \(target) \(Int(offset.width)),\(Int(offset.height))") }
        clicks.reset = { target in log.append("reset \(target)") }
        clicks.finish = { commit in log.append(commit ? "commit" : "restore") }
        clicks.type = { target in log.append("type \(target)") }
        clicks.interval = 0.01

        clicks.down(1, at: .zero, clicks: 1)
        clicks.dragged(to: .zero)
        clicks.up(at: .zero)
        #expect(log.isEmpty, "a click without movement changes nothing")
        clicks.down(1, at: CGPoint(x: 10, y: 10), clicks: 1)
        clicks.dragged(to: CGPoint(x: 14, y: 7))
        clicks.up(at: CGPoint(x: 20, y: 7))
        #expect(log == ["begin 1", "drag 1 4,-3", "drag 1 10,-3", "commit"])

        log = []
        clicks.down(1, at: .zero, clicks: 1); clicks.up(at: .zero)
        clicks.down(1, at: .zero, clicks: 2)
        clicks.dragged(to: CGPoint(x: 30, y: 0))
        clicks.up(at: .zero)
        #expect(log == ["begin 1", "reset 1"], "the default waits and ignores drags")
        clicks.down(1, at: .zero, clicks: 3)
        #expect(log == ["begin 1", "reset 1", "restore", "type 1"])
        #expect(!clicks.active)

        log = []
        clicks.down(2, at: .zero, clicks: 2); clicks.up(at: .zero)
        try await Task.sleep(for: .milliseconds(80))
        #expect(log == ["begin 2", "reset 2", "commit"], "no third click records the default")

        log = []
        clicks.down(2, at: .zero, clicks: 2); clicks.up(at: .zero)
        clicks.down(3, at: .zero, clicks: 3)
        #expect(log == ["begin 2", "reset 2", "commit", "type 3"], "another control's click records the shown default")
        log = []
        clicks.down(2, at: .zero, clicks: 2); clicks.up(at: .zero)
        clicks.cancel()
        try await Task.sleep(for: .milliseconds(80))
        #expect(log == ["begin 2", "reset 2", "restore"], "Escape restores and the timer no longer records")

        log = []
        refuse = true
        clicks.down(1, at: .zero, clicks: 1)
        clicks.dragged(to: CGPoint(x: 5, y: 0))
        clicks.dragged(to: CGPoint(x: 9, y: 0))
        clicks.up(at: CGPoint(x: 9, y: 0))
        clicks.down(1, at: .zero, clicks: 2); clicks.up(at: .zero)
        #expect(log == ["begin 1", "begin 1"])
        #expect(!clicks.active)
    }

    @Test func entryFieldsStayInsideTheHostAndReadTheInspectorUnits() {
        let bounds = CGRect(x: 0, y: 0, width: 200, height: 64)
        #expect(ValueEntryField.frame(for: CGRect(x: 180, y: 29, width: 12, height: 18), in: bounds) == CGRect(x: 128, y: 27, width: 72, height: 22))
        #expect(ValueEntryField.frame(for: CGRect(x: 10, y: 10, width: 100, height: 30), in: bounds) == CGRect(x: 10, y: 10, width: 100, height: 30))
        #expect(MixEntry.text(.gain, 0.5) == "-6.0" && MixEntry.text(.gain, 0) == "−∞" && MixEntry.text(.pan, -0.25) == "-25.0")
        #expect(MixEntry.value(.gain, "−∞") == 0)
        #expect(abs((MixEntry.value(.gain, "-12 dB") ?? 0) - Float(pow(10, -12.0 / 20))) < 0.000001)
        #expect(MixEntry.value(.gain, "13") == nil && MixEntry.value(.gain, "loud") == nil)
        #expect(MixEntry.value(.pan, "-50 %") == -0.5 && MixEntry.value(.pan, "101") == nil && MixEntry.value(.mute, "1") == nil)
        #expect(MixScale.dragged(.gain, from: 1, by: 0) == 1 && MixScale.dragged(.pan, from: 0.5, by: 0) == 0.5)
        #expect(MixScale.dragged(.gain, from: 0, by: -1) == 0 && MixScale.dragged(.pan, from: 0.5, by: 1) == 1)
    }

    @Test func contractHeaderDoubleClickResetsAndTripleClickTypesWithOneUndo() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-value-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Lead")
        let track = try #require(controller.selection)
        #expect(await controller.setMixer(track, field: .gain, value: 0.5))
        #expect(await controller.setMixer(track, field: .pan, value: 0.25))
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 200, height: 64), styleMask: [.borderless],
                              backing: .buffered, defer: true)
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 200, height: 64))
        window.contentView = header
        header.clicks.interval = 0.01
        var completion: Task<Void, Never>?
        header.mixActions = MixActions(begin: { controller.beginFader($0, field: $1) }, change: { controller.changeFader($0) },
            finish: { commit in completion = Task { await controller.finishFader(commit: commit) } },
            set: { id, field, value in completion = Task {
                if controller.faderPreview != nil { await controller.finishFader(commit: true) }
                await controller.setMixer(id, field: field, value: value)
            } })
        func show() throws { header.setSnapshot(try #require(controller.snapshot), selected: track) }
        let layout = TrackControlLayout(row: header.bounds)
        let gain = CGPoint(x: layout.gain.midX, y: layout.gain.midY)
        let pan = CGPoint(x: layout.pan.midX, y: layout.pan.midY)
        let before = controller.undoCount

        // A double click shows unity gain at once and records it after the interval.
        try show()
        try click(header, at: gain, count: 2)
        #expect(controller.faderPreview?.gain == 1)
        #expect(controller.mixer(track)?.gain.value == 0.5)
        #expect(controller.undoCount == before)
        try await Task.sleep(for: .milliseconds(100))
        await completion?.value
        #expect(controller.mixer(track)?.gain.value == 1)
        #expect(controller.faderPreview == nil)
        #expect(controller.undoCount == before + 1)
        await controller.undo()
        #expect(controller.mixer(track)?.gain.value == 0.5)

        // A third click restores the value and types; Return records once.
        try show()
        try click(header, at: gain, count: 3)
        await completion?.value
        #expect(controller.faderPreview == nil)
        #expect(controller.mixer(track)?.gain.value == 0.5)
        let field = try #require(header.entry)
        #expect(field.stringValue == "-6.0")
        #expect(window.firstResponder === field.currentEditor())
        // Space is typed into the field, never taken by the playback shortcut.
        #expect(field.performKeyEquivalent(with: try key(" ", code: 49, in: window)))
        #expect(field.currentEditor()?.string.contains(" ") == true)
        try type("loud", into: field)
        await field.pending?.value
        #expect(field.invalid && header.entry === field && !field.closed)
        try type("-12 dB", into: field)
        await field.pending?.value
        await completion?.value
        #expect(field.closed && header.entry == nil)
        #expect(window.firstResponder === header)
        #expect(abs(try #require(controller.mixer(track)).gain.value - Float(pow(10, -12.0 / 20))) < 0.00001)
        #expect(controller.undoCount == before + 1)
        await controller.undo()
        #expect(controller.mixer(track)?.gain.value == 0.5)
        #expect(controller.undoCount == before)

        // Escape closes the field without a change; pan types percent.
        try show()
        try click(header, at: pan, count: 3)
        await completion?.value
        let panField = try #require(header.entry)
        #expect(panField.stringValue == "25.0")
        #expect(panField.performKeyEquivalent(with: try key("\u{1b}", code: 53, in: window)))
        #expect(panField.closed && header.entry == nil)
        #expect(controller.mixer(track)?.pan.value == 0.25)
        #expect(controller.undoCount == before)
        try click(header, at: pan, count: 3)
        await completion?.value
        try type("-50", into: try #require(header.entry))
        await header.entry?.pending?.value
        await completion?.value
        #expect(controller.mixer(track)?.pan.value == -0.5)
        #expect(controller.undoCount == before + 1)
        await controller.close()
        await controller.open(url)
        #expect(controller.mixer(track)?.pan.value == -0.5)
        #expect(controller.mixer(track)?.gain.value == 0.5)
        await controller.close()
    }

    @Test func contractClipHandlesResetOnDoubleClickAndTypeOnTripleClick() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-value-clip-\(UUID())")
        defer { removeTestFile(root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let wav = root.appendingPathComponent("clip.wav")
        try wavMono(rate: 48_000, frames: 48_000).write(to: wav)
        let url = root.appendingPathComponent("Project.echode")
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Audio")
        #expect(await controller.importAudio(wav))
        let track = try #require(controller.selection)
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 1200, height: 128), styleMask: [.borderless],
                              backing: .buffered, defer: true)
        let pane = ArrangePaneView(frame: CGRect(x: 0, y: 0, width: 1200, height: 128))
        window.contentView = pane
        pane.layout()
        let timeline = pane.timeline
        timeline.clicks.interval = 0.01
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
        func present() throws -> ClipControlLayout {
            pane.setSnapshot(try #require(controller.snapshot), selected: track)
            pane.setControlSelection(clip: id, preview: nil)
            timeline.setViewport(camera, header: ArrangeHeaderInput(width: 160, palette: ArrangeHeaderPalette()))
            let frame = ArrangeFrame(input: timeline.renderInput)
            timeline.present(frame)
            return try #require(frame.clipControlLayout)
        }
        #expect(await controller.setClipGain(id, gain: 2))
        #expect(await controller.setClipFades(id, fadeIn: 70_560_000, fadeOut: 0))
        let before = controller.undoCount

        var layout = try present()
        let gain = CGPoint(x: layout.gain.midX, y: layout.gain.midY)
        try click(timeline, at: gain, count: 2)
        #expect(timeline.renderInput.clipDraft?.gain == 1)
        #expect(controller.snapshot?.clipMix(id)?.gain == 2)
        try await Task.sleep(for: .milliseconds(100))
        await completion?.value
        #expect(controller.snapshot?.clipMix(id)?.gain == 1)
        #expect(controller.undoCount == before + 1)

        layout = try present()
        let fade = CGPoint(x: layout.entering.midX, y: layout.entering.midY)
        try click(timeline, at: fade, count: 3)
        #expect(timeline.renderInput.clipDraft == nil)
        let field = try #require(timeline.entry)
        #expect(field.stringValue == "100.0")
        try type("250 ms", into: field)
        await field.pending?.value
        await completion?.value
        let typedFade: Int64 = 250 * 705_600
        #expect(controller.snapshot?.clipMix(id)?.fadeIn == typedFade)
        #expect(controller.snapshot?.clipMix(id)?.fadeOut == 0)
        #expect(controller.undoCount == before + 2)

        layout = try present()
        try click(timeline, at: CGPoint(x: layout.gain.midX, y: layout.gain.midY), count: 3)
        let gainField = try #require(timeline.entry)
        #expect(gainField.stringValue == "0.0")
        try type("-6", into: gainField)
        await gainField.pending?.value
        await completion?.value
        #expect(abs(try #require(controller.snapshot?.clipMix(id)).gain - Float(pow(10, -6.0 / 20))) < 0.00001)
        #expect(controller.undoCount == before + 3)
        await controller.undo()
        #expect(controller.snapshot?.clipMix(id)?.gain == 1)
        await controller.close()
        await controller.open(url)
        #expect(controller.snapshot?.clipMix(id)?.fadeIn == typedFade)
        await controller.close()
    }
}
