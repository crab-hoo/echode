import AppKit
import EchodeBridge
import Foundation
import Testing
@testable import EchodeAppCore

@MainActor
struct WorkspaceTests {
    @Test func failedStartupOpenReenablesNewAndOpen() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-startup-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let previous = root.appendingPathComponent("Previous.echode")
        let fresh = root.appendingPathComponent("Fresh.echode")
        let controller = ProjectController()
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        await controller.open(previous, creating: true)
        #expect(controller.snapshot != nil)
        await controller.close()
        // Replace only this fixture's metadata with the failing initialization case.
        try FileManager.default.removeItem(at: previous.appendingPathComponent(".git"))
        let config = previous.appendingPathComponent(".git/config")
        try FileManager.default.createDirectory(at: config, withIntermediateDirectories: true)
        await controller.open(previous)
        #expect(controller.errorMessage != nil && controller.snapshot == nil)
        #expect(!controller.busy && session.enabled(.newProject) && session.enabled(.openProject))
        controller.errorMessage = nil
        await controller.open(fresh, creating: true)
        #expect(controller.snapshot != nil && controller.errorMessage == nil)
        #expect(controller.projectURL == fresh && !controller.busy)
        await controller.addTrack(name: "ready")
        #expect(controller.tracks.map(\.name) == ["ready"])
        await controller.close()
    }

    @Test func layoutsFitTheMinimumWindowAndKeepPreferencesWhileHidden() throws {
        var layout = WorkspaceLayout()
        for mask in 0..<8 {
            layout.inspector = mask & 1 != 0; layout.browser = mask & 2 != 0; layout.editor = mask & 4 != 0
            let size = CGSize(width: layout.minimumWidth, height: layout.minimumHeight - 80)
            let dimensions = layout.dimensions(in: size)
            let sideDividers = (layout.inspector ? 5.0 : 0) + (layout.browser ? 5.0 : 0)
            #expect(size.width - dimensions.inspector - dimensions.browser - sideDividers >= 520)
            #expect(dimensions.arrange >= 220)
            if layout.inspector { #expect(dimensions.inspector >= 180) }
            if layout.browser { #expect(dimensions.browser >= 180) }
            if layout.editor { #expect(dimensions.editor >= 260) }
        }
        let suite = "echode-workspace-\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let state = WorkspaceState(defaults: defaults)
        state.layout.resize(.inspector, to: 250, in: CGSize(width: 1200, height: 650))
        state.layout.resize(.editor, to: 360, in: CGSize(width: 1200, height: 650))
        state.toggle(.inspector)
        let reopened = WorkspaceState(defaults: defaults)
        #expect(reopened.layout == state.layout)
        reopened.toggle(.inspector)
        #expect(reopened.layout.inspectorWidth == 250 && reopened.layout.editorHeight == 360)
        reopened.layout.resize(.browser, to: .nan, in: CGSize(width: 1200, height: 650))
        #expect(reopened.layout.browserWidth.isFinite)
    }

    @Test func numericDraftsRejectPartialNonfiniteZeroAndOverflowValues() {
        #expect(NoteDraft.number(" 1.25 ", locale: Locale(identifier: "en_US")) == 1.25)
        #expect(NoteDraft.number("1,25", locale: Locale(identifier: "de_DE")) == 1.25)
        #expect(NoteDraft.number("−1.5", locale: Locale(identifier: "en_US")) == -1.5)
        #expect(NoteDraft.number("–2", locale: Locale(identifier: "ja_JP")) == -2)
        for value in ["1x", "NaN", "inf", "", "1.2.3"] { #expect(NoteDraft.number(value, locale: Locale(identifier: "en_US")) == nil) }
        var draft = NoteDraft()
        #expect(draft.geometry == NoteGeometry(start: 0, length: 705_600, pitch: 60))
        for value in ["0", "-1", "0.00000000001", "1e40", "1x"] { draft.length = value; #expect(draft.geometry == nil) }
        draft.length = "1"; draft.beat = "-1"; #expect(draft.geometry == nil)
        draft.beat = "0"; draft.pitch = 128; #expect(draft.geometry == nil)
    }

    @Test func pendingDraftsFollowNoteIdentityAndExternalChanges() async throws {
        let url = temporaryProject()
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true); await controller.addTrack(name: "notes")
        await controller.insertNote(pitch: 60, beat: 0, length: 1)
        await controller.insertNote(pitch: 60, beat: 0, length: 1)
        let track = try #require(controller.selection)
        let notes = try #require(controller.snapshot).pianoNotes(track: track, originTick: 0, widthTicks: 705_600 * 4,
            topPitch: 72, heightPitches: 24, pixelsPerTick: 72.0 / 705_600, pixelsPerPitch: 20)
        #expect(notes.count == 2)
        let state = WorkspaceState(defaults: .init())
        controller.noteFocus = notes[0].id; state.synchronize(controller)
        state.draft.beat = "3"
        state.synchronize(controller)
        #expect(state.draft.beat == "3" && state.draft.canApply)
        controller.noteFocus = notes[1].id; state.synchronize(controller)
        #expect(state.draft.geometry == NoteGeometry(notes[1]) && !state.draft.canApply)
        state.draft.beat = "5"
        await controller.editNote(notes[1].id, geometry: NoteGeometry(start: 705_600 * 2, length: 705_600, pitch: 62))
        state.synchronize(controller)
        #expect(state.draft.geometry?.start == 705_600 * 2 && !state.draft.canApply)
        await controller.undo(); state.synchronize(controller)
        #expect(state.draft.geometry == NoteGeometry(notes[1]))
        await controller.close()
    }

    @Test func selectionAndCapturedEditTargetsStayConsistentWithoutAView() async throws {
        let url = temporaryProject()
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true); let document = try #require(controller.documentID)
        await controller.addTrack(name: "first"); let first = try #require(controller.selection)
        await controller.insertNote(pitch: 60, beat: 0, length: 1)
        let note = try #require(controller.snapshot?.pianoNeighbor(track: first, current: nil, backwards: false))
        controller.noteFocus = note.id
        await controller.addTrack(name: "second"); let second = try #require(controller.selection)
        #expect(controller.pianoTrack == first && controller.noteFocus == note.id)
        controller.noteFocus = note.id
        #expect(controller.noteFocus == note.id)
        #expect(await controller.insertPlugin("dev.echode.test-gain", on: first))
        #expect(controller.selection == second && controller.snapshot?.devicePlugin(onTrack: 0, at: 0) == "dev.echode.test-gain")
        #expect(controller.snapshot?.deviceCount(onTrack: 1) == 1)
        #expect(await controller.insertNote(pitch: 64, beat: 2, length: 1, on: first))
        #expect(controller.snapshot?.pianoNeighbor(track: second, current: nil, backwards: false) == nil)
        let before = controller.undoCount
        #expect(!(await controller.insertNote(pitch: 60, beat: 0, length: 0.00000000001)))
        #expect(controller.undoCount == before)
        controller.selection = first; controller.noteFocus = note.id
        #expect(await controller.deleteNote(note.id))
        #expect(controller.noteFocus == nil)
        await controller.close(); #expect(controller.documentID == nil)
        await controller.open(url); #expect(controller.documentID != document)
        await controller.close()
    }

    @Test func editsCapturedBeforeOpeningACopyCannotMutateItsIdenticalIDs() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-context-\(UUID())")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let original = root.appendingPathComponent("original.echode"), copy = root.appendingPathComponent("copy.echode")
        let controller = ProjectController()
        await controller.open(original, creating: true); await controller.addTrack(name: "pad")
        await controller.insertNote(pitch: 60, beat: 0, length: 1)
        let track = try #require(controller.selection), document = try #require(controller.documentID)
        let note = try #require(controller.snapshot?.pianoNeighbor(track: track, current: nil, backwards: false))
        await controller.close()
        try FileManager.default.copyItem(at: original, to: copy)
        await controller.open(copy)
        #expect(controller.selection == track && controller.snapshot?.pianoNote(note.id) != nil)
        let geometry = NoteGeometry(start: note.start, length: note.length, pitch: 65)
        #expect(!(await controller.editNote(note.id, geometry: geometry, documentID: document)))
        #expect(!(await controller.deleteNote(note.id, documentID: document)))
        #expect(!(await controller.insertNote(pitch: 64, beat: 1, length: 1, on: track, documentID: document)))
        #expect(!(await controller.insertPlugin("dev.echode.test-gain", on: track, documentID: document)))
        #expect(controller.undoCount == 0 && controller.snapshot?.pianoNote(note.id)?.pitch == 60)
        #expect(await controller.editNote(note.id, geometry: geometry, documentID: try #require(controller.documentID)))
        #expect(controller.snapshot?.pianoNote(note.id)?.pitch == 65)
        await controller.close()
    }

    @Test func commandCapabilitiesAndNonmodalPluginDocumentOwnershipStayConsistent() async throws {
        let url = temporaryProject()
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        let state = WorkspaceState(defaults: .init())
        let session = WorkspaceSession(controller: controller, state: state)
        #expect(session.enabled(.newProject) && !session.enabled(.undo) && !session.enabled(.plugins))
        await controller.open(url, creating: true)
        #expect(session.enabled(.addTrack) && !session.enabled(.plugins))
        #expect(session.enabled(.loop) && session.enabled(.metronome))
        await controller.addTrack(name: "first"); let first = try #require(controller.selection)
        #expect(session.enabled(.undo) && session.enabled(.plugins))
        session.perform(.palette)
        for command in WorkspaceCommand.allCases {
            #expect(!session.enabled(command))
            let paletteEnabled = session.enabled(command, fromPalette: true)
            state.presentation = nil
            #expect(paletteEnabled == session.enabled(command))
            state.presentation = .palette
        }
        state.presentation = nil; session.perform(.plugins)
        let document = try #require(controller.documentID)
        #expect(state.presentation == nil && state.pluginBrowserDocument == document && state.showsDevices)
        await controller.addTrack(name: "second")
        #expect(controller.selection != first && state.pluginBrowserDocument == document)
        #expect(session.enabled(.undo) && session.enabled(.addTrack))
        state.choosingFile = true
        #expect(!session.enabled(.openProject) && !session.enabled(.palette))
        state.choosingFile = false
        state.pluginBrowserDocument = DocumentID()
        state.synchronize(controller)
        #expect(state.pluginBrowserDocument == nil)
        state.presentation = .history
        state.synchronize(controller)
        #expect(state.presentation == .history)
        await controller.close(); state.synchronize(controller)
        #expect(state.presentation == nil)
    }

    @Test func listNavigationUsesTheSelectedResultAndStopsAtTheEnds() {
        #expect(movedSelection(nil, in: ["a", "b", "c"], offset: 1) == "a")
        #expect(movedSelection("a", in: ["a", "b", "c"], offset: 1) == "b")
        #expect(movedSelection("c", in: ["a", "b", "c"], offset: 1) == "c")
        #expect(movedSelection("a", in: ["a", "b", "c"], offset: -1) == "a")
        #expect(movedSelection("b", in: [String](), offset: 1) == nil)
        #expect(retainedSelection("b", in: ["a", "b"]) == "b")
        #expect(retainedSelection("c", in: ["a", "b"]) == "a")
        #expect(retainedSelection(nil as String?, in: ["a"]) == "a")
        #expect(retainedSelection("a", in: [String]()) == nil)
        #expect(preservedSelection("b", in: ["a", "b"]) == "b")
        #expect(preservedSelection("c", in: ["a", "b"]) == "a")
        #expect(preservedSelection(nil as String?, in: ["a"]) == nil)
        #expect(preservedSelection("a", in: [String]()) == nil)
        let precise = WheelScroll.points(x: 3, y: -2, precise: true, lineWidth: 72, lineHeight: 64)
        let lines = WheelScroll.points(x: 1, y: -3, precise: false, lineWidth: 72, lineHeight: 64)
        let rejected = WheelScroll.points(x: .nan, y: 1, precise: false, lineWidth: 72, lineHeight: 64)
        #expect(precise.x == 3 && precise.y == -2)
        #expect(lines.x == 72 && lines.y == -192)
        #expect(rejected.x == 0 && rejected.y == 0)
    }

    @Test func folderNavigationFiltersProjectsAndLatestRequestWins() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("echode-browser-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        let first = root.appendingPathComponent("first"), second = root.appendingPathComponent("second")
        try FileManager.default.createDirectory(at: first, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: second.appendingPathComponent("Song.echode"), withIntermediateDirectories: true)
        try Data().write(to: second.appendingPathComponent("notes.txt"))
        let browser = FileBrowserState()
        var started = false
        let older = Task { started = true; await browser.load(first) }
        while !started { await Task.yield() }
        await browser.load(second)
        await older.value
        #expect(browser.folder == second && !browser.loading && browser.files.map(\.url.lastPathComponent) == ["Song.echode"])
        #expect(browser.canOpenParent && !FileBrowserState.canOpenParent(of: URL(fileURLWithPath: "/")))
        browser.selection = browser.files.last?.url
        browser.query = "song"
        #expect(browser.filtered.count == 1 && browser.filtered[0].project && browser.filtered[0].icon == "waveform")
        #expect(browser.selection == browser.filtered[0].url)
        browser.query = "missing"
        #expect(browser.selection == nil)
        browser.query = ""
        #expect(browser.selection == nil)
        browser.query = "notes"
        #expect(browser.selection == nil)
        await browser.load(root.appendingPathComponent("missing"))
        #expect(browser.error != nil && !browser.loading && browser.folder == second)
    }

    @Test func overlappingCatalogReloadsKeepTheLaterRequest() {
        var scans = CoalescedReload()
        let first = scans.begin()
        #expect(first == 1 && scans.begin() == nil && scans.finish(1) == 2 && scans.finish(2) == nil)
        #expect(scans.begin() == 3)
        #expect(scans.begin() == nil)
        scans.abandon()
        #expect(scans.begin() == 5)
        #expect(!InspectorNotes.showsPrompt(noteFocused: true, audioClipFocused: false))
        #expect(!InspectorNotes.showsPrompt(noteFocused: false, audioClipFocused: true))
        #expect(InspectorNotes.showsPrompt(noteFocused: false, audioClipFocused: false))
        #expect(!LoopCommit.needsSend(soundingStart: nil, soundingEnd: nil, start: 0, end: 4))
        #expect(!LoopCommit.needsSend(soundingStart: 0, soundingEnd: 4, start: 0, end: 4))
        #expect(LoopCommit.needsSend(soundingStart: 0, soundingEnd: 4, start: 0, end: 8))
    }

    @Test func rendererFeedbackRejectsOldGenerationsAndOutOfOrderFrames() {
        var gate = FrameFeedbackGate()
        let first = gate.restart()
        let accepted = gate.accept(first, revision: 5)
        let older = gate.accept(first, revision: 4)
        let same = gate.accept(first, revision: 5)
        #expect(accepted && !older && same)
        gate.invalidate()
        let stopped = gate.accept(first, revision: 6)
        #expect(!stopped)
        let second = gate.restart()
        let obsolete = gate.accept(first, revision: 100)
        let fresh = gate.accept(second, revision: 1)
        #expect(!obsolete && fresh)
    }

    @Test func closingAndRecreatingCanvasesPreservesViewportsWithoutOpeningWindows() async throws {
        let url = temporaryProject()
        defer { try? FileManager.default.removeItem(at: url) }
        let controller = ProjectController()
        await controller.open(url, creating: true)
        for index in 0..<12 { await controller.addTrack(name: "track \(index)") }
        let track = try #require(controller.tracks.first?.id)
        await controller.insertNote(pitch: 60, beat: 0, length: 1, on: track)
        let snapshot = try #require(controller.snapshot)
        let note = try #require(snapshot.pianoNeighbor(track: track, current: nil, backwards: false))
        let navigation = CanvasNavigationState()
        navigation.prepare(document: controller.documentID, tracks: Set(controller.tracks.map(\.id)))
        let pane = ArrangePaneView(frame: NSRect(x: 0, y: 0, width: 650, height: 180))
        pane.setNavigation(navigation, documentID: navigation.document); pane.setSnapshot(snapshot, selected: track); pane.layout()
        pane.headers.onScroll?(-144, -200); pane.resizeHeader(to: 200)
        let saved = try #require(navigation.arrange)
        let recreated = ArrangePaneView(frame: pane.frame)
        recreated.setNavigation(navigation, documentID: navigation.document); recreated.setSnapshot(snapshot, selected: track); recreated.layout()
        #expect(recreated.viewport == saved && recreated.headers.bounds.width == 200)
        let piano = PianoCamera(originBeat: 12, topPitch: 90.5, pixelsPerBeat: 120, pixelsPerPitch: 28)
        navigation.pianoCamera = piano
        let grid = PianoGridView(frame: NSRect(x: 0, y: 0, width: 650, height: 180))
        grid.setNavigation(navigation, documentID: navigation.document); grid.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(), busy: false)
        #expect(grid.camera == piano && grid.window == nil && !grid.rendererAvailable)
        grid.setSnapshot(snapshot, track: track, focus: note.id, key: DisplayKey(tonic: 2), busy: true)
        #expect(grid.camera == piano)
        await controller.addTrack(name: "unrelated")
        grid.setSnapshot(try #require(controller.snapshot), track: track, focus: note.id, key: DisplayKey(tonic: 2), busy: false)
        #expect(grid.camera == piano)
        navigation.prepare(document: controller.documentID, tracks: [])
        grid.stop()
        #expect(navigation.pianoCamera == piano)
        navigation.prepare(document: DocumentID(), tracks: [track])
        grid.stop(); pane.layout()
        #expect(navigation.arrange == nil && navigation.headerWidth == 160 && navigation.pianoCamera == nil)
        await controller.close()
    }

    @Test func pianoZoomKeepsTheBeatAndPitchUnderThePointer() {
        var camera = PianoCamera(originBeat: 5, topPitch: 90.5)
        let point = CGPoint(x: 180, y: 55)
        let beat = camera.originBeat + point.x / camera.pixelsPerBeat
        let pitch = camera.topPitch - point.y / camera.pixelsPerPitch
        camera.zoom(factor: 1.5, horizontal: true, vertical: true, anchor: point)
        #expect(abs(camera.originBeat + point.x / camera.pixelsPerBeat - beat) < 1e-10)
        #expect(abs(camera.topPitch - point.y / camera.pixelsPerPitch - pitch) < 1e-10)
        let saved = camera
        camera.zoom(factor: .infinity, horizontal: true, vertical: true, anchor: point)
        #expect(camera == saved)
    }

    private func temporaryProject() -> URL { FileManager.default.temporaryDirectory.appendingPathComponent("echode-workspace-\(UUID()).echode") }
}
