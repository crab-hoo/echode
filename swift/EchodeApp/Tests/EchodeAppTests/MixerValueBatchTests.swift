import AppKit
import EchodeBridge
import Testing

@testable import EchodeAppCore

@MainActor
struct MixerValueBatchTests {
    @Test func contractTypedHeaderValuesPreserveGroupOffsetsAsOneUndoAndReopen() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-mixer-value-\(UUID()).echode")
        defer { removeTestFile(url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        var tracks: [UUID] = []
        for name in ["anchor", "quieter", "outside"] {
            await controller.addTrack(name: name)
            tracks.append(try #require(controller.selection))
        }
        #expect(await controller.setMixer(tracks[0], field: .gain, value: 0.5))
        #expect(await controller.setMixer(tracks[1], field: .gain, value: 0.25))
        #expect(await controller.setMixer(tracks[0], field: .pan, value: -0.5))
        #expect(await controller.setMixer(tracks[1], field: .pan, value: 0.25))
        let document = try #require(controller.documentID)
        let undo = controller.undoCount
        controller.selection = tracks[0]
        controller.selectTrack(tracks[1], change: .toggle)
        let window = NSWindow(
            contentRect: CGRect(x: 0, y: 0, width: 220, height: 192), styleMask: [.borderless], backing: .buffered,
            defer: true)
        let header = ArrangeHeaderView(frame: CGRect(x: 0, y: 0, width: 220, height: 192))
        window.contentView = header
        header.onSelectTrack = { controller.selectTrack($0, change: $1) }
        header.mixActions = controller.arrangeMixActions(document: document)
        let controls = TrackControlLayout(row: CGRect(x: 0, y: 0, width: 220, height: 64))
        func enter(_ field: MixerField, _ typed: String) async throws {
            header.setSnapshot(
                try #require(controller.snapshot), selected: controller.selection,
                selectedTracks: controller.selectedTracks)
            let rect = controls.rect(field)
            let mouse = try #require(
                NSEvent.mouseEvent(
                    with: .leftMouseDown,
                    location: header.convert(CGPoint(x: rect.midX, y: rect.midY), to: nil), modifierFlags: [],
                    timestamp: 0,
                    windowNumber: window.windowNumber, context: nil, eventNumber: 1, clickCount: 3, pressure: 1))
            header.mouseDown(with: mouse)
            let entry = try #require(header.entry)
            let editor = try #require(entry.currentEditor() as? NSTextView)
            editor.selectAll(nil)
            editor.insertText(typed, replacementRange: NSRange(location: NSNotFound, length: 0))
            let key = try #require(
                NSEvent.keyEvent(
                    with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                    windowNumber: window.windowNumber, context: nil, characters: "\r",
                    charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36))
            let before = controller.undoCount
            #expect(entry.performKeyEquivalent(with: key))
            // Capture occurs on Return, before the edit task can observe another selection.
            controller.selection = tracks[2]
            await entry.pending?.value
            let deadline = ContinuousClock.now.advanced(by: .seconds(5))
            while controller.undoCount == before && ContinuousClock.now < deadline { await Task.yield() }
            #expect(controller.undoCount == before + 1)
        }
        try await enter(.gain, "0 dB")
        #expect(controller.mixer(tracks[0])?.gain.value == 1)
        #expect(
            controller.mixer(tracks[1])?.gain.value == 0.5,
            "typed volume must preserve every captured selected track's dB offset")
        #expect(controller.mixer(tracks[2])?.gain.value == 1 && controller.selection == tracks[2])
        #expect(controller.undoCount == undo + 1)
        await controller.undo()
        #expect(controller.mixer(tracks[0])?.gain.value == 0.5 && controller.mixer(tracks[1])?.gain.value == 0.25)
        await controller.redo()
        #expect(controller.mixer(tracks[1])?.gain.value == 0.5)
        controller.selection = tracks[0]
        controller.selectTrack(tracks[1], change: .toggle)
        try await enter(.pan, "0 %")
        #expect(controller.mixer(tracks[0])?.pan.value == 0)
        #expect(controller.mixer(tracks[1])?.pan.value == 0.75, "typed pan must apply one common position difference")
        #expect(controller.mixer(tracks[2])?.pan.value == 0 && controller.undoCount == undo + 2)
        let recorded = tracks.map { controller.mixer($0) }
        await controller.close()
        await controller.open(url)
        #expect(tracks.map { controller.mixer($0) } == recorded)
        await controller.close()
    }
}
