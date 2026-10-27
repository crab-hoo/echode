import AppKit
import Carbon.HIToolbox
import EchodeBridge
import Testing
@testable import EchodeAppCore

@MainActor
struct PianoWorkflowTests {
    @Test func contractTrackSelectionKeepsThePianoTargetAndNoteSelection() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-piano-target-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Chords")
        let chords = try #require(controller.selection)
        #expect(await controller.insertNote(pitch: 60, beat: 1, length: 2))
        let note = try #require(controller.snapshot?.pianoNeighbor(track: chords, current: nil))
        await controller.addTrack(name: "Melody")
        let melody = try #require(controller.selection)
        controller.selection = chords
        controller.noteFocus = note.id
        controller.selectTrack(melody)
        #expect(controller.pianoTrack == chords && controller.selection == melody)
        controller.selectNotes([note.id])
        #expect(controller.noteFocus == note.id, "arrange selection must not replace the piano editing target")
        await controller.close()
    }

    @Test func contractReturnLeavesPianoInsertionToExplicitActions() throws {
        #expect(WorkspaceCommand.pause.shortcut?.key == .return && WorkspaceCommand.pause.shortcut?.modifiers == [])
        #expect(WorkspaceCommand.pause.shortcutLabel == text("Return"))
        let grid = PianoGridView(frame: .zero)
        let receiver = PianoWorkflowKeyReceiver()
        grid.nextResponder = receiver
        for code in [kVK_Return, kVK_ANSI_KeypadEnter] {
            let event = try Self.key(code)
            grid.keyDown(with: event)
            #expect(receiver.events.last === event, "Return must reach pause/resume instead of inserting a note")
        }
    }

    @Test func contractArrangeDoubleClickOpensAnIndependentPianoLocation() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("echode-arrange-open-piano-\(UUID()).echode")
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        await controller.addTrack(name: "Chords")
        await controller.addTrack(name: "Melody")
        let melody = try #require(controller.selection), document = try #require(controller.documentID)
        let snapshot = try #require(controller.snapshot)
        await controller.setPlaybackStart(tick: 123)
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        state.synchronize(controller)
        let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 900, height: 240))
        defer { pane.stop() }
        pane.layout(); pane.setSnapshot(snapshot, selected: controller.selection)
        let timeline = pane.timeline
        timeline.setRuler(start: controller.playbackStart, loop: nil, busy: false)
        timeline.present(ArrangeFrame(input: timeline.renderInput))
        var opens = 0
        timeline.onOpenPiano = { track, tick in
            opens += 1
            session.openPiano(PianoTrackTarget(document: document, track: track), tick: tick)
        }
        let point = CGPoint(x: timeline.renderInput.timelineOrigin + 3 * timeline.camera.pixelsPerBeat,
                            y: timeline.renderInput.arrangeTop + 1.5 * timeline.camera.pixelsPerTrack)
        let event = try #require(NSEvent.mouseEvent(with: .leftMouseDown, location: timeline.convert(point, to: nil), modifierFlags: [],
            timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 2, pressure: 1))
        let undo = controller.undoCount
        timeline.mouseDown(with: event)
        #expect(opens == 1 && controller.pianoTrack == melody && state.showsEditor,
                "a blank arrange double click must explicitly open that piano track and location")
        #expect(controller.playbackStart == 123 && controller.audio.tick == 123 && controller.undoCount == undo)
        let location = try #require(controller.pianoLocation)
        #expect(location.tick == 3 * 705_600)
        let piano = PianoGridView(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        defer { piano.stop() }
        piano.setNavigation(state.navigation, documentID: document)
        piano.setSnapshot(snapshot, track: melody, focus: nil, key: DisplayKey(), busy: false)
        piano.setLocation(location)
        #expect(piano.camera.originBeat == 3 && state.navigation.pianoLocationID == location.id)
        // Recreating the editor restores a later manual scroll, not the old open request.
        var moved = piano.camera; moved.originBeat = 7
        state.navigation.pianoCamera = moved
        let recreated = PianoGridView(frame: piano.frame)
        defer { recreated.stop() }
        recreated.setNavigation(state.navigation, documentID: document)
        recreated.setSnapshot(snapshot, track: melody, focus: nil, key: DisplayKey(), busy: false)
        recreated.setLocation(location)
        #expect(recreated.camera.originBeat == 7)
        await controller.close(); await controller.open(url)
        let target = controller.pianoTrack
        session.openPiano(PianoTrackTarget(document: document, track: melody), tick: 10)
        #expect(controller.pianoTrack == target && controller.pianoLocation == nil)
        await controller.close()
    }

    @Test func contractBrowserListsOwnReturnBeforeTheWorkspaceShortcut() throws {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 300, height: 220), styleMask: [.titled], backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let files = FileListView(), file = BrowserFile(url: URL(fileURLWithPath: "/samples/kick.wav"), directory: false)
        files.show([file], searching: false, selection: file.url)
        let plugins = PluginListView()
        plugins.table.plugin = { _ in "test-plugin" }
        var imported = 0, inserted = 0
        files.table.onOpen = { _ in imported += 1 }
        plugins.table.onInsert = { id, closes in
            #expect(id == "test-plugin" && !closes)
            inserted += 1
        }
        for (list, table) in [(files as NSScrollView, files.table as NSTableView), (plugins, plugins.table)] {
            window.contentView = list
            #expect(window.makeFirstResponder(table))
            for code in [kVK_Return, kVK_ANSI_KeypadEnter] {
                #expect(table.performKeyEquivalent(with: try Self.key(code)))
            }
        }
        #expect(imported == 2 && inserted == 2, "focused browser lists keep their local Return action")
    }

    static func key(_ code: Int, flags: NSEvent.ModifierFlags = [], repeating: Bool = false) throws -> NSEvent {
        try #require(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags,
            timestamp: 0, windowNumber: 0, context: nil, characters: "", charactersIgnoringModifiers: "",
            isARepeat: repeating, keyCode: UInt16(code)))
    }
}

@MainActor
private final class PianoWorkflowKeyReceiver: NSResponder {
    var events: [NSEvent] = []
    override func keyDown(with event: NSEvent) { events.append(event) }
}
